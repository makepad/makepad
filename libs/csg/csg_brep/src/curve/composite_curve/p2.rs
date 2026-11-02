//! `SmCompositeCurve`, part 2: reading composite curves and their segments from a
//! database, joining the segments into one B-spline, and the simultaneous offsetter of
//! composite curves (raw offsets, corner fills, intersection and trimming, bow-tie
//! repair, classification and B-spline approximation).
//!
//! Upstream: `source/SMLib/src/SmCompositeCurve.cpp`, `source/SMLib/inc/SmCompositeCurve.h`.
//!
//! | Rust                                                   | Upstream                                              |
//! |--------------------------------------------------------|-------------------------------------------------------|
//! | [`CompositeCurveSegment::new`]                         | `SmCompositeCurveSegment(pParentCurve, bSameSense, dGapStart, dGapEnd)` |
//! | [`CompositeCurveSegment::read_from_db`]                | `SmCompositeCurveSegment::ReadFromDB` (static)        |
//! | [`OffsetMapAttribute::make_copy`]                      | `SmOffsetMapAttribute::MakeCopy`                      |
//! | [`check_bow_ties`]                                     | `sm_CheckBowTies` (static)                            |
//! | [`fix_bow_tie`]                                        | `sm_FixBowTie`                                        |
//! | [`CompositeCurve::create_offsets_of_many_curves`], [`OffsetParams`], [`TrimmedOffsets`], [`OffsetPiece`] | `CreateOffsetsOfManyCurves` (static) |
//! | [`CompositeCurve::intersect_and_trim_offsets`]         | `IntersectAndTrimOffsets` (static)                    |
//! | [`CompositeCurve::create_trimmed_offsets`], [`OffsetDirection`] | `CreateTrimmedOffsets`, `SmOffsetDirectionType` |
//! | [`CompositeCurve::make_composite_nurb`]                | `MakeCompositeNurb`                                   |
//! | [`CompositeCurve::read_from_db`]                       | `SmCompositeCurve::ReadFromDB` (static)               |
//!
//! Deviations:
//! - **Attributes.** A [`Curve`] carries no attribute list, so the offsetters pass each
//!   curve with its offset map attribute as an [`OffsetPiece`]. Copies and trims keep
//!   the attribute (upstream's `SM_AB_COPY` behaviour copies it with `MakeCopy`), and
//!   the B-spline approximation of a piece receives it (`RemoveAttribute` /
//!   `AddAttribute`). An attribute's originator (`m_pOriginator`, the positively
//!   oriented copy of the segment curve that was offset) is named by
//!   [`ObjectRef`] whose key is the curve's index in the array of original curves the
//!   offsets are classified against (upstream's pointer, which dangles once
//!   `CreateOffsetsOfManyCurves` returns).
//! - **Out-params.** The offsetters return their curves (and the approximation gaps,
//!   which upstream writes only when asked) as values; `SmContext` arguments are
//!   dropped. [`check_bow_ties`] and [`fix_bow_tie`] fill caller vectors because the
//!   caller ignores their statuses and uses what was found before a failure.
//!   `IntersectAndTrimOffsets` trims its input curves in place, as upstream (it takes
//!   them `&mut`). The raw offsets copy their base curve (upstream's `SmOffsetCurve`
//!   points at the base curve without owning it).
//! - **Curve caches.** Upstream keeps each curve's cache on the curve
//!   (`GetOrCreateObjectCache`) until the curve changes; `IntersectAndTrimOffsets`
//!   keeps each offset's cache across its offset pairs (dropped when the offset is
//!   trimmed, rebuilt when its handle changes) and each original curve's cache across
//!   the pieces it classifies, instead of building them per call. Performance only:
//!   the build is deterministic, so the solutions are the same.
//! - **Statuses ignored upstream.** `ReverseParameterization` of a reversed segment
//!   copy, the bow-tie check and repair, each `GlobalCurveIntersect` and
//!   `GlobalPropertyAnalysis` (a failed solve contributes no solutions: upstream would
//!   use those it found before failing), the in-place `Trim` of an overlapping
//!   offset end and the fallback corner constructions: the port ignores their errors
//!   too, except [`SolidError::Unported`], which is always propagated. The segment
//!   statuses of `SmCompositeCurve::ReadFromDB` are ignored upstream, leaving a
//!   segment with a `NULL` curve that every later use dereferences; the port returns
//!   [`SolidError::InvalidInput`] for a segment without a curve (no curve stored, or a
//!   curve that fails to read).
//! - **Undefined behaviour upstream.** These return [`SolidError::InvalidInput`]:
//!   `sm_CheckBowTies` without original curves (`crOrigCurves[0]`) or with fewer
//!   offsets than original curves (it reads `crOffsets[lNumOrig - 1]`; its caller
//!   ignores the status, so the port then goes on without bow-tie points);
//!   `CreateOffsetsOfManyCurves` with fewer directions than composites; a segment
//!   curve whose `Copy` fails (upstream offsets the `NULL` copy); an originator index
//!   outside the original curves; `MakeCompositeNurb` of a segment that is not an
//!   `SmBSplineCurve` (upstream joins a `NULL` curve); ASCII segment fields that cannot
//!   be read (upstream stores uninitialised values), a transition code outside
//!   `SmContinuityType`, and a segment count the remaining input cannot hold (upstream
//!   `calloc`s it, `SM_UNDEF_ULONG` segments when the count is unreadable).
//! - **`MakeCompositeNurb`** joins copies of the segments' B-spline data and writes
//!   the degree-raised B-spline segments back (upstream joins the segments in place,
//!   so a `const` method changes them); segments that are analytic curves derived from
//!   `SmBSplineCurve` (lines, ellipses, ...) join through a copy of their B-spline part
//!   and are left unchanged.
//! - `CreateOffsetsOfCurve`, `AddCurve`, `WriteToDB`, `Dump`, `Draw` and the region
//!   constructor belong to other units or are unused.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::{Curve, CurveKind, CurvePropertyType};
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve_cache::CurveCache;
use crate::curve::offset_curve::OffsetCurve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::attribute::{self, ObjectKey};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{ObjectKind, SolutionArray, SolutionObject, SolutionType};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

use super::p1::{
    convert_to_bspline_curves_indexed, create_corner, curve_classify_cached, insert_sorted_breaks,
    trim_first_break, trim_last_break, CompositeCurve, CompositeCurveSegment, OffsetCornerType,
    OffsetMapAttribute, OrigCurveCaches,
};

const OP_SEG_READ: &str = "curve::composite_curve::p2::CompositeCurveSegment::read_from_db";
const OP_READ: &str = "curve::composite_curve::p2::CompositeCurve::read_from_db";
const OP_BOW: &str = "curve::composite_curve::p2::check_bow_ties";
const OP_OFFSETS: &str =
    "curve::composite_curve::p2::CompositeCurve::create_offsets_of_many_curves";
const OP_TRIM: &str = "curve::composite_curve::p2::CompositeCurve::intersect_and_trim_offsets";
const OP_NURB: &str = "curve::composite_curve::p2::CompositeCurve::make_composite_nurb";

/// `SM_UNDEF_ULONG`: the segment count of a composite made for reading.
const UNDEF_ULONG: u64 = 123_456_789;

// ---- SmCompositeCurveSegment ----------------------------------------------------------

/// `SmContinuityType` of a stored transition code.
fn continuity_from_code(code: u64) -> Option<ContinuityType> {
    use ContinuityType as C;
    Some(match code {
        0 => C::Undefined,
        1 => C::Discontinuous,
        2 => C::C0,
        3 => C::G1,
        4 => C::G1R,
        5 => C::G1G2,
        6 => C::G1G2G3,
        7 => C::C1,
        8 => C::C1G2,
        9 => C::C1G2G3,
        10 => C::C1C2,
        11 => C::C1C2G3,
        12 => C::C1C2C3,
        13 => C::CInfinity,
        _ => return None,
    })
}

impl CompositeCurveSegment {
    /// `SmCompositeCurveSegment(pParentCurve, bSameSense, dGapStart, dGapEnd)`: a
    /// segment of `curve` with transition `SM_CT_C0`, not owning its curve.
    #[must_use]
    pub const fn new(curve: Curve, same_sense: bool, gap_start: f64, gap_end: f64) -> Self {
        Self {
            transition: ContinuityType::C0,
            same_sense,
            curve,
            gap_start,
            gap_end,
            owns_curve: false,
        }
    }

    /// `SmCompositeCurveSegment::ReadFromDB`: a segment (transition, sense, gaps and
    /// its curve, read through [`Curve::read_from_db`]) owning its curve.
    ///
    /// # Errors
    ///
    /// Those of the binary reads and of the curve's reader;
    /// [`SolidError::InvalidInput`] for an ASCII field that cannot be read, a
    /// transition code outside `SmContinuityType` and a segment without a curve
    /// (upstream stores `NULL`).
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let unreadable = || {
            SolidError::invalid_input(
                OP_SEG_READ,
                "unreadable segment field (upstream reads an uninitialised value)",
            )
        };

        // locals
        let transition_code: u64;
        let same_sense: i32;
        let mut gap_start = 0.0;
        let mut gap_end = 0.0;
        let has_curve: i32;

        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            let t = db.in_stream_mut().and_then(InputStream::extract_ulong);
            db.go_to_next_line();
            let s = db.in_stream_mut().and_then(InputStream::extract_int);
            db.go_to_next_line();
            if let Some(x) = db.in_stream_mut().and_then(InputStream::extract_double) {
                gap_start = x;
            }
            db.go_to_next_line();
            if let Some(x) = db.in_stream_mut().and_then(InputStream::extract_double) {
                gap_end = x;
            }
            db.go_to_next_line();
            let h = db.in_stream_mut().and_then(InputStream::extract_int);
            db.go_to_next_line();
            transition_code = t.ok_or_else(unreadable)?;
            same_sense = s.ok_or_else(unreadable)?;
            has_curve = h.ok_or_else(unreadable)?;
        } else {
            transition_code = db.read_ulong()?;
            same_sense = db.read_boolean()?;
            gap_start = db.read_double()?;
            gap_end = db.read_double()?;
            has_curve = db.read_boolean()?;
        }

        // contained curve
        if ascii {
            db.go_to_next_line();
        }
        if has_curve == 0 {
            return Err(SolidError::invalid_input(
                OP_SEG_READ,
                "segment without a curve (upstream stores NULL)",
            ));
        }
        let header = db.read_type(true)?;
        let parent_type = header.type_code.unwrap_or(0);
        let parent_dim = header.dim.unwrap_or(0);
        let curve = Curve::read_from_db(k, parent_type, db, parent_dim, version)?;

        // load object
        let transition = continuity_from_code(transition_code).ok_or_else(|| {
            SolidError::invalid_input(OP_SEG_READ, "transition code outside SmContinuityType")
        })?;
        Ok(Self {
            transition,
            same_sense: same_sense != 0,
            curve,
            gap_start,
            gap_end,
            owns_curve: true,
        })
    }
}

impl OffsetMapAttribute {
    /// `MakeCopy`: an equal attribute (the copy constructor).
    #[must_use]
    pub const fn make_copy(&self) -> Self {
        *self
    }
}

// ---- solution handles -----------------------------------------------------------------

/// The handle of a curve named by the offsetters' solutions: its index among the
/// curves of the call, whether it is the cache's analytic replacement, and its
/// natural interval at the time of the call.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PieceHandle(usize, bool, Extent1d);

impl SolutionObject for PieceHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.2)
    }
}

/// The cache of offset `index` from `caches` when it was built for `handle` (a
/// cache names its handle in its solutions), else `None` (a stale one is dropped).
fn take_cache(
    caches: &mut [Option<(PieceHandle, CurveCache<PieceHandle>)>],
    index: usize,
    handle: Option<PieceHandle>,
) -> Option<CurveCache<PieceHandle>> {
    let slot = 2 * index + handle.map_or(0, |h| usize::from(h.1));
    let (built_for, cache) = caches.get_mut(slot)?.take()?;
    (Some(built_for) == handle).then_some(cache)
}

/// Puts the cache of offset `index`, built for `handle`, back into `caches`.
fn put_cache(
    caches: &mut [Option<(PieceHandle, CurveCache<PieceHandle>)>],
    index: usize,
    handle: Option<PieceHandle>,
    cache: Option<CurveCache<PieceHandle>>,
) {
    if let (Some(h), Some(c)) = (handle, cache) {
        let slot = 2 * index + usize::from(h.1);
        if let Some(s) = caches.get_mut(slot) {
            *s = Some((h, c));
        }
    }
}

// ---- bow ties -------------------------------------------------------------------------

/// The distance of the closest point of `orig` to `point` (`GlobalPointSolve`
/// minimising over the natural interval within `tol`, best solution), if any.
fn closest_distance(
    k: &Kernel,
    orig: &Curve,
    index: usize,
    point: Point3,
    tol: f64,
) -> Result<Option<f64>> {
    let oi = orig.natural_interval();
    let opts = GlobalPointSolveOptions {
        handle: Some(PieceHandle(index, false, oi)),
        analytic_handle: Some(PieceHandle(index, true, oi)),
        ..GlobalPointSolveOptions::default()
    };
    let sols = orig.global_point_solve(
        k,
        &oi,
        SolverOperation::Minimize,
        point,
        tol,
        SolutionRequested::Single,
        &opts,
    )?;
    Ok(sols.get(0).map(|s| s.start.solution_value))
}

/// `sm_CheckBowTies`: when the original curves (taken as one chain) do not close
/// within `xsect_tol_3d`, adds to `points` the chain's start point if the start of
/// the first offset lies closer than `offset_distance * (1 - SM_EFF_ZERO)` to any
/// original curve, and likewise its end point for the end of offset
/// `orig_curves.len() - 1`. Such an offset end lies in the "forbidden zone", where an
/// intersection of the offsets is missed; [`fix_bow_tie`] adds the breaks for it.
/// `points` is cleared first; what was found before an error is kept.
///
/// # Errors
///
/// Those of the point solves; [`SolidError::InvalidInput`] without original curves
/// or with fewer offsets than original curves (upstream reads past the arrays);
/// [`SolidError::Unported`] from the end point evaluations (their other failures are
/// ignored, as upstream).
pub fn check_bow_ties(
    k: &Kernel,
    offsets: &[&Curve],
    offset_distance: f64,
    xsect_tol_3d: f64,
    orig_curves: &[&Curve],
    points: &mut Vec<Point3>,
) -> Result<()> {
    points.clear();

    let num_orig = orig_curves.len();

    // composite curve start and end points
    let (Some(start_curve), Some(end_curve)) = (orig_curves.first(), orig_curves.last()) else {
        return Err(SolidError::invalid_input(
            OP_BOW,
            "no original curves (upstream reads crOrigCurves[0])",
        ));
    };
    let start_ivl = start_curve.natural_interval();
    let end_ivl = end_curve.natural_interval();
    let start_pt = ignored(start_curve.evaluate_point(k, start_ivl.min()))?.unwrap_or(Vec3::UNDEF);
    let end_pt = ignored(end_curve.evaluate_point(k, end_ivl.max()))?.unwrap_or(Vec3::UNDEF);

    // no work - composite curve is closed
    if start_pt.distance(end_pt) < xsect_tol_3d {
        return Ok(());
    }

    // Tolerance for checking too close: see discussion in sm_CurveClassify().
    let cutoff = offset_distance * (1.0 - EFF_ZERO);

    // Check start point.
    let first = offsets.first().ok_or_else(|| {
        SolidError::invalid_input(OP_BOW, "no offsets (upstream reads crOffsets[0])")
    })?;
    let start_offset_pt =
        ignored(first.evaluate_point(k, first.natural_interval().min()))?.unwrap_or(Vec3::UNDEF);

    // for every original curve - check for curves closer than the cutoff to the point
    for (ii, orig) in orig_curves.iter().enumerate() {
        if let Some(d) = closest_distance(k, orig, ii, start_offset_pt, xsect_tol_3d)? {
            if d < cutoff {
                points.push(start_pt);
                break;
            }
        }
    }

    // Now check end point.
    let last = num_orig
        .checked_sub(1)
        .and_then(|i| offsets.get(i))
        .ok_or_else(|| {
            SolidError::invalid_input(
                OP_BOW,
                "fewer offsets than original curves (upstream reads past crOffsets)",
            )
        })?;
    let end_offset_pt =
        ignored(last.evaluate_point(k, last.natural_interval().max()))?.unwrap_or(Vec3::UNDEF);

    for (ii, orig) in orig_curves.iter().enumerate() {
        if let Some(d) = closest_distance(k, orig, ii, end_offset_pt, xsect_tol_3d)? {
            if d < cutoff {
                points.push(end_pt);
                break;
            }
        }
    }
    Ok(())
}

/// `sm_FixBowTie`: inserts into the sorted `breaks` (with
/// [`insert_sorted_breaks`]) every parameter of `offset_curve` at distance
/// `offset_distance` from one of `points` (`GlobalPropertyAnalysis` of
/// `SM_CP_DISTANCE_TO_POINT` within `SM_EFF_ZERO`, status ignored).
///
/// # Errors
///
/// [`SolidError::Unported`] from the property analyses.
pub fn fix_bow_tie(
    k: &Kernel,
    offset_curve: &Curve,
    points: &[Point3],
    offset_distance: f64,
    breaks: &mut Vec<f64>,
) -> Result<()> {
    // no work - no test points
    if points.is_empty() {
        return Ok(());
    }

    let ivl = offset_curve.natural_interval();

    // for every test point - find curve locations a fixed distance to the point
    for p in points {
        let vectors = [*p];
        let opts = PropertyAnalysisOptions {
            property_value: Some(offset_distance),
            opt_vectors: &vectors,
            want_comb: false,
            handle: Some(PieceHandle(0, false, ivl)),
        };
        let sols = ignored(offset_curve.global_property_analysis(
            k,
            &ivl,
            CurvePropertyType::DistanceToPoint,
            EFF_ZERO,
            &opts,
        ))?
        .map_or_else(SolutionArray::new, |a| a.solutions);

        // for every solution
        for jj in 0..sols.len() {
            if let Some(s) = sols.get(jj) {
                insert_sorted_breaks(breaks, s.start.parameters[0]);
            }
        }
    }
    Ok(())
}

// ---- offsetting -----------------------------------------------------------------------

/// A curve of the offsetters with its offset map attribute (`SM_AI_OFFSETMAP`), if
/// any.
#[derive(Debug, Clone)]
pub struct OffsetPiece {
    /// The curve.
    pub curve: Curve,
    /// Its `SmOffsetMapAttribute`.
    pub attribute: Option<OffsetMapAttribute>,
}

impl OffsetPiece {
    /// The piece copied ([`Curve::copy`], the attribute through
    /// [`OffsetMapAttribute::make_copy`]).
    fn copy(&self, k: &Kernel) -> Result<Self> {
        Ok(Self {
            curve: self.curve.copy(k)?,
            attribute: self.attribute.as_ref().map(OffsetMapAttribute::make_copy),
        })
    }
}

/// The side to offset to, seen from the positive side of the offset plane
/// (`SmOffsetDirectionType`); the discriminant is the upstream value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetDirection {
    /// `SM_OD_RIGHT_HAND_SIDE`.
    RightHandSide = 0,
    /// `SM_OD_LEFT_HAND_SIDE`.
    LeftHandSide = 1,
    /// `SM_OD_BOTH_SIDES`.
    BothSides = 2,
    /// `SM_OD_UNKNOWN`.
    Unknown = 3,
}

/// The settings of [`CompositeCurve::create_offsets_of_many_curves`] and
/// [`CompositeCurve::create_trimmed_offsets`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetParams {
    /// `dXSectTol3d`: offset end gaps at least this wide are filled with corner
    /// curves; the intersection, classification and short-piece tolerance.
    pub xsect_tol_3d: f64,
    /// `dApproxTol3d`: tolerance of the B-spline approximations of the offsets.
    pub approx_tol_3d: f64,
    /// `crOffsetPlaneNormal`: with the curve direction, defines right and left.
    pub plane_normal: Vec3,
    /// `eOffsetCornerType`: how corner gaps are filled.
    pub corner: OffsetCornerType,
    /// `bTrimResults`: intersect the raw offsets and trim them back (and drop
    /// degenerate raw offsets).
    pub trim_results: bool,
    /// `dOffsetDistance`: a negative distance swaps right and left.
    pub offset_distance: f64,
    /// `bOptMatchParameterization` of the approximations (upstream default `FALSE`).
    pub match_parameterization: bool,
}

/// Result of [`CompositeCurve::create_offsets_of_many_curves`].
#[derive(Debug, Clone)]
pub struct TrimmedOffsets {
    /// `rTrimmedOffsets`: the B-spline approximations (`SmBSplineCurve` or an analytic
    /// curve derived from it) with the attributes of the curves they approximate.
    pub offsets: Vec<OffsetPiece>,
    /// `pOptMaxGaps3d`: the approximation error of each.
    pub max_gaps: Vec<f64>,
}

/// The offset map attribute of a corner of composite `comp` between its original
/// curves `curve` and `other`.
fn corner_attribute(comp: usize, curve: usize, other: usize) -> OffsetMapAttribute {
    OffsetMapAttribute::new(
        attribute::ids::OFFSETMAP,
        true,
        false,
        comp as u64,
        curve as u64,
        other as u64,
        None,
        0.0,
    )
}

/// The ends of the original curves meeting at a corner of composite `comp`.
struct CornerSite<'a> {
    prev: &'a Curve,
    prev_ivl: Extent1d,
    this: &'a Curve,
    this_ivl: Extent1d,
    /// The previous curve's end (the centre of fillet corners).
    corner: Point3,
    comp: usize,
    this_idx: usize,
    prev_idx: usize,
}

/// The corner fill of one side (`signed_distance`) of a corner: when the offsets'
/// ends are at least `xsect_tol_3d` apart, the corner curves of `sm_CreateCorner`
/// that are not degenerate (shorter than the tolerance), or when all are, a polyline
/// through the extensions' meeting point (a line between the ends when the
/// extensions do not meet), appended to `offsets`.
fn add_corner(
    k: &Kernel,
    site: &CornerSite<'_>,
    p: &OffsetParams,
    distance: f64,
    signed_distance: f64,
    offsets: &mut Vec<OffsetPiece>,
) -> Result<()> {
    let tol = p.xsect_tol_3d;

    // build offset curves again, just in case, because degenerate offset curves were not saved
    let prev_off = Curve::from(OffsetCurve::new(
        3,
        site.prev.clone(),
        site.prev_ivl,
        p.plane_normal,
        signed_distance,
        false,
    )?);
    let curr_off = Curve::from(OffsetCurve::new(
        3,
        site.this.clone(),
        site.this_ivl,
        p.plane_normal,
        signed_distance,
        false,
    )?);

    // get offset curveEnd positions and tangents
    let pv1 = prev_off.evaluate(k, site.prev_ivl.max(), 1, true, true)?;
    let pv2 = curr_off.evaluate(k, site.this_ivl.min(), 1, true, true)?;
    let p1 = pv1.first().copied().unwrap_or(Vec3::UNDEF);
    let v1 = pv1.get(1).copied().unwrap_or(Vec3::UNDEF);
    let p2 = pv2.first().copied().unwrap_or(Vec3::UNDEF);
    let v2 = pv2.get(1).copied().unwrap_or(Vec3::UNDEF);

    // when the distance between end points is less than tolerance
    if p1.distance(p2) < tol {
        return Ok(());
    }

    // build a corner curve(s) to fill in the corner gap
    let cc = create_corner(k, site.corner, p1, v1, p2, v2, p.corner, distance, tol)?;
    let degenerate = |c: &Option<Curve>| -> Result<bool> {
        match c {
            None => Ok(true),
            Some(c) => Ok(c.approximate_length(k, &c.natural_interval(), 3, false)?.0 < tol),
        }
    };
    let c1_degenerate = degenerate(&cc.curve1)?;
    let c2_degenerate = degenerate(&cc.curve2)?;
    let c3_degenerate = degenerate(&cc.curve3)?;

    // If no usable results, try to connect points with a simple curve [B632]
    if c1_degenerate && c2_degenerate && c3_degenerate {
        // find linear extension xsect point - handle zero length tangents
        let mut params = None;
        if v1.length_squared() > EFF_ZERO_SQ && v2.length_squared() > EFF_ZERO_SQ {
            params = ignored(line_line_closest_point(p1, v1, p2, v2))?;
        }

        let bsc = if let Some((t1, t2)) = params {
            let end1 = p1 + t1 * v1;
            let end2 = p2 + t2 * v2;
            let x_pnt = (end1 + end2) / 2.0;
            let cps = [p1, x_pnt, p2];
            let kms = [2, 1, 2];
            let kns = [0.0, 0.5, 1.0];
            ignored(BSplineCurve::create_canonical(&CanonicalData {
                dim: 3,
                degree: 1,
                control_points: &cps,
                form: BSplineCurveForm::PolylineForm,
                knot_multiplicities: &kms,
                knots: &kns,
                knot_type: KnotType::UniformKnots,
                weights: None,
                trim: None,
            }))?
        } else {
            ignored(BSplineCurve::create_line_segment(3, p1, p2))?
        };

        if let Some(b) = bsc {
            // remember the parent curve in an attribute and add the curve to the offsets
            offsets.push(OffsetPiece {
                curve: Curve::from(b),
                attribute: Some(corner_attribute(site.comp, site.this_idx, site.prev_idx)),
            });
        }
    }

    // If pCorner1 is good, use it
    if let (false, Some(c)) = (c1_degenerate, cc.curve1) {
        offsets.push(OffsetPiece {
            curve: c,
            attribute: Some(corner_attribute(site.comp, site.prev_idx, site.this_idx)),
        });
    }

    // when given pCorner2
    if let (false, Some(c)) = (c2_degenerate, cc.curve2) {
        offsets.push(OffsetPiece {
            curve: c,
            attribute: Some(corner_attribute(site.comp, site.this_idx, site.prev_idx)),
        });
    }

    // when given pCorner3
    if let (false, Some(c)) = (c3_degenerate, cc.curve3) {
        offsets.push(OffsetPiece {
            curve: c,
            attribute: Some(corner_attribute(site.comp, site.this_idx, site.prev_idx)),
        });
    }
    Ok(())
}

/// The raw offset of `base` over `ivl` at `signed_distance` with its attribute, or
/// `None` when it is degenerate and degenerate offsets are discarded.
#[allow(clippy::too_many_arguments)]
fn raw_offset(
    k: &Kernel,
    base: &Curve,
    ivl: Extent1d,
    p: &OffsetParams,
    signed_distance: f64,
    left: bool,
    comp: usize,
    seg: usize,
    originator: ObjectRef,
) -> Result<Option<OffsetPiece>> {
    let off = Curve::from(OffsetCurve::new(
        3,
        base.clone(),
        ivl,
        p.plane_normal,
        signed_distance,
        false,
    )?);

    // discard degenerate offsets
    if off.is_degenerate(k, EFF_ZERO, None)? && p.trim_results {
        return Ok(None);
    }

    // tag the offset curve with a OffsetMapAttribute so that the offset remembers its parent curve.
    Ok(Some(OffsetPiece {
        curve: off,
        attribute: Some(OffsetMapAttribute::new(
            attribute::ids::OFFSETMAP,
            false,
            left,
            comp as u64,
            seg as u64,
            0,
            Some(originator),
            0.0,
        )),
    }))
}

impl CompositeCurve {
    /// `SmCompositeCurve::ReadFromDB`: a composite of dimension `dim` with the stored
    /// segment count, closed flag and segments ([`CompositeCurveSegment::read_from_db`]).
    ///
    /// # Errors
    ///
    /// Those of the binary reads; [`SolidError::InvalidInput`] for a segment count the
    /// remaining input cannot hold (`SM_UNDEF_ULONG` when unreadable) and a segment
    /// that fails to read (upstream leaves a `NULL` segment curve);
    /// [`SolidError::Unported`] from the segment curves' readers.
    pub fn read_from_db(
        k: &Kernel,
        db: &mut DatabaseReader,
        dim: u64,
        version: u64,
    ) -> Result<Self> {
        // init output object
        let mut num_segments = UNDEF_ULONG;
        let mut closed = 0;

        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            if let Some(n) = db.in_stream_mut().and_then(InputStream::extract_ulong) {
                num_segments = n;
            }
            db.go_to_next_line();
            if let Some(c) = db.in_stream_mut().and_then(InputStream::extract_int) {
                closed = c;
            }
            db.go_to_next_line();
        } else {
            num_segments = db.read_ulong()?;
            closed = db.read_boolean()?;
        }

        // Bound the segment count (a segment takes at least 32 binary bytes, or one
        // ASCII character per field). Upstream preallocates the array; we grow it as
        // segments are read, since a segment is far larger in memory than on disk.
        let n = usize::try_from(num_segments).map_err(|_| {
            SolidError::invalid_input(OP_READ, "segment count exceeds the remaining input")
        })?;
        db.ensure_available(n, if ascii { 3 } else { 32 }, OP_READ)?;
        let mut segments = Vec::new();

        // for every CompositeCurveSegment
        for _ in 0..n {
            match CompositeCurveSegment::read_from_db(k, db, version) {
                Ok(s) => segments.push(s),
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    return Err(SolidError::invalid_input(
                        OP_READ,
                        "segment failed to read (upstream leaves a NULL segment curve)",
                    ))
                }
            }
        }

        Ok(Self {
            dim,
            segments,
            closed: closed != 0,
        })
    }

    /// `MakeCompositeNurb`: one B-spline joining the segments in order with their
    /// senses ([`BSplineCurve::create_by_joining`]); B-spline segments are brought to
    /// the common degree and dimension in place, as upstream (see the module docs).
    ///
    /// # Errors
    ///
    /// Those of the join; [`SolidError::InvalidInput`] for a segment that is not an
    /// `SmBSplineCurve` (upstream joins a `NULL` curve).
    pub fn make_composite_nurb(&mut self) -> Result<BSplineCurve> {
        // get arrays of composite curve segments and their senses
        let mut bsplines = Vec::with_capacity(self.segments.len());
        let mut senses = Vec::with_capacity(self.segments.len());
        for s in &self.segments {
            let b = match s.curve.kind() {
                CurveKind::BSpline(b) => b.copy(),
                CurveKind::Line(l) => l.base_with_nurb()?.into_owned(),
                CurveKind::Ellipse(e) => e.base().copy(),
                CurveKind::Hyperbola(h) => h.base().copy(),
                CurveKind::Parabola(p) => p.base().copy(),
                _ => {
                    return Err(SolidError::invalid_input(
                        OP_NURB,
                        "segment is not an SmBSplineCurve (upstream joins NULL)",
                    ))
                }
            };
            bsplines.push(b);
            senses.push(s.same_sense);
        }

        // create the joined curve
        let joined = BSplineCurve::create_by_joining(&mut bsplines, Some(&senses))?;

        // the join raised the B-spline segments in place
        for (s, b) in self.segments.iter_mut().zip(bsplines) {
            if let CurveKind::BSpline(x) = s.curve.kind_mut() {
                *x = b;
            }
        }
        Ok(joined)
    }

    /// `CreateTrimmedOffsets`: [`Self::create_offsets_of_many_curves`] of this curve
    /// alone, to `direction` (a negative distance is made positive with right and
    /// left swapped; [`OffsetDirection::Unknown`] offsets to both sides).
    ///
    /// # Errors
    ///
    /// Those of [`Self::create_offsets_of_many_curves`].
    pub fn create_trimmed_offsets(
        &self,
        k: &Kernel,
        direction: OffsetDirection,
        params: &OffsetParams,
    ) -> Result<Vec<OffsetPiece>> {
        let mut p = *params;
        let mut direction = direction;

        // Offset distance must be positive.
        if p.offset_distance < 0.0 {
            p.offset_distance = -p.offset_distance;
            if direction == OffsetDirection::LeftHandSide {
                direction = OffsetDirection::RightHandSide;
            } else if direction == OffsetDirection::RightHandSide {
                direction = OffsetDirection::LeftHandSide;
            }
        }

        let side = match direction {
            OffsetDirection::LeftHandSide => 1,
            OffsetDirection::RightHandSide => 2,
            OffsetDirection::BothSides | OffsetDirection::Unknown => 3,
        };

        // pass the call along
        Ok(Self::create_offsets_of_many_curves(k, &[self], &[side], &p)?.offsets)
    }

    /// `CreateOffsetsOfManyCurves`: the simultaneous offset of `curves` at
    /// `params.offset_distance`, curve `i` to side `directions[i]` (`1` left, `2`
    /// right, `3` both; a negative distance swaps left and right).
    ///
    /// Each segment curve is copied with a positive orientation and offset
    /// ([`OffsetCurve`] in the plane with normal `params.plane_normal`); with
    /// `trim_results`, degenerate raw offsets are dropped. A composite is closed when
    /// it says so or its ends meet within `SM_EFF_ZERO` (scaled). At every corner
    /// (and between the last and first curves of a closed composite) turning by more
    /// than `SM_EFF_ZERO_SQRT` and less than `pi - SM_EFF_ZERO_SQRT` towards the offset
    /// side, the gap between the offsets' ends is filled with corner curves
    /// (`sm_CreateCorner`). With `trim_results` the offsets are intersected, split and
    /// classified ([`Self::intersect_and_trim_offsets`]). Every curve is finally
    /// approximated by a B-spline within `approx_tol_3d` (failed approximations are
    /// dropped). Each output carries the offset map attribute of what it came from.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a direction other than `1`, `2`, `3` and when no offset is made;
    /// [`SolidError::InvalidInput`] for fewer directions than curves; those of the
    /// segment copies, the end evaluations and angles, the corner construction and
    /// [`Self::intersect_and_trim_offsets`]; [`SolidError::Unported`] from unported
    /// callees.
    #[allow(clippy::too_many_lines)]
    pub fn create_offsets_of_many_curves(
        k: &Kernel,
        curves: &[&Self],
        directions: &[u64],
        params: &OffsetParams,
    ) -> Result<TrimmedOffsets> {
        let mut p = *params;

        // source change (source fork 54148db): corners that keep the curves'
        // curvature are made by CreateTaperExtrude only so far; here they would get no
        // corner curves at all.
        if p.corner == OffsetCornerType::CurveExtension {
            return Err(SolidError::invalid_input(
                OP_OFFSETS,
                "SM_OC_CURVE_EXTENSION is for CreateTaperExtrude only",
            ));
        }

        // Offset distance must be positive.
        let mut flip = false;
        if p.offset_distance < 0.0 {
            p.offset_distance = -p.offset_distance;
            flip = true;
        }
        let distance = p.offset_distance;

        // locals
        let mut total_orig: Vec<Curve> = Vec::new();
        let mut offsets: Vec<OffsetPiece> = Vec::new();
        let mut closed_curves: Vec<bool> = Vec::with_capacity(curves.len());

        for (ii, comp) in curves.iter().enumerate() {
            let mut orig: Vec<Curve> = Vec::with_capacity(comp.segments.len());
            let mut closed = false;
            let base_index = total_orig.len();

            // eOffsetDirection: Don't forget negative offset distances remembered in bFlip value.
            let dir = *directions.get(ii).ok_or_else(|| {
                SolidError::invalid_input(OP_OFFSETS, "fewer directions than curves")
            })?;
            let side = match dir {
                1 if flip => OffsetDirection::RightHandSide,
                1 => OffsetDirection::LeftHandSide,
                2 if flip => OffsetDirection::LeftHandSide,
                2 => OffsetDirection::RightHandSide,
                3 => OffsetDirection::BothSides,
                _ => return Err(SolidError::sm_err(OP_OFFSETS)),
            };
            let right = matches!(
                side,
                OffsetDirection::RightHandSide | OffsetDirection::BothSides
            );
            let left = matches!(
                side,
                OffsetDirection::LeftHandSide | OffsetDirection::BothSides
            );

            // for every segment of this Comp Curve.
            let n = comp.segments.len();
            let mut start_pt = Vec3::UNDEF;
            for (jj, seg) in comp.segments.iter().enumerate() {
                // copy of segment base curve with a positive parameter direction
                // (upstream ignores the copy's status and uses the NULL copy)
                let mut parent_copy = seg.curve.copy(k)?;
                let mut ivl = parent_copy.natural_interval();
                if !seg.same_sense {
                    if let Some(new_ivl) = ignored(parent_copy.reverse_parameterization(k, &ivl))? {
                        ivl = new_ivl;
                    }
                }

                // Check for closed loop: start point of 1st curve, end point of last.
                if jj == 0 {
                    start_pt = parent_copy.evaluate_point(k, ivl.min())?;
                }
                if jj + 1 == n {
                    let end_pt = parent_copy.evaluate_point(k, ivl.max())?;

                    // when distance between start/end points is less than scaled tolerance
                    let scale = 1.0 + start_pt.max_dimension();
                    if start_pt.distance(end_pt) < EFF_ZERO * scale {
                        closed = true;
                    }

                    // when composite curve knows it's a closed curve
                    if comp.closed {
                        closed = true;
                    }
                }

                let originator = ObjectRef {
                    type_code: parent_copy.type_code(),
                    key: ObjectKey((base_index + jj) as u64),
                };

                // Offset to the Right if appropriate.
                if right {
                    if let Some(o) = raw_offset(
                        k,
                        &parent_copy,
                        ivl,
                        &p,
                        distance,
                        false,
                        ii,
                        jj,
                        originator,
                    )? {
                        offsets.push(o);
                    }
                }

                // Offset to the Left if appropriate.
                if left {
                    if let Some(o) = raw_offset(
                        k,
                        &parent_copy,
                        ivl,
                        &p,
                        -distance,
                        true,
                        ii,
                        jj,
                        originator,
                    )? {
                        offsets.push(o);
                    }
                }

                orig.push(parent_copy);
            }
            closed_curves.push(closed);

            // Now Generate Corners for concave cases
            let m = orig.len();
            for jj in 0..m {
                // Previous curve index
                let prev_idx = if jj > 0 {
                    jj - 1
                } else if closed {
                    m - 1
                } else {
                    // skip beginning of un-closed composite curve corner.
                    continue;
                };

                // orig curves and CurveEndTangents making up this corner
                let (Some(prev), Some(this)) = (orig.get(prev_idx), orig.get(jj)) else {
                    continue;
                };
                let prev_ivl = prev.natural_interval();
                let this_ivl = this.natural_interval();
                let pv1 = prev.evaluate(k, prev_ivl.max(), 1, true, true)?;
                let pv2 = this.evaluate(k, this_ivl.min(), 1, true, true)?;
                let d1 = pv1.get(1).copied().unwrap_or(Vec3::UNDEF);
                let d2 = pv2.get(1).copied().unwrap_or(Vec3::UNDEF);

                // cornerPoint = PrevCrv endPoint (the center of a circular arc fillet corner)
                let site = CornerSite {
                    prev,
                    prev_ivl,
                    this,
                    this_ivl,
                    corner: pv1.first().copied().unwrap_or(Vec3::UNDEF),
                    comp: ii,
                    this_idx: jj,
                    prev_idx,
                };

                // Check the angle between the two orig curves where they meet.
                let angle = p.plane_normal.ccw_angle_between(d1, d2)?;

                // Generate a right corner when the angle is non zero up to just under 180 degrees
                if angle > EFF_ZERO_SQRT && angle < PI - EFF_ZERO_SQRT && right {
                    add_corner(k, &site, &p, distance, distance, &mut offsets)?;
                }

                // Generate a left corner when the angle is non zero down to just over -180 degrees
                if angle < -EFF_ZERO_SQRT && angle > -PI + EFF_ZERO_SQRT && left {
                    add_corner(k, &site, &p, distance, -distance, &mut offsets)?;
                }
            }

            // append sOrigCurve array to sTotalOrigCurves array
            total_orig.extend(orig);
        }

        if offsets.is_empty() {
            return Err(SolidError::sm_err(OP_OFFSETS));
        }

        // If we are not trimming then we are done here.
        if p.trim_results {
            let mut untrimmed = offsets;
            let orig_refs: Vec<&Curve> = total_orig.iter().collect();
            offsets = Self::intersect_and_trim_offsets(
                k,
                &mut untrimmed,
                distance,
                p.xsect_tol_3d,
                &orig_refs,
            )?;
        }

        // Convert the offset curves to SmBSplineCurves for output.
        let refs: Vec<&Curve> = offsets.iter().map(|o| &o.curve).collect();
        let (approx, max_gaps, sources) = convert_to_bspline_curves_indexed(
            k,
            &refs,
            p.approx_tol_3d,
            p.match_parameterization,
            false,
        )?;

        // Transfer the OffsetMap attributes
        let out = approx
            .into_iter()
            .zip(sources)
            .map(|(curve, src)| OffsetPiece {
                curve,
                attribute: offsets.get(src).and_then(|o| o.attribute),
            })
            .collect();
        Ok(TrimmedOffsets {
            offsets: out,
            max_gaps,
        })
    }

    /// `IntersectAndTrimOffsets`: the pieces of the raw `offsets` that lie no closer
    /// than `offset_distance` to `orig_curves`.
    ///
    /// Every offset is intersected with itself and every later offset
    /// (`GlobalCurveIntersect` within `xsect_tol_3d / 1000`, parameters clamped to the
    /// intervals). A coincident range covering an end of both curves trims the
    /// current offset in place (and its break list); other solutions add their
    /// parameters to both curves' sorted break lists. Bow-tie breaks
    /// ([`check_bow_ties`], [`fix_bow_tie`]) are added, then every offset is split at
    /// its breaks (spans under `SM_EFF_ZERO_SQRT` and pieces shorter than
    /// `xsect_tol_3d / 100` dropped) and each piece is kept when
    /// [`curve_classify`](super::p1::curve_classify) says so (with its attribute's originator: the attribute's
    /// [`ObjectRef`] key indexes `orig_curves`).
    ///
    /// # Errors
    ///
    /// Those of the copies, trims, length estimates and classification;
    /// [`SolidError::InvalidInput`] for an originator index outside `orig_curves`;
    /// [`SolidError::Unported`] from unported callees.
    pub fn intersect_and_trim_offsets(
        k: &Kernel,
        offsets: &mut [OffsetPiece],
        offset_distance: f64,
        xsect_tol_3d: f64,
        orig_curves: &[&Curve],
    ) -> Result<Vec<OffsetPiece>> {
        // First, check for 'incomplete bow-ties' that can exist at the end of a curve.
        let mut points: Vec<Point3> = Vec::new();
        {
            let refs: Vec<&Curve> = offsets.iter().map(|o| &o.curve).collect();
            // (status ignored upstream)
            let _ = ignored(check_bow_ties(
                k,
                &refs,
                offset_distance,
                xsect_tol_3d,
                orig_curves,
                &mut points,
            ))?;
        }

        // locals
        let num_offs = offsets.len();
        let mut trimmed: Vec<OffsetPiece> = Vec::new();
        let mut breaks: Vec<Vec<f64>> = vec![Vec::new(); num_offs];
        // every offset's curve cache with the handle it was built for (performance
        // only: upstream keeps the cache on the curve until the curve changes; see
        // `Curve::global_curve_intersect_cached`)
        // Two slots per offset: one for the analytic handle and one for the
        // non-analytic handle (upstream keeps both on the curve).
        let mut caches: Vec<Option<(PieceHandle, CurveCache<PieceHandle>)>> =
            (0..2 * num_offs).map(|_| None).collect();

        // Intersect every offset curve with every offset curve, including itself.
        for ii in 0..num_offs {
            let mut curr_ivl = offsets
                .get(ii)
                .map(|o| o.curve.natural_interval())
                .unwrap_or_default();

            // build list of all intersections of pCurr with all other offset curves
            if let Some(b) = breaks.get_mut(ii) {
                insert_sorted_breaks(b, curr_ivl.min());
                insert_sorted_breaks(b, curr_ivl.max());
            }

            for jj in ii..num_offs {
                // Find all intersections between this pair of curves (possibly the same curve).
                let (Some(curr), Some(other)) = (offsets.get(ii), offsets.get(jj)) else {
                    continue;
                };
                let other_ivl = other.curve.natural_interval();
                let opts = CurveIntersectOptions {
                    handles: [
                        Some(PieceHandle(ii, false, curr_ivl)),
                        Some(PieceHandle(jj, false, other_ivl)),
                    ],
                    analytic_handles: [
                        Some(PieceHandle(ii, true, curr_ivl)),
                        Some(PieceHandle(jj, true, other_ivl)),
                    ],
                    ..CurveIntersectOptions::default()
                };
                // the slots of the two caches (built for these handles, or empty)
                let mut slot1 = take_cache(&mut caches, ii, opts.handles[0]);
                let mut slot2 = if jj == ii {
                    None
                } else {
                    take_cache(&mut caches, jj, opts.handles[1])
                };
                // (status ignored upstream)
                let result = curr.curve.global_curve_intersect_cached(
                    k,
                    &curr_ivl,
                    &other.curve,
                    &other_ivl,
                    xsect_tol_3d / 1000.0,
                    &opts,
                    &mut slot1,
                    &mut slot2,
                );
                put_cache(&mut caches, ii, opts.handles[0], slot1);
                if jj != ii {
                    put_cache(&mut caches, jj, opts.handles[1], slot2);
                }
                let sols = ignored(result)?.unwrap_or_else(SolutionArray::new);

                // Record each intersection parameter value in the breaks in an ascending order
                for kk in 0..sols.len() {
                    let Some(mut sol) = sols.get(kk).copied() else {
                        continue;
                    };

                    // clamp parameters slightly outside the domains [B386]
                    sol.start.parameters[0] = curr_ivl.clamp_value(sol.start.parameters[0]);
                    sol.start.parameters[1] = other_ivl.clamp_value(sol.start.parameters[1]);

                    // If range of values at interval ends - remove one segment from the
                    // intervals: with overlapping end curves we only need to keep one [B127]
                    let mut done_with_sol = false;
                    if sol.solution_type == SolutionType::RangeOfValues {
                        sol.end.parameters[0] = curr_ivl.clamp_value(sol.end.parameters[0]);
                        sol.end.parameters[1] = other_ivl.clamp_value(sol.end.parameters[1]);

                        // classify the ends
                        let curr_start = curr_ivl.classify_point(sol.start.parameters[0], EFF_ZERO);
                        let curr_end = curr_ivl.classify_point(sol.end.parameters[0], EFF_ZERO);
                        let other_start =
                            other_ivl.classify_point(sol.start.parameters[1], EFF_ZERO);
                        let other_end = other_ivl.classify_point(sol.end.parameters[1], EFF_ZERO);

                        // if intervals are overlapping end
                        let inside = ExtentPointType::Inside;
                        if (curr_start != inside || curr_end != inside)
                            && (other_start != inside || other_end != inside)
                        {
                            // trim the overlap off the break point array
                            let mut trim_ivl = Extent1d::new();
                            if let Some(b) = breaks.get_mut(ii) {
                                if curr_start == ExtentPointType::Start {
                                    trim_first_break(b, sol.end.parameters[0]);
                                    let _ =
                                        trim_ivl.set_min_max(sol.end.parameters[0], curr_ivl.max());
                                } else {
                                    trim_last_break(b, sol.start.parameters[0]);
                                    let _ = trim_ivl
                                        .set_min_max(curr_ivl.min(), sol.start.parameters[0]);
                                }
                            }
                            // may snap the interval by tol to existing knots (status ignored)
                            if let Some(c) = offsets.get_mut(ii) {
                                // the curve changes: its cache is rebuilt
                                if let Some(slot) = caches.get_mut(ii) {
                                    *slot = None;
                                }
                                let _ = ignored(c.curve.trim(k, &mut trim_ivl, TrimMode::Notify))?;
                            }
                            curr_ivl = trim_ivl;
                            done_with_sol = true;
                        }
                    }

                    // point solutions and nonOverlappingEnd range solutions
                    if !done_with_sol {
                        if let Some(b) = breaks.get_mut(ii) {
                            insert_sorted_breaks(b, sol.start.parameters[0]);
                        }
                        if let Some(b) = breaks.get_mut(jj) {
                            insert_sorted_breaks(b, sol.start.parameters[1]);
                        }

                        // If range of values not at interval ends - add those points as well
                        if sol.solution_type == SolutionType::RangeOfValues {
                            if let Some(b) = breaks.get_mut(ii) {
                                insert_sorted_breaks(b, sol.end.parameters[0]);
                            }
                            if let Some(b) = breaks.get_mut(jj) {
                                insert_sorted_breaks(b, sol.end.parameters[1]);
                            }
                        }
                    }
                }
            }

            let (Some(curr), Some(this_breaks)) = (offsets.get(ii), breaks.get_mut(ii)) else {
                continue;
            };

            // Before we classify, check for 'incomplete bow-ties' (status ignored upstream).
            let _ = ignored(fix_bow_tie(
                k,
                &curr.curve,
                &points,
                offset_distance,
                this_breaks,
            ))?;

            // copy and trim pCurr into intervals for every pair of break points
            for w in this_breaks.windows(2) {
                let (Some(&b0), Some(&b1)) = (w.first(), w.last()) else {
                    continue;
                };

                // Filter out really small spans.
                if b1 - b0 < EFF_ZERO_SQRT {
                    continue;
                }

                // Copy and trim the curve.
                let mut piece = curr.copy(k)?;
                let mut trim_ivl = Extent1d::from_min_max(b0, b1);
                piece.curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;

                // Filter out really small curves.
                let length = piece.curve.approximate_length(k, &trim_ivl, 5, false)?.0;
                if length < xsect_tol_3d / 100.0 {
                    drop(piece);
                } else {
                    trimmed.push(piece);
                }
            }
        }

        // classify each trimmed offset to see which are kept
        let mut classified: Vec<OffsetPiece> = Vec::new();
        // the original curves' caches, kept across the pieces (performance only)
        let mut orig_caches = OrigCurveCaches::default();
        for piece in &trimmed {
            let originator = match piece.attribute.and_then(|a| a.originator) {
                Some(r) => Some(
                    usize::try_from(r.key.0)
                        .ok()
                        .and_then(|i| orig_curves.get(i))
                        .copied()
                        .ok_or_else(|| {
                            SolidError::invalid_input(
                                OP_TRIM,
                                "originator outside the original curves",
                            )
                        })?,
                ),
                None => None,
            };

            // see if it's closer than dOffsetDistance to any of the OriginalCurves
            let keep = curve_classify_cached(
                k,
                &piece.curve,
                offset_distance,
                xsect_tol_3d,
                orig_curves,
                originator,
                &mut orig_caches,
            )?;

            // place a copy of the keep intervals into the output
            if keep {
                classified.push(piece.copy(k)?);
            }
        }
        Ok(classified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::database_io::DatabaseReader;

    fn line(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(
            BSplineCurve::create_line_segment(3, Vec3::from_array(a), Vec3::from_array(b)).unwrap(),
        )
    }

    fn square() -> CompositeCurve {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let curves = (0..4)
            .map(|i| line(pts[i], pts[(i + 1) % 4]))
            .collect::<Vec<_>>();
        CompositeCurve::with_curves(3, curves, true, None, None).unwrap()
    }

    fn ascii(text: &str) -> DatabaseReader {
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        db
    }

    #[test]
    fn segment_new_defaults() {
        let s = CompositeCurveSegment::new(line([0.0; 3], [1.0, 0.0, 0.0]), false, 1.0, 2.0);
        assert_eq!(s.transition, ContinuityType::C0);
        assert!(!s.same_sense);
        assert!(!s.owns_curve);
        assert_eq!((s.gap_start, s.gap_end), (1.0, 2.0));
    }

    #[test]
    fn make_copy_is_equal() {
        let a = corner_attribute(1, 2, 3);
        assert_eq!(a.make_copy(), a);
        assert!(a.corner_offset);
    }

    #[test]
    fn continuity_codes_round_trip() {
        for c in 0..=13u64 {
            assert_eq!(continuity_from_code(c).map(|t| t as u64), Some(c));
        }
        assert!(continuity_from_code(14).is_none());
    }

    #[test]
    fn make_composite_nurb_joins_square() {
        let mut sq = square();
        let j = sq.make_composite_nurb().unwrap();
        let ivl = j.natural_interval();
        let p0 = j.evaluate_point(ivl.min()).unwrap();
        let p1 = j.evaluate_point(ivl.max()).unwrap();
        assert!(p0.distance(p1) < 1e-12);
    }

    #[test]
    fn make_composite_nurb_rejects_hermite() {
        let h = Curve::from(crate::curve::hermite_curve::HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        ));
        let mut c = CompositeCurve::with_curves(3, vec![h], false, None, None).unwrap();
        assert!(matches!(
            c.make_composite_nurb(),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn read_rejects_segment_without_curve() {
        let k = Kernel::default();
        let mut db = ascii("1 count\n0 closed\n2\n1\n0\n0\n0\n\n");
        assert!(matches!(
            CompositeCurve::read_from_db(&k, &mut db, 3, 30),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn read_empty_composite() {
        let k = Kernel::default();
        let mut db = ascii("0 count\n1 closed\n");
        let c = CompositeCurve::read_from_db(&k, &mut db, 2, 30).unwrap();
        assert_eq!(c.num_segments(), 0);
        assert!(c.is_closed());
        assert_eq!(c.dim(), 2);
    }

    #[test]
    fn read_rejects_unreadable_count() {
        let k = Kernel::default();
        let mut db = ascii("");
        assert!(CompositeCurve::read_from_db(&k, &mut db, 3, 30).is_err());
    }

    #[test]
    fn bow_ties_need_original_curves() {
        let k = Kernel::default();
        let mut pts = Vec::new();
        assert!(check_bow_ties(&k, &[], 1.0, 1e-3, &[], &mut pts).is_err());
    }

    #[test]
    fn bow_ties_closed_chain_has_none() {
        let k = Kernel::default();
        let sq = square();
        let refs: Vec<&Curve> = sq.segments().iter().map(|s| &s.curve).collect();
        let mut pts = vec![Vec3::new(9.0, 9.0, 9.0)];
        check_bow_ties(&k, &refs, 0.1, 1e-3, &refs, &mut pts).unwrap();
        assert!(pts.is_empty());
    }

    #[test]
    fn fix_bow_tie_without_points_is_noop() {
        let k = Kernel::default();
        let c = line([0.0; 3], [1.0, 0.0, 0.0]);
        let mut b = vec![0.0, 1.0];
        fix_bow_tie(&k, &c, &[], 0.5, &mut b).unwrap();
        assert_eq!(b, vec![0.0, 1.0]);
    }

    #[test]
    fn bad_direction_is_sm_err() {
        let k = Kernel::default();
        let sq = square();
        let p = OffsetParams {
            xsect_tol_3d: 1e-3,
            approx_tol_3d: 1e-3,
            plane_normal: Vec3::new(0.0, 0.0, 1.0),
            corner: OffsetCornerType::FilletCorner,
            trim_results: false,
            offset_distance: 0.1,
            match_parameterization: false,
        };
        assert!(matches!(
            CompositeCurve::create_offsets_of_many_curves(&k, &[&sq], &[4], &p),
            Err(SolidError::Upstream { .. })
        ));
        assert!(matches!(
            CompositeCurve::create_offsets_of_many_curves(&k, &[&sq], &[], &p),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn untrimmed_square_offsets() {
        let k = Kernel::default();
        let sq = square();
        let p = OffsetParams {
            xsect_tol_3d: 1e-3,
            approx_tol_3d: 1e-3,
            plane_normal: Vec3::new(0.0, 0.0, 1.0),
            corner: OffsetCornerType::LinearExtension,
            trim_results: false,
            offset_distance: 0.1,
            match_parameterization: false,
        };
        match CompositeCurve::create_offsets_of_many_curves(&k, &[&sq], &[2], &p) {
            Ok(r) => {
                assert_eq!(r.offsets.len(), r.max_gaps.len());
                assert!(r.offsets.iter().all(|o| o.attribute.is_some()));
            }
            Err(e) => assert!(e.is_unported()),
        }
    }
}

//! The loop helper classes of `SmTrimmingTools.h`: curve loops kept as one list of
//! curves with per-loop counts (`SmCurveLoops`), the segment of an iso curve between two
//! loop junctions (`SmIsoXSectSegment`) and an iso curve's intersections with the loop
//! curves (`SmCurveIntersections`).
//!
//! Upstream: `source/SMLib/src/SmTrimmingTools.cpp`, `source/SMLib/inc/SmTrimmingTools.h`.
//!
//! | Rust                                         | Upstream                                                  |
//! |----------------------------------------------|-----------------------------------------------------------|
//! | [`CurveIntersections`], [`CurveIntersections::new`] | `SmCurveIntersections`, its constructor            |
//! | [`IsoXSectSegment`], [`IsoXSectSegment::new`] | `SmIsoXSectSegment`, its constructor (`~` is `Drop`)     |
//! | [`IsoXSectSegment::min_index`], [`IsoXSectSegment::max_index`] | `GetMinIndex`, `GetMaxIndex`             |
//! | [`CurveLoops`], [`CurveLoops::new`]          | `SmCurveLoops`, `SmCurveLoops()` (`~` is `Drop`)          |
//! | [`CurveLoops::from_loops`]                   | `SmCurveLoops(crCurveLoops, cpOptUVCurves, cpOpt3DCurves, crCurveOrientations)` |
//! | [`CurveLoops::reset`]                        | `SmCurveLoops::Reset`                                     |
//! | [`CurveLoops::is_valid`], [`CurveLoops::num_loops`] | `IsValid`, `GetNumLoops`                           |
//! | [`CurveLoops::loop_start_index`]             | `GetLoopStartIndex`                                       |
//! | [`CurveLoops::loop_start_end`]               | `GetLoopStartEnd`                                         |
//! | [`CurveLoops::find_adjacent_loop_curve`]     | `FindAdjacentLoopCurve`                                   |
//! | [`CurveLoops::find_entering_and_leaving`]    | `FindEnteringAndLeaving`                                  |
//! | [`CurveLoops::sample_nearby_points`]         | `SampleNearbyPoints`                                      |
//! | [`CurveLoops::is_same_side_intersection`]    | `IsSameSideIntersection`                                  |
//! | [`CurveLoops::append_loop`]                  | `AppendLoop`                                              |
//! | [`CurveLoops::insert_into_loops`]            | `InsertIntoLoops`                                         |
//! | [`CurveLoops::insert_iso_xsect_segment`]     | `InsertIsoXSectSegment`                                   |
//! | [`CurveLoops::update_solution_objects`]      | `UpdateSolutionObjects`                                   |
//! | [`StoreDomains`]                             | the `SmCurve*` dereference of `SmSolution::GetParameterInterval` |
//!
//! Deviations:
//! - **Curves.** Upstream keeps `SmCurve*`s it does not own; here a curve is a
//!   [`CurveKey`], an index into a curve store (`&[Curve]`) the caller owns and passes to
//!   the methods that evaluate curves. Pointer identity is key identity.
//!   `InsertIsoXSectSegment` appends its two new iso curve copies to the store.
//! - **Intersections.** The `m_pNext` linked list of [`CurveIntersections`] is a slice in
//!   list order. A solution's objects are [`LoopObject`]s (the loop curve is
//!   `m_apObjects[1]`).
//! - **Indices** are `usize` with upstream's `ULONG` wrap-around arithmetic.
//!   `GetLoopStartEnd` sets the start of a later loop to the previous loop's last index,
//!   as upstream does (`plan/upstream-issues.md` WR-96). An index past the curve list, which
//!   upstream reads out of bounds, is [`SolidError::InvalidInput`].
//! - **Ignored statuses.** `SampleNearbyPoints` ignores `FindAdjacentLoopCurve`'s status
//!   (index `0` when the curve is not in the loops) and `FindAdjacentLoopCurve` ignores
//!   `GetLoopStartEnd`'s (the values it left); [`SolidError::Unported`] still propagates.
//!   A failing curve evaluation, whose status upstream ignores, is returned.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, ParameterDomains, SolutionArray, SolutionObject};
use crate::geom::tol;
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;

use super::{invalid_input_status, OP};

/// The unset index of an [`IsoXSectSegment`] (upstream `9999`).
pub const NO_INDEX: usize = 9999;

/// A curve in the caller's curve store (an upstream `SmCurve*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CurveKey(pub usize);

/// An object a loop/iso-curve intersection solution refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopObject {
    /// A curve of the store.
    Curve(CurveKey),
    /// Any other object, by kind.
    Other(ObjectKind),
}

impl SolutionObject for LoopObject {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(_) => ObjectKind::Curve,
            Self::Other(k) => *k,
        }
    }
}

/// The parameter domains of [`LoopObject`]s read from the caller's curve store
/// (upstream dereferences the `SmCurve*`: `SmCurve::GetNaturalInterval`), for
/// [`SolutionArray::add_sorted_solution_in`]. Other kinds ask the handle itself.
#[derive(Debug, Clone, Copy)]
pub struct StoreDomains<'a>(pub &'a [Curve]);

impl ParameterDomains<LoopObject> for StoreDomains<'_> {
    fn natural_interval(&self, curve: &LoopObject) -> Option<Extent1d> {
        match curve {
            LoopObject::Curve(key) => self.0.get(key.0).map(Curve::natural_interval),
            LoopObject::Other(_) => curve.natural_interval(),
        }
    }
}

/// An iso curve's intersections with the loop curves (`SmCurveIntersections`).
#[derive(Debug, Clone)]
pub struct CurveIntersections {
    /// `m_pIsoCurve`: the iso-parametric curve intersected.
    pub iso_curve: Option<CurveKey>,
    /// `m_sSolArray`: the iso curve / loop curve intersections (loop curve second).
    pub solutions: SolutionArray<LoopObject>,
    /// `m_eSrfDir`: [`SurfParamType::U`] or [`SurfParamType::V`], the constant
    /// parameter of the iso curve.
    pub srf_dir: SurfParamType,
    /// `m_dIsoParam`: the iso curve's constant parameter value.
    pub iso_param: f64,
}

impl CurveIntersections {
    /// `SmCurveIntersections(pIsoCurve, eSrfDir, dIsoParam, pNext)` (the list link is the
    /// position in the caller's slice).
    #[must_use]
    pub fn new(iso_curve: Option<CurveKey>, srf_dir: SurfParamType, iso_param: f64) -> Self {
        Self {
            iso_curve,
            solutions: SolutionArray::new(),
            srf_dir,
            iso_param,
        }
    }
}

/// A segment of an iso curve between two loop junctions (`SmIsoXSectSegment`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoXSectSegment {
    /// `m_pIsoCurve`: the curve to insert (copied, not owned).
    pub iso_curve: CurveKey,
    /// `m_sInterval`: the interval of the curve between enter and leave.
    pub interval: Extent1d,
    /// `m_eOrient`: the curve's orientation.
    pub orient: OrientType,
    /// `m_lStartEnter`: the loop curve entering the segment's start.
    pub start_enter: usize,
    /// `m_lStartLeave`: the loop curve leaving the segment's start.
    pub start_leave: usize,
    /// `m_lEndEnter`: the loop curve entering the segment's end.
    pub end_enter: usize,
    /// `m_lEndLeave`: the loop curve leaving the segment's end.
    pub end_leave: usize,
}

impl IsoXSectSegment {
    /// `SmIsoXSectSegment(pIsoCurve)`: no interval, [`OrientType::Same`], every index
    /// [`NO_INDEX`].
    #[must_use]
    pub const fn new(iso_curve: CurveKey) -> Self {
        Self {
            iso_curve,
            interval: Extent1d::new(),
            orient: OrientType::Same,
            start_enter: NO_INDEX,
            start_leave: NO_INDEX,
            end_enter: NO_INDEX,
            end_leave: NO_INDEX,
        }
    }

    /// `GetMinIndex`: the smaller of the two entering indices.
    #[must_use]
    pub fn min_index(&self) -> usize {
        self.start_enter.min(self.end_enter)
    }

    /// `GetMaxIndex`: the larger of the two leaving indices.
    #[must_use]
    pub fn max_index(&self) -> usize {
        self.start_leave.max(self.end_leave)
    }
}

/// Curve loops as one curve list with orientations and per-loop counts
/// (`SmCurveLoops`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveLoops {
    /// `m_sCurves`: the curves of all loops.
    pub curves: Vec<CurveKey>,
    /// `m_sOrients`: each curve's orientation.
    pub orients: Vec<OrientType>,
    /// `m_sLoopCounts`: the number of curves in each loop.
    pub loop_counts: Vec<usize>,
    /// `m_bIsValid`: `false` when built without curves.
    pub is_valid: bool,
}

impl Default for CurveLoops {
    fn default() -> Self {
        Self::new()
    }
}

fn get<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i).copied().ok_or(SolidError::invalid_input(
        OP,
        "curve loop index out of range",
    ))
}

fn curve_of(store: &[Curve], key: CurveKey) -> Result<&Curve> {
    store
        .get(key.0)
        .ok_or(SolidError::invalid_input(OP, "curve key not in the store"))
}

impl CurveLoops {
    /// `SmCurveLoops()`: empty and valid.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            curves: Vec::new(),
            orients: Vec::new(),
            loop_counts: Vec::new(),
            is_valid: true,
        }
    }

    /// `SmCurveLoops(crCurveLoops, cpOptUVCurves, cpOpt3DCurves, crCurveOrientations)`:
    /// the UV curves when given, else the 3D curves; invalid (and without curves) when
    /// neither is given.
    #[must_use]
    pub fn from_loops(
        loop_counts: &[usize],
        uv_curves: Option<&[CurveKey]>,
        curves_3d: Option<&[CurveKey]>,
        orients: &[OrientType],
    ) -> Self {
        let (curves, is_valid) = match (uv_curves, curves_3d) {
            (Some(c), _) | (None, Some(c)) => (c.to_vec(), true),
            (None, None) => (Vec::new(), false),
        };
        Self {
            curves,
            orients: orients.to_vec(),
            loop_counts: loop_counts.to_vec(),
            is_valid,
        }
    }

    /// `Reset`: empties the lists; valid.
    pub fn reset(&mut self) {
        self.curves.clear();
        self.orients.clear();
        self.loop_counts.clear();
        self.is_valid = true;
    }

    /// `IsValid`.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.is_valid
    }

    /// `GetNumLoops`.
    #[must_use]
    pub fn num_loops(&self) -> usize {
        self.loop_counts.len()
    }

    /// `GetLoopStartIndex`: the index of the first curve of loop `loop_index`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` for a loop index out of range.
    pub fn loop_start_index(&self, loop_index: usize) -> Result<usize> {
        if loop_index >= self.loop_counts.len() {
            return Err(invalid_input_status());
        }
        Ok(self
            .loop_counts
            .iter()
            .take(loop_index)
            .fold(0usize, |s, &c| s.wrapping_add(c)))
    }

    /// `GetLoopStartEnd` with its outputs as left on failure: the status, `rlStart` and
    /// `rlEnd`.
    fn loop_start_end_raw(&self, curve_index: usize) -> (bool, usize, usize) {
        let mut start = 0usize;
        let mut end = 0usize;
        for (ii, &c) in self.loop_counts.iter().enumerate() {
            end = end.wrapping_add(c);
            if ii == 0 {
                end = end.wrapping_sub(1);
            }
            if curve_index <= end {
                return (true, start, end);
            }
            start = end;
        }
        (false, start, end)
    }

    /// `GetLoopStartEnd`: the first and last curve index of the loop holding
    /// `curve_index` (a later loop's start is the previous loop's last index, as
    /// upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the index is past every loop.
    pub fn loop_start_end(&self, curve_index: usize) -> Result<(usize, usize)> {
        match self.loop_start_end_raw(curve_index) {
            (true, s, e) => Ok((s, e)),
            _ => Err(SolidError::sm_err(OP)),
        }
    }

    /// `FindAdjacentLoopCurve`: the index of the loop curve before `key` (when `param`
    /// lies in the first half of its natural interval) or after it, cyclically in its
    /// loop.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `key` is not in the loops; [`SolidError::InvalidInput`] for a key not
    /// in `store`.
    pub fn find_adjacent_loop_curve(
        &self,
        store: &[Curve],
        key: CurveKey,
        param: f64,
    ) -> Result<usize> {
        let Some(idx) = self.curves.iter().position(|&c| c == key) else {
            return Err(SolidError::sm_err(OP));
        };
        // status ignored upstream
        let (_, start, end) = self.loop_start_end_raw(idx);
        let mid = curve_of(store, key)?.natural_interval().mid();
        Ok(if param < mid {
            if idx == start {
                end
            } else {
                idx.wrapping_sub(1)
            }
        } else if idx == end {
            start
        } else {
            idx.wrapping_add(1)
        })
    }

    /// `FindEnteringAndLeaving`: of two consecutive curves of a loop, the entering and
    /// the leaving one (across the loop's end: the last curve enters, the first leaves).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without loops, for curves in different loops or not consecutive.
    pub fn find_entering_and_leaving(
        &self,
        index1: usize,
        index2: usize,
    ) -> Result<(usize, usize)> {
        if self.loop_counts.is_empty() {
            return Err(SolidError::sm_err(OP));
        }

        let mut loop_number = 0usize;
        let mut next_loop_start = 0usize;
        for (ii, &c) in self.loop_counts.iter().enumerate() {
            next_loop_start = next_loop_start.wrapping_add(c);
            if index1 < next_loop_start {
                loop_number = ii;
                break;
            }
        }

        let count = get(&self.loop_counts, loop_number)?;
        let start = next_loop_start.wrapping_sub(count);
        let end = next_loop_start.wrapping_sub(1);

        if index2 < start || index2 > end {
            return Err(SolidError::sm_err(OP));
        }

        if count > 2 {
            if index1 == index2.wrapping_add(1) {
                return Ok((index2, index1));
            } else if index2 == index1.wrapping_add(1) {
                return Ok((index1, index2));
            }
        }

        if (index1 == start && index2 == end) || (index2 == start && index1 == end) {
            Ok((end, start))
        } else if index2 == index1.wrapping_add(1) {
            Ok((index1, index2))
        } else if index1 == index2.wrapping_add(1) {
            Ok((index2, index1))
        } else {
            Err(SolidError::sm_err(OP))
        }
    }

    /// `SampleNearbyPoints`: points of the loop just before and after `param` on `key`:
    /// on the curve itself when `param` is interior (by a thousandth of the interval),
    /// else one on the curve and one on the adjacent loop curve near their junction.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a key or adjacent index out of range; those of the
    /// evaluations.
    pub fn sample_nearby_points(
        &self,
        k: &Kernel,
        store: &[Curve],
        key: CurveKey,
        param: f64,
    ) -> Result<(Point3, Point3)> {
        let crv = curve_of(store, key)?;
        let domain = crv.natural_interval();
        let tol = domain.length() / 1000.0;

        // when dParam is in the interior by more than tol
        if domain.contains_value(param, -tol) {
            let t = 0.8 * param + 0.2 * domain.min();
            let p1 = crv.evaluate_point(k, t)?;
            let t = 0.8 * param + 0.2 * domain.max();
            let p2 = crv.evaluate_point(k, t)?;
            return Ok((p1, p2));
        }

        // find adjacent loop curve (status ignored, index 0 on failure)
        let adj = match self.find_adjacent_loop_curve(store, key, param) {
            Ok(a) => a,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => 0,
        };
        let adj_crv = curve_of(store, get(&self.curves, adj)?)?;
        let adj_domain = adj_crv.natural_interval();
        let adj_orient = get(&self.orients, adj)?;

        if param < domain.mid() {
            let p1 = crv.evaluate_point(k, domain.evaluate(0.2))?;
            let t = if adj_orient == OrientType::Same {
                adj_domain.evaluate(0.8)
            } else {
                adj_domain.evaluate(0.2)
            };
            Ok((p1, adj_crv.evaluate_point(k, t)?))
        } else {
            let p1 = crv.evaluate_point(k, domain.evaluate(0.8))?;
            let t = if adj_orient == OrientType::Same {
                adj_domain.evaluate(0.2)
            } else {
                adj_domain.evaluate(0.8)
            };
            Ok((p1, adj_crv.evaluate_point(k, t)?))
        }
    }

    /// `IsSameSideIntersection`: whether the loop near `param` on `key` stays on one
    /// side of the iso curve of `ints` (a touching rather than a crossing intersection),
    /// from [`CurveLoops::sample_nearby_points`] (dropped to `surface` over `domain`
    /// when the curves are 3D; a failed drop answers `false`).
    ///
    /// # Errors
    ///
    /// Those of [`CurveLoops::sample_nearby_points`]; [`SolidError::Unported`] from the
    /// solve.
    #[allow(clippy::too_many_arguments)]
    pub fn is_same_side_intersection(
        &self,
        k: &Kernel,
        store: &[Curve],
        key: CurveKey,
        param: f64,
        ints: &CurveIntersections,
        surface: &Surface,
        domain: &Extent2d,
        zone_tol_3d: f64,
    ) -> Result<bool> {
        let (p1, p2) = self.sample_nearby_points(k, store, key, param)?;
        let mut uv1 = p1;
        let mut uv2 = p2;

        // when Curves are in 3d - drop nearby Pts to Surface UV domain
        if curve_of(store, key)?.dim() == 3 {
            for (p, uv) in [(p1, &mut uv1), (p2, &mut uv2)] {
                let sols = match surface.global_point_solve::<NoObject>(
                    k,
                    None,
                    domain,
                    SolverOperation::Intersect,
                    p,
                    zone_tol_3d,
                    None,
                    SolutionRequested::Single,
                ) {
                    Ok(s) => s,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Ok(false),
                };
                if sols.len() != 1 {
                    return Ok(false);
                }
                let s = sols
                    .get(0)
                    .ok_or(SolidError::invalid_input(OP, "solution"))?;
                *uv = Point3::new(s.start.parameters[0], s.start.parameters[1], 0.0);
            }
        }

        let (v1, v2) = if ints.srf_dir == SurfParamType::U {
            (uv1.x, uv2.x)
        } else {
            (uv1.y, uv2.y)
        };
        let iso = ints.iso_param;
        Ok((v1 - iso) * (v2 - iso) > 0.0)
    }

    /// `AppendLoop`: appends loop `other_loop` of `other`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` for a loop index out of range; [`SolidError::InvalidInput`]
    /// for counts past `other`'s curves.
    pub fn append_loop(&mut self, other: &Self, other_loop: usize) -> Result<()> {
        if other_loop >= other.num_loops() {
            return Err(invalid_input_status());
        }
        let start = other.loop_start_index(other_loop)?;
        let count = get(&other.loop_counts, other_loop)?;
        let next = start.wrapping_add(count);
        let mut ii = start;
        while ii < next {
            self.curves.push(get(&other.curves, ii)?);
            self.orients.push(get(&other.orients, ii)?);
            ii += 1;
        }
        self.loop_counts.push(count);
        Ok(())
    }

    /// `InsertIntoLoops`: inserts `new_curve` after `loop_curve`, with its orientation,
    /// in the same loop (a split loop curve).
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` when `loop_curve` is not in the loops;
    /// [`SolidError::InvalidInput`] for orientations shorter than the curves.
    pub fn insert_into_loops(&mut self, new_curve: CurveKey, loop_curve: CurveKey) -> Result<()> {
        let Some(idx) = self.curves.iter().position(|&c| c == loop_curve) else {
            return Err(invalid_input_status());
        };
        let o = get(&self.orients, idx)?;
        self.curves.insert(idx + 1, new_curve);
        if idx + 1 > self.orients.len() {
            return Err(SolidError::invalid_input(OP, "orientation list too short"));
        }
        self.orients.insert(idx + 1, o);

        let mut total = 0usize;
        for c in &mut self.loop_counts {
            if idx < total.wrapping_add(*c) {
                *c = c.wrapping_add(1);
                break;
            }
            total = total.wrapping_add(*c);
        }
        Ok(())
    }

    /// `InsertIsoXSectSegment`: splits the loop holding the segment's junctions in two
    /// with copies of the iso curve trimmed to `seg.interval` (snapped there by the
    /// trim): the first loop runs the new curve then the loop from `end_leave` to
    /// `start_enter`, the second the reversed copy then `start_leave` to `end_enter`.
    /// The copies are appended to `store`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the junction indices are not within one loop; those of the copy and
    /// trim; [`SolidError::InvalidInput`] for indices past the curves.
    pub fn insert_iso_xsect_segment(
        &mut self,
        k: &Kernel,
        store: &mut Vec<Curve>,
        seg: &mut IsoXSectSegment,
    ) -> Result<()> {
        let mut new_loops: Vec<usize> = Vec::new();
        let mut new_curves: Vec<CurveKey> = Vec::new();
        let mut new_orients: Vec<OrientType> = Vec::new();

        // Find which loop the iso edge will go into.
        let min_idx = seg.min_index();
        if min_idx >= self.curves.len() {
            return Err(SolidError::sm_err(OP));
        }

        let mut loop_number = self.loop_counts.len();
        let mut total = 0usize;
        for (ii, &c) in self.loop_counts.iter().enumerate() {
            if min_idx < total.wrapping_add(c) {
                loop_number = ii;
                break;
            }
            total = total.wrapping_add(c);
        }
        if loop_number >= self.loop_counts.len() {
            return Err(SolidError::sm_err(OP));
        }

        // check state - highest index is in the same loop.
        let max_idx = seg.max_index();
        let this_count = get(&self.loop_counts, loop_number)?;
        if max_idx >= total.wrapping_add(this_count) {
            return Err(SolidError::sm_err(OP));
        }

        // Copy any previous loops into the new loop structures.
        let mut start_idx = 0usize;
        for ii in 0..loop_number {
            let c = get(&self.loop_counts, ii)?;
            new_loops.push(c);
            for jj in 0..c {
                new_curves.push(get(&self.curves, start_idx + jj)?);
                new_orients.push(get(&self.orients, start_idx + jj)?);
            }
            start_idx += c;
        }

        // Create a new Forward curve.
        let mut new_iso1 = curve_of(store, seg.iso_curve)?.copy(k)?;
        new_iso1.trim(k, &mut seg.interval, TrimMode::Notify)?;
        let key1 = CurveKey(store.len());
        store.push(new_iso1);

        // Start a new Forward loop from there: from End-Leave to Start-Enter.
        new_curves.push(key1);
        new_orients.push(OrientType::Same);
        new_loops.push(1);

        let num = self.curves.len();
        let mut this = seg.end_leave;
        let start_curve = total;
        let end_curve = total.wrapping_add(this_count);

        // 'fake' for-loop: action, check, increment
        for _ in 0..=num {
            new_curves.push(get(&self.curves, this)?);
            new_orients.push(get(&self.orients, this)?);
            if let Some(c) = new_loops.get_mut(loop_number) {
                *c += 1;
            }
            if this == seg.start_enter {
                break;
            }
            this = this.wrapping_add(1);
            if this >= end_curve {
                this = start_curve;
            }
        }

        // Create a new Backwards curve (already trimmed).
        let new_iso2 = curve_of(store, key1)?.copy(k)?;
        let key2 = CurveKey(store.len());
        store.push(new_iso2);
        new_curves.push(key2);
        new_orients.push(OrientType::Opposite);
        new_loops.push(1);

        loop_number += 1;
        this = seg.start_leave;
        for _ in 0..=num {
            new_curves.push(get(&self.curves, this)?);
            new_orients.push(get(&self.orients, this)?);
            if let Some(c) = new_loops.get_mut(loop_number) {
                *c += 1;
            }
            if this == seg.end_enter {
                break;
            }
            this = this.wrapping_add(1);
            if this >= end_curve {
                this = start_curve;
            }
        }

        // Now copy any following loops into the new loop structures.
        let mut total = 0usize;
        for ii in 0..loop_number {
            total = total.wrapping_add(get(&self.loop_counts, ii)?);
        }
        while loop_number < self.loop_counts.len() {
            let c = get(&self.loop_counts, loop_number)?;
            new_loops.push(c);
            let mut jj = total;
            while jj < total.wrapping_add(c) {
                new_curves.push(get(&self.curves, jj)?);
                new_orients.push(get(&self.orients, jj)?);
                jj += 1;
            }
            total = total.wrapping_add(c);
            loop_number += 1;
        }

        // Update the loop structure for return.
        self.loop_counts = new_loops;
        self.curves = new_curves;
        self.orients = new_orients;
        Ok(())
    }

    /// `UpdateSolutionObjects`: after `orig` was split into `orig` and `new`, a solution of
    /// any of `ints` on `orig` whose loop curve parameter left `orig`'s interval (within
    /// `zone_tol_3d` mapped to the parameter) is moved to `new` when it lies in `new`'s.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for keys not in `store`; those of the evaluation.
    pub fn update_solution_objects(
        &self,
        k: &Kernel,
        store: &[Curve],
        ints: &mut [CurveIntersections],
        orig: CurveKey,
        new: CurveKey,
        zone_tol_3d: f64,
    ) -> Result<()> {
        let orig_crv = curve_of(store, orig)?;
        let dom_orig = orig_crv.natural_interval();
        let dom_new = curve_of(store, new)?.natural_interval();

        for ci in ints.iter_mut() {
            for ii in 0..ci.solutions.len() {
                let Some(sol) = ci.solutions.get_mut(ii) else {
                    continue;
                };
                // This is the loop curve (SM_CAST_PTR: NULL unless a curve).
                if sol.objects[1] != Some(LoopObject::Curve(orig)) {
                    continue;
                }
                let param = sol.start.parameters[1];
                let deriv = orig_crv
                    .evaluate(k, param, 1, true, true)?
                    .get(1)
                    .copied()
                    .ok_or(SolidError::invalid_input(OP, "derivative"))?;
                let param_tol = tol::map_to_1d(zone_tol_3d, deriv, &dom_orig);

                // (a parameter in neither interval: SE(SM_ERR), keep going)
                if !dom_orig.contains_value(param, param_tol)
                    && dom_new.contains_value(param, param_tol)
                {
                    sol.objects[1] = Some(LoopObject::Curve(new));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loops(counts: &[usize]) -> CurveLoops {
        let n: usize = counts.iter().sum();
        let keys: Vec<CurveKey> = (0..n).map(CurveKey).collect();
        CurveLoops::from_loops(counts, None, Some(&keys), &vec![OrientType::Same; n])
    }

    #[test]
    fn start_index_and_start_end() {
        let l = loops(&[3, 4]);
        assert_eq!(l.loop_start_index(1).unwrap(), 3);
        assert!(l.loop_start_index(2).is_err());
        assert_eq!(l.loop_start_end(1).unwrap(), (0, 2));
        // upstream's start of a later loop is the previous loop's last index
        assert_eq!(l.loop_start_end(4).unwrap(), (2, 6));
        assert!(l.loop_start_end(7).is_err());
    }

    #[test]
    fn entering_and_leaving() {
        let l = loops(&[4]);
        assert_eq!(l.find_entering_and_leaving(1, 2).unwrap(), (1, 2));
        assert_eq!(l.find_entering_and_leaving(2, 1).unwrap(), (1, 2));
        assert_eq!(l.find_entering_and_leaving(0, 3).unwrap(), (3, 0));
        assert!(l.find_entering_and_leaving(0, 2).is_err());
        assert!(CurveLoops::new().find_entering_and_leaving(0, 1).is_err());
    }

    #[test]
    fn append_and_insert() {
        let a = loops(&[2, 3]);
        let mut b = CurveLoops::new();
        b.append_loop(&a, 1).unwrap();
        assert_eq!(b.loop_counts, vec![3]);
        assert_eq!(b.curves, vec![CurveKey(2), CurveKey(3), CurveKey(4)]);
        b.insert_into_loops(CurveKey(9), CurveKey(3)).unwrap();
        assert_eq!(
            b.curves,
            vec![CurveKey(2), CurveKey(3), CurveKey(9), CurveKey(4)]
        );
        assert_eq!(b.loop_counts, vec![4]);
        assert!(b.insert_into_loops(CurveKey(9), CurveKey(7)).is_err());
        assert!(b.append_loop(&a, 2).is_err());
    }

    #[test]
    fn segment_indices() {
        let mut s = IsoXSectSegment::new(CurveKey(0));
        assert_eq!(s.min_index(), NO_INDEX);
        s.start_enter = 2;
        s.start_leave = 3;
        s.end_enter = 5;
        s.end_leave = 6;
        assert_eq!(s.min_index(), 2);
        assert_eq!(s.max_index(), 6);
    }
}

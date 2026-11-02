//! Curve classification, part 2: assignment and reset of a classification, interval
//! insertion, merging and editing, boundary queries (zone tolerance, location, dead
//! topology), the UV ends of an interval and the classification curve / edge
//! intersection refinement onto surf/surf/surf points.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust                                                        | Upstream                                              |
//! |-------------------------------------------------------------|-------------------------------------------------------|
//! | [`CurveClassification::assign_from`]                        | `SmCurveClassification::operator=`                    |
//! | [`CurveClassification::point_obj_zone_tol_3d`]              | `GetPointObjZoneTol3d`                                |
//! | [`CurveClassification::has_dead_topology_point_object`]     | `HasDeadTopologyPointObject`                          |
//! | [`CurveClassification::classify_crv_loc_by_index`]          | `GetClassifyCrvLoc(ULONG, double*)`                   |
//! | [`CurveClassification::insert_at`]                          | `InsertAt`                                            |
//! | [`CurveClassification::reset`], [`ResetOptions`]            | `ReSet`                                               |
//! | [`CurveClassification::connect_intervals`]                  | `ConnectIntervals`                                    |
//! | [`CurveClassification::update_point_class`]                 | `UpdatePointClass`                                    |
//! | [`CurveClassification::edit_interval_min`]                  | `EditIntervalMin`                                     |
//! | [`CurveClassification::edit_interval_max`]                  | `EditIntervalMax`                                     |
//! | [`CurveClassification::interval_min_uv`]                    | `SmCurveInterval::GetMinUV`                           |
//! | [`CurveClassification::interval_max_uv`]                    | `SmCurveInterval::GetMaxUV`                           |
//! | [`refine_cci_to_surfaces`], [`RefineCci`]                   | `sm_RefineCCIToSurfaces`                              |
//! | [`refine_cci_to_surfaces_in`]                               | `sm_RefineCCIToSurfaces`, through an adapter          |
//! | [`refine_cci_to_surfaces_mut`]                              | `sm_RefineCCIToSurfaces` (with its large-gap part)    |
//!
//! Deviations:
//! - **Handles.** As in part 1, every object is a caller handle `O` resolved through a
//!   [`ClassGeometry`]. `operator=` copies the curve handle also for an owned curve
//!   (upstream deep-copies it; the copy has the same geometry) when `SmCurve::Copy`
//!   succeeds ([`Curve::copy`](crate::curve::curve::p2::Curve::copy)); when it fails
//!   (status ignored: `SmHermiteCurve`, `SmConic` and `SmIsoCurve` have no `Copy`) the
//!   curve stays what it was, `NULL` if the old curve was owned (freed). `m_cpContext` is
//!   not stored. `SmCurveInterval::GetMinUV` / `GetMaxUV` reach the classification through
//!   the interval's back pointer, which is not ported: they are members of the
//!   classification taking the interval index.
//! - **Borrowed interval storage** (`ReSet`'s `nDataSize` / `pOptBorrowedData`) is a
//!   memory-management feature that is not modelled (see part 1): [`ResetOptions`] has no
//!   such field, and a reset with a new curve always owns a fresh array.
//! - **Out-params** become return values: `(point, parameter)` of
//!   [`CurveClassification::classify_crv_loc_by_index`], the UV point of
//!   [`CurveClassification::interval_min_uv`] (the caller's `uv` when upstream leaves it
//!   unwritten), [`RefineCci`].
//! - **Undefined behaviour upstream** is an error here (`InvalidInput`): boundary or
//!   interval indices outside the classification, a `NULL` curve where it is
//!   dereferenced, a reset with a new curve but no interval, and `InsertAt(0, ..)` into a
//!   non-empty classification (upstream's unsigned shift loop `ii >= 0` never ends and
//!   reads before the array; see `plan/upstream-issues.md`).
//! - **`EditIntervalMin` reading past the end.** When collapsing the neighbour removes
//!   the interval before the last one, upstream reads the target interval at its old
//!   index, which is now the stale slot past `m_lSize` (a copy of the last interval, as
//!   `RemoveAt` leaves it), and its `CollapseInterval` then fails (status ignored). This
//!   port reads the copy of the last interval the same way.
//! - **`sm_RefineCCIToSurfaces`**: [`refine_cci_to_surfaces_in`] resolves the edge, its
//!   faces, edge uses, UV trim curves and Boolean mate through the adapter
//!   (`sm_GetCCIGeometry`), measures the gaps (`sm_GetCCISurfaceGaps`) and returns
//!   unchanged when they are within the edge zone tolerance / 2000 or a point fails to
//!   drop, as upstream; past that gate it needs to edit the B-rep and a read-only
//!   adapter defers (a dispatch gap). [`refine_cci_to_surfaces_mut`] runs the large-gap
//!   part over a [`ClassGeometryMut`] adapter: the surfaces and UV trim curves are copied
//!   first (the part edits the B-rep they live in; upstream's `SmSurface*` comparisons
//!   become comparisons of each surface's first position in the list), the
//!   classification curve is edited on a copy written back in place, the edge (and a
//!   Boolean mate in the same B-rep; one in another B-rep is a dispatch gap) through
//!   [`apply_point_solution_in`](super::p1::apply_point_solution_in), and the edge uses'
//!   UV trim curves are dropped through the edge's B-rep. Statuses upstream ignores
//!   (evaluations, the local and global intersections, `RefinePoint`, whose
//!   `bFoundAnswer` is already `TRUE` when its local intersection fails, and
//!   `sm_ApplyPointSolution`, whose failure leaves its out-params as its entry reset
//!   them) are ignored; the `SER`'d curve/curve intersection's failing status is not
//!   returned (every caller ignores it), only the refinement stops. Where upstream
//!   indexes its surface or UV point lists past their end (a failed gap measurement
//!   leaves the UV point list short) the port returns `InvalidInput`.
//!   [`refine_cci_to_surfaces`], which has no adapter, still defers at the edge.
//! - **Recursion.** `sm_RefineCCIToSurfaces` recurses from its last part through
//!   upstream's own shared counter (`lRecursionCount`, nothing done past 5, so at most
//!   7 nested calls), which [`RefineCci::recursion_count`] carries.
//! - **Topology callees** return [`SolidError::Unported`]: `SmEdge::GetCurve`
//!   (`topology.edge`) and `SmFace::GetSurface` (`topology.face_p2`) are called on
//!   opaque edge / face handles that carry no B-rep the port can reach (as in part 3
//!   and `intersect::point_class`: the stubs wait for handle-to-B-rep resolution, not
//!   for those units); `SmBrep::IsLiveTopologyMember` (`topology.brep_p1`, not ported)
//!   and the topology zone tolerances of point classifications (through
//!   `intersect.point_class`).

use crate::curve::curve::p2::{Curve, CurveKind};
use crate::curve::curve::p3::PointSolveOptions;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::{Solution, SolutionType};
use crate::geom::tol::{
    ang_tol_rad, are_parallel, are_perpendicular, are_same, eff_zero, scaled_zero_sq_of,
    xsect_tol_3d_from_zones, zone_tol3d, ScaledZero, UNINIT_TOL, ZONE_TOL_3D,
};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, PointClassType, PointClassification,
};
use crate::intersect::surface_intersector::p1::{IntersectionPointType, SurfaceIntersector};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::Surface;
use crate::topology::brep::p1::is_live_topology_member;
use crate::topology::face::p2 as face_p2;
use crate::topology::{Brep, EdgeuseId};
use crate::EntityRef;

use super::p1::{
    xsect_tol_2d_or_3d_for_classify_pair, CurveClassification, CurveInterval, IntervalPosition,
};
use super::p3::curve_is_degenerate;
use super::p5::ClassGeometryMut;
use super::p7::NoObject;

const OP_CC: &str = "intersect::curve_class::p2::CurveClassification";
const OP_INSERT: &str = "intersect::curve_class::p2::insert_at";
const OP_RESET: &str = "intersect::curve_class::p2::reset";
const OP_CONNECT: &str = "intersect::curve_class::p2::connect_intervals";
const OP_EDIT: &str = "intersect::curve_class::p2::edit_interval";
const OP_UV: &str = "intersect::curve_class::p2::interval_uv";
const OP_REFINE: &str = "intersect::curve_class::p2::refine_cci_to_surfaces";

/// `lMaxRecursion` of `sm_RefineCCIToSurfaces`.
const MAX_REFINE_RECURSION: u64 = 5;

/// The arguments of [`CurveClassification::reset`] (`ReSet`), with upstream's defaults
/// from [`Default`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResetOptions<O> {
    /// `cpOptCurve` (default `NULL`): with `uv_curve`, `None` for both keeps the current
    /// curve and only resets the intervals.
    pub curve: Option<O>,
    /// `cpOptInterval` (default `NULL`): the interval of the new curve, required with a
    /// new curve.
    pub interval: Option<Extent1d>,
    /// `cpOptUVCurve` (default `NULL`).
    pub uv_curve: Option<O>,
    /// `sZoneTol3d`, an `SmZoneTol3d` (default `SM_ZONE_TOL_3D`): the zone tolerance of
    /// the curve's source, given to the first interval's point classifications.
    pub zone_tol_3d: f64,
    /// `bIsCurveOwned` (default `FALSE`).
    pub is_curve_owned: bool,
    /// `bIsCurveCurrent` (default `TRUE`).
    pub is_curve_current: bool,
    /// `cpSurface1` (default `NULL`).
    pub surface1: Option<O>,
    /// `cpSurface2` (default `NULL`).
    pub surface2: Option<O>,
    /// `cpUVCurve2` (default `NULL`).
    pub uv_curve2: Option<O>,
    /// `cpTI` (default `NULL`).
    pub ti: Option<O>,
}

impl<O> Default for ResetOptions<O> {
    fn default() -> Self {
        Self {
            curve: None,
            interval: None,
            uv_curve: None,
            zone_tol_3d: ZONE_TOL_3D,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        }
    }
}

/// The result of [`refine_cci_to_surfaces`] (its out-params; the status is always
/// `SM_SUCCESS` when no error is returned).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineCci {
    /// `bSSSPointFlag`: use the surf/surf/surf intersection point.
    pub sss_point_flag: bool,
    /// `vSSSPoint` out: the surf/surf/surf point (the caller's value unless set).
    pub sss_point: Point3,
    /// `dDeviation`: the largest curve / surf/surf/surf point distance.
    pub deviation: f64,
    /// `lRecursionCount` out: the recursion counter after the call.
    pub recursion_count: u64,
}

impl<O: ClassObject> CurveClassification<O> {
    /// Makes this classification a copy of `other` (`operator=`): every member, the
    /// target loops and the intervals (the interval array is sized with
    /// [`CurveClassification::set_size`], so an empty `other` frees it). An owned curve
    /// of `other` is deep-copied upstream: its handle is copied when the curve can be
    /// copied, else (`SmCurve::Copy` fails, status ignored) this classification keeps its
    /// curve, or none when it owned it (upstream freed it first).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `other` owns a handle that is not a curve; those
    /// of [`CurveClassification::set_size`] (none for a valid `other`).
    pub fn assign_from(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &Self,
    ) -> Result<()> {
        // when needed - free old curve
        if self.is_curve_owned && self.curve.is_some() {
            self.curve = None;
        }

        // copy member values
        self.is_curve_owned = other.is_curve_owned;
        match other.curve {
            Some(c) if self.is_curve_owned => {
                // deep copy (the handle stands for the copy); on failure m_cpCurve is
                // left unchanged (status ignored)
                let curve = geometry.curve(c).ok_or(SolidError::invalid_input(
                    OP_CC,
                    "owned classification curve is not a curve",
                ))?;
                if ignored(curve.copy(k))?.is_some() {
                    self.curve = Some(c);
                }
            }
            _ => self.curve = other.curve,
        }

        // sm_CopiedUVCurve1: the copy keeps `other`'s curve handle, so the UV curve of the
        // copied SmCrvOnSurf is `other`'s UV curve
        self.uv_curve1 = other.uv_curve1;
        self.is_curve_current = other.is_curve_current;
        self.surface1 = other.surface1;
        self.surface2 = other.surface2;
        self.doing_cross_hatch = other.doing_cross_hatch;
        self.doing_fillets = other.doing_fillets;
        self.ok_to_set_tolerance = other.ok_to_set_tolerance;
        self.tgt_loops_only = other.tgt_loops_only;
        self.tgt_loops.clear();
        self.tgt_loops.extend_from_slice(&other.tgt_loops);
        self.other_classification = other.other_classification;
        self.face = other.face;
        self.uv_curve2 = other.uv_curve2;
        self.ti = other.ti;

        self.src_zone_tol_3d = other.src_zone_tol_3d;

        self.interval = other.interval;
        self.set_size(k, other.data.len())?;
        for (dst, src) in self.data.iter_mut().zip(&other.data) {
            *dst = *src;
        }
        self.ignore_tangency = other.ignore_tangency;

        // all done
        Ok(())
    }

    /// The zone tolerance of the object of the point classification at interval
    /// boundary `index` (`GetPointObjZoneTol3d`).
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::point_class_by_index`] and
    /// [`PointClassification::obj_zone_tol_3d`].
    pub fn point_obj_zone_tol_3d(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<f64> {
        let (point_class, _) = self.point_class_by_index(index)?;

        // all done: return the object's zone tolerance
        point_class.obj_zone_tol_3d(k, geometry)
    }

    /// Whether a vertex, edge or face of an interval boundary classification is no
    /// longer live topology of `brep` (`HasDeadTopologyPointObject`); false for no brep
    /// and for an empty classification. The objects resolve through
    /// [`ClassGeometry::topology_entity`]; one of another B-rep is not a member of `brep`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for a topology object the adapter cannot resolve to a
    /// B-rep (`SmBrep::IsLiveTopologyMember`, unit `topology.brep_p1`).
    pub fn has_dead_topology_point_object(
        &self,
        geometry: &impl ClassGeometry<O>,
        brep: Option<&Brep>,
    ) -> Result<bool> {
        let Some(brep) = brep else {
            return Ok(false);
        };
        if self.size() == 0 {
            return Ok(false); // empty classification has no interval boundaries to inspect
        }

        // point classifications: 0..IvlCnt inclusive
        for i in 0..=self.size() {
            let (point_class, _) = self.point_class_by_index(i)?;
            let Some(obj) = point_class.object() else {
                continue;
            };
            let obj_class = point_class.point_class();
            if obj_class == PointClassType::Vertex
                || obj_class == PointClassType::Edge
                || obj_class == PointClassType::Face
            {
                let (b, e) = geometry.topology_entity(obj).ok_or(SolidError::unported(
                    "SmBrep::IsLiveTopologyMember",
                    "topology.brep_p1",
                ))?;
                if !(std::ptr::eq(b, brep) && is_live_topology_member(brep, Some(e))) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// The location of the point classification at interval boundary `index` and its
    /// curve parameter (`GetClassifyCrvLoc(ULONG, double*)`): its surf/surf/surf point
    /// when set, else the curve point.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::point_class_by_index`] and
    /// [`CurveClassification::classify_crv_loc`].
    pub fn classify_crv_loc_by_index(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<(Point3, f64)> {
        let (point_class, param) = self.point_class_by_index(index)?;

        // all done: return SSS point if possible else m_cpCurve(dCurveParameter)
        Ok((
            self.classify_crv_loc(k, geometry, point_class, param)?,
            param,
        ))
    }

    /// Inserts `interval` at `index` (`InsertAt`): past the end the classification
    /// grows to `index + 1` intervals (the new ones in between as
    /// [`CurveClassification::set_size`] makes them), else the intervals from `index` on
    /// move up by one.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index == 0` in a non-empty classification
    /// (upstream's shift loop never ends) and for more than
    /// [`MAX_INTERVALS`](super::MAX_INTERVALS) intervals.
    pub fn insert_at(
        &mut self,
        k: &Kernel,
        index: usize,
        interval: CurveInterval<O>,
    ) -> Result<()> {
        let size = self.size();
        if index >= size {
            // adding after the end of the array: grow so nIndex is valid
            let new_size = index
                .checked_add(1)
                .ok_or(SolidError::invalid_input(OP_INSERT, "index too large"))?;
            self.set_size(k, new_size)?;
        } else {
            // inserting in the middle of the array
            if index == 0 {
                // for(ii = m_lSize-1-1; ii >= 0; ii--) on an unsigned index never ends
                return Err(SolidError::invalid_input(
                    OP_INSERT,
                    "insert_at(0) into a non-empty classification",
                ));
            }
            self.set_size(k, size + 1)?; // grow it to new size

            // shift old data up to fill gap
            let mut i1 = size;
            while i1 > index {
                let src = *self.at(i1 - 1)?;
                *self.at_mut(i1)? = src;
                i1 -= 1;
            }

            // re-init slots we copied from
            self.at_mut(index)?.reset(k);
        }

        // insert new value in the gap
        *self.at_mut(index)? = interval;
        Ok(())
    }

    /// Resets the classification (`ReSet`). With a new curve or UV curve every member is
    /// set as the classification constructor does from `options` and the intervals
    /// become one unclassified interval over `options.interval`; without, only the
    /// curve-current flag changes and an allocated interval array becomes one
    /// unclassified interval over the current interval. The point classifications of
    /// that interval take `options.zone_tol_3d`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a new curve without `options.interval` (nothing
    /// is changed).
    pub fn reset(&mut self, k: &Kernel, options: &ResetOptions<O>) -> Result<()> {
        let is_new_curve = options.curve.is_some() || options.uv_curve.is_some();
        // (checked before any change, so the error leaves the classification unchanged;
        // upstream dereferences the missing interval)
        if is_new_curve && options.interval.is_none() {
            return Err(SolidError::invalid_input(
                OP_RESET,
                "a new curve needs its interval",
            ));
        }

        // set the modified curve bit
        self.is_curve_current = options.is_curve_current;

        let zone_tol_3d = options.zone_tol_3d;

        // when new curve is given - reset to default constructor state
        if let Some(interval) = options.interval.filter(|_| is_new_curve) {
            // remove existing curve and data
            self.data.clear();
            self.allocated = false;

            // set new values
            self.is_curve_owned = options.is_curve_owned;
            self.curve = options.curve;
            self.src_zone_tol_3d = if zone_tol_3d > 0.0 {
                zone_tol_3d
            } else {
                zone_tol3d(k, UNINIT_TOL)
            };

            self.uv_curve1 = options.uv_curve;
            self.is_curve_current = options.is_curve_current;
            self.surface1 = options.surface1;
            self.surface2 = options.surface2;
            self.doing_cross_hatch = false;
            self.doing_fillets = false;
            // (m_bOKToSetTolerance propagates through resets)
            self.tgt_loops_only = false;
            self.tgt_loops.clear();
            self.other_classification = None;

            self.uv_curve2 = options.uv_curve2;
            self.ti = options.ti;

            self.interval = interval;
            // SetSize(SM_MIN_ARRAY_SIZE), then m_lSize = 1
            self.allocated = true;
            self.ignore_tangency = false;

            // init first curve interval
            let first = self.first_interval(k, &interval, zone_tol_3d);
            self.data.push(first);
        } else {
            // reset the classification to 1 unclassified interval when mem has already
            // been allocated
            if self.allocated {
                let interval = self.interval;
                let first = self.first_interval(k, &interval, zone_tol_3d);
                self.data.clear();
                self.data.push(first);
            }
        }
        Ok(())
    }

    /// `SmCurveInterval(interval, ctx, this)` with its point classifications' source zone
    /// tolerance set to `zone_tol_3d`.
    fn first_interval(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        zone_tol_3d: f64,
    ) -> CurveInterval<O> {
        let mut first = CurveInterval::with_interval(k, interval, Some(self));
        first.start.set_src_zone_tol_3d(zone_tol_3d);
        first.mid.set_src_zone_tol_3d(zone_tol_3d);
        first.end.set_src_zone_tol_3d(zone_tol_3d);
        first
    }

    /// Removes interval `index` by squeezing its `end_to_squeeze` end out
    /// (`ConnectIntervals`): an end interval squeezed at the curve end is deleted and the
    /// classified interval shortened (only with `allow_removal_of_ends`; otherwise the
    /// other end is squeezed), else the neighbour on the squeezed side takes over the
    /// interval's range and far end classification. Positions other than
    /// [`IntervalPosition::Start`] and [`IntervalPosition::End`] change nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `index >= size()`.
    pub fn connect_intervals(
        &mut self,
        index: usize,
        end_to_squeeze: IntervalPosition,
        allow_removal_of_ends: bool,
    ) -> Result<()> {
        // check state - lIntervalIndex within bounds
        let size = self.size();
        if index >= size {
            return Err(SolidError::sm_err(OP_CONNECT));
        }
        let mut end_to_squeeze = end_to_squeeze;

        // If we don't allow removal of ends then swap the end to squeeze
        if !allow_removal_of_ends {
            if end_to_squeeze == IntervalPosition::Start && index == 0 {
                end_to_squeeze = IntervalPosition::End;
            }
            if end_to_squeeze == IntervalPosition::End && index == size - 1 {
                end_to_squeeze = IntervalPosition::Start;
            }
        }

        // First take care of easy situation where squeezePoint is at the CurveEnd
        if (end_to_squeeze == IntervalPosition::Start && index == 0)
            || (end_to_squeeze == IntervalPosition::End && index == size - 1)
        {
            // Remove the interval
            self.remove_at(index)?;

            // Now shorten m_vInterval when any other intervals are left
            if let (Some(first), Some(last)) = (self.data.first(), self.data.last()) {
                let (min, max) = (first.interval.min(), last.interval.max());
                ignore_status(self.interval.set_min_max(min, max))?;
            }
        }
        // merge with previous interval
        else if end_to_squeeze == IntervalPosition::Start {
            let ivl = *self.at(index)?;
            let prev = self.at_mut(index - 1)?;
            prev.interval.add_value(ivl.interval.max());
            prev.end = ivl.end;
            self.remove_at(index)?;
        }
        // merge with next interval
        else if end_to_squeeze == IntervalPosition::End {
            let ivl = *self.at(index)?;
            let next = self.at_mut(index + 1)?;
            next.interval.add_value(ivl.interval.min());
            next.start = ivl.start;
            self.remove_at(index)?;
        }

        // all done
        Ok(())
    }

    /// Sets the point classification at interval boundary `index` and its mate (see
    /// [`CurveClassification::point_class_mate_by_index`]) to `new_values`
    /// (`UpdatePointClass`).
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::point_class_by_index`] and
    /// [`CurveClassification::point_class_mate_by_index`]; the boundary classification
    /// is set by then.
    pub fn update_point_class(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        new_values: &PointClassification<O>,
    ) -> Result<()> {
        // copy rNewValues into PointClassification
        *self.point_class_by_index_mut(index)?.0 = *new_values;

        // copy rNewValues into corresponding PC in adjacent interval: the mate is the
        // end of an interval
        if let Some(i) = self.point_class_mate_interval(k, geometry, index)? {
            self.at_mut(i)?.end = *new_values;
        }

        // all done
        Ok(())
    }

    /// Moves the start of interval `index` to `new_param` and the end of its predecessor
    /// with it (`EditIntervalMin`; upstream default `collapse_degen_intervals = TRUE`).
    /// With `collapse_degen_intervals`, the predecessor and then the interval are
    /// collapsed ([`CurveClassification::collapse_interval`], status ignored) when
    /// degenerate within the pair tolerance of their ends.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `index >= size()` or `new_param` outside
    /// `[previous min (or the classified interval's min), max]`;
    /// [`SolidError::InvalidInput`] for no curve; those of the tolerances.
    pub fn edit_interval_min(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        new_param: f64,
        collapse_degen_intervals: bool,
    ) -> Result<()> {
        // check input
        if index >= self.size() {
            return Err(SolidError::sm_err(OP_EDIT));
        }

        // Get allowed displacement range for the new Param value
        let lo = if index > 0 {
            self.at(index - 1)?.interval.min()
        } else {
            self.interval.min()
        };
        let allowed = Extent1d::from_min_max(lo, self.at(index)?.interval.max());

        // check input - new MinParam Value is in allowed range
        if !allowed.contains_value(new_param, 0.0) {
            return Err(SolidError::sm_err(OP_EDIT));
        }

        // modify the target interval boundary position
        let moved = self.at_mut(index)?.set_interval_min(new_param).is_ok();

        // when there is a neighbor that needs to be updated
        if moved && index > 0 {
            // modifiy the matching neighbor interval boundary position (status ignored)
            ignore_status(self.at_mut(index - 1)?.set_interval_max(new_param))?;

            let tol = self.neighbour_tol(k, geometry, index - 1)?;

            // if the neighbor interval has shrunk to zero length
            if collapse_degen_intervals {
                let curve = self.curve_geometry(geometry)?;
                if curve_is_degenerate(k, Some(curve), &self.at(index - 1)?.interval, tol)? {
                    // remove the interval from the SmCurveClassification (status ignored)
                    ignore_status(self.collapse_interval(k, geometry, index - 1))?;
                }
            }
        }

        // when target interval has shrunk to zero length; past the end after a collapse,
        // upstream reads the stale slot, a copy of the last interval
        let ivl = match self.data.get(index).or_else(|| self.data.last()) {
            Some(ivl) => *ivl,
            None => return Err(SolidError::invalid_input(OP_EDIT, "no intervals left")),
        };
        self.collapse_if_degenerate(k, geometry, index, &ivl, collapse_degen_intervals)?;

        // all done
        Ok(())
    }

    /// Moves the end of interval `index` to `new_param` and the start of its successor
    /// with it (`EditIntervalMax`; upstream default `collapse_degen_intervals = TRUE`).
    /// With `collapse_degen_intervals`, the successor and then the interval are
    /// collapsed when degenerate within the pair tolerance of their ends.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `index >= size()` or `new_param` outside
    /// `[min, next max (or the classified interval's max)]`;
    /// [`SolidError::InvalidInput`] for no curve; those of the tolerances.
    pub fn edit_interval_max(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        new_param: f64,
        collapse_degen_intervals: bool,
    ) -> Result<()> {
        // check input
        let size = self.size();
        if index >= size {
            return Err(SolidError::sm_err(OP_EDIT));
        }

        // Get allowed displacement range for the new Param value
        let hi = if index < size - 1 {
            self.at(index + 1)?.interval.max()
        } else {
            self.interval.max()
        };
        let allowed = Extent1d::from_min_max(self.at(index)?.interval.min(), hi);

        // check input - new MaxParam Value is in allowed range
        if !allowed.contains_value(new_param, 0.0) {
            return Err(SolidError::sm_err(OP_EDIT));
        }

        // modify the target interval boundary position
        let moved = self.at_mut(index)?.set_interval_max(new_param).is_ok();

        // when there is a neighbor that needs to be updated
        if moved && index < size - 1 {
            // modifiy the matching neighbor interval boundary position (status ignored)
            ignore_status(self.at_mut(index + 1)?.set_interval_min(new_param))?;

            let tol = self.neighbour_tol(k, geometry, index + 1)?;

            // if the neighbor interval has shrunk to zero length
            if collapse_degen_intervals {
                let curve = self.curve_geometry(geometry)?;
                if curve_is_degenerate(k, Some(curve), &self.at(index + 1)?.interval, tol)? {
                    // remove the interval from the SmCurveClassification (status ignored)
                    ignore_status(self.collapse_interval(k, geometry, index + 1))?;
                }
            }
        }

        // when target interval has shrunk to zero length
        let ivl = *self.at(index)?;
        self.collapse_if_degenerate(k, geometry, index, &ivl, collapse_degen_intervals)?;

        // all done
        Ok(())
    }

    /// The collapse tolerance of neighbour interval `neighbour` in `EditIntervalMin` /
    /// `EditIntervalMax`: for a 3D curve the intersection tolerance of its end objects'
    /// zone tolerances, for a 2D curve the pair tolerance of the first interval's ends
    /// (as upstream).
    fn neighbour_tol(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        neighbour: usize,
    ) -> Result<f64> {
        let curve = self.curve_geometry(geometry)?;
        if curve.dim() == 3 {
            let ivl = self.at(neighbour)?;
            Ok(xsect_tol_3d_from_zones(
                k,
                ivl.start.obj_zone_tol_3d(k, geometry)?,
                ivl.end.obj_zone_tol_3d(k, geometry)?,
            ))
        } else {
            // the curve being classified is in 2d
            let first = self.at(0)?;
            xsect_tol_2d_or_3d_for_classify_pair(
                k,
                curve,
                self.surface1_geometry(geometry)?,
                first.interval.min(),
                first.start.obj_zone_tol_3d(k, geometry)?,
                first.interval.max(),
                first.end.obj_zone_tol_3d(k, geometry)?,
            )
        }
    }

    /// The tail of `EditIntervalMin` / `EditIntervalMax`: the pair tolerance of `ivl`'s
    /// ends and, with `collapse`, `CollapseInterval(index)` (status ignored) when `ivl`
    /// is degenerate within it.
    fn collapse_if_degenerate(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        ivl: &CurveInterval<O>,
        collapse: bool,
    ) -> Result<()> {
        let curve = self.curve_geometry(geometry)?;
        let tol = xsect_tol_2d_or_3d_for_classify_pair(
            k,
            curve,
            self.surface1_geometry(geometry)?,
            ivl.interval.min(),
            ivl.start.obj_zone_tol_3d(k, geometry)?,
            ivl.interval.max(),
            ivl.end.obj_zone_tol_3d(k, geometry)?,
        )?;
        if collapse && curve_is_degenerate(k, Some(curve), &ivl.interval, tol)? {
            // remove the interval from the SmCurveClassification (status ignored)
            ignore_status(self.collapse_interval(k, geometry, index))?;
        }
        Ok(())
    }

    /// The UV point of the start of interval `index` on the classified face's surface
    /// (`SmCurveInterval::GetMinUV`): the point of a 2D curve, the UV point of a curve on
    /// a surface (the caller's `uv` when that evaluation fails, its status being
    /// ignored), else the curve point dropped to the face's surface.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()` or no curve; `SM_ERR` for a 3D
    /// curve without a face or face surface; [`SolidError::Unported`] for a face the
    /// adapter cannot resolve to a B-rep ([`ClassGeometry::topology_entity`];
    /// `SmFace::GetSurface`, unit `topology.face_p2`); those of the surface drop.
    pub fn interval_min_uv(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        uv: Point2,
    ) -> Result<Point2> {
        let param = self.at(index)?.interval.min();
        self.interval_uv_at(k, geometry, param, uv)
    }

    /// The UV point of the end of interval `index` (`SmCurveInterval::GetMaxUV`; see
    /// [`CurveClassification::interval_min_uv`]).
    ///
    /// # Errors
    ///
    /// As [`CurveClassification::interval_min_uv`].
    pub fn interval_max_uv(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        uv: Point2,
    ) -> Result<Point2> {
        let param = self.at(index)?.interval.max();
        self.interval_uv_at(k, geometry, param, uv)
    }

    /// `GetMinUV` / `GetMaxUV` at curve parameter `param`.
    fn interval_uv_at(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        param: f64,
        uv: Point2,
    ) -> Result<Point2> {
        let curve = self.curve_geometry(geometry)?;

        // set output - UV point
        if curve.dim() == 2 {
            // (status ignored: an undefined point on failure)
            let p = ignored(curve.evaluate_point(k, param))?.unwrap_or(Vec3::UNDEF);
            return Ok(Point2::new(p.x, p.y));
        }
        if let CurveKind::CrvOnSurf(c) = curve.kind() {
            // (status ignored: rUV unchanged on failure)
            return Ok(ignored(c.evaluate_uv_point(k, param))?.unwrap_or(uv));
        }

        // Curve is 3d branch - Use drop point
        let Some(face) = self.face else {
            // check state - the surface must be given
            return Err(SolidError::sm_err(OP_UV));
        };
        let surface = match geometry.topology_entity(face) {
            Some((b, EntityRef::Face(f))) => face_p2::surface(b, f)?,
            Some(_) => {
                return Err(SolidError::invalid_input(
                    OP_UV,
                    "face handle is not a face",
                ))
            }
            None => {
                return Err(SolidError::unported(
                    "SmFace::GetSurface",
                    "topology.face_p2",
                ))
            }
        };
        let surface = surface.ok_or(SolidError::sm_err(OP_UV))?;

        // (EvaluatePoint's status is ignored upstream: an undefined point on failure)
        let point = ignored(curve.evaluate_point(k, param))?.unwrap_or(Vec3::UNDEF);
        let drop = surface.drop_point::<NoObject>(
            k,
            point,
            &surface.natural_uv_domain_with_nurb(k)?,
            &DropPointOptions::default(),
        )?;
        Ok(drop.result_uv)
    }
}

/// Tightens a classification-curve / `edge` intersection `solution` onto its surfaces
/// (`sm_RefineCCIToSurfaces`). The recursion counter is bumped first and nothing is
/// done past `MAX_REFINE_RECURSION` (5) levels; nothing is done either for a solution
/// that is not a point (a range whose curve parameters differ at both ends) or for a
/// classification without both its surfaces. `sss_point` is the caller's value of the
/// output point.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for no edge where the refinement starts (upstream
/// dereferences it); [`SolidError::Unported`] from there on: the edge's curve
/// (`SmEdge::GetCurve`, unit `topology.edge`) comes first.
pub fn refine_cci_to_surfaces<O: ClassObject, S: Copy>(
    classification: &CurveClassification<O>,
    solution: &Solution<S>,
    edge: Option<O>,
    sss_point: Point3,
    recursion_count: u64,
) -> Result<RefineCci> {
    // init outputs
    let out = RefineCci {
        sss_point_flag: false,
        sss_point,
        deviation: 0.0,
        recursion_count: recursion_count.saturating_add(1),
    };

    if recursion_count > MAX_REFINE_RECURSION {
        return Ok(out); // just return with no changes.
    }

    // no work case - Not an intersection curve point solution
    let point_solution = solution.solution_type == SolutionType::SingleValue
        || are_same(solution.start.param(0)?, solution.end.param(0)?)
        || are_same(solution.start.param(1)?, solution.end.param(1)?);
    if !point_solution || classification.surface1().is_none() || classification.surface2().is_none()
    {
        return Ok(out);
    }

    // get ClassCurve and EdgeCurve xsect points (pEdge->GetCurve() comes first)
    if edge.is_none() {
        return Err(SolidError::invalid_input(OP_CC, "no edge"));
    }
    // this entry point has no adapter to resolve the edge ([`refine_cci_to_surfaces_in`]
    // has one)
    // UNPORTED(topology.edge): SmEdge::GetCurve
    // Proof (unreachable): B-rep-less entry point; library callers use the `_in` twin, only
    // difftest and a unit test call this one; 0 hits in the green difftest sweep.
    Err(SolidError::unported("SmEdge::GetCurve", "topology.edge"))
}

/// [`refine_cci_to_surfaces`] with an adapter: the edge, its faces, edge uses, UV trim
/// curves and Boolean mate resolve through `geometry`
/// ([`get_cci_geometry_in`](super::p1::get_cci_geometry_in)). When the classification
/// curve, the edge curve and their surfaces already meet within the no-work tolerance
/// (edge zone tolerance / 2000), or a point does not drop to a connected surface,
/// nothing changes, as upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] when the gaps are large: upstream then moves the point to a
/// surf/surf/surf intersection and rewrites the classification curve, the edge curve
/// and its UV trim curves, which a read-only adapter cannot (the mutable adapter's
/// [`refine_cci_to_surfaces_mut`] does); those of
/// [`get_cci_geometry_in`](super::p1::get_cci_geometry_in) and
/// [`get_cci_surface_gaps`](super::p1::get_cci_surface_gaps);
/// [`SolidError::InvalidInput`] for a classification without a curve or an edge
/// without one (upstream dereferences them).
pub fn refine_cci_to_surfaces_in<O: ClassObject, S: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &CurveClassification<O>,
    solution: &Solution<S>,
    edge: O,
    sss_point: Point3,
    recursion_count: u64,
) -> Result<RefineCci> {
    let (out, large_gap) = refine_head(
        k,
        geometry,
        classification,
        solution,
        edge,
        sss_point,
        recursion_count,
    )?;
    if large_gap.is_none() {
        return Ok(out);
    }
    // Part 2 - Fix Large Gaps: rewrites the classification curve, the edge curve and its
    // UV trim curves through the mutable adapter ([`refine_cci_to_surfaces_mut`]).
    // DISPATCH GAP: SmBrep mutable owning context (read-only adapter without a Brep)
    Err(SolidError::unported(
        "SmBrep mutable owning context (sm_RefineCCIToSurfaces)",
        "topology.brep_p1",
    ))
}

/// [`refine_cci_to_surfaces_in`] over a mutable adapter: when the gaps are large the
/// intersection point moves to a surf/surf/surf intersection (part 2a), a surface/curve
/// intersection of the edge's other surface (2b) or of the classification curve's
/// other surface (2c), or is refined on both surface pairs (the last part), and the
/// classification curve (in place, through [`ClassGeometryMut::curve_mut`]), the edge
/// curve and the edge's UV trim curves (through
/// [`ClassGeometryMut::topology_entity_mut`]) are rewritten through it
/// (`sm_ApplyPointSolution`), the solution's parameters moving with them; the last part
/// then refines again while the gaps stay large (at most [`MAX_REFINE_RECURSION`]
/// levels, upstream's `lRecursionCount`).
///
/// # Errors
///
/// Those of [`refine_cci_to_surfaces_in`] other than its large-gap deferral;
/// [`SolidError::Unported`] (a dispatch gap) when the adapter cannot lend the
/// classification curve or the edge mutably, or the edge's Boolean mate lies in another
/// B-rep; [`SolidError::InvalidInput`] where upstream reads past its surface or UV point
/// lists (a failed gap measurement leaves a short UV point list).
pub fn refine_cci_to_surfaces_mut<O: ClassObject, G: ClassGeometryMut<O>, S: Copy>(
    k: &Kernel,
    geometry: &mut G,
    classification: &mut CurveClassification<O>,
    solution: &mut Solution<S>,
    edge: O,
    sss_point: Point3,
    recursion_count: u64,
) -> Result<RefineCci> {
    let (out, large_gap) = refine_head(
        k,
        &*geometry,
        classification,
        solution,
        edge,
        sss_point,
        recursion_count,
    )?;
    let Some(large_gap) = large_gap else {
        return Ok(out);
    };
    refine_large_gap(k, geometry, classification, solution, edge, out, large_gap)
}

/// The state `sm_RefineCCIToSurfaces` carries into its large-gap part, owned (the
/// surfaces and UV trim curves are copies, since the part edits the B-rep they live
/// in; `ids` tells the surfaces apart as upstream's `SmSurface*` comparisons do).
struct LargeGap<O> {
    /// `pSrcCurvePt3d`, `sEdgeCurvePoint`.
    src_curve_pt: Point3,
    edge_curve_pt: Point3,
    /// `sXSectTol3d`, `sNoWorkXSectTol3d`.
    xsect_tol_3d: f64,
    no_work_xsect_tol_3d: f64,
    /// `dCurveEdgeGap`, `dCurveSurfaceGap`, `dEdgeSurfaceGap`.
    curve_edge_gap: f64,
    curve_surface_gap: f64,
    edge_surface_gap: f64,
    /// `sUVPoints`.
    uv_points: Vec<Point2>,
    /// `pEMate`.
    emate: Option<O>,
    /// `sSurfaces`, with the identity of each (the index of its first occurrence).
    surfaces: Vec<Surface>,
    ids: Vec<usize>,
    /// `sEdgeuses`, `sUVCurves`, `lEdgeSurfaceCount`.
    edgeuses: Vec<Option<EdgeuseId>>,
    uv_curves: Vec<Option<Curve>>,
    edge_surface_count: usize,
}

/// `sm_RefineCCIToSurfaces` up to its large-gap part: the output with nothing changed,
/// and the large-gap state when the gaps are large.
fn refine_head<O: ClassObject, S: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &CurveClassification<O>,
    solution: &Solution<S>,
    edge: O,
    sss_point: Point3,
    recursion_count: u64,
) -> Result<(RefineCci, Option<LargeGap<O>>)> {
    // init outputs
    let out = RefineCci {
        sss_point_flag: false,
        sss_point,
        deviation: 0.0,
        recursion_count: recursion_count.saturating_add(1),
    };

    if recursion_count > MAX_REFINE_RECURSION {
        return Ok((out, None)); // just return with no changes.
    }

    // no work case - Not an intersection curve point solution
    let point_solution = solution.solution_type == SolutionType::SingleValue
        || are_same(solution.start.param(0)?, solution.end.param(0)?)
        || are_same(solution.start.param(1)?, solution.end.param(1)?);
    if !point_solution || classification.surface1().is_none() || classification.surface2().is_none()
    {
        return Ok((out, None));
    }

    // get ClassCurve and EdgeCurve xsect points
    let src_curve = classification.curve_geometry(geometry)?;
    let edge_curve = super::p3::edge_curve(geometry, edge)?
        .ok_or(SolidError::invalid_input(OP_CC, "edge without a curve"))?;

    let edge_zone_tol_3d =
        crate::intersect::point_class::object_zone_tol_3d(k, geometry, Some(edge))?;
    let xsect_tol_3d = xsect_tol_3d_from_zones(
        k,
        edge_zone_tol_3d,
        super::p1::src_zone_tol_3d_of(k, Some(classification)),
    );
    let no_work_xsect_tol_3d = edge_zone_tol_3d / 2000.0; // usually about 1e-8 = ApproxTol/1000 (2 gets ApproxTol)

    // (the statuses of both evaluations are ignored upstream)
    let src_curve_pt =
        ignored(src_curve.evaluate_point(k, solution.start.param(0)?))?.unwrap_or(Vec3::UNDEF);
    let edge_curve_pt =
        ignored(edge_curve.evaluate_point(k, solution.start.param(1)?))?.unwrap_or(Vec3::UNDEF);

    // get cci connected geometry and topology (sm_GetCCIGeometry always succeeds)
    let cci = super::p1::get_cci_geometry_in(geometry, classification, edge)?;

    // get CurvePoint/EdgePoint, CurvePoint/Surface, and EdgePoint/Surface gaps
    let gaps = super::p1::get_cci_surface_gaps(
        k,
        src_curve_pt,
        edge_curve_pt,
        solution,
        &cci.surfaces,
        &cci.uv_curves,
        cci.edge_surface_count,
    )?;

    // no work case - gaps are fine or current solution failed to project to all surfaces
    if !gaps.success
        || (gaps.curve_edge_gap < no_work_xsect_tol_3d
            && gaps.curve_surface_gap < no_work_xsect_tol_3d
            && gaps.edge_surface_gap < no_work_xsect_tol_3d)
    {
        return Ok((out, None));
    }

    // the large-gap part edits the B-rep the surfaces and trim curves live in: copy them
    let ids = cci
        .surfaces
        .iter()
        .map(|s| {
            cci.surfaces
                .iter()
                .position(|t| core::ptr::eq(*s, *t))
                .unwrap_or(0)
        })
        .collect();
    let large_gap = LargeGap {
        src_curve_pt,
        edge_curve_pt,
        xsect_tol_3d,
        no_work_xsect_tol_3d,
        curve_edge_gap: gaps.curve_edge_gap,
        curve_surface_gap: gaps.curve_surface_gap,
        edge_surface_gap: gaps.edge_surface_gap,
        uv_points: gaps.uv_points,
        emate: cci.emate,
        surfaces: cci.surfaces.iter().map(|s| (*s).clone()).collect(),
        ids,
        edgeuses: cci.edgeuses,
        uv_curves: cci.uv_curves.iter().map(|c| c.cloned()).collect(),
        edge_surface_count: cci.edge_surface_count,
    };
    Ok((out, Some(large_gap)))
}

/// `SM_TOL_LINE double dTripleProdLim` of part 2a.
const TRIPLE_PROD_LIM: f64 = 0.0043633092847466;

/// The entry of a list upstream indexes unchecked (`InvalidInput` past its end).
fn entry<T: Copy>(list: &[T], index: usize) -> Result<T> {
    list.get(index).copied().ok_or(SolidError::invalid_input(
        OP_REFINE,
        "index past the surface list",
    ))
}

/// `SmSurface::Evaluate(uv, 1, 1, FALSE, FALSE, FALSE, aSrfEval)` (status ignored
/// upstream): upstream's 1d layout `[D, Dv, Du, Duv]`, undefined for a failed
/// evaluation.
fn srf_eval_11(k: &Kernel, surface: &Surface, uv: Point2) -> Result<[Vec3; 4]> {
    let mut vals = [Vec3::UNDEF; 4];
    let flags = EvaluateFlags {
        u_side: Side::Right,
        v_side: Side::Right,
        only_upper_half: false,
        non_zero_tangents: true,
    };
    if let Some(d) = ignored(surface.evaluate(k, uv, 1, 1, flags))? {
        let at = |i: usize, j: usize| d.get(i).and_then(|r| r.get(j)).copied();
        for (slot, (i, j)) in vals.iter_mut().zip([(0, 0), (0, 1), (1, 0), (1, 1)]) {
            if let Some(v) = at(i, j) {
                *slot = v;
            }
        }
    }
    Ok(vals)
}

/// `SmCurve::Evaluate(t, 1, bFromLeft, pv)` (status ignored upstream): the point and
/// tangent, undefined for a failed evaluation.
fn crv_eval_1(k: &Kernel, curve: &Curve, t: f64, from_left: bool) -> Result<[Vec3; 2]> {
    let mut pv = [Vec3::UNDEF; 2];
    if let Some(d) = ignored(curve.evaluate(k, t, 1, from_left, true))? {
        for (slot, v) in pv.iter_mut().zip(d) {
            *slot = v;
        }
    }
    Ok(pv)
}

/// `EvaluateNormal(uv, FALSE, FALSE, n)` with its status ignored (an undefined vector
/// for a failure).
fn normal_or_undef(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Vec3> {
    Ok(ignored(surface.evaluate_normal(k, uv, Side::Right, Side::Right))?.unwrap_or(Vec3::UNDEF))
}

/// `SmSurfaceIntersector(s1, s1->GetNaturalUVDomain(), s2, ..).RefinePoint(uv1, uv2,
/// SM_IP_CROSSING, &normal, NULL, FALSE, bPtWasRefined, sUVs)` with its status ignored:
/// `bFoundAnswer` is set `TRUE` before a failing local intersection, and `sUVs` keep
/// their (undefined) values when nothing is refined.
fn refine_on_pair(
    k: &Kernel,
    s1: &Surface,
    s2: &Surface,
    uv1: Point2,
    uv2: Point2,
    normal: Vec3,
) -> Result<(bool, [Point2; 2])> {
    let ssi = SurfaceIntersector::new(
        k,
        s1,
        &s1.natural_uv_domain_with_nurb(k)?,
        s2,
        &s2.natural_uv_domain_with_nurb(k)?,
        false,
    )?;
    let r = ssi.refine_point(
        k,
        uv1,
        uv2,
        IntersectionPointType::Crossing,
        Some(normal),
        None,
        false,
    );
    Ok(match ignored(r)? {
        Some(Some(uvs)) => (true, uvs),
        Some(None) => (false, [Point2::UNDEF; 2]),
        None => (true, [Point2::UNDEF; 2]),
    })
}

/// The B-rep edge behind `edge` (`SmEdge::GetInterval` and the like).
fn tail_edge_interval<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: O,
) -> Result<Extent1d> {
    let (b, e) = super::p7::edge_of(geometry, edge, "SmEdge::GetInterval")?;
    crate::topology::edge::interval(b, e)
}

/// `pEdge->GetCurve()`, copied.
fn tail_edge_curve<O: ClassObject>(geometry: &impl ClassGeometry<O>, edge: O) -> Result<Curve> {
    Ok(super::p3::edge_curve(geometry, edge)?
        .ok_or(SolidError::invalid_input(OP_REFINE, "edge without a curve"))?
        .clone())
}

/// Whether `surface` is owned by face `f` of `b` (`sFaceList[ii] ==
/// pCSurfaceUnique->GetOwner()`): the owner reference is the face's (its recorded
/// geometry owner key, its notify reference, or the default owner of the face's own
/// surface).
fn is_owned_by_face(b: &Brep, f: crate::topology::FaceId, surface: &Surface) -> bool {
    let Some(owner) = surface.owner() else {
        return false;
    };
    let entity = EntityRef::Face(f);
    b.geometry_owner_key(entity) == Some(owner)
        || b.notify_serial()
            .is_some_and(|s| crate::topology::brep::p3::topology_ref(s, entity) == owner)
        || (b.face(f).ok().and_then(|r| r.surface.as_ref()?.owner()) == Some(owner)
            && crate::topology::brep::p1::default_face_owner(b, f) == owner)
}

/// What [`tail_apply`] changed (`bModifiedCurve`, `bModifiedEdgeCurve`,
/// `bModifiedMateCurve`).
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct Modified {
    curve: bool,
    edge_curve: bool,
    mate_curve: bool,
}

/// `sm_ApplyPointSolution(with_curve ? pCurveClassification : NULL, edge, emate,
/// surfaces..., rSolution, vSSSPoint, bSSSPointFlag, .., dDeviation)` over the mutable
/// adapter: the classification curve is edited on a copy written back in place, the
/// edge (and its mate, in the same B-rep) through
/// [`apply_point_solution_in`](super::p1::apply_point_solution_in). The status is
/// ignored upstream; a failure leaves the out-params as its entry reset them.
#[allow(clippy::too_many_arguments)]
fn tail_apply<O: ClassObject, G: ClassGeometryMut<O>, S: Copy>(
    k: &Kernel,
    geometry: &mut G,
    classification: &CurveClassification<O>,
    with_curve: bool,
    edge: Option<O>,
    emate: Option<O>,
    surfaces: [(&Surface, Point2); 4],
    solution: &mut Solution<S>,
    out: &mut RefineCci,
) -> Result<Modified> {
    let src_zone_tol_3d = super::p1::src_zone_tol_3d_of(k, with_curve.then_some(classification));
    let mut src_curve = None;
    if with_curve {
        let h = classification.curve().ok_or(SolidError::invalid_input(
            OP_REFINE,
            "no classification curve",
        ))?;
        if geometry.curve_mut(h).is_none() {
            return Err(mutable_gap(
                "SmBrep mutable owning context (sm_SetXSectCurveThroughPoint)",
            ));
        }
        src_curve = Some((h, classification.curve_geometry(&*geometry)?.clone()));
    }
    let mate_edge = match (edge, emate) {
        (Some(_), Some(m)) => Some(super::p7::edge_of(&*geometry, m, "SmEdge::GetCurve")?.1),
        _ => None,
    };
    let [s1, s2, s3, s4] = surfaces;
    let input = super::p1::ApplyPointSolutionInput::<O> {
        src_zone_tol_3d,
        edge: None,
        emate: None,
        class_curve_surface1: Some(s1),
        class_curve_surface2: Some(s2),
        edge_curve_surface1: Some(s3),
        edge_curve_surface2: Some(s4),
    };
    let edges = match edge {
        Some(e) => match geometry.topology_entity_mut(e) {
            Some((brep, EntityRef::Edge(edge))) => Some(super::p1::ApplyPointSolutionEdges {
                brep,
                edge,
                mate: mate_edge.map(|me| (None, me)),
            }),
            _ => {
                return Err(mutable_gap(
                    "SmBrep mutable owning context (sm_SetXSectEdgeThroughPoint)",
                ))
            }
        },
        None => None,
    };
    let r = ignored(super::p1::apply_point_solution_in(
        k,
        src_curve.as_mut().map(|(_, c)| c),
        &input,
        edges,
        solution,
        out.sss_point,
    ))?;
    // the classification curve is edited in place upstream
    if let Some((h, c)) = src_curve {
        if let Some(slot) = geometry.curve_mut(h) {
            *slot = c;
        }
    }
    if let Some(r) = r {
        out.sss_point = r.sss_point;
        out.sss_point_flag = r.sss_point_flag;
        out.deviation = r.deviation;
        Ok(Modified {
            curve: r.modified_curve,
            edge_curve: r.modified_edge_curve,
            mate_curve: r.modified_mate_curve,
        })
    } else {
        out.sss_point_flag = false;
        out.deviation = 0.0;
        Ok(Modified {
            curve: false,
            edge_curve: false,
            mate_curve: false,
        })
    }
}

/// The edits after each `sm_ApplyPointSolution` of the large-gap part: the curves are
/// read again, the classification curve is marked not current when it changed, and
/// every edge use's UV trim curve is dropped when an edge curve changed
/// (`SetUVTrimCurve(NULL, 0.0, TRUE)`; the local list loses every UV curve).
fn tail_after_apply<O: ClassObject, G: ClassGeometryMut<O>>(
    geometry: &mut G,
    classification: &mut CurveClassification<O>,
    edge: O,
    modified: Modified,
    st: &mut LargeGap<O>,
) -> Result<(Curve, Curve)> {
    let src_curve = classification.curve_geometry(&*geometry)?.clone();
    let edge_curve = tail_edge_curve(&*geometry, edge)?;
    if modified.curve {
        classification.set_is_curve_current(false);
    }
    if modified.edge_curve || modified.mate_curve {
        for (ii, eu) in st.edgeuses.iter().enumerate() {
            if let Some(eu) = eu {
                match geometry.topology_entity_mut(edge) {
                    Some((b, EntityRef::Edge(_))) => {
                        crate::topology::edgeuse::p1::delete_uv_trim_curve(b, *eu)?;
                    }
                    _ => {
                        return Err(mutable_gap(
                            "SmBrep mutable owning context (SmEdgeuse::SetUVTrimCurve)",
                        ))
                    }
                }
            }
            if let Some(c) = st.uv_curves.get_mut(ii) {
                *c = None;
            }
        }
    }
    Ok((src_curve, edge_curve))
}

/// `SER(pSrcCurve3d->LocalCurveIntersect(..))` and the solution update after it:
/// `false` for a failing status (upstream returns it; every caller ignores it).
#[allow(clippy::too_many_arguments)]
fn tail_curve_curve<O: ClassObject, S: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &CurveClassification<O>,
    edge: O,
    src_curve: &Curve,
    edge_curve: &Curve,
    xsect_tol_3d: f64,
    solution: &mut Solution<S>,
    out: &mut RefineCci,
) -> Result<bool> {
    let edge_interval = tail_edge_interval(geometry, edge)?;
    let r = ignored(crate::curve::curve::p3::local_curve_intersect(
        k,
        src_curve,
        &classification.interval(),
        edge_curve,
        &edge_interval,
        xsect_tol_3d,
        solution.start.param(0)?,
        solution.start.param(1)?,
    ))?;
    let Some(r) = r else {
        return Ok(false);
    };
    if let Some(ci) = r {
        out.deviation = ci.deviation;
        solution.start.solution_value = ci.deviation;
        *solution.start.param_mut(0)? = ci.my_param;
        *solution.start.param_mut(1)? = ci.other_param;
    }
    // (else upstream only warns: refined curve/edge intersection failed)
    Ok(true)
}

/// The mutable-adapter dispatch gap of the large-gap part: an adapter that cannot lend
/// what upstream edits (`upstream` names the edit).
fn mutable_gap(upstream: &'static str) -> SolidError {
    // DISPATCH GAP: SmBrep mutable owning context (an adapter that cannot lend the
    // object mutably)
    SolidError::unported(upstream, "topology.brep_p1")
}

/// Part 2 of `sm_RefineCCIToSurfaces` (Fix Large Gaps), over the mutable adapter.
#[allow(clippy::too_many_lines)]
fn refine_large_gap<O: ClassObject, G: ClassGeometryMut<O>, S: Copy>(
    k: &Kernel,
    geometry: &mut G,
    classification: &mut CurveClassification<O>,
    solution: &mut Solution<S>,
    edge: O,
    mut out: RefineCci,
    mut st: LargeGap<O>,
) -> Result<RefineCci> {
    // a Boolean mate in another B-rep would need both B-reps lent at once
    if let Some(m) = st.emate {
        let (eb, _) = super::p7::edge_of(&*geometry, edge, "SmEdge::GetCurve")?;
        let (mb, _) = super::p7::edge_of(&*geometry, m, "SmEdge::GetCurve")?;
        if !core::ptr::eq(eb, mb) {
            return Err(mutable_gap("SmBrep mutable owning context (sm_RefineCCIToSurfaces, Boolean mate in another B-rep)"));
        }
    }

    // locals (OLDTOL: dThisAngTolRad = 6 * GetAngTolRad(), sScaledZero = GetEffZero())
    let this_ang_tol_rad = 6.0 * ang_tol_rad();
    let scaled_zero = eff_zero();
    let xsect_tol_3d = st.xsect_tol_3d;
    let no_work = st.no_work_xsect_tol_3d;
    let src_curve_pt = st.src_curve_pt;
    let edge_curve_pt = st.edge_curve_pt;
    let (curve_edge_gap, curve_surface_gap, edge_surface_gap) =
        (st.curve_edge_gap, st.curve_surface_gap, st.edge_surface_gap);
    let mut src_curve = classification.curve_geometry(&*geometry)?.clone();
    let mut edge_curve = tail_edge_curve(&*geometry, edge)?;
    let mut new_class_curve_point = Vec3::UNDEF;
    let mut new_edge_curve_point = Vec3::UNDEF;

    let n_surfaces = st.surfaces.len();
    let surfaces = core::mem::take(&mut st.surfaces);
    let srf = |i: usize| -> Result<&Surface> {
        surfaces.get(i).ok_or(SolidError::invalid_input(
            OP_REFINE,
            "index past the surface list",
        ))
    };
    let ids = core::mem::take(&mut st.ids);
    let id = |i: usize| -> Result<usize> { entry(&ids, i) };
    let surface1 = srf(0)?;
    let surface2 = srf(1)?;
    let e_surface_unique_ix = if st.edge_surface_count > 1 {
        2
    } else {
        n_surfaces.saturating_sub(1)
    };
    let e_surface_shared_ix = (2 + st.edge_surface_count).saturating_sub(1);
    let e_surface_unique = srf(e_surface_unique_ix)?;
    let e_surface_shared = srf(e_surface_shared_ix)?;
    let defined = |p: Point2| p.x != UNDEF_DOUBLE;
    let uv0 = entry(&st.uv_points, 0)?;
    let uv1 = entry(&st.uv_points, 1)?;
    let have_srf1_eval = defined(uv0);
    let have_srf2_eval = defined(uv1);
    let mut srf1_norm = Vec3::UNDEF;
    let mut srf2_norm = Vec3::UNDEF;
    if have_srf1_eval {
        let eval = srf_eval_11(k, surface1, uv0)?;
        srf1_norm = entry(&eval, 1)?.cross(entry(&eval, 2)?);
        match srf1_norm.unitize() {
            Ok(n) => srf1_norm = n,
            Err(_) => match ignored(surface1.evaluate_normal(k, uv0, Side::Right, Side::Right))? {
                Some(n) => srf1_norm = n,
                None => return Ok(out),
            },
        }
    }
    if have_srf2_eval {
        let eval = srf_eval_11(k, surface2, uv1)?;
        srf2_norm = entry(&eval, 1)?.cross(entry(&eval, 2)?);
        match srf2_norm.unitize() {
            Ok(n) => srf2_norm = n,
            Err(_) => match ignored(surface2.evaluate_normal(k, uv1, Side::Right, Side::Right))? {
                Some(n) => srf2_norm = n,
                None => return Ok(out),
            },
        }
    }

    // Part 2a - surf/surf/surf intersection with each edge surface that is not a curve
    // surface (skipped when the curve surfaces are parallel)
    let mut ii = 2;
    if have_srf1_eval
        && have_srf2_eval
        && are_parallel(srf1_norm, srf2_norm, Some(this_ang_tol_rad))
    {
        ii = n_surfaces + 1;
    }
    while ii < n_surfaces {
        let surf = srf(ii)?;
        let this = ii;
        ii += 1;
        if id(this)? == id(0)? || id(this)? == id(1)? {
            continue;
        }
        let uv_ii = entry(&st.uv_points, this)?;
        if have_srf1_eval && have_srf2_eval && defined(uv_ii) {
            let this_norm = normal_or_undef(k, surf, uv_ii)?;
            if are_parallel(this_norm, srf1_norm, Some(this_ang_tol_rad)) {
                continue;
            }
            if are_parallel(this_norm, srf2_norm, Some(this_ang_tol_rad)) {
                continue;
            }
            let triple_prod = this_norm.dot(srf1_norm.cross(srf2_norm));
            if triple_prod.abs() < TRIPLE_PROD_LIM {
                continue;
            }
        }
        let expanded = |s: &Surface| -> Result<crate::geom::extent2d::Extent2d> {
            let mut d = s.natural_uv_domain_with_nurb(k)?;
            d.expand_absolute(0.01);
            Ok(d)
        };
        let uv_0 = entry(&st.uv_points, 0)?;
        let uv_1 = entry(&st.uv_points, 1)?;
        let found: Option<[Point2; 3]> = if defined(uv_0) && defined(uv_1) && defined(uv_ii) {
            ignored(surface1.local_surface_surface_intersect(
                k,
                &expanded(surface1)?,
                surface2,
                &expanded(surface2)?,
                surf,
                &expanded(surf)?,
                uv_0,
                uv_1,
                uv_ii,
            ))?
            .flatten()
            .map(|s| [s.uv1, s.uv2, s.uv3])
        } else {
            match ignored(surface1.local_surface_surface_intersect_xyz::<NoObject>(
                k,
                &expanded(surface1)?,
                surface2,
                &expanded(surface2)?,
                surf,
                &expanded(surf)?,
                src_curve_pt,
                Some(edge_curve_pt),
            ))? {
                Some((true, sol)) => Some([
                    Point2::new(sol.start.param(0)?, sol.start.param(1)?),
                    Point2::new(sol.start.param(2)?, sol.start.param(3)?),
                    Point2::new(sol.start.param(4)?, sol.start.param(5)?),
                ]),
                _ => None,
            }
        };
        if let Some([uv_point1, uv_point2, uv_point3]) = found {
            let uv_point4 = if id(e_surface_shared_ix)? == id(0)? {
                uv_point1
            } else {
                uv_point2
            };
            let modified = tail_apply(
                k,
                geometry,
                classification,
                true,
                Some(edge),
                st.emate,
                [
                    (surface1, uv_point1),
                    (surface2, uv_point2),
                    (surf, uv_point3),
                    (e_surface_shared, uv_point4),
                ],
                solution,
                &mut out,
            )?;
            tail_after_apply(geometry, classification, edge, modified, &mut st)?;
            return Ok(out);
        }
    }

    // Part 2b - the classification curve is on its surfaces but the edge is not: move
    // the point to the classification curve / unique edge surface intersection
    if curve_surface_gap <= no_work
        && edge_surface_gap > no_work
        && e_surface_unique_ix != e_surface_shared_ix
    {
        let uv_unique = entry(&st.uv_points, e_surface_unique_ix)?;
        let mut found: Option<(f64, Point2)> = None;
        if defined(uv_unique) {
            let crv_pt_tan = crv_eval_1(k, &src_curve, solution.start.param(0)?, false)?;
            let tan = entry(&crv_pt_tan, 1)?.unitize_or_z();
            let srf_norm = normal_or_undef(k, e_surface_unique, uv_unique)?;
            if !are_perpendicular(tan, srf_norm, Some(this_ang_tol_rad)) {
                found = ignored(e_surface_unique.local_curve_intersect(
                    k,
                    &e_surface_unique.natural_uv_domain_with_nurb(k)?,
                    &src_curve,
                    &src_curve.natural_interval(),
                    xsect_tol_3d, /* was 1.0, too big [B254] */
                    uv_unique,
                    solution.start.param(0)?,
                ))?
                .flatten()
                .map(|ci| (ci.t, ci.uv));
            }
        } else {
            let sols = ignored(e_surface_unique.global_curve_intersect_in::<NoObject>(
                k,
                [None, None],
                None,
                &e_surface_unique.natural_uv_domain_with_nurb(k)?,
                &src_curve,
                &src_curve.natural_interval(),
                xsect_tol_3d, /* was 1.0, too big [b254] */
            ))?;
            if let Some(sol) = sols.as_ref().and_then(|s| s.get(0)) {
                found = Some((
                    sol.start.param(0)?,
                    Point2::new(sol.start.param(1)?, sol.start.param(2)?),
                ));
            }
        }
        if let Some((t, uv)) = found {
            if let Some(p) = ignored(src_curve.evaluate_point(k, t))? {
                new_class_curve_point = p;
            }
            if let Some(p) = ignored(e_surface_unique.evaluate_point(k, uv))? {
                new_edge_curve_point = p;
            }
            let new_edge_curve_gap = new_edge_curve_point.distance_squared(edge_curve_pt);
            let new_class_curve_gap = new_class_curve_point.distance_squared(src_curve_pt);
            if new_edge_curve_gap < scaled_zero_sq_of(&new_edge_curve_point)
                && new_class_curve_gap < scaled_zero_sq_of(&new_class_curve_point)
            {
                return Ok(out);
            }
            let gaps = new_gaps(
                k,
                new_class_curve_point,
                new_edge_curve_point,
                solution,
                &surfaces,
                &st,
            )?;
            st.uv_points = gaps.uv_points;
            if gaps.success
                && gaps.curve_edge_gap <= curve_edge_gap + scaled_zero
                && gaps.curve_surface_gap <= curve_surface_gap + scaled_zero
                && gaps.edge_surface_gap <= edge_surface_gap + scaled_zero
            {
                let mut pt_was_refined = gaps.edge_surface_gap < EFF_ZERO;
                let mut move_dist = 0.0;
                if !pt_was_refined && id(e_surface_unique_ix)? != id(e_surface_shared_ix)? {
                    let pv = crv_eval_1(k, &edge_curve, solution.start.param(1)?, true)?;
                    let mut degenerate = false;
                    let crv_tan = entry(&pv, 1)?.unitize_or_z();
                    let uv_shared = entry(&st.uv_points, e_surface_shared_ix)?;
                    let unique_normal = normal_or_undef(k, e_surface_unique, uv)?;
                    let shared_normal = normal_or_undef(k, e_surface_shared, uv_shared)?;
                    if are_parallel(unique_normal, shared_normal, Some(this_ang_tol_rad)) {
                        degenerate = true;
                    }
                    if !degenerate && are_parallel(crv_tan, unique_normal, Some(this_ang_tol_rad)) {
                        degenerate = true;
                    }
                    if !degenerate && are_parallel(crv_tan, shared_normal, Some(this_ang_tol_rad)) {
                        degenerate = true;
                    }
                    let mut uvs = [Point2::UNDEF; 2];
                    if degenerate {
                        pt_was_refined = false;
                        move_dist = BIG_DOUBLE;
                    } else {
                        (pt_was_refined, uvs) = refine_on_pair(
                            k,
                            e_surface_unique,
                            e_surface_shared,
                            uv,
                            uv_shared,
                            entry(&pv, 1)?,
                        )?;
                    }
                    if pt_was_refined {
                        set_uv(&mut st.uv_points, e_surface_unique_ix, uvs[0])?;
                        set_uv(&mut st.uv_points, e_surface_shared_ix, uvs[1])?;
                    }
                } else if !pt_was_refined {
                    move_dist = edge_curve_pt.distance(new_edge_curve_point);
                    if move_dist > 1.5 * edge_surface_gap {
                        let drop = ignored(edge_curve.local_point_solve::<NoObject>(
                            k,
                            &tail_edge_interval(&*geometry, edge)?,
                            SolverOperation::Minimize,
                            new_edge_curve_point,
                            &PointSolveOptions::default(),
                            solution.start.param(1)?,
                        ))?;
                        if let Some((true, sol)) = drop {
                            move_dist = sol.start.solution_value;
                        }
                    }
                }
                if pt_was_refined || move_dist < 1.5 * edge_surface_gap {
                    let uvp = |i: usize| entry(&st.uv_points, i);
                    let args = [
                        (surface1, uvp(0)?),
                        (surface2, uvp(1)?),
                        (e_surface_unique, uvp(e_surface_unique_ix)?),
                        (e_surface_shared, uvp(e_surface_shared_ix)?),
                    ];
                    let modified = tail_apply(
                        k,
                        geometry,
                        classification,
                        false,
                        Some(edge),
                        st.emate,
                        args,
                        solution,
                        &mut out,
                    )?;
                    let (src, edg) =
                        tail_after_apply(geometry, classification, edge, modified, &mut st)?;
                    tail_curve_curve(
                        k,
                        &*geometry,
                        classification,
                        edge,
                        &src,
                        &edg,
                        xsect_tol_3d,
                        solution,
                        &mut out,
                    )?;
                    return Ok(out);
                }
            }
        }
    }

    // Part 2c - the edge is on its surfaces but the classification curve is not: move
    // the point to the edge curve / unique classification-curve surface intersection
    if edge_surface_gap <= no_work && curve_surface_gap > no_work {
        let c_surface_unique_ix = usize::from(id(0)? == id(e_surface_shared_ix)?);
        let c_surface_shared_ix = 1 - c_surface_unique_ix;
        let c_surface_unique = srf(c_surface_unique_ix)?;
        let c_surface_shared = srf(c_surface_shared_ix)?;
        let mut is_edge_of_face = false;
        {
            let (eb, e) = super::p7::edge_of(&*geometry, edge, "SmEdge::GetFaces")?;
            for f in crate::topology::edge::faces(eb, e, None)? {
                if is_owned_by_face(eb, f, c_surface_unique) {
                    is_edge_of_face = true;
                    break;
                }
            }
        }
        let mut found: Option<(f64, Point2)> = None;
        if !is_edge_of_face {
            let uv_unique = entry(&st.uv_points, c_surface_unique_ix)?;
            if defined(uv_unique) {
                let crv_pt_tan = crv_eval_1(k, &edge_curve, solution.start.param(1)?, false)?;
                let tan = entry(&crv_pt_tan, 1)?.unitize_or_z();
                let srf_norm = normal_or_undef(k, c_surface_unique, uv_unique)?;
                if !are_perpendicular(tan, srf_norm, Some(this_ang_tol_rad)) {
                    found = ignored(c_surface_unique.local_curve_intersect(
                        k,
                        &c_surface_unique.natural_uv_domain_with_nurb(k)?,
                        &edge_curve,
                        &edge_curve.natural_interval(),
                        xsect_tol_3d, /* was 1.0 , which is too big [B254] */
                        uv_unique,
                        solution.start.param(1)?,
                    ))?
                    .flatten()
                    .map(|ci| (ci.t, ci.uv));
                }
            } else {
                let sols = ignored(c_surface_unique.global_curve_intersect_in::<NoObject>(
                    k,
                    [None, None],
                    None,
                    &c_surface_unique.natural_uv_domain_with_nurb(k)?,
                    &edge_curve,
                    &edge_curve.natural_interval(),
                    xsect_tol_3d, /* was 1.0, too big [B254] */
                ))?;
                if let Some(sol) = sols.as_ref().and_then(|s| s.get(0)) {
                    found = Some((
                        sol.start.param(0)?,
                        Point2::new(sol.start.param(1)?, sol.start.param(2)?),
                    ));
                }
            }
        }
        if let Some((t, uv)) = found {
            if let Some(p) = ignored(edge_curve.evaluate_point(k, t))? {
                new_edge_curve_point = p;
            }
            if let Some(p) = ignored(c_surface_unique.evaluate_point(k, uv))? {
                new_class_curve_point = p;
            }
            let new_edge_curve_gap = new_edge_curve_point.distance_squared(edge_curve_pt);
            let new_class_curve_gap = new_class_curve_point.distance_squared(src_curve_pt);
            if new_edge_curve_gap < 10000.0 * EFF_ZERO_SQ
                && new_class_curve_gap < 10000.0 * EFF_ZERO_SQ
            {
                return Ok(out);
            }
            let gaps = new_gaps(
                k,
                new_class_curve_point,
                new_edge_curve_point,
                solution,
                &surfaces,
                &st,
            )?;
            st.uv_points = gaps.uv_points;
            if gaps.success
                && gaps.curve_edge_gap <= curve_edge_gap + EFF_ZERO
                && gaps.curve_surface_gap <= curve_surface_gap + EFF_ZERO
                && gaps.edge_surface_gap <= edge_surface_gap + EFF_ZERO
            {
                let mut pt_was_refined = gaps.curve_surface_gap < EFF_ZERO;
                if !pt_was_refined {
                    // (upstream evaluates the classification curve at the edge parameter)
                    let pv3d = crv_eval_1(k, &src_curve, solution.start.param(1)?, true)?;
                    let uvs;
                    (pt_was_refined, uvs) = refine_on_pair(
                        k,
                        c_surface_unique,
                        c_surface_shared,
                        uv,
                        entry(&st.uv_points, c_surface_shared_ix)?,
                        entry(&pv3d, 1)?,
                    )?;
                    set_uv(&mut st.uv_points, c_surface_unique_ix, uvs[0])?;
                    set_uv(&mut st.uv_points, c_surface_shared_ix, uvs[1])?;
                }
                if pt_was_refined {
                    let uvp = |i: usize| entry(&st.uv_points, i);
                    let args = [
                        (surface1, uvp(0)?),
                        (surface2, uvp(1)?),
                        (e_surface_unique, uvp(e_surface_unique_ix)?),
                        (e_surface_shared, uvp(e_surface_shared_ix)?),
                    ];
                    let modified = tail_apply(
                        k,
                        geometry,
                        classification,
                        true,
                        None,
                        None,
                        args,
                        solution,
                        &mut out,
                    )?;
                    let (src, edg) =
                        tail_after_apply(geometry, classification, edge, modified, &mut st)?;
                    tail_curve_curve(
                        k,
                        &*geometry,
                        classification,
                        edge,
                        &src,
                        &edg,
                        xsect_tol_3d,
                        solution,
                        &mut out,
                    )?;
                    return Ok(out);
                }
            }
        }
    }

    // Part 2d - refine the point on both surface pairs
    let pv3d_1 = crv_eval_1(k, &src_curve, solution.start.param(0)?, true)?;
    let pv3d_2 = crv_eval_1(k, &edge_curve, solution.start.param(1)?, true)?;
    let mut pt_was_refined1 = false;
    let mut pt_was_refined2 = false;
    let mut uv1s = [Point2::UNDEF; 2];
    let mut uv2s = [Point2::UNDEF; 2];
    if curve_surface_gap >= no_work {
        let mut degenerate = false;
        if are_parallel(srf1_norm, srf2_norm, Some(this_ang_tol_rad)) {
            degenerate = true;
        }
        if !degenerate {
            let crv_tan = entry(&pv3d_1, 1)?.unitize_or_z();
            if are_parallel(crv_tan, srf1_norm, Some(this_ang_tol_rad)) {
                degenerate = true;
            }
            if !degenerate && are_parallel(crv_tan, srf2_norm, Some(this_ang_tol_rad)) {
                degenerate = true;
            }
        }
        if !degenerate {
            (pt_was_refined1, uv1s) = refine_on_pair(
                k,
                surface1,
                surface2,
                entry(&st.uv_points, 0)?,
                entry(&st.uv_points, 1)?,
                entry(&pv3d_1, 1)?,
            )?;
            // (else upstream only warns: the curve/curve point failed to project)
        }
        if pt_was_refined1 {
            set_uv(&mut st.uv_points, 0, uv1s[0])?;
            set_uv(&mut st.uv_points, 1, uv1s[1])?;
        }
    }
    if edge_surface_gap >= no_work && id(e_surface_unique_ix)? != id(e_surface_shared_ix)? {
        let mut degenerate = false;
        let crv_tan = entry(&pv3d_2, 1)?.unitize_or_z();
        let unique_normal = normal_or_undef(
            k,
            e_surface_unique,
            entry(&st.uv_points, e_surface_unique_ix)?,
        )?;
        let shared_normal = normal_or_undef(
            k,
            e_surface_shared,
            entry(&st.uv_points, e_surface_shared_ix)?,
        )?;
        if are_parallel(unique_normal, shared_normal, Some(this_ang_tol_rad)) {
            degenerate = true;
        }
        if !degenerate && are_parallel(crv_tan, unique_normal, Some(this_ang_tol_rad)) {
            degenerate = true;
        }
        if !degenerate && are_parallel(crv_tan, shared_normal, Some(this_ang_tol_rad)) {
            degenerate = true;
        }
        if !degenerate {
            (pt_was_refined2, uv2s) = refine_on_pair(
                k,
                e_surface_unique,
                e_surface_shared,
                entry(&st.uv_points, e_surface_unique_ix)?,
                entry(&st.uv_points, e_surface_shared_ix)?,
                entry(&pv3d_2, 1)?,
            )?;
        }
        if pt_was_refined2 {
            set_uv(&mut st.uv_points, e_surface_unique_ix, uv2s[0])?;
            set_uv(&mut st.uv_points, e_surface_shared_ix, uv2s[1])?;
        }
    }
    if pt_was_refined1 || pt_was_refined2 {
        let modified = tail_apply(
            k,
            geometry,
            classification,
            pt_was_refined1,
            pt_was_refined2.then_some(edge),
            if pt_was_refined2 { st.emate } else { None },
            [
                (surface1, uv1s[0]),
                (surface2, uv1s[1]),
                (e_surface_unique, uv2s[0]),
                (e_surface_shared, uv2s[1]),
            ],
            solution,
            &mut out,
        )?;
        let (src, edg) = tail_after_apply(geometry, classification, edge, modified, &mut st)?;
        src_curve = src;
        edge_curve = edg;
        if !tail_curve_curve(
            k,
            &*geometry,
            classification,
            edge,
            &src_curve,
            &edge_curve,
            xsect_tol_3d,
            solution,
            &mut out,
        )? {
            return Ok(out);
        }
        if let Some(p) = ignored(src_curve.evaluate_point(k, solution.start.param(0)?))? {
            new_class_curve_point = p;
        }
        if let Some(p) = ignored(edge_curve.evaluate_point(k, solution.start.param(1)?))? {
            new_edge_curve_point = p;
        }
        let new_edge_curve_gap = new_edge_curve_point.distance_squared(edge_curve_pt);
        let new_class_curve_gap = new_class_curve_point.distance_squared(src_curve_pt);
        if new_edge_curve_gap < new_edge_curve_point.scaled_zero()
            && new_class_curve_gap < new_class_curve_point.scaled_zero()
        {
            return Ok(out);
        }
        let gaps = new_gaps(
            k,
            new_class_curve_point,
            new_edge_curve_point,
            solution,
            &surfaces,
            &st,
        )?;
        if gaps.success
            && (gaps.curve_edge_gap > no_work
                || gaps.curve_surface_gap > no_work
                || gaps.edge_surface_gap > no_work)
        {
            // the recursion resets bSSSPointFlag and dDeviation and shares the counter
            // (input-bounded: at most MAX_REFINE_RECURSION levels)
            out = refine_cci_to_surfaces_mut(
                k,
                geometry,
                classification,
                solution,
                edge,
                out.sss_point,
                out.recursion_count,
            )?;
        }
    }
    Ok(out)
}

/// `sm_GetCCISurfaceGaps` of the large-gap part over its copied surfaces and its
/// current UV curve list.
fn new_gaps<O, S: Copy>(
    k: &Kernel,
    class_curve_point: Point3,
    edge_curve_point: Point3,
    solution: &Solution<S>,
    surfaces: &[Surface],
    st: &LargeGap<O>,
) -> Result<super::p1::CciSurfaceGaps> {
    let srfs: Vec<&Surface> = surfaces.iter().collect();
    let uvs: Vec<Option<&Curve>> = st.uv_curves.iter().map(Option::as_ref).collect();
    super::p1::get_cci_surface_gaps(
        k,
        class_curve_point,
        edge_curve_point,
        solution,
        &srfs,
        &uvs,
        st.edge_surface_count,
    )
}

/// `sUVPoints[i] = uv` (`InvalidInput` past the list, which upstream writes unchecked).
fn set_uv(points: &mut [Point2], i: usize, uv: Point2) -> Result<()> {
    *points.get_mut(i).ok_or(SolidError::invalid_input(
        OP_REFINE,
        "index past the UV point list",
    ))? = uv;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::Curve;
    use crate::curve::line::Line;
    use crate::intersect::point_class::ClassObjectKind;
    use crate::surface::surface::Surface;
    use crate::surface::volume::Volume;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    struct G(Vec<Curve>);

    impl ClassGeometry<H> for G {
        fn curve(&self, o: H) -> Option<&Curve> {
            self.0.get(o.0)
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _o: H) -> Option<Point3> {
            None
        }
    }

    fn line() -> G {
        let c = Line::from_points(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0), 3);
        G(vec![Curve::from(c)])
    }

    fn cc(k: &Kernel, bounds: &[f64]) -> CurveClassification<H> {
        let mut c = CurveClassification::new(k);
        c.set_curve(Some(H(0, ClassObjectKind::Curve)), false);
        c.set_size(k, bounds.len() - 1).ok();
        for (i, w) in bounds.windows(2).enumerate() {
            c.at_mut(i)
                .unwrap()
                .set_interval(&Extent1d::from_min_max(w[0], w[1]));
        }
        c.interval = Extent1d::from_min_max(bounds[0], *bounds.last().unwrap());
        c
    }

    #[test]
    fn insert_reset_assign() {
        let k = Kernel::default();
        let mut c = cc(&k, &[0.0, 0.5, 1.0]);
        let ivl = CurveInterval::with_interval(&k, &Extent1d::from_min_max(0.2, 0.3), None);
        assert!(c.insert_at(&k, 0, ivl).is_err());
        c.insert_at(&k, 1, ivl).unwrap();
        assert_eq!(c.size(), 3);
        assert_eq!(c.at(1).unwrap().min(), 0.2);
        assert_eq!(c.at(2).unwrap().min(), 0.5);
        c.insert_at(&k, 5, ivl).unwrap();
        assert_eq!(c.size(), 6);

        let mut d = CurveClassification::new(&k);
        d.assign_from(&k, &line(), &c).unwrap();
        assert_eq!(d, c);

        c.reset(&k, &ResetOptions::default()).unwrap();
        assert_eq!(c.size(), 1);
        assert_eq!(c.at(0).unwrap().start.src_zone_tol_3d(), ZONE_TOL_3D);
        let opts = ResetOptions {
            uv_curve: Some(H(0, ClassObjectKind::Curve)),
            zone_tol_3d: 0.0,
            ..ResetOptions::default()
        };
        assert!(c.reset(&k, &opts).is_err());
        let opts = ResetOptions {
            interval: Some(Extent1d::from_min_max(0.0, 2.0)),
            ..opts
        };
        c.reset(&k, &opts).unwrap();
        assert_eq!(c.curve(), None);
        assert_eq!(c.src_zone_tol_3d(), UNINIT_TOL);
        assert_eq!(c.interval().max(), 2.0);
    }

    #[test]
    fn assign_owned_curve_without_copy() {
        use crate::curve::hermite_curve::HermiteCurve;
        let k = Kernel::default();
        let mut g = line();
        let h = HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            3,
        );
        g.0.push(Curve::from(h));
        let line_h = H(0, ClassObjectKind::Curve);
        let herm_h = H(1, ClassObjectKind::Curve);
        let mut src = CurveClassification::new(&k);
        src.set_curve(Some(herm_h), true);
        // SmHermiteCurve::Copy fails: a non-owned curve stays, an owned one is freed
        let mut d = CurveClassification::new(&k);
        d.set_curve(Some(line_h), false);
        d.assign_from(&k, &g, &src).unwrap();
        assert_eq!(d.curve(), Some(line_h));
        assert!(d.is_curve_owned());
        let mut d = CurveClassification::new(&k);
        d.set_curve(Some(line_h), true);
        d.assign_from(&k, &g, &src).unwrap();
        assert_eq!(d.curve(), None);
        // a line copies
        src.set_curve(Some(line_h), true);
        d.assign_from(&k, &g, &src).unwrap();
        assert_eq!(d.curve(), Some(line_h));
    }

    #[test]
    fn connect_and_edit() {
        let k = Kernel::default();
        let g = line();
        let mut c = cc(&k, &[0.0, 0.25, 0.5, 1.0]);
        assert!(c
            .connect_intervals(3, IntervalPosition::Start, true)
            .is_err());
        c.connect_intervals(1, IntervalPosition::End, true).unwrap();
        assert_eq!(c.size(), 2);
        assert_eq!(c.at(1).unwrap().min(), 0.25);
        c.connect_intervals(0, IntervalPosition::Start, true)
            .unwrap();
        assert_eq!(c.interval().min(), 0.25);

        let mut c = cc(&k, &[0.0, 0.25, 0.5, 1.0]);
        assert!(c.edit_interval_min(&k, &g, 1, 0.6, true).is_err());
        c.edit_interval_min(&k, &g, 1, 0.1, true).unwrap();
        assert_eq!(c.at(0).unwrap().max(), 0.1);
        c.edit_interval_max(&k, &g, 1, 1.0, true).unwrap();
        assert_eq!(c.size(), 2);
        // the moved start of the first interval collapses it onto the second
        let mut c = cc(&k, &[0.0, 0.25, 0.5, 1.0]);
        c.edit_interval_min(&k, &g, 2, 0.25, true).unwrap();
        assert_eq!(c.size(), 2);
    }

    #[test]
    fn queries() {
        let k = Kernel::default();
        let g = line();
        let c = cc(&k, &[0.0, 0.5, 1.0]);
        let (p, t) = c.classify_crv_loc_by_index(&k, &g, 2).unwrap();
        assert_eq!((p.x, t), (1.0, 1.0));
        assert!(c.classify_crv_loc_by_index(&k, &g, 3).is_err());
        assert!(!c
            .has_dead_topology_point_object(&g, Some(&Brep::new()))
            .unwrap());
        assert!(c
            .interval_min_uv(&k, &g, 0, Point2::new(0.0, 0.0))
            .is_err_and(|e| !e.is_unported()));
        let s: Solution<H> = Solution::new();
        let r = refine_cci_to_surfaces(&c, &s, None, Point3::new(1.0, 2.0, 3.0), 6).unwrap();
        assert_eq!(r.recursion_count, 7);
        assert_eq!(r.sss_point.y, 2.0);
    }
}

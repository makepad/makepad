//! Curve classification, part 7: queries over the interval end classifications (point
//! classifications by boundary index and their mates, touches, vertices and edges met,
//! edge crossings, face crossings, near misses), interval collapse, the
//! coincident-maybe and partial-coincidence heuristics, face classification of an
//! interval and the fillet geometry hand-over.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust                                                        | Upstream                                              |
//! |-------------------------------------------------------------|-------------------------------------------------------|
//! | [`CurveClassification::point_class_by_index`], [`CurveClassification::point_class_by_index_mut`] | `GetPointClassByIndex` |
//! | [`CurveClassification::point_class_mate_by_index`]          | `GetPointClassMateByIndex`                            |
//! | [`CurveClassification::collapse_interval`]                  | `CollapseInterval`                                    |
//! | [`CurveClassification::point_classifies_connected_to`]      | `GetPointClassifiesConnectedTo`                       |
//! | [`CurveClassification::find_vertex_objects`], [`VertexObjects`] | `FindVertexObjects`                               |
//! | [`CurveClassification::find_edge_objects`]                  | `FindEdgeObjects`                                     |
//! | [`CurveClassification::is_crossed_by_edge`]                 | `IsCrossedByEdge`                                     |
//! | [`CurveClassification::is_crossing_face`]                   | `IsCrossingFace`                                      |
//! | [`CurveClassification::has_near_misses`]                    | `HasNearMisses`                                       |
//! | [`CurveClassification::are_coincident_maybe`], [`CoincidentMaybe`] | `AreCoincidentMaybe`                          |
//! | [`CurveClassification::classify_face_interval`], [`CurveClassification::classify_face_interval_mut`] | `ClassifyFaceInterval` |
//! | [`CurveClassification::test_for_partial_coincident_edges`], [`PartialCoincidence`] | `TestForPartialCoincidentEdges` |
//! | [`set_do_filleting_geometry`], [`set_do_filleting_geometry_mut`] | `sm_SetDoFilletingGeometry`                     |
//! | [`CurveClassification::curve`] of the owner                 | `SmCurveInterval::GetCurve`                           |
//! | [`curve_is_degenerate`] (`intersect.curve_class_p3`)        | `SmTol::IsDegenerate(const SmCurve*, SmExtent1d, SmXSectTol3d*)` (CO-PORTED from `geom.tol`, with the tolerance given) |
//! | `tol_is_closed_curve` (private)                             | `SmTol::IsClosed(const SmObject*, SmXSectTol3d*)` for a curve (CO-PORTED from `geom.tol`, with the tolerance given) |
//!
//! Deviations:
//! - **Handles.** As in part 1, every object is a caller handle `O` resolved through a
//!   [`ClassGeometry`]. `SmCurveInterval::GetCurve` reads the owner's curve through its
//!   back pointer, which is not ported: use [`CurveClassification::curve`] of the
//!   classification holding the interval.
//! - **Out-params** become return values: [`VertexObjects`], [`PartialCoincidence`],
//!   `(CoincidentMaybe, IntervalPosition)`, `(point classification, parameter)`. The
//!   touches of [`CurveClassification::point_classifies_connected_to`] carry
//!   `pt_classify = None` (a point classification inside a classification has no
//!   handle): `pt_classify_index` 0 is the first interval's start, `i > 0` the end of
//!   interval `i - 1`.
//! - **The other classification** of `sm_SetDoFilletingGeometry` is reached through
//!   `GetOtherClassification` upstream; here the caller passes the classification the
//!   handle names. `sm_SetDoFilletingGeometry` reads the target classification only
//!   (upstream edits the topology it classifies to).
//! - **`SmCrvOnSurf` in `AreCoincidentMaybe`** is built only where it is evaluated (the
//!   edge-coincidence branch), not at the top of each pass; it has no other effect.
//! - **`ClassifyFaceInterval`**: the `SmClassifyProblem` causes are not returned by the
//!   face queries (only the commented-out step 3 read them); `lFaceIndx` is passed on
//!   as upstream (debugging only).
//! - **`TestForPartialCoincidentEdges`** re-runs the coincidence check into the array
//!   it is iterating, and keeps reading its solution reference `rSol`: here `rSol` is
//!   the re-run's solution at the same index, or the old one when the re-run found
//!   fewer (upstream's fixed 16-entry buffer keeps the old entry).
//! - **Undefined behaviour upstream** is an error here (`InvalidInput`): interval or
//!   boundary indices past the intervals, the first interval of an empty
//!   classification, a missing curve where it is dereferenced, and a coincident edge
//!   whose curve is not of a kind derived from `SmBSplineCurve` (`SM_CAST_PTR` gives
//!   `NULL`; the copy slices a line or conic to its B-spline part).
//! - **Debug-only work** (`SM_DEBUG_CODE`, `SM_DBG_WARN`, `bReportProb` messages, the
//!   unused `AreCoincidentMaybe` angle check) is dropped; the angle is still computed.
//! - **Topology members.** `SmEdge::GetCurve` / `GetInterval` / `IsConnectedTo`,
//!   `SmVertex::GetPoint` / `IsConnectedTo` and the edge tolerances of
//!   `SmTol::GetXSectTol3d(obj, obj)` are the ported ones (units `topology.edge`,
//!   `topology.vertex`), reached through [`ClassGeometry::topology_entity`]; a handle whose
//!   adapter lacks that hook keeps the earlier [`SolidError::Unported`] deferral, and a
//!   connection target in another B-rep is connected to nothing. `IsConnectedTo(NULL)` is
//!   `TRUE` before any member is read, as upstream. The edits of `sm_SetDoFilletingGeometry`
//!   (`SmVertex::SetPoint`, `SmEdge::SetCurve`) on the mutable path ([`set_do_filleting_geometry_mut`])
//!   update the B-rep entities directly; the read-only path ([`set_do_filleting_geometry`]) returns
//!   [`SolidError::Unported`].
//! - **Face classifiers.** `SmFace::EdgeCurveOnClassify` / `Point3DClassify`
//!   ([`crate::query::face::p1`]) run on the B-rep and face that the adapter resolves
//!   the face handle to. Upstream's members are `const` but may add UV trim curves and
//!   trimmed surface caches to the B-rep.
//!   [`CurveClassification::classify_face_interval_mut`] (a [`ClassGeometryMut`]) runs
//!   them on that B-rep itself, so those additions stay, in upstream's order.
//!   **Deviation (#614, kept):** the read-only
//!   [`CurveClassification::classify_face_interval`] runs them on a copy of the B-rep,
//!   so their additions are discarded. A handle the adapter cannot resolve keeps the
//!   [`SolidError::Unported`] dispatch-gap deferral. An edge of another B-rep has no
//!   use on the face: not inside, as upstream's use walk finds. **Cache history:** a
//!   face query on the mutable path may build the face surface's trimmed surface cache
//!   with the face (`smsurf_GetSurfaceCache`), whose UV trim curves raise vertex and
//!   edge tolerances, or reuse the one the surface already holds (made earlier, with or
//!   without the face). The surface's cache slot keeps that history
//!   ([`crate::query::face::p2::face_surface_cache`]), so the query does what upstream
//!   does and the raised tolerances stay on the B-rep (`plan/findings.md`, "trimmed
//!   surface cache history: tolerance-growth deferral removed").
//! - **`SmFace::FindVertexSector`** runs on the mutable path only; the read-only
//!   path returns [`SolidError::Unported`] (`topology.face_p4`).
//! - **`SmFace::PointClassify`** ([`crate::query::face::p2::point_classify`]) runs on
//!   the resolved B-rep on the mutable path, so the UV trim curves its loop walk makes
//!   stay there. The read-only path runs the same classification over the unchanged
//!   topology ([`crate::query::face::p2::point_classify_prepared`], no copy): a loop
//!   walk that needs a UV trim curve made returns [`SolidError::Unported`].
//! - **Callees in units not ported yet** return [`SolidError::Unported`]: the topology
//!   zone tolerances of point classifications (through `intersect.point_class`).
//! - **Recursion.** None.

use std::borrow::Cow;

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, SolutionType};
use crate::geom::tol::{eff_zero_ang_rad, scaled_zero, xsect_tol_3d_from_zones, ScaledZero};
use crate::intersect::curve_class::p3::curve_is_degenerate;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    PointClassification, Precedence,
};
use crate::kernel::Kernel;
use crate::math::{max2, min2, Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p1 as qf1;
use crate::query::face::p2 as qf2;
use crate::surface::surface::Surface;
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, VertexId};

use super::p1::{
    get_curve_curve_angle, src_zone_tol_3d_of, xsect_tol_2d_or_3d_for_classify_pair,
    CurveClassification, CurveInterval, IntervalPosition, TouchData,
};

const OP_INDEX: &str = "intersect::curve_class::p7::point_class_by_index";
const OP_COLLAPSE: &str = "intersect::curve_class::p7::collapse_interval";
const OP_COIN: &str = "intersect::curve_class::p7::are_coincident_maybe";
const OP_FACE: &str = "intersect::curve_class::p7::classify_face_interval";
const OP_PARTIAL: &str = "intersect::curve_class::p7::test_for_partial_coincident_edges";
const OP_FILLET: &str = "intersect::curve_class::p7::set_do_filleting_geometry";
const OP_GEOM: &str = "intersect::curve_class::p7";

/// `SM_UNDEF_ULONG`: the loop index of a touch found without a target list.
pub const UNDEF_LOOP_INDEX: u64 = 123_456_789;

/// The verdict of [`CurveClassification::are_coincident_maybe`] (upstream `SmBoolean`
/// `FALSE`, `TRUE`, `UNSURE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoincidentMaybe {
    /// `FALSE`: not a coincident-maybe case, or not a fixable one.
    No,
    /// `TRUE`: a fixable coincident-maybe case (remove the reported interval end).
    Fixable,
    /// `UNSURE`: an edge classification whose object is not an edge.
    Unsure,
}

impl CoincidentMaybe {
    /// The upstream value (`FALSE` 0, `TRUE` 1, `UNSURE` 2).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::No => 0,
            Self::Fixable => 1,
            Self::Unsure => 2,
        }
    }
}

/// The vertices met by the classification curve and their curve parameters
/// (`FindVertexObjects`' `rVertices`, `pOptClassifyParams`; the count is
/// `vertices.len()`).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexObjects<O> {
    /// The vertices, without repeats.
    pub vertices: Vec<O>,
    /// The curve parameter of every vertex end met (one per end, so repeats of a vertex
    /// add parameters).
    pub params: Vec<f64>,
}

/// The result of [`CurveClassification::test_for_partial_coincident_edges`]
/// (`rdMaxPartialCoinGap`, `rpNewCurveToTest`).
#[derive(Debug, Clone, Default)]
pub struct PartialCoincidence {
    /// The largest gap at the ends of the coincident range; `0.0` for none.
    pub max_partial_coin_gap: f64,
    /// A copy of the edge curve trimmed to the coincident range, when the curve is
    /// partially coincident to an edge.
    pub new_curve_to_test: Option<BSplineCurve>,
}

/// A solution object naming nothing (the coincidence checks here pass no objects).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NoObject;

impl SolutionObject for NoObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

impl<O: ClassObject> CurveClassification<O> {
    /// The point classification at interval boundary `index` (`0 ..= size()`) and its
    /// curve parameter (`GetPointClassByIndex`): the start of interval `index`, or the
    /// end of the last interval for `index == size()`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index > size()` or an empty classification
    /// (upstream reads outside the intervals).
    pub fn point_class_by_index(&self, index: usize) -> Result<(&PointClassification<O>, f64)> {
        let size = self.size();
        if size == 0 || index > size {
            return Err(bad_boundary());
        }
        if index == size {
            let ivl = self.at(index - 1)?;
            return Ok((&ivl.end, ivl.interval.max()));
        }
        let ivl = self.at(index)?;
        Ok((&ivl.start, ivl.interval.min()))
    }

    /// Where [`CurveClassification::point_class_by_index`]'s pointer points: the
    /// interval, and whether it is that interval's end (else its start).
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::point_class_by_index`].
    pub(crate) fn point_class_by_index_interval(&self, index: usize) -> Result<(usize, bool)> {
        let size = self.size();
        if size == 0 || index > size {
            return Err(bad_boundary());
        }
        if index == size {
            return Ok((index - 1, true));
        }
        Ok((index, false))
    }

    /// [`CurveClassification::point_class_by_index`], mutable.
    ///
    /// # Errors
    ///
    /// As [`CurveClassification::point_class_by_index`].
    pub fn point_class_by_index_mut(
        &mut self,
        index: usize,
    ) -> Result<(&mut PointClassification<O>, f64)> {
        let size = self.size();
        if size == 0 || index > size {
            return Err(bad_boundary());
        }
        if index == size {
            let ivl = self.at_mut(index - 1)?;
            let param = ivl.interval.max();
            return Ok((&mut ivl.end, param));
        }
        let ivl = self.at_mut(index)?;
        let param = ivl.interval.min();
        Ok((&mut ivl.start, param))
    }

    /// The point classification sharing boundary `index` with the one
    /// [`CurveClassification::point_class_by_index`] returns
    /// (`GetPointClassMateByIndex`): the end of interval `index - 1`. The first and
    /// last boundaries have no mate, except that boundary 0 of a curve closed over the
    /// classified interval mates the end of the last interval.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index > size()`, a missing curve at the first
    /// or last boundary (upstream dereferences it), and an empty classification whose
    /// curve is closed; those of [`Curve::is_closed`].
    pub fn point_class_mate_by_index(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<Option<&PointClassification<O>>> {
        match self.point_class_mate_interval(k, geometry, index)? {
            Some(i) => Ok(Some(&self.at(i)?.end)),
            None => Ok(None),
        }
    }

    /// The interval whose end [`CurveClassification::point_class_mate_by_index`]
    /// returns (where its pointer points), `None` for no mate.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::point_class_mate_by_index`].
    pub(crate) fn point_class_mate_interval(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<Option<usize>> {
        let size = self.size();
        if index > size {
            return Err(bad_boundary());
        }

        // No mate for first and last one unless interval is closed
        if index == size || index == 0 {
            let zone_tol_3d = src_zone_tol_3d_of(k, Some(self));
            let xsect_tol_3d = xsect_tol_3d_from_zones(k, zone_tol_3d, zone_tol_3d);

            let curve = self.curve_geometry(geometry)?;
            if curve.is_closed(k, &self.interval(), xsect_tol_3d)? && index == 0 {
                if size == 0 {
                    return Err(bad_boundary());
                }
                return Ok(Some(size - 1));
            }
            return Ok(None);
        }

        Ok(Some(index - 1))
    }

    /// Removes interval `index` whose length is degenerate (`CollapseInterval`): the
    /// combined end classification (see [`PointClassification::combine`]) becomes both
    /// ends and the neighbours' shared ends; interval parameters are not changed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `index >= size()`, fewer than two intervals, or an interval that is
    /// not degenerate within the pair tolerance of its ends
    /// ([`xsect_tol_2d_or_3d_for_classify_pair`]); [`SolidError::InvalidInput`] for a
    /// missing curve; those of the tolerance and degeneracy computations and of
    /// [`PointClassification::combine`].
    pub fn collapse_interval(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<()> {
        // check input - lInterval index is valid
        if index >= self.size() {
            return Err(SolidError::sm_err(OP_COLLAPSE));
        }

        // Do not squeeze out the only interval.  [B232]
        if self.size() < 2 {
            return Err(SolidError::sm_err(OP_COLLAPSE));
        }

        // locals
        let ivl = *self.at(index)?;

        let start_tol = ivl.start.obj_zone_tol_3d(k, geometry)?;
        let end_tol = ivl.end.obj_zone_tol_3d(k, geometry)?;
        let curve = self.curve_geometry(geometry)?;
        let surface1 = self.surface1_geometry(geometry)?;
        let xsect_tol_2d_or_3d = xsect_tol_2d_or_3d_for_classify_pair(
            k,
            curve,
            surface1,
            ivl.min(),
            start_tol,
            ivl.max(),
            end_tol,
        )?;

        // no work and an error - CurveClassification interval is not degenerate
        if !curve_is_degenerate(k, Some(curve), &ivl.interval, xsect_tol_2d_or_3d)? {
            return Err(SolidError::sm_err(OP_COLLAPSE));
        }

        // pick the PtClassification data to persist
        let mut start = ivl.start;
        let kept = match start.combine(&ivl.end)? {
            Precedence::This => start,
            Precedence::Other => ivl.end,
        };
        {
            let target = self.at_mut(index)?;
            target.start = kept;
            target.end = kept;
        }

        // propagate the persistent PtClassification data to the neighbour boundaries
        if index >= 1 {
            self.at_mut(index - 1)?.end = kept;
        }
        if index + 2 <= self.size() {
            self.at_mut(index + 1)?.start = kept;
        }

        // compress the m_vData array
        self.remove_at(index)
    }

    /// The interval end classifications to vertices and edges, each boundary once, in
    /// curve order (`GetPointClassifiesConnectedTo`): all of them for an empty
    /// `targets`, else those whose object is connected to one of `targets` (the touch
    /// then records which). Touches carry `loop_dir` [`CrvDirOnFaceType::Uninit`]
    /// and, without targets, `loop_index` [`UNDEF_LOOP_INDEX`].
    ///
    /// [`CrvDirOnFaceType::Uninit`]: super::p1::CrvDirOnFaceType::Uninit
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty classification (upstream reads its
    /// missing first interval); those of [`PointClassification::is_connected_to_any`].
    pub fn point_classifies_connected_to(
        &self,
        geometry: &impl ClassGeometry<O>,
        targets: &[O],
    ) -> Result<Vec<TouchData<O>>> {
        // locals
        let mut touches = Vec::new();
        let mut found_index = UNDEF_LOOP_INDEX;
        let first = self.at(0).map_err(|_| empty_classification(OP_GEOM))?;

        let push = |touches: &mut Vec<TouchData<O>>, param: f64, index: usize, found_index: u64| {
            touches.push(TouchData {
                connected_to: if targets.is_empty() {
                    None
                } else {
                    usize::try_from(found_index)
                        .ok()
                        .and_then(|i| targets.get(i).copied())
                },
                loop_dir: super::p1::CrvDirOnFaceType::Uninit,
                loop_index: found_index,
                param,
                pt_classify_index: index as u64,
                pt_classify: None,
            });
        };

        // When First Interval Start Edge/Vertex Object is connected to a target
        if is_vertex_or_edge(&first.start)
            && (targets.is_empty()
                || connected_any(geometry, &first.start, targets, &mut found_index)?)
        {
            push(&mut touches, first.interval.min(), 0, found_index);
        }

        // for every Interval End
        for (ii, ivl) in self.intervals().iter().enumerate() {
            if is_vertex_or_edge(&ivl.end)
                && (targets.is_empty()
                    || connected_any(geometry, &ivl.end, targets, &mut found_index)?)
            {
                push(&mut touches, ivl.interval.max(), ii + 1, found_index);
            }
        }

        Ok(touches)
    }

    /// The vertices the classification curve meets (`FindVertexObjects`; upstream
    /// default `pOptConnectedTarget = NULL`): the first start and every interval end
    /// classified to a vertex connected to `connected_target` (any, for `None`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty classification;
    /// [`SolidError::Unported`] for a vertex (`SmVertex::IsConnectedTo`, unit
    /// `topology.vertex`).
    pub fn find_vertex_objects(
        &self,
        geometry: &impl ClassGeometry<O>,
        connected_target: Option<O>,
    ) -> Result<VertexObjects<O>> {
        // init output
        let mut out = VertexObjects {
            vertices: Vec::new(),
            params: Vec::new(),
        };

        // locals
        let first = self.at(0).map_err(|_| empty_classification(OP_GEOM))?;

        // for the first CurveInterval Start - add any VertexObject
        if let Some(v) = first.start.vertex_object() {
            if vertex_is_connected_to(geometry, v, connected_target)? {
                out.vertices.push(v);
                out.params.push(first.interval.min());
            }
        }

        // for every interval
        for ivl in self.intervals() {
            if let Some(v) = ivl.end.vertex_object() {
                if vertex_is_connected_to(geometry, v, connected_target)? {
                    if !out.vertices.contains(&v) {
                        out.vertices.push(v);
                    }
                    out.params.push(ivl.interval.max());
                }
            }
        }

        Ok(out)
    }

    /// The edges the classification curve meets (`FindEdgeObjects`; upstream defaults
    /// `pOptConnectedTarget = NULL`, `lWorkFlag = 3`): with bit 1 of `work_flag` the
    /// edges an interval is coincident with, with bit 2 the edges crossed at an
    /// interval end not next to a coincident interval; only edges connected to
    /// `connected_target` (any, for `None`), each once.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty classification;
    /// [`SolidError::Unported`] for an edge tested for connection
    /// (`SmEdge::IsConnectedTo`, unit `topology.edge`).
    pub fn find_edge_objects(
        &self,
        geometry: &impl ClassGeometry<O>,
        connected_target: Option<O>,
        work_flag: u64,
    ) -> Result<Vec<O>> {
        // init output
        let mut edges: Vec<O> = Vec::new();

        // locals
        let get_coins = work_flag & 1 != 0;
        let get_xsects = work_flag & 2 != 0;
        let first = self.at(0).map_err(|_| empty_classification(OP_GEOM))?;
        let start_edge = first.start.edge_object();
        let mut mid_edge = first.mid.edge_object();
        let mut mid_edge_next = mid_edge;

        // for the first CurveInterval Start - add any XSecting EdgeObject
        if get_xsects {
            if let Some(e) = start_edge {
                if edge_is_connected_to(geometry, e, connected_target)? && mid_edge.is_none() {
                    edges.push(e);
                }
            }
        }

        // for every interval - add Mid and End Edges as appropriate
        let size = self.size();
        for ii in 0..size {
            mid_edge = mid_edge_next;
            let end_edge = self.at(ii)?.end.edge_object();
            mid_edge_next = if ii < size - 1 {
                self.at(ii + 1)?.mid.edge_object()
            } else {
                None
            };

            // when asked and appropriate - add Coincident mid edges
            if get_coins {
                if let Some(e) = mid_edge {
                    if edge_is_connected_to(geometry, e, connected_target)? && !edges.contains(&e) {
                        edges.push(e);
                    }
                }
            }

            // when asked and appropriate - add XSecting end edges
            if get_xsects {
                if let Some(e) = end_edge {
                    if edge_is_connected_to(geometry, e, connected_target)?
                        && mid_edge.is_none()
                        && mid_edge_next.is_none()
                        && !edges.contains(&e)
                    {
                        edges.push(e);
                    }
                }
            }
        }

        Ok(edges)
    }

    /// Whether an edge crosses the classification curve (`IsCrossedByEdge`): an
    /// interval end on an edge, away from the edge's ends, where neither the interval
    /// nor its neighbour on that side is coincident with the edge.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for an interval end on an edge (`SmEdge::GetInterval`,
    /// unit `topology.edge`).
    pub fn is_crossed_by_edge(&self, geometry: &impl ClassGeometry<O>) -> Result<bool> {
        let size = self.size();

        // for every classification interval
        for ii in 0..size {
            let ivl = self.at(ii)?;

            // When interval starts on an EDGE
            if ivl.start.point_class() == PointClassType::Edge {
                let t = ivl.start.t_param();
                let edge = ivl.start.edge_object();
                let edge_ivl = edge_interval(geometry, edge)?;
                let sz = t.scaled_zero();

                // not crossing when XSection happens at bndry of pEdge Interval
                if edge_ivl.is_value_on_boundary(t, sz).0 {
                    continue;
                }

                // not crossing if pEdge is coincident with Classification Curve
                if ivl.mid.point_class() == PointClassType::Edge && edge == ivl.mid.edge_object() {
                    continue;
                }

                // not crossing if previous CrvIvl is coincident with this Edge
                if ii > 0 {
                    let prev = self.at(ii - 1)?;
                    if prev.end.point_class() == PointClassType::Edge
                        && prev.end.edge_object() == edge
                        && prev.mid.point_class() == PointClassType::Edge
                        && prev.mid.edge_object() == edge
                    {
                        continue;
                    }
                }

                // an Edge was found crossing the Classification Curve
                return Ok(true);
            }

            // When Interval ends on an Edge
            if ivl.end.point_class() == PointClassType::Edge {
                let t = ivl.end.t_param();
                let edge = ivl.end.edge_object();
                let edge_ivl = edge_interval(geometry, edge)?;
                let sz = t.scaled_zero();

                // not crossing if the intersection happens at boundary of the pEdge Interval
                if edge_ivl.is_value_on_boundary(t, sz).0 {
                    continue;
                }

                // not crossing if pEdge is coincident with Classification Curve
                if ivl.mid.point_class() == PointClassType::Edge && edge == ivl.mid.edge_object() {
                    continue;
                }

                // not crossing if next CrvIvl is coincident with this Edge
                if ii < size - 1 {
                    let next = self.at(ii + 1)?;
                    if next.start.point_class() == PointClassType::Edge
                        && next.start.edge_object() == edge
                        && next.mid.point_class() == PointClassType::Edge
                        && next.mid.edge_object() == edge
                    {
                        continue;
                    }
                }

                // an Edge was found crossing the Classification Curve
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Whether an interval interior is classified to a face (`IsCrossingFace`; upstream
    /// default `pOptTgtFace = NULL`): to `target_face`, or to any face for `None`.
    #[must_use]
    pub fn is_crossing_face(&self, target_face: Option<O>) -> bool {
        self.intervals().iter().any(|ivl| {
            ivl.mid.point_class() == PointClassType::Face
                && (target_face.is_none() || ivl.mid.object() == target_face)
        })
    }

    /// Whether a vertex end or a coincident edge is only a near miss of the curve
    /// (`HasNearMisses`; upstream default `bInSurfaceGaps = FALSE`): its gap is at
    /// least `1000 * SM_EFF_ZERO`, measured within the surface-1 tangent plane with
    /// `in_surface_gaps` and a surface 1 (see [`PointClassification::gap_3d`]).
    ///
    /// # Errors
    ///
    /// Those of [`PointClassification::gap_3d`] with a surface;
    /// [`SolidError::InvalidInput`] for an unresolved surface handle.
    pub fn has_near_misses(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        in_surface_gaps: bool,
    ) -> Result<bool> {
        // locals
        let surface = if in_surface_gaps {
            self.surface1_geometry(geometry)?
        } else {
            None
        };
        let near_miss_dist_3d = EFF_ZERO * 1000.0;

        for ivl in self.intervals() {
            let beg = &ivl.start;
            let mid = &ivl.mid;
            let end = &ivl.end;

            if (beg.point_class() == PointClassType::Vertex
                && beg.gap_3d(k, geometry, surface)? >= near_miss_dist_3d)
                || (mid.point_class() == PointClassType::Edge
                    && mid.gap_3d(k, geometry, surface)? >= near_miss_dist_3d)
                || (end.point_class() == PointClassType::Vertex
                    && end.gap_3d(k, geometry, surface)? >= near_miss_dist_3d)
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Whether interval `index` of `self` or of its partner `other` (`self` again to
    /// check one classification) only looks coincident with an edge and can be fixed by
    /// removing the interval (`AreCoincidentMaybe`; upstream default
    /// `bReportProb = FALSE`, whose messages are not reproduced). Returns the verdict
    /// and the interval end to remove ([`IntervalPosition::Inside`] when none was
    /// picked; a picked end stays reported when the case turns out not fixable, as
    /// upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index` past either classification or a missing
    /// curve; [`SolidError::Unported`] in the edge-coincidence branch (edge tolerances
    /// and curves, the topology units).
    pub fn are_coincident_maybe(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &Self,
        index: usize,
        report_prob: bool,
    ) -> Result<(CoincidentMaybe, IntervalPosition)> {
        let _ = report_prob;

        // init output
        let mut position = IntervalPosition::Inside;
        let mut rtn = false;

        let this_ivl = self.at(index)?;
        let other_ivl = other.at(index)?;

        // no work - neither interval coincident (maybe) with an edge
        let edge = PointClassType::Edge;
        if (this_ivl.mid.point_class() != edge
            || (this_ivl.end.point_class() != edge && this_ivl.start.point_class() != edge))
            && (other_ivl.mid.point_class() != edge
                || (other_ivl.end.point_class() != edge && other_ivl.start.point_class() != edge))
        {
            return Ok((CoincidentMaybe::No, position));
        }

        // check four times: this(Start, Mid), other(Start, Mid), this(End, Mid),
        // other(End, Mid)
        for ii in 0..4 {
            // select tgt classification and interval
            let (tgt_class, tgt_ivl, mate_ivl, mate_owner) = if ii == 0 || ii == 2 {
                (self, this_ivl, other_ivl, other)
            } else {
                (other, other_ivl, this_ivl, self)
            };
            let tgt_curve_3d = tgt_class.curve_geometry(geometry)?;
            let on_surface = tgt_curve_3d.dim() == 2 && tgt_class.surface1().is_some();

            // select target end of the tgt interval
            let (tgt_beg, tgt_end, mate_beg, end_beg, end_end, tgt_param) = if ii < 2 {
                (
                    &tgt_ivl.start,
                    &tgt_ivl.end,
                    &mate_ivl.start,
                    IntervalPosition::Start,
                    IntervalPosition::End,
                    tgt_ivl.interval.min(),
                )
            } else {
                (
                    &tgt_ivl.end,
                    &tgt_ivl.start,
                    &mate_ivl.end,
                    IntervalPosition::End,
                    IntervalPosition::Start,
                    tgt_ivl.interval.max(),
                )
            };
            let tgt_mid = &tgt_ivl.mid;
            let mate_mid = &mate_ivl.mid;

            // when this interval mid and beg point look coincident with an edge
            if tgt_beg.point_class() == edge
                && tgt_mid.point_class() == edge
                && tgt_mid.object() == tgt_beg.object()
            {
                let xsect_tol_3d = xsect_tol_3d_from_zones(
                    k,
                    src_zone_tol_3d_of(k, Some(mate_owner)),
                    tgt_mid.obj_zone_tol_3d(k, geometry)?,
                );

                // get Edge, Interval, and Curve lengths
                let Some(edge_obj) = tgt_beg
                    .object()
                    .filter(|o| o.class_kind() == ClassObjectKind::Edge)
                else {
                    // probable bug - PointClassify classifies to SmEdge but its object
                    // is not an SmEdge
                    return Ok((CoincidentMaybe::Unsure, position));
                };
                let edge_param = tgt_beg.t_param();

                let edge_curve = edge_curve(geometry, edge_obj)?;
                let edge_ivl = edge_interval(geometry, Some(edge_obj))?;
                let tgt_curve: Cow<'_, Curve> = if on_surface {
                    Cow::Owned(crv_on_surf(
                        k,
                        tgt_curve_3d,
                        tgt_class.surface1_geometry(geometry)?,
                    )?)
                } else {
                    Cow::Borrowed(tgt_curve_3d)
                };
                let edge_len = edge_curve.approximate_length(k, &edge_ivl, 4, false)?.0;
                let cm_len = tgt_curve
                    .approximate_length(k, &tgt_ivl.interval, 4, false)?
                    .0;
                let total_cc_len = tgt_curve
                    .approximate_length(k, &self.interval(), 4, false)?
                    .0;

                // curve/edge angle at curve/edge intersection
                let angle_rad = get_curve_curve_angle(
                    k,
                    Some(edge_curve),
                    edge_param,
                    Some(&tgt_curve),
                    tgt_param,
                    xsect_tol_3d,
                )?;

                // case 1: original heuristic - left in to preserve current behavior
                let this_zone_tol_3d = src_zone_tol_3d_of(k, Some(self)) / 1000.0;
                let tgt_beg_gap_3d = tgt_beg.gap_3d(k, geometry, None)?;

                if (tgt_class.ignore_tangency() || tgt_beg_gap_3d > this_zone_tol_3d)
                    && (cm_len < edge_len / 10.0
                        || (self.ignore_tangency() && cm_len < total_cc_len / 2.0))
                {
                    // conclude this target interval is mislabeled as coincident
                    position = end_beg;
                    rtn = true;
                }

                // case 2: Curve runs close to vertex and intersects an existing edge
                // attached to the vertex
                let near_vertex = tgt_end.gap_3d(k, geometry, None)? > scaled_zero() * 100.0;
                let tight_crv_xsect = tgt_beg.gap_3d(k, geometry, None)? < scaled_zero() * 100.0;

                if !rtn
                    && tgt_end.point_class() == PointClassType::Vertex
                    && near_vertex
                    && tight_crv_xsect
                    && (cm_len < edge_len / 10.0
                        || cm_len < total_cc_len / 10.0
                        || cm_len < EFF_ZERO_SQRT)
                {
                    // remove the non-vertex end of the interval
                    position = end_end;
                    rtn = true;
                }

                // case 3: not yet an action (a place for a break upstream)
                let _ = !rtn && angle_rad > eff_zero_ang_rad() * 10.0;

                // when a coincident segment was identified
                if rtn {
                    let mb = mate_beg.point_class();
                    let mm = mate_mid.point_class();
                    let mate_mergeable = (mb == mm
                        && (mb == PointClassType::Edge
                            || mb == PointClassType::Face
                            || mb == PointClassType::Region))
                        || (mm == PointClassType::Face && mb == PointClassType::Unknown);

                    if mate_mergeable {
                        return Ok((CoincidentMaybe::Fixable, position));
                    }
                    // look for other fixable cases of coincident-maybe
                    rtn = false;
                }
            }
        }

        // arrive here when no removable coincident-maybe segments were found
        Ok((CoincidentMaybe::No, position))
    }

    /// Classifies interval `index`, known to lie on `face`, against it
    /// (`ClassifyFaceInterval`; upstream defaults `bUVSpaceOkay = TRUE`,
    /// `lFaceIndx = SM_UNDEF_ULONG`): through the vertex sector or edge side at its
    /// classified ends, else (with `do_point_classify`) by classifying a point near its
    /// middle (in UV space through the surface-1 UV curve when there is one and
    /// `uv_space_okay`). An interval found inside the face gets its interior, and any
    /// unknown or face end, classified to it. Only intervals whose interior is unknown,
    /// or classified to `old_face` (when given), are considered.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()` or a missing curve where it is
    /// used; `SM_ERR_NULL_POINTER` for a vertex or edge end without its object; those
    /// of the curve evaluations and face queries; [`SolidError::Unported`] for
    /// `FindVertexSector` (unit `topology.face_p4`), for a `PointClassify` that needs
    /// a UV trim curve made, and for face or edge handles the adapter cannot resolve.
    pub fn classify_face_interval(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        do_point_classify: bool,
        face: O,
        old_face: Option<O>,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<()> {
        self.classify_face_interval_with(
            k,
            &mut ReadOnly(geometry),
            index,
            do_point_classify,
            face,
            old_face,
            uv_space_okay,
            face_index,
        )
    }

    /// [`CurveClassification::classify_face_interval`] with mutable access to the
    /// B-rep behind the face handle: the face queries (`SmFace::EdgeCurveOnClassify`,
    /// `Point3DClassify`, `FindVertexSector`, `PointClassify`) run on the B-rep that
    /// [`ClassGeometryMut::topology_entity_mut`] resolves the face handle to, so the
    /// UV trim curves and caches they make stay there, as upstream's `const` members
    /// leave them on the face's B-rep.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::classify_face_interval`]; a face handle that
    /// resolves read-only but not mutably keeps the read-only path's behaviour.
    #[allow(clippy::too_many_arguments)] // upstream's arguments
    pub fn classify_face_interval_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        index: usize,
        do_point_classify: bool,
        face: O,
        old_face: Option<O>,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<()> {
        self.classify_face_interval_with(
            k,
            &mut Mutable(geometry),
            index,
            do_point_classify,
            face,
            old_face,
            uv_space_okay,
            face_index,
        )
    }

    #[allow(clippy::too_many_arguments)] // upstream's arguments
    fn classify_face_interval_with(
        &mut self,
        k: &Kernel,
        access: &mut impl FaceQueries<O>,
        index: usize,
        do_point_classify: bool,
        face: O,
        old_face: Option<O>,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<()> {
        // locals
        let mut ivl = *self.at(index)?;
        let src_zone_tol_3d = src_zone_tol_3d_of(k, Some(&*self));
        let ctx = FaceIntervalContext {
            curve: self.curve(),
            uv_curve1: self.uv_curve1(),
            src_zone_tol_3d,
            face,
            old_face,
            do_point_classify,
            uv_space_okay,
            face_index,
        };
        let r = classify_face_interval_on(k, access, &ctx, &mut ivl);
        *self.at_mut(index)? = ivl;
        r
    }

    /// Whether the classification and its partner `other` (`self` again to check one)
    /// are only partially coincident with an edge (`TestForPartialCoincidentEdges`):
    /// their first (or last) intervals lie on edges without starting (ending) on a
    /// vertex, or the single interval runs between vertices at one end pair only, and
    /// the two edges have a coincident range. The result then holds the largest gap at
    /// the range ends and a copy of the edge curve trimmed to the range; otherwise
    /// (empty classifications, a closed curve, complete coincidence) gap `0` and no
    /// curve.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unresolved curve handle or a coincident
    /// edge curve that is not a B-spline; those of [`Curve::is_closed`] and the
    /// coincidence checks; [`SolidError::Unported`] where edges are examined (unit
    /// `topology.edge`).
    pub fn test_for_partial_coincident_edges(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &Self,
    ) -> Result<PartialCoincidence> {
        // init output
        let none = PartialCoincidence::default();
        let src_zone_tol_3d = src_zone_tol_3d_of(k, Some(self));
        let xsect_tol_3d = xsect_tol_3d_from_zones(k, src_zone_tol_3d, src_zone_tol_3d);

        // no work - one of the classifications has no segments
        if self.size() == 0 || other.size() == 0 {
            return Ok(none);
        }

        // no work - closed curve
        let curve = match self.curve() {
            Some(c) => Some(geometry.curve(c).ok_or(SolidError::invalid_input(
                OP_PARTIAL,
                "unresolved curve handle",
            ))?),
            None => None,
        };
        if tol_is_closed_curve(k, curve, xsect_tol_3d)? {
            return Ok(none);
        }

        // First check initial interval for edge coincidence that does not start on a
        // vertex.
        {
            let ivl = self.at(0)?;
            let o_ivl = other.at(0)?;
            if ivl.mid.point_class() == PointClassType::Edge
                && o_ivl.mid.point_class() == PointClassType::Edge
            {
                let vert_on_start = ivl.start.point_class() == PointClassType::Vertex
                    || o_ivl.start.point_class() == PointClassType::Vertex;
                if !vert_on_start {
                    if let Some(found) =
                        partial_coincidence(k, geometry, &ivl.mid, &o_ivl.mid, CoinCheck::Range)?
                    {
                        return Ok(found);
                    }
                }
            }
        }

        // Second, check last interval for edge coincidence that does not end on a
        // vertex.
        {
            let ivl = self.at(self.size() - 1)?;
            let o_ivl = other.at(other.size() - 1)?;
            if ivl.mid.point_class() == PointClassType::Edge
                && o_ivl.mid.point_class() == PointClassType::Edge
            {
                let vert_on_end = ivl.end.point_class() == PointClassType::Vertex
                    || o_ivl.end.point_class() == PointClassType::Vertex;
                if !vert_on_end {
                    if let Some(found) =
                        partial_coincidence(k, geometry, &ivl.mid, &o_ivl.mid, CoinCheck::Range)?
                    {
                        return Ok(found);
                    }
                }
            }
        }

        // when classification has just 1 segment
        if self.size() == 1 {
            let ivl = self.at(0)?;
            let o_ivl = other.at(0)?;

            if ivl.mid.point_class() == PointClassType::Edge
                && o_ivl.mid.point_class() == PointClassType::Edge
            {
                // remember if both segments start on a vertex (upstream tests this
                // interval's ends, then the other's)
                let coin1 = ivl.end.point_class() == PointClassType::Vertex
                    && ivl.start.point_class() == PointClassType::Vertex;
                let coin2 = o_ivl.start.point_class() == PointClassType::Vertex
                    && o_ivl.end.point_class() == PointClassType::Vertex;

                // both segments align to vertices - return no edge to check
                if coin1 && coin2 {
                    return Ok(none);
                }

                if coin1 {
                    if let Some(found) =
                        partial_coincidence(k, geometry, &ivl.mid, &o_ivl.mid, CoinCheck::Vertices)?
                    {
                        return Ok(found);
                    }
                }

                if coin2 {
                    if let Some(found) =
                        partial_coincidence(k, geometry, &ivl.mid, &o_ivl.mid, CoinCheck::Vertices)?
                    {
                        return Ok(found);
                    }
                }
            }
        }

        // all done
        Ok(none)
    }

    /// The resolved surface 1, if any.
    pub(super) fn surface1_geometry<'g>(
        &self,
        geometry: &'g impl ClassGeometry<O>,
    ) -> Result<Option<&'g Surface>> {
        match self.surface1() {
            Some(s) => geometry
                .surface(s)
                .map(Some)
                .ok_or(SolidError::invalid_input(
                    OP_GEOM,
                    "unresolved surface handle",
                )),
            None => Ok(None),
        }
    }
}

/// Copies fillet geometry from the other classification's topology onto the topology
/// interval `index` of `this` was merged into (`sm_SetDoFilletingGeometry`): with bit
/// 1 of `todo` the start vertex position, bit 2 the end vertex position, bit 4 the
/// interior edge curve (the other edge's curve trimmed to the matching range,
/// reparametrised to this edge's interval and reversed when it runs the other way).
/// Nothing is done unless `this` is doing fillets with an other classification;
/// `other` is the classification that handle names. Processing stops at the first
/// requested item whose ends are not both classified to vertices (edges).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `index` past either classification or a missing
/// `other` when `this` names one; `SM_ERR_NULL_POINTER` for a missing vertex or edge
/// object; [`SolidError::Unported`] for the topology edits (units `topology.vertex`,
/// `topology.edge`).
pub fn set_do_filleting_geometry<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    index: usize,
    this: &CurveClassification<O>,
    other: Option<&CurveClassification<O>>,
    todo: u64,
) -> Result<()> {
    set_do_filleting_geometry_with(k, &mut ReadOnly(geometry), index, this, other, todo, false)
}

/// [`set_do_filleting_geometry`] with mutable access to the B-rep behind the topology
/// handles: the vertex and edge modifications are applied directly to the entities.
///
/// # Errors
///
/// Those of [`set_do_filleting_geometry`].
pub fn set_do_filleting_geometry_mut<O: ClassObject>(
    k: &Kernel,
    geometry: &mut impl ClassGeometryMut<O>,
    index: usize,
    this: &CurveClassification<O>,
    other: Option<&CurveClassification<O>>,
    todo: u64,
) -> Result<()> {
    set_do_filleting_geometry_with(k, &mut Mutable(geometry), index, this, other, todo, false)
}

/// The same operation with an explicitly linked, actual local peer.
/// # Errors
/// The shared fillet geometry operation fails or lacks editing context.
pub(super) fn set_do_filleting_geometry_linked<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    index: usize,
    this: &CurveClassification<O>,
    other: &CurveClassification<O>,
    todo: u64,
) -> Result<()> {
    set_do_filleting_geometry_with(
        k,
        &mut ReadOnly(geometry),
        index,
        this,
        Some(other),
        todo,
        true,
    )
}

/// The linked-peer operation over actual mutable topology.
/// # Errors
/// The shared fillet geometry operation fails or lacks editing context.
pub(super) fn set_do_filleting_geometry_mut_linked<O: ClassObject>(
    k: &Kernel,
    geometry: &mut impl ClassGeometryMut<O>,
    index: usize,
    this: &CurveClassification<O>,
    other: &CurveClassification<O>,
    todo: u64,
) -> Result<()> {
    set_do_filleting_geometry_with(
        k,
        &mut Mutable(geometry),
        index,
        this,
        Some(other),
        todo,
        true,
    )
}

fn set_do_filleting_geometry_with<O: ClassObject>(
    k: &Kernel,
    access: &mut impl FilletAccess<O>,
    index: usize,
    this: &CurveClassification<O>,
    other: Option<&CurveClassification<O>>,
    todo: u64,
    linked: bool,
) -> Result<()> {
    // no work - don't have DoingFillet Data
    if !this.doing_fillets() || (!linked && this.other_classification().is_none()) {
        return Ok(());
    }

    // locals
    let other = other.ok_or(SolidError::invalid_input(
        OP_FILLET,
        "no other classification",
    ))?;
    let this_ivl = this.at(index)?;
    let other_ivl = other.at(index)?;

    // when asked - set StartVertex Geometry
    if todo & 1 != 0 {
        if this_ivl.start.point_class() != PointClassType::Vertex
            || other_ivl.start.point_class() != PointClassType::Vertex
        {
            return Ok(());
        }
        let this_v = this_ivl
            .start
            .vertex_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let other_v = other_ivl
            .start
            .vertex_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let point = vertex_point(access.geometry(), other_v)?;
        access.set_vertex_point(this_v, point)?;
    }

    // when asked - set EndVertex Geometry
    if todo & 2 != 0 {
        if this_ivl.end.point_class() != PointClassType::Vertex
            || other_ivl.end.point_class() != PointClassType::Vertex
        {
            return Ok(());
        }
        let this_v = this_ivl
            .end
            .vertex_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let other_v = other_ivl
            .end
            .vertex_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let point = vertex_point(access.geometry(), other_v)?;
        access.set_vertex_point(this_v, point)?;
    }

    // when asked - set MidEdge Geometry
    if todo & 4 != 0 {
        if this_ivl.mid.point_class() != PointClassType::Edge
            || other_ivl.mid.point_class() != PointClassType::Edge
        {
            return Ok(());
        }

        // this mid edge
        let this_edge = this_ivl
            .mid
            .edge_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let this_edge_curve = edge_curve(access.geometry(), this_edge)?;
        let this_edge_ivl = edge_interval(access.geometry(), Some(this_edge))?;

        // other mid edge
        let other_edge = other_ivl
            .mid
            .edge_object()
            .ok_or(SolidError::null_pointer(OP_FILLET))?;
        let other_edge_curve = edge_curve(access.geometry(), other_edge)?;
        let other_edge_ivl = edge_interval(access.geometry(), Some(other_edge))?;

        // DropPoint locals
        let other_start_guess = other_edge_ivl.min();
        let other_end_guess = other_edge_ivl.max();
        let xsect_tol_3d = edge_pair_xsect_tol_3d(k, access.geometry(), this_edge, other_edge)?;

        // this and other edge end point samples (statuses ignored)
        let sample = |c: &Curve, t: f64| -> Result<[Vec3; 2]> {
            Ok(match ignored(c.evaluate(k, t, 1, true, true))? {
                Some(v) => [
                    v.first().copied().unwrap_or(Vec3::UNDEF),
                    v.get(1).copied().unwrap_or(Vec3::UNDEF),
                ],
                None => [Vec3::UNDEF; 2],
            })
        };
        let this_start = sample(this_edge_curve, this_edge_ivl.min())?;
        let this_end = sample(this_edge_curve, this_edge_ivl.max())?;
        let other_start = sample(other_edge_curve, other_edge_ivl.min())?;
        let other_end = sample(other_edge_curve, other_edge_ivl.max())?;

        let start_start_close = this_start[0].closer_than(xsect_tol_3d, other_start[0]);
        let start_end_close = this_start[0].closer_than(xsect_tol_3d, other_end[0]);
        let end_start_close = this_end[0].closer_than(xsect_tol_3d, other_start[0]);
        let end_end_close = this_end[0].closer_than(xsect_tol_3d, other_end[0]);

        // ThisStartPoint drop to OtherEdge->Curve
        let start_guess = if start_start_close {
            Some(other_start_guess)
        } else if start_end_close {
            Some(other_end_guess)
        } else {
            None
        };
        let start_drop = other_edge_curve.drop_point(
            k,
            &other_edge_ivl,
            this_start[0],
            None,
            xsect_tol_3d,
            start_guess,
            SolverOperation::Minimize,
        );

        // ThisEndPoint drop to OtherEdge->Curve
        let end_guess = if end_end_close {
            Some(other_end_guess)
        } else if end_start_close {
            Some(other_start_guess)
        } else {
            None
        };
        let end_drop = other_edge_curve.drop_point(
            k,
            &other_edge_ivl,
            this_end[0],
            None,
            xsect_tol_3d,
            end_guess,
            SolverOperation::Minimize,
        );

        // check both drops worked - do no harm
        let (Some(start_drop), Some(end_drop)) = (ignored(start_drop)?, ignored(end_drop)?) else {
            return Ok(());
        };
        if !start_drop.success || !end_drop.success {
            return Ok(());
        }
        let other_start_param = start_drop.parameter;
        let other_end_param = end_drop.parameter;

        // copy pOtherEdgeCurve
        let mut copy = other_edge_curve.copy(k)?;

        // Trim interval bounded by DropPoint Params (may snap to existing knots)
        let mut final_ivl = Extent1d::new();
        ignored(final_ivl.set_min_max(
            min2(other_start_param, other_end_param),
            max2(other_start_param, other_end_param),
        ))?;
        ignored(copy.trim(k, &mut final_ivl, TrimMode::Notify))?;

        // reparameterize the OtherCurveCopy to the ThisEdge Interval
        ignored(copy.edit_parameterization(k, &this_edge_ivl))?;

        // when needed reverse parameterization
        if (((start_start_close && start_end_close) || (end_start_close && end_end_close))
            && this_start[1].dot(other_start[1]) <= 0.0)
            || (((!start_start_close || !start_end_close) && (!end_start_close || !end_end_close))
                && other_start_param > other_end_param)
        {
            ignored(copy.reverse_parameterization(k, &this_edge_ivl))?;
        }

        // Set ThisEdge->Curve from OtherEdge->Curve Copy
        access.set_edge_curve(this_edge, copy)?;
    }

    Ok(())
}

// ---- classify face interval ------------------------------------------------------------

/// The classification data `ClassifyFaceInterval` reads besides the interval.
struct FaceIntervalContext<O> {
    curve: Option<O>,
    uv_curve1: Option<O>,
    src_zone_tol_3d: f64,
    face: O,
    old_face: Option<O>,
    do_point_classify: bool,
    uv_space_okay: bool,
    face_index: Option<u64>,
}

/// The answer of a face query on an interval end: inside the face, and whether the
/// answer is to be doubted (`bPossibleProblem`).
#[derive(Debug, Clone, Copy, Default)]
struct EndInside {
    inside: bool,
    possible_problem: bool,
}

/// The classification curve `m_cpCurve` of a [`FaceIntervalContext`].
fn context_curve<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    ctx: &FaceIntervalContext<O>,
) -> Result<&'g Curve> {
    let c = ctx.curve.ok_or(SolidError::invalid_input(
        OP_FACE,
        "no classification curve",
    ))?;
    geometry.curve(c).ok_or(SolidError::invalid_input(
        OP_FACE,
        "unresolved curve handle",
    ))
}

/// The classification's surface-1 UV curve (`GetUVCurve1()`) of a
/// [`FaceIntervalContext`].
fn context_uv_curve1<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    ctx: &FaceIntervalContext<O>,
) -> Result<Option<&'g Curve>> {
    match ctx.uv_curve1 {
        Some(c) => geometry.curve(c).map(Some).ok_or(SolidError::invalid_input(
            OP_FACE,
            "unresolved UV curve handle",
        )),
        None => Ok(None),
    }
}

/// `*m_cpCurve` and `GetUVCurve1()` of a [`FaceIntervalContext`], copied out of the
/// adapter (in that order: the first unresolved one is the error).
fn context_curves<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    ctx: &FaceIntervalContext<O>,
) -> Result<(Curve, Option<Curve>)> {
    let curve = context_curve(geometry, ctx)?.clone();
    let uv_curve = context_uv_curve1(geometry, ctx)?.cloned();
    Ok((curve, uv_curve))
}

/// The body of [`CurveClassification::classify_face_interval`] on a copy of the
/// interval. The curves are copied for the face queries, which may need the adapter
/// mutably.
fn classify_face_interval_on<O: ClassObject>(
    k: &Kernel,
    access: &mut impl FaceQueries<O>,
    ctx: &FaceIntervalContext<O>,
    ivl: &mut CurveInterval<O>,
) -> Result<()> {
    // no work - ClassifMid not classified to UNKNOWN or FACE when given pOldFace
    let mid_class = ivl.mid.point_class();
    if !(mid_class == PointClassType::Unknown
        || (ctx.old_face.is_some() && mid_class == PointClassType::Face))
    {
        return Ok(());
    }

    // no work - OldFace not ivl's classifObj
    if ctx.old_face.is_some()
        && ivl.mid.face_object().is_some()
        && ctx.old_face != ivl.mid.face_object()
    {
        return Ok(());
    }

    let face = ctx.face;
    let mut did_min = false;
    let mut did_mid = false;
    let mut did_max = false;
    let mut min = EndInside::default();
    let mut max = EndInside::default();

    // 1. When StartPoint is classified to a vertex or edge (looking forwards)
    match ivl.start.point_class() {
        PointClassType::Vertex => {
            did_min = true;
            let v = ivl
                .start
                .object()
                .ok_or(SolidError::null_pointer(OP_FACE))?;
            let (curve, uv_curve) = context_curves(access.geometry(), ctx)?;
            min = access.find_vertex_sector(
                k,
                face,
                v,
                &curve,
                uv_curve.as_ref(),
                ivl.interval.min(),
                ivl.interval.max(),
                OrientType::Same,
                ctx.src_zone_tol_3d,
                ctx.uv_space_okay,
            )?;
        }
        PointClassType::Edge => {
            did_min = true;
            let e = ivl
                .start
                .object()
                .ok_or(SolidError::null_pointer(OP_FACE))?;
            let curve = context_curve(access.geometry(), ctx)?.clone();
            min = access.edge_curve_on_classify(
                k,
                face,
                e,
                ivl.start.t_param(),
                &curve,
                &ivl.interval,
                OrientType::Same,
                ctx.src_zone_tol_3d,
                ctx.uv_space_okay,
                None,
            )?;
        }
        _ => {}
    }

    // 2. when EndPoint is classified to a vertex or edge (looking backwards)
    match ivl.end.point_class() {
        PointClassType::Vertex => {
            did_max = true;
            let v = ivl.end.object().ok_or(SolidError::null_pointer(OP_FACE))?;
            let (curve, uv_curve) = context_curves(access.geometry(), ctx)?;
            max = access.find_vertex_sector(
                k,
                face,
                v,
                &curve,
                uv_curve.as_ref(),
                ivl.interval.max(),
                ivl.interval.min(),
                OrientType::Opposite,
                ctx.src_zone_tol_3d,
                ctx.uv_space_okay,
            )?;
        }
        PointClassType::Edge => {
            did_max = true;
            let e = ivl.end.object().ok_or(SolidError::null_pointer(OP_FACE))?;
            let curve = context_curve(access.geometry(), ctx)?.clone();
            max = access.edge_curve_on_classify(
                k,
                face,
                e,
                ivl.end.t_param(),
                &curve,
                &ivl.interval,
                OrientType::Opposite,
                ctx.src_zone_tol_3d,
                ctx.uv_space_okay,
                ctx.face_index,
            )?;
        }
        _ => {}
    }

    // 4. Do a PointClassification when interval couldn't be classified through
    // endPoint classifications
    if ctx.do_point_classify && !did_min && !did_max {
        did_mid = true;

        // pick a near interval mid-point curve parameter
        let param = ivl.interval.evaluate(0.4567);

        let uv = match context_uv_curve1(access.geometry(), ctx)? {
            Some(uv_curve) if ctx.uv_space_okay => {
                let pnt = uv_curve.evaluate_point(k, param)?;
                Some(Point2::new(pnt.x, pnt.y))
            }
            _ => None,
        };
        if let Some(uv) = uv {
            // Don't put this face's classification directly into m_vMid [B97]
            let mut temp = PointClassification::new(ctx.src_zone_tol_3d);
            access.point_classify(k, face, uv, ctx.src_zone_tol_3d, false, true, &mut temp)?;
            if temp.point_class() != PointClassType::Unknown {
                ivl.mid = temp;
            }
        } else {
            let pnt = context_curve(access.geometry(), ctx)?.evaluate_point(k, param)?;

            // Don't put this face's classification directly into m_vMid [B97]
            let mut temp = PointClassification::new(ctx.src_zone_tol_3d);
            // status ignored (assume the point was not on the surface)
            ignored(access.point_3d_classify(k, face, pnt, ctx.src_zone_tol_3d, false, &mut temp))?;
            if temp.point_class() != PointClassType::Unknown {
                ivl.mid = temp;
            }
        }
    }

    // 5. check for endPoint-based classification conflicts
    if did_min && did_max && min.inside != max.inside {
        // Fix 1: use lone classification not marked as problematic
        if min.possible_problem && !max.possible_problem {
            min.inside = max.inside;
        } else if !min.possible_problem && max.possible_problem {
            max.inside = min.inside;
        } else {
            // Fix 2: use classification from endPoint with smallest deviation
            let start_dev = ivl.start.deviation();
            let end_dev = ivl.end.deviation();
            if start_dev > end_dev + 1.0E-10 {
                min.inside = max.inside;
            } else if start_dev < end_dev - 1.0E-10 {
                max.inside = min.inside;
            } else {
                return Ok(());
            }
        }
    }

    // 6. check for successful inside classification of interval to the face
    if (did_min && min.inside)
        || (did_max && max.inside)
        || (did_mid
            && ivl.mid.point_class() == PointClassType::Face
            && ivl.mid.object() == Some(face))
    {
        // when the classification was done through the endPoints
        if !did_mid {
            ivl.mid.set_class_object(PointClassType::Face, Some(face));
        }

        // set any StartPoint or EndPoint with face (or no) classifications to the
        // MidPoint's face
        let mid_obj = ivl.mid.object();
        if matches!(
            ivl.start.point_class(),
            PointClassType::Face | PointClassType::Unknown
        ) {
            ivl.start.set_class_object(PointClassType::Face, mid_obj);
        }
        if matches!(
            ivl.end.point_class(),
            PointClassType::Face | PointClassType::Unknown
        ) {
            ivl.end.set_class_object(PointClassType::Face, mid_obj);
        }
    }

    Ok(())
}

// ---- partial coincidence ---------------------------------------------------------------

/// Which return rule a coincidence block of `TestForPartialCoincidentEdges` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoinCheck {
    /// First / last interval: return the first coincident range.
    Range,
    /// Single interval between vertices at one end pair: stop at the first coincident
    /// range, reporting it only when its gap exceeds a third of the tolerance.
    Vertices,
}

/// One coincidence block of `TestForPartialCoincidentEdges` over the edges of two
/// interval interiors: `Some` to return (with or without a curve), `None` to go on.
fn partial_coincidence<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    mid: &PointClassification<O>,
    o_mid: &PointClassification<O>,
    check: CoinCheck,
) -> Result<Option<PartialCoincidence>> {
    let edge = mid.object().ok_or(SolidError::null_pointer(OP_PARTIAL))?;
    let o_edge = o_mid.object().ok_or(SolidError::null_pointer(OP_PARTIAL))?;
    let curve = edge_curve(geometry, edge)?;
    let o_curve = edge_curve(geometry, o_edge)?;
    let edge_ivl = edge_interval(geometry, Some(edge))?;
    let o_edge_ivl = edge_interval(geometry, Some(o_edge))?;
    let xsect_tol_3d = edge_pair_xsect_tol_3d(k, geometry, edge, o_edge)?;

    let coincide = |tol: f64| -> Result<Vec<Solution<NoObject>>> {
        Ok(curve
            .global_coincidence_checker(k, &edge_ivl, o_curve, &o_edge_ivl, tol, true, [None; 2])?
            .solutions)
    };

    // find coincident points and spans between the two segments with larger tol
    let mut solutions = coincide(3.0 * xsect_tol_3d)?;

    let mut i = 0;
    while i < solutions.len() {
        let Some(mut sol) = solutions.get(i).copied() else {
            break;
        };

        // skip points of coincidence
        if sol.solution_type != SolutionType::RangeOfValues {
            i += 1;
            continue;
        }

        // set maxTol to max deviation at coincident range end points
        let mut max_gap = max2(sol.start.solution_value, sol.end.solution_value);

        // when MaxGap is larger tol
        if max_gap > xsect_tol_3d {
            // check for coincidence with normal tolerance (into the same array:
            // rSol now reads the re-run's entry i, or keeps the old one)
            solutions = coincide(xsect_tol_3d)?;
            if let Some(s) = solutions.get(i) {
                sol = *s;
            }

            // when segments still have coincident regions
            if let Some(sol2) = solutions.first() {
                if sol2.solution_type == SolutionType::RangeOfValues {
                    max_gap = max2(sol.start.solution_value, sol.end.solution_value);
                }
            }
        }

        if check == CoinCheck::Vertices && max_gap <= xsect_tol_3d / 3.0 {
            return Ok(Some(PartialCoincidence::default()));
        }

        // return a copy of the edge curve trimmed to the coincident range
        // `new SmBSplineCurve(*pBSC)`: a line or conic is sliced to its B-spline part
        let bsc = as_b_spline(curve).ok_or(SolidError::invalid_input(
            OP_PARTIAL,
            "coincident edge curve is not a B-spline",
        ))?;
        let mut copy = bsc.clone();
        let mut coin_domain = Extent1d::from_min_max(
            sol.start.parameters.first().copied().unwrap_or(0.0),
            sol.end.parameters.first().copied().unwrap_or(0.0),
        );
        ignored(copy.trim(k, &mut coin_domain, TrimMode::Notify))?;

        return Ok(Some(PartialCoincidence {
            max_partial_coin_gap: max_gap,
            new_curve_to_test: Some(copy),
        }));
    }

    Ok(None)
}

// ---- helpers -----------------------------------------------------------------------------

/// `SM_CAST_PTR(SmBSplineCurve, ..)`: the B-spline (base) of the kinds derived from
/// `SmBSplineCurve` (`SmLine`, `SmEllipse`/`SmCircle`, `SmHyperbola`, `SmParabola`), `None`
/// for the others.
const fn as_b_spline(c: &Curve) -> Option<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

fn bad_boundary() -> SolidError {
    SolidError::invalid_input(OP_INDEX, "boundary index out of range")
}

fn empty_classification(op: &'static str) -> SolidError {
    SolidError::invalid_input(op, "empty classification")
}

fn is_vertex_or_edge<O: ClassObject>(pc: &PointClassification<O>) -> bool {
    matches!(
        pc.point_class(),
        PointClassType::Vertex | PointClassType::Edge
    )
}

/// `IsConnectedTo(crTgtOwners, lFoundIndx)`: on a match the found index is updated.
fn connected_any<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    pc: &PointClassification<O>,
    targets: &[O],
    found_index: &mut u64,
) -> Result<bool> {
    match pc.is_connected_to_any(geometry, targets)? {
        Some(i) => {
            *found_index = i as u64;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `SmTol::IsClosed(const SmObject*, SmXSectTol3d*)` for a curve with the tolerance
/// given: closed over its natural interval; false for no curve.
fn tol_is_closed_curve(k: &Kernel, curve: Option<&Curve>, xsect_tol_3d: f64) -> Result<bool> {
    match curve {
        None => Ok(false),
        Some(c) => c.is_closed(k, &c.natural_interval(), xsect_tol_3d),
    }
}

/// `SmCrvOnSurf(uv_curve, surface)` as a curve.
fn crv_on_surf(k: &Kernel, uv_curve: &Curve, surface: Option<&Surface>) -> Result<Curve> {
    let Some(surface) = surface else {
        return Err(SolidError::invalid_input(OP_COIN, "no surface 1"));
    };
    Ok(Curve::from(CrvOnSurf::new(
        k,
        uv_curve.clone_for_evaluation()?,
        surface.clone_for_evaluation()?,
        None,
        0,
    )?))
}

// ---- callees in units not ported yet ---------------------------------------------------

/// The B-rep edge behind edge handle `edge` ([`ClassGeometry::topology_entity`]); an
/// adapter without that hook keeps the deferral on `upstream`.
pub(super) fn edge_of<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    edge: O,
    upstream: &'static str,
) -> Result<(&'g Brep, EdgeId)> {
    match geometry.topology_entity(edge) {
        Some((b, EntityRef::Edge(e))) => Ok((b, e)),
        Some(_) => Err(SolidError::invalid_input(OP_GEOM, "handle is not an edge")),
        None => Err(SolidError::unported(upstream, "topology.edge")),
    }
}

/// The B-rep vertex behind vertex handle `vertex`; an adapter without the
/// [`ClassGeometry::topology_entity`] hook keeps the deferral on `upstream`.
pub(super) fn vertex_of<'g, O: ClassObject>(
    geometry: &'g impl ClassGeometry<O>,
    vertex: O,
    upstream: &'static str,
) -> Result<(&'g Brep, VertexId)> {
    match geometry.topology_entity(vertex) {
        Some((b, EntityRef::Vertex(v))) => Ok((b, v)),
        Some(_) => Err(SolidError::invalid_input(OP_GEOM, "handle is not a vertex")),
        None => Err(SolidError::unported(upstream, "topology.vertex")),
    }
}

/// The entity of connection target `target` in B-rep `b`; `None` for a target in
/// another B-rep, connected to nothing here.
fn connect_target<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    b: &Brep,
    target: O,
    upstream: &'static str,
    unit: &'static str,
) -> Result<Option<EntityRef>> {
    let (tb, e) = geometry
        .topology_entity(target)
        .ok_or(SolidError::unported(upstream, unit))?;
    Ok(std::ptr::eq(b, tb).then_some(e))
}

/// `SmEdge::GetCurve`; a `NULL` curve is dereferenced by every caller.
fn edge_curve<O: ClassObject>(geometry: &impl ClassGeometry<O>, edge: O) -> Result<&Curve> {
    let (b, e) = edge_of(geometry, edge, "SmEdge::GetCurve")?;
    edge::curve(b, e)?.ok_or(SolidError::null_pointer(OP_GEOM))
}

/// `SmEdge::GetInterval` (of the edge object of a classification; a `NULL` edge is
/// dereferenced upstream).
fn edge_interval<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: Option<O>,
) -> Result<Extent1d> {
    let edge = edge.ok_or(SolidError::null_pointer(OP_GEOM))?;
    let (b, e) = edge_of(geometry, edge, "SmEdge::GetInterval")?;
    edge::interval(b, e)
}

/// `SmEdge::IsConnectedTo`.
fn edge_is_connected_to<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: O,
    target: Option<O>,
) -> Result<bool> {
    const UPSTREAM: &str = "SmEdge::IsConnectedTo";
    // no work - no Topo (IsConnectedTo(NULL) is TRUE before any member is read)
    let Some(target) = target else {
        return Ok(true);
    };
    let (b, e) = edge_of(geometry, edge, UPSTREAM)?;
    match connect_target(geometry, b, target, UPSTREAM, "topology.edge")? {
        Some(t) => edge::is_connected_to(b, e, Some(t)),
        None => Ok(false),
    }
}

/// `SmVertex::IsConnectedTo`.
fn vertex_is_connected_to<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    vertex: O,
    target: Option<O>,
) -> Result<bool> {
    const UPSTREAM: &str = "SmVertex::IsConnectedTo";
    // no work - no Topo (IsConnectedTo(NULL) is TRUE before any member is read)
    let Some(target) = target else {
        return Ok(true);
    };
    let (b, v) = vertex_of(geometry, vertex, UPSTREAM)?;
    match connect_target(geometry, b, target, UPSTREAM, "topology.vertex")? {
        Some(t) => vertex::is_connected_to(b, v, Some(t)),
        None => Ok(false),
    }
}

/// `SmVertex::GetPoint`.
fn vertex_point<O: ClassObject>(geometry: &impl ClassGeometry<O>, vertex: O) -> Result<Point3> {
    let (b, v) = vertex_of(geometry, vertex, "SmVertex::GetPoint")?;
    vertex::point(b, v)
}

/// `SmTol::GetXSectTol3d(const SmObject*, const SmObject*)` of two edges: the sum of
/// their zone tolerances (`GetTolerance`).
fn edge_pair_xsect_tol_3d<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    edge: O,
    other: O,
) -> Result<f64> {
    Ok(xsect_tol_3d_from_zones(
        k,
        object_zone_tol_3d(k, geometry, Some(edge))?,
        object_zone_tol_3d(k, geometry, Some(other))?,
    ))
}

// ---- face queries ------------------------------------------------------------------------

/// The face queries of `ClassifyFaceInterval` (upstream's `const` `SmFace` members,
/// which may add UV trim curves and caches to the face's B-rep) over either kind of
/// adapter.
trait FaceQueries<O: ClassObject> {
    type Geometry: ClassGeometry<O>;
    fn geometry(&self) -> &Self::Geometry;
    /// `SmFace::FindVertexSector(pV, *m_cpCurve, GetUVCurve1(), from, to, direction,
    /// sSrcZoneTol3d, TRUE, pVUFound, bPossibleProblem, eProblem, bUVSpaceOkay)`:
    /// inside when a vertex use sector of `face` at `vertex` contains the curve leaving
    /// `from` towards `to`.
    #[allow(clippy::too_many_arguments)] // upstream's arguments
    fn find_vertex_sector(
        &mut self,
        k: &Kernel,
        face: O,
        vertex: O,
        curve: &Curve,
        uv_curve: Option<&Curve>,
        from: f64,
        to: f64,
        direction: OrientType,
        src_zone_tol_3d: f64,
        uv_space_okay: bool,
    ) -> Result<EndInside>;
    /// `SmFace::EdgeCurveOnClassify(pE, dEdgeParam, *m_cpCurve, ivl, eCurveDirection,
    /// sSrcZoneTol3d, rbInside, rbPossibleProblem, reProblem, bUVSpaceOkay[,
    /// lFaceIndx])`.
    #[allow(clippy::too_many_arguments)] // upstream's arguments
    fn edge_curve_on_classify(
        &mut self,
        k: &Kernel,
        face: O,
        edge: O,
        edge_param: f64,
        curve: &Curve,
        interval: &Extent1d,
        direction: OrientType,
        src_zone_tol_3d: f64,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<EndInside>;
    /// `SmFace::PointClassify(uv, tol, bDoXSects, bUseUV, pc)`.
    #[allow(clippy::too_many_arguments)] // upstream's arguments
    fn point_classify(
        &mut self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        use_uv: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()>;
    /// `SmFace::Point3DClassify(point, tol, bDoXSects, pc)`.
    fn point_3d_classify(
        &mut self,
        k: &Kernel,
        face: O,
        point: Point3,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()>;
}

/// The mutable and read-only topology operations of [`set_do_filleting_geometry_with`].
trait FilletAccess<O: ClassObject> {
    type Geometry: ClassGeometry<O>;
    fn geometry(&self) -> &Self::Geometry;
    fn set_vertex_point(&mut self, vertex: O, point: Point3) -> Result<()>;
    fn set_edge_curve(&mut self, edge: O, curve: Curve) -> Result<()>;
}

/// A read-only adapter: `EdgeCurveOnClassify` and `Point3DClassify` run on a copy of
/// the face's B-rep (#614); `PointClassify` runs over the unchanged topology (a UV trim
/// it would make defers); `FindVertexSector` defers until the caller gives mutable
/// access ([`CurveClassification::classify_face_interval_mut`]).
struct ReadOnly<'g, G>(&'g G);

impl<O: ClassObject, G: ClassGeometry<O>> FilletAccess<O> for ReadOnly<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn set_vertex_point(&mut self, _vertex: O, _point: Point3) -> Result<()> {
        // UNPORTED(topology.vertex): SmVertex::SetPoint
        // Proof (unreachable): the ReadOnly view serves queries that only make caches, and
        // 0 hits in the green difftest sweep show none met this; Mutable and the B-rep
        // impls resolve it.
        Err(SolidError::unported(
            "SmVertex::SetPoint",
            "topology.vertex",
        ))
    }
    fn set_edge_curve(&mut self, _edge: O, _curve: Curve) -> Result<()> {
        // UNPORTED(topology.edge): SmEdge::SetCurve
        // Proof (unreachable): the ReadOnly view serves queries that only make caches, and
        // 0 hits in the green difftest sweep show none met this; Mutable and the B-rep
        // impls resolve it.
        Err(SolidError::unported("SmEdge::SetCurve", "topology.edge"))
    }
}

impl<O: ClassObject, G: ClassGeometry<O>> FaceQueries<O> for ReadOnly<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn find_vertex_sector(
        &mut self,
        _k: &Kernel,
        _face: O,
        _vertex: O,
        _curve: &Curve,
        _uv_curve: Option<&Curve>,
        _from: f64,
        _to: f64,
        _direction: OrientType,
        _src_zone_tol_3d: f64,
        _uv_space_okay: bool,
    ) -> Result<EndInside> {
        // UNPORTED(topology.face_p4): SmFace::FindVertexSector
        // Proof (unreachable): the ReadOnly view serves queries that only make caches, and
        // 0 hits in the green difftest sweep show none met this; Mutable and the B-rep
        // impls resolve it.
        Err(SolidError::unported(
            "SmFace::FindVertexSector",
            "topology.face_p4",
        ))
    }
    fn edge_curve_on_classify(
        &mut self,
        k: &Kernel,
        face: O,
        edge: O,
        edge_param: f64,
        curve: &Curve,
        interval: &Extent1d,
        direction: OrientType,
        src_zone_tol_3d: f64,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<EndInside> {
        let unresolved = || {
            // DISPATCH GAP: the adapter cannot resolve the face or edge handle to its
            // `Brep` entity (`ClassGeometry::topology_entity`).
            SolidError::unported("SmFace::EdgeCurveOnClassify", "query.face_p1")
        };
        let (b, f) = resolve_face(self.0, face).ok_or_else(unresolved)?;
        let Some(e) = same_brep_edge(self.0, b, edge).ok_or_else(unresolved)?? else {
            return Ok(EndInside::default());
        };
        let mut copy = b.clone();
        edge_curve_on_classify_in(
            k,
            &mut copy,
            f,
            e,
            edge_param,
            curve,
            interval,
            direction,
            src_zone_tol_3d,
            uv_space_okay,
            face_index,
        )
    }
    fn point_classify(
        &mut self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        use_uv: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()> {
        let geometry = self.0;
        let (b, f) = resolve_face(geometry, face).ok_or_else(unresolved_uv)?;
        let mut pc = point_class.map_objects(&mut |o| {
            geometry
                .topology_entity(o)
                .map(|(_, e)| e)
                .ok_or_else(unresolved_uv)
        })?;
        // read-only topology: a UV trim the walk would create is `Unported`
        let r = qf2::point_classify_prepared(
            k,
            b,
            f,
            uv,
            &mut pc,
            point_classify_options(src_zone_tol_3d, do_xsects, use_uv),
        );
        *point_class =
            pc.map_objects(&mut |e| geometry.topology_handle_in(b, e).ok_or_else(unresolved_uv))?;
        r
    }
    fn point_3d_classify(
        &mut self,
        k: &Kernel,
        face: O,
        point: Point3,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()> {
        let geometry = self.0;
        let (b, f) = resolve_face(geometry, face).ok_or_else(unresolved_3d)?;
        let mut copy = b.clone();
        point_3d_classify_in(
            k,
            geometry,
            &mut copy,
            b,
            f,
            point,
            src_zone_tol_3d,
            do_xsects,
            point_class,
        )
    }
}

/// A mutable adapter: the face queries run on the B-rep that
/// [`ClassGeometryMut::topology_entity_mut`] resolves the face handle to. A face handle
/// it cannot resolve falls back to [`ReadOnly`].
struct Mutable<'g, G>(&'g mut G);

impl<O: ClassObject, G: ClassGeometryMut<O>> FaceQueries<O> for Mutable<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn find_vertex_sector(
        &mut self,
        k: &Kernel,
        face: O,
        vertex: O,
        curve: &Curve,
        uv_curve: Option<&Curve>,
        from: f64,
        to: f64,
        direction: OrientType,
        src_zone_tol_3d: f64,
        uv_space_okay: bool,
    ) -> Result<EndInside> {
        let unresolved = || {
            // DISPATCH GAP: the adapter cannot resolve the face or vertex handle to its
            // `Brep` entity (`ClassGeometry::topology_entity`).
            SolidError::unported("SmFace::FindVertexSector", "topology.face_p4")
        };
        let v = {
            let geometry = &*self.0;
            let (b, _) = resolve_face(geometry, face).ok_or_else(unresolved)?;
            same_brep_vertex(geometry, b, vertex).ok_or_else(unresolved)??
        };
        let Some(v) = v else {
            return Ok(EndInside::default());
        };
        let Some((b, f)) = resolve_face_mut(self.0, face) else {
            return ReadOnly(&*self.0).find_vertex_sector(
                k,
                face,
                vertex,
                curve,
                uv_curve,
                from,
                to,
                direction,
                src_zone_tol_3d,
                uv_space_okay,
            );
        };
        let r = find_vertex_sector_in(
            k,
            b,
            f,
            v,
            curve,
            uv_curve,
            from,
            to,
            direction,
            src_zone_tol_3d,
            uv_space_okay,
        )?;
        Ok(r)
    }
    fn edge_curve_on_classify(
        &mut self,
        k: &Kernel,
        face: O,
        edge: O,
        edge_param: f64,
        curve: &Curve,
        interval: &Extent1d,
        direction: OrientType,
        src_zone_tol_3d: f64,
        uv_space_okay: bool,
        face_index: Option<u64>,
    ) -> Result<EndInside> {
        let unresolved = || {
            // DISPATCH GAP: the adapter cannot resolve the face or edge handle to its
            // `Brep` entity (`ClassGeometry::topology_entity`).
            SolidError::unported("SmFace::EdgeCurveOnClassify", "query.face_p1")
        };
        let e = {
            let geometry = &*self.0;
            let (b, _) = resolve_face(geometry, face).ok_or_else(unresolved)?;
            same_brep_edge(geometry, b, edge).ok_or_else(unresolved)??
        };
        let Some(e) = e else {
            return Ok(EndInside::default());
        };
        let Some((b, f)) = resolve_face_mut(self.0, face) else {
            return ReadOnly(&*self.0).edge_curve_on_classify(
                k,
                face,
                edge,
                edge_param,
                curve,
                interval,
                direction,
                src_zone_tol_3d,
                uv_space_okay,
                face_index,
            );
        };
        let r = edge_curve_on_classify_in(
            k,
            b,
            f,
            e,
            edge_param,
            curve,
            interval,
            direction,
            src_zone_tol_3d,
            uv_space_okay,
            face_index,
        )?;
        Ok(r)
    }
    fn point_classify(
        &mut self,
        k: &Kernel,
        face: O,
        uv: Point2,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        use_uv: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()> {
        resolve_face(&*self.0, face).ok_or_else(unresolved_uv)?;
        let mut pc = point_class.map_objects(&mut |o| {
            self.0
                .topology_entity(o)
                .map(|(_, e)| e)
                .ok_or_else(unresolved_uv)
        })?;
        let Some((b, f)) = resolve_face_mut(self.0, face) else {
            return ReadOnly(&*self.0).point_classify(
                k,
                face,
                uv,
                src_zone_tol_3d,
                do_xsects,
                use_uv,
                point_class,
            );
        };
        let r = qf2::point_classify(
            k,
            b,
            f,
            uv,
            &mut pc,
            point_classify_options(src_zone_tol_3d, do_xsects, use_uv),
        );
        let geometry = &*self.0;
        let (b, _) = resolve_face(geometry, face).ok_or_else(unresolved_uv)?;
        *point_class =
            pc.map_objects(&mut |e| geometry.topology_handle_in(b, e).ok_or_else(unresolved_uv))?;
        r
    }
    fn point_3d_classify(
        &mut self,
        k: &Kernel,
        face: O,
        point: Point3,
        src_zone_tol_3d: f64,
        do_xsects: bool,
        point_class: &mut PointClassification<O>,
    ) -> Result<()> {
        resolve_face(&*self.0, face).ok_or_else(unresolved_3d)?;
        let mut pc = point_class.map_objects(&mut |o| {
            self.0
                .topology_entity(o)
                .map(|(_, e)| e)
                .ok_or_else(unresolved_3d)
        })?;
        let Some((b, f)) = resolve_face_mut(self.0, face) else {
            return ReadOnly(&*self.0).point_3d_classify(
                k,
                face,
                point,
                src_zone_tol_3d,
                do_xsects,
                point_class,
            );
        };
        let r = qf1::point_3d_classify(
            k,
            b,
            f,
            point,
            src_zone_tol_3d,
            do_xsects,
            &mut pc,
            qf1::Point3dClassifyOptions::default(),
        );
        let geometry = &*self.0;
        let (b, _) = resolve_face(geometry, face).ok_or_else(unresolved_3d)?;
        *point_class =
            pc.map_objects(&mut |e| geometry.topology_handle_in(b, e).ok_or_else(unresolved_3d))?;
        r
    }
}

impl<O: ClassObject, G: ClassGeometryMut<O>> FilletAccess<O> for Mutable<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn set_vertex_point(&mut self, vertex: O, point: Point3) -> Result<()> {
        match self.0.topology_entity_mut(vertex) {
            Some((b, EntityRef::Vertex(v))) => {
                b.vertex_mut(v)?.point = point;
                Ok(())
            }
            _ => ReadOnly(&*self.0).set_vertex_point(vertex, point),
        }
    }
    fn set_edge_curve(&mut self, edge: O, curve: Curve) -> Result<()> {
        self.0.set_filleting_edge_curve(edge, curve)
    }
}

/// The B-rep and face behind face handle `face` ([`ClassGeometry::topology_entity`]);
/// `None` for an adapter that cannot resolve it.
fn resolve_face<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    face: O,
) -> Option<(&Brep, FaceId)> {
    match geometry.topology_entity(face)? {
        (b, EntityRef::Face(f)) => Some((b, f)),
        _ => None,
    }
}

/// [`resolve_face`], mutably for the classification queries
/// ([`ClassGeometryMut::topology_entity_for_query`]).
fn resolve_face_mut<O: ClassObject>(
    geometry: &mut impl ClassGeometryMut<O>,
    face: O,
) -> Option<(&mut Brep, FaceId)> {
    match geometry.topology_entity_for_query(face)? {
        (b, EntityRef::Face(f)) => Some((b, f)),
        _ => None,
    }
}

/// The edge behind edge handle `edge` when it is one of B-rep `b`'s: `Some(None)` for
/// an edge of another B-rep (no edge use on a face of `b`), `None` for a handle the
/// adapter cannot resolve.
fn same_brep_edge<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    b: &Brep,
    edge: O,
) -> Option<Result<Option<EdgeId>>> {
    let (edge_brep, entity) = geometry.topology_entity(edge)?;
    let EntityRef::Edge(e) = entity else {
        return Some(Err(SolidError::invalid_input(
            OP_FACE,
            "edge class object not an edge",
        )));
    };
    Some(Ok(core::ptr::eq(b, edge_brep).then_some(e)))
}

/// The vertex behind vertex handle `vertex` when it is one of B-rep `b`'s: `Some(None)`
/// for a vertex of another B-rep (no vertex use on a face of `b`), `None` for a handle the
/// adapter cannot resolve.
fn same_brep_vertex<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    b: &Brep,
    vertex: O,
) -> Option<Result<Option<VertexId>>> {
    let (vertex_brep, entity) = geometry.topology_entity(vertex)?;
    let EntityRef::Vertex(v) = entity else {
        return Some(Err(SolidError::invalid_input(
            OP_FACE,
            "vertex class object not a vertex",
        )));
    };
    Some(Ok(core::ptr::eq(b, vertex_brep).then_some(v)))
}

/// [`crate::topology::face::p4::find_vertex_sector`] of face `f` of `b`.
#[allow(clippy::too_many_arguments)]
fn find_vertex_sector_in(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    v: VertexId,
    curve: &Curve,
    uv_curve: Option<&Curve>,
    from: f64,
    to: f64,
    direction: OrientType,
    src_zone_tol_3d: f64,
    uv_space_okay: bool,
) -> Result<EndInside> {
    let sector = crate::topology::face::p4::find_vertex_sector(
        k,
        b,
        f,
        crate::topology::face::p4::FindVertexSectorArgs {
            vertex: v,
            curve,
            uv_curve,
            param_at_vertex: from,
            param_ivl_end: to,
            orientation: direction,
            zone_tol_3d: src_zone_tol_3d,
            validate_sector: true,
            uv_space_okay,
        },
    )?;
    Ok(EndInside {
        inside: sector.vertexuse.is_some(),
        possible_problem: sector.possible_problem,
    })
}

/// [`qf1::edge_curve_on_classify`] of face `f` of `b` (an edge of another B-rep has no
/// edge use on this face: not inside, no problem, as upstream's edge use walk finds).
#[allow(clippy::too_many_arguments)]
fn edge_curve_on_classify_in(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    e: EdgeId,
    edge_param: f64,
    curve: &Curve,
    interval: &Extent1d,
    direction: OrientType,
    src_zone_tol_3d: f64,
    uv_space_okay: bool,
    face_index: Option<u64>,
) -> Result<EndInside> {
    let class = qf1::edge_curve_on_classify(
        k,
        b,
        f,
        Some(e),
        curve,
        &qf1::EdgeCurveOnOptions {
            edge_param,
            curve_interval: *interval,
            curve_orientation: direction,
            curve_tolerance: src_zone_tol_3d,
            uv_space_okay,
            face_index,
        },
    )?;
    Ok(EndInside {
        inside: class.inside,
        possible_problem: class.possible_problem,
    })
}

/// The error of a `Point3DClassify` whose face or classified entity the adapter
/// cannot resolve.
fn unresolved_3d() -> SolidError {
    // DISPATCH GAP: the adapter cannot resolve the face handle to its `Brep` face
    // (`ClassGeometry::topology_entity`) or a classified entity back to a handle.
    SolidError::unported("SmFace::Point3DClassify", "query.face_p1")
}

/// The error of a `PointClassify` whose face or classified entity the adapter cannot
/// resolve.
fn unresolved_uv() -> SolidError {
    // DISPATCH GAP: the adapter cannot resolve the face handle to its `Brep` face
    // (`ClassGeometry::topology_entity`) or a classified entity back to a handle.
    SolidError::unported("SmFace::PointClassify", "query.face_p2")
}

/// `PointClassify(uv, sSrcZoneTol3d, bDoXSects, bUseUV, pc)`'s arguments (the others
/// at their defaults).
fn point_classify_options(
    src_zone_tol_3d: f64,
    do_xsects: bool,
    use_uv: bool,
) -> qf2::PointClassifyOptions<'static> {
    qf2::PointClassifyOptions {
        src_zone_tol_3d,
        do_boundary_intersections: do_xsects,
        use_uv_classification: use_uv,
        ..qf2::PointClassifyOptions::default()
    }
}

/// [`qf1::point_3d_classify`] of face `f` on `work` (the face's B-rep `b`, or a copy
/// of it), its classification handed back in the caller's handles
/// ([`ClassGeometry::topology_handle_in`] of `b`). `point_class` holds what the
/// classification wrote also when it fails, as upstream's.
#[allow(clippy::too_many_arguments)]
fn point_3d_classify_in<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    work: &mut Brep,
    b: &Brep,
    f: FaceId,
    point: Point3,
    src_zone_tol_3d: f64,
    do_xsects: bool,
    point_class: &mut PointClassification<O>,
) -> Result<()> {
    let mut pc = point_class.map_objects(&mut |o| {
        geometry
            .topology_entity(o)
            .map(|(_, e)| e)
            .ok_or_else(unresolved_3d)
    })?;
    let r = qf1::point_3d_classify(
        k,
        work,
        f,
        point,
        src_zone_tol_3d,
        do_xsects,
        &mut pc,
        qf1::Point3dClassifyOptions::default(),
    );
    *point_class =
        pc.map_objects(&mut |e| geometry.topology_handle_in(b, e).ok_or_else(unresolved_3d))?;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::intersect::point_class::ClassObjectKind as K;
    use crate::surface::volume::Volume;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, K);

    impl ClassObject for H {
        fn class_kind(&self) -> K {
            self.1
        }
    }

    struct Geo {
        curves: Vec<Curve>,
    }

    impl ClassGeometry<H> for Geo {
        fn curve(&self, o: H) -> Option<&Curve> {
            self.curves.get(o.0)
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

    fn line(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(Line::bounded(
            Vec3::from_array(a),
            Vec3::from_array([b[0] - a[0], b[1] - a[1], b[2] - a[2]]),
            &Extent1d::from_min_max(0.0, 1.0),
            &BoundedLineOptions::default(),
        ))
    }

    fn classification(k: &Kernel, params: &[f64]) -> CurveClassification<H> {
        let mut cc = CurveClassification::new(k);
        cc.set_size(k, params.len() - 1).unwrap();
        for (i, w) in params.windows(2).enumerate() {
            cc.at_mut(i)
                .unwrap()
                .set_interval(&Extent1d::from_min_max(w[0], w[1]));
        }
        cc.set_curve(Some(H(0, K::Curve)), false);
        cc
    }

    #[test]
    fn boundary_indices_and_mates() {
        let k = Kernel::default();
        let geo = Geo {
            curves: vec![line([0.0; 3], [1.0, 0.0, 0.0])],
        };
        let cc = classification(&k, &[0.0, 0.25, 1.0]);
        assert_eq!(cc.point_class_by_index(0).unwrap().1, 0.0);
        assert_eq!(cc.point_class_by_index(1).unwrap().1, 0.25);
        assert_eq!(cc.point_class_by_index(2).unwrap().1, 1.0);
        assert!(cc.point_class_by_index(3).is_err());
        assert!(cc.point_class_mate_by_index(&k, &geo, 0).unwrap().is_none());
        assert!(cc.point_class_mate_by_index(&k, &geo, 2).unwrap().is_none());
        assert!(cc.point_class_mate_by_index(&k, &geo, 1).unwrap().is_some());
        assert!(CurveClassification::<H>::new(&k)
            .point_class_by_index(0)
            .is_err());
    }

    #[test]
    fn crossing_face_and_touches() {
        let k = Kernel::default();
        let g = Geo { curves: Vec::new() };
        let mut cc = classification(&k, &[0.0, 0.5, 1.0]);
        let face = H(7, K::Face);
        cc.at_mut(1)
            .unwrap()
            .mid
            .set_class_object(PointClassType::Face, Some(face));
        assert!(cc.is_crossing_face(None));
        assert!(cc.is_crossing_face(Some(face)));
        assert!(!cc.is_crossing_face(Some(H(8, K::Face))));
        assert!(cc
            .point_classifies_connected_to(&g, &[])
            .unwrap()
            .is_empty());
        assert!(!cc.is_crossed_by_edge(&g).unwrap());
        assert!(cc
            .find_vertex_objects(&g, None)
            .unwrap()
            .vertices
            .is_empty());
        assert!(cc.find_edge_objects(&g, None, 3).unwrap().is_empty());
    }

    #[test]
    fn collapse_degenerate_interval() {
        let k = Kernel::default();
        let geo = Geo {
            curves: vec![line([0.0; 3], [1.0, 0.0, 0.0])],
        };
        let mut cc = classification(&k, &[0.0, 0.5, 0.5, 1.0]);
        assert!(cc.collapse_interval(&k, &geo, 0).is_err());
        cc.collapse_interval(&k, &geo, 1).unwrap();
        assert_eq!(cc.size(), 2);
        assert!(cc.collapse_interval(&k, &geo, 5).is_err());
    }

    #[test]
    fn partial_coincidence_needs_intervals() {
        let k = Kernel::default();
        let geo = Geo {
            curves: vec![line([0.0; 3], [1.0, 0.0, 0.0])],
        };
        let cc = classification(&k, &[0.0, 1.0]);
        let r = cc.test_for_partial_coincident_edges(&k, &geo, &cc).unwrap();
        assert_eq!(r.max_partial_coin_gap, 0.0);
        assert!(r.new_curve_to_test.is_none());
    }
}

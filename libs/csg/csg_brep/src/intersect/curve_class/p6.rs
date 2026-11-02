//! Interval insertion, homogenization and endpoint repair for curve classifications.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`CurveClassification::homogenize`], [`CurveClassification::homogenize_mut`] | `SmCurveClassification::Homogenize` |
//! | [`CurveClassification::insert_curve_interval`], [`CurveClassification::insert_curve_interval_mut`] | `SmCurveClassification::InsertCurveInterval` |
//! | [`CurveClassification::fix_end_point_class`] | `SmCurveClassification::FixEndPointClass` |
//! | [`CurveClassification::merge_interval`], [`CurveClassification::merge_interval_mut`] | `SmCurveClassification::MergeInterval` |
//! | [`MergeIntervalBrep`], [`MergeContext`] | the non-const `SmVertex`, `SmEdge`, `SmFace`, `SmRegion` and `SmBrep` objects behind the classification's pointers |
//!
//! Deviations: handles and caller-provided geometry replace pointers, as in p1/p3.
//! Status/out-parameters become Results. The optional classify-curve parameter passed
//! to `ComputePointParameters` is explicitly unused upstream and is omitted here.
//! Undefined indices return an error. Debug-only
//! diagnostics are omitted. Insertion restarts after a squeeze in a loop, in upstream
//! order, bounded by the initial interval count plus one (every restart removes one).
//! Homogenization's final squeezes must reduce the count. Nonempty boundary lookups
//! await p2, as does interval splicing/squeezing.
//!
//! **`MergeInterval` edits the B-rep.** [`CurveClassification::merge_interval_mut`]
//! makes the edits (`MergePointClass`, `GlueVertices`, `MakeWireEdge`,
//! `FindVertexSector`, `MakeEdgeInFaceWithVU`, the edge and vertex tolerances) on the
//! arena a [`MergeIntervalBrep`] hands out for each topology handle, through the ported
//! `topology` functions. [`CurveClassification::merge_interval`] runs the same code over
//! a read-only adapter, which defers at the first edit (`SmBrep mutable owning
//! context`, unit `topology.brep_p1`, as part 5's read-only entry points do). The
//! vertex uses of the face branch stay arena ids of the face's B-rep (upstream's
//! `SmVertexuse*` locals). A classification object upstream casts to a face, edge or
//! vertex that is missing or of another kind, a face without surface or B-rep, and an
//! edge without vertices are dereferenced upstream: errors here. `FindVertexSector`
//! of a vertex of another B-rep finds no sector, as upstream (the vertex has no use on
//! the face). `GlueVertices` gets no notification environment (as the B-rep
//! entry point `topology::brep::p5::glue_vertices`). `SetExact` is the no-op of the
//! reference build (no `SM_TOLERANT_WATCH`). The reference build is `SM_USE_OLDTOL`, so
//! the coincident-edge branch raises the edge's tolerance to the source tolerance and
//! refreshes the edge's first vertex, as upstream's `SM_OLDTOL_LINE` code.
//!
//! **`Combine` moves a vertex.** A surf/surf/surf point `Combine` propagates to a vertex
//! classification moves that vertex (`SmVertex::SetPoint`, which notifies
//! `ChangeGeometry` and sets the point; no tolerance or cache side effect).
//! [`CurveClassification::insert_curve_interval_mut`] (and part 3's
//! `insert_point_class_mut`) make the move on the arena a [`MergeIntervalBrep`] hands
//! out for the vertex, at the moment upstream makes it; the read-only entry points defer
//! there (`Unported(topology.vertex)`).

use super::p1::{
    compute_face_point_class, compute_face_point_class_in, src_zone_tol_3d_of, MutableParams,
    PointParams, ReadOnlyParams,
};
use super::p3::{
    are_coincident_maybe, classify_crv_loc_by_index, connect_intervals, point_class_loc_by_index,
    Coincident,
};
use super::p5::ClassGeometryMut;
use super::{
    curve_is_degenerate, fix_problems_by_squeezing, squeeze_out_smallest_interval,
    CurveClassification, CurveInterval, IntervalPosition,
};
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::ObjectRef;
use crate::geom::tol::{xsect_tol_3d_from_zones, zone_tol3d, ScaledZero};
use crate::intersect::point_class::{
    object_zone_tol_3d, BrepMergeWithIdentity, ClassGeometry, ClassObject, ClassObjectKind,
    MergeOutputs, PointClassType as Pc, PointClassification, Precedence,
};
use crate::kernel::Kernel;
use crate::math::{Point3, BIG_DOUBLE};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::brep::p9::EdgeInFaceInput;
use crate::topology::face::p4::FindVertexSectorArgs;
use crate::topology::{
    edge, edge as topo_edge, vertex as topo_vertex, Brep, EdgeId, VertexId, VertexuseId,
};

/// Topology created by [`CurveClassification::merge_interval`].
/// These replace the three optional output arrays, which upstream clears on entry.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeIntervalOutput<O> {
    /// Faces created by splitting an existing face.
    pub new_faces: Vec<O>,
    /// Edges created or reused by splitting or merging.
    pub new_edges: Vec<O>,
    /// Vertices created by merging the interval's boundaries.
    pub new_vertices: Vec<O>,
}

impl<O> Default for MergeIntervalOutput<O> {
    fn default() -> Self {
        Self {
            new_faces: Vec::new(),
            new_edges: Vec::new(),
            new_vertices: Vec::new(),
        }
    }
}

const OP: &str = "intersect::curve_class::p6";

/// `target = target.Combine(other)` through adapter `a` (which makes or defers the
/// vertex move of the propagated surf/surf/surf point).
fn combine_in<O: ClassObject>(
    a: &mut impl Imprint<O>,
    target: &mut PointClassification<O>,
    other: &PointClassification<O>,
) -> Result<()> {
    if a.combine(target, other)? == Precedence::Other {
        *target = *other;
    }
    Ok(())
}

/// Peer supplied through legacy handle metadata or an explicitly linked local pair.
#[derive(Clone, Copy)]
pub(super) enum FilletPeer<'a, O> {
    Metadata(Option<&'a CurveClassification<O>>),
    Linked(&'a CurveClassification<O>),
}

impl<O: ClassObject> CurveClassification<O> {
    /// Inserts each distinct boundary into the paired classification (`Homogenize`).
    /// Near boundaries retain upstream's sliding/squeezing heuristics.
    ///
    /// # Errors
    /// Propagates classification/geometry failures and unported p2/p4/p7 calls.
    pub fn homogenize(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &mut Self,
    ) -> Result<()> {
        self.homogenize_in(k, &mut ReadOnlyParams(geometry), other)
    }

    /// [`CurveClassification::homogenize`] over topology the boundary point drops may
    /// change: a drop onto a face makes the face's trimmed surface cache on the B-rep
    /// [`ClassGeometryMut::topology_entity_mut`] resolves it to and keeps it, as
    /// upstream (see [`PointClassification::compute_point_parameters_mut`]).
    ///
    /// # Errors
    /// Those of [`CurveClassification::homogenize`].
    pub fn homogenize_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        other: &mut Self,
    ) -> Result<()> {
        self.homogenize_in(k, &mut MutableParams(geometry), other)
    }

    /// `Homogenize` over either kind of access.
    pub(super) fn homogenize_in(
        &mut self,
        k: &Kernel,
        access: &mut impl PointParams<O>,
        other: &mut Self,
    ) -> Result<()> {
        homogenize_boundaries(k, access, self, other)?;
        homogenize_boundaries(k, access, other, self)?;
        fix_problems_by_squeezing(k, access.geometry(), self, other)?;
        fix_problems_by_squeezing(k, access.geometry(), other, self)?;
        while self.size() > other.size() {
            let count = self.size();
            ignore_status(squeeze_out_smallest_interval(self))?;
            if self.size() >= count {
                return Err(SolidError::sm_err(OP));
            }
        }
        while other.size() > self.size() {
            let count = other.size();
            ignore_status(squeeze_out_smallest_interval(other))?;
            if other.size() >= count {
                return Err(SolidError::sm_err(OP));
            }
        }
        Ok(())
    }

    /// Integrates a homogeneous interval, combining classifications in upstream order.
    ///
    /// # Errors
    /// Propagates tolerance, geometry and unported p2/p7 failures. Invalid indices and
    /// a restart without progress return an error instead of recursing indefinitely.
    /// A surf/surf/surf point propagated to a vertex classification defers
    /// (`SmVertex::SetPoint`; [`Self::insert_curve_interval_mut`] makes the move).
    pub fn insert_curve_interval(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        input: &CurveInterval<O>,
    ) -> Result<()> {
        self.insert_curve_interval_in(&mut ReadOnly { k, g: geometry }, input)
    }

    /// [`CurveClassification::insert_curve_interval`] moving the vertex of a vertex
    /// classification a combined surf/surf/surf point propagates to
    /// (`SmVertex::SetPoint`) in the B-rep a [`MergeIntervalBrep`] hands out.
    ///
    /// # Errors
    /// Those of [`CurveClassification::insert_curve_interval`], without the deferral of
    /// the move; [`SolidError::InvalidInput`] for a vertex the adapter cannot resolve
    /// mutably.
    pub fn insert_curve_interval_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        input: &CurveInterval<O>,
    ) -> Result<()> {
        self.insert_curve_interval_in(&mut Mutable { k, g: geometry }, input)
    }

    /// `InsertCurveInterval` over either kind of adapter.
    fn insert_curve_interval_in(
        &mut self,
        a: &mut impl Imprint<O>,
        input: &CurveInterval<O>,
    ) -> Result<()> {
        let mut restarts = self.size().saturating_add(1);
        loop {
            if !self.insert_curve_interval_step(a, input)? {
                return Ok(());
            }
            restarts = restarts
                .checked_sub(1)
                .ok_or_else(|| SolidError::sm_err(OP))?;
        }
    }

    // true means upstream's tail-recursive restart after removal of an interval.
    fn insert_curve_interval_step(
        &mut self,
        a: &mut impl Imprint<O>,
        input: &CurveInterval<O>,
    ) -> Result<bool> {
        let tol = input.mid.obj_zone_tol_3d(a.kernel(), a.geometry())?;
        let fa = self.find_interval(a.kernel(), a.geometry(), input.min(), BIG_DOUBLE, tol)?;
        let fb = self.find_interval(a.kernel(), a.geometry(), input.max(), BIG_DOUBLE, tol)?;
        let (mut i, mut j, mut pa, mut pb) = (fa.index, fb.index, fa.position, fb.position);
        if pb == IntervalPosition::Start && j > i {
            pb = IntervalPosition::End;
            j -= 1;
        }
        if pa == IntervalPosition::End && i < j {
            pa = IntervalPosition::Start;
            i += 1;
        }
        if i != j {
            for index in [i, j] {
                let (coin, end) =
                    are_coincident_maybe(a.kernel(), a.geometry(), self, self, index)?;
                match coin {
                    Coincident::Unsure => return Err(SolidError::sm_err(OP)),
                    Coincident::Yes => {
                        connect_intervals(self, index, end, false)?;
                        return Ok(true);
                    }
                    Coincident::No => {}
                }
            }
            if (input.start.point_class() == Pc::Face
                && input.mid.point_class() == Pc::Face
                && input.end.point_class() == Pc::Face)
                || (input.start.point_class() == Pc::Edge
                    && input.mid.point_class() == Pc::Edge
                    && input.end.point_class() == Pc::Edge)
            {
                for index in i..=j {
                    let ivl = self.at_mut(index)?;
                    combine_in(a, &mut ivl.start, &input.start)?;
                    combine_in(a, &mut ivl.mid, &input.mid)?;
                    combine_in(a, &mut ivl.end, &input.end)?;
                }
                return Ok(false);
            }
            while i < j {
                let first = *self.at(i)?;
                let last = *self.at(j)?;
                if interval_degenerate(a.kernel(), a.geometry(), self, &first)? {
                    i += 1;
                } else if interval_degenerate(a.kernel(), a.geometry(), self, &last)? {
                    j -= 1;
                } else {
                    break;
                }
            }
            if j == i + 1 {
                let first = *self.at(i)?;
                let last = *self.at(j)?;
                if first.start.point_class() != Pc::Vertex {
                    if first.interval.length() < last.interval.length() * 10.0 {
                        connect_intervals(self, i, IntervalPosition::End, false)?;
                        return Ok(true);
                    }
                    if last.interval.length() < first.interval.length() * 10.0 {
                        connect_intervals(self, j, IntervalPosition::Start, false)?;
                        return Ok(true);
                    }
                }
                for (index, ivl) in [(i, first), (j, last)] {
                    if ivl.start.point_class() == Pc::Vertex
                        && ivl.end.point_class() == Pc::Vertex
                        && ivl.interval.length() < self.interval().length() / 1000.0
                    {
                        if ivl.start.deviation() < ivl.end.deviation() / 1000.0 {
                            // RemovePointClass's status is ignored upstream.
                            ignore_status(self.remove_point_class(index, IntervalPosition::End))?;
                            return Ok(true);
                        } else if ivl.end.deviation() < ivl.start.deviation() / 1000.0 {
                            ignore_status(self.remove_point_class(index, IntervalPosition::Start))?;
                            return Ok(true);
                        }
                    }
                }
            }
            if i != j && input.mid.point_class() == Pc::Edge {
                return Ok(false);
            }
        }
        if pa == pb && matches!(pa, IntervalPosition::Start | IntervalPosition::End) {
            let ivl = self.at_mut(i)?;
            let end = if pa == IntervalPosition::Start {
                &mut ivl.start
            } else {
                &mut ivl.end
            };
            combine_in(a, end, &input.start)?;
            combine_in(a, end, &input.end)?;
            return Ok(false);
        }
        if pa == IntervalPosition::Start {
            combine_in(a, &mut self.at_mut(i)?.start, &input.start)?;
        } else if pa == IntervalPosition::Inside {
            if !self.insert_point_class_in(a, &input.start, input.min(), BIG_DOUBLE, false)? {
                return Ok(false);
            }
            i += 1;
        } else {
            return Err(SolidError::sm_err(OP));
        }
        if pb == IntervalPosition::End {
            combine_in(a, &mut self.at_mut(i)?.end, &input.end)?;
        } else if pb == IntervalPosition::Inside {
            let found =
                self.find_interval(a.kernel(), a.geometry(), input.max(), BIG_DOUBLE, tol)?;
            if found.position != IntervalPosition::Inside {
                return Ok(false);
            }
            self.insert_point_class_in(a, &input.end, input.max(), BIG_DOUBLE, false)?;
        } else {
            return Err(SolidError::sm_err(OP));
        }
        combine_in(a, &mut self.at_mut(i)?.mid, &input.mid)?;
        Ok(false)
    }

    /// Repairs an endpoint classified to an edge (`FixEndPointClass`).
    /// Returns whether the classification changed.
    ///
    /// # Errors
    /// Propagates p7 boundary lookup, p2 splicing and geometry failures.
    pub fn fix_end_point_class(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        point: Point3,
    ) -> Result<bool> {
        if self.size() == 0 || index > self.size() {
            return Err(SolidError::invalid_input(OP, "invalid boundary index"));
        }
        let loc = point_class_loc_by_index(self, index)?;
        let pc = *self.pc_at(loc)?;
        if pc.point_class() != Pc::Edge {
            return Ok(false);
        }
        let mut open = true;
        if index > 0 && self.at(index - 1)?.mid.point_class() == Pc::Edge {
            open = false;
        }
        if index < self.size() && self.at(index)?.mid.point_class() == Pc::Edge {
            open = false;
        }
        if open && super::change_edge_point_class_to_near_vertex(k, geometry, index, self, point)? {
            return Ok(true);
        }
        if index > 0 && index < self.size() {
            let prev = &self.at(index - 1)?.mid;
            let next = &self.at(index)?.mid;
            if prev.point_class() == Pc::Edge
                && next.point_class() == Pc::Edge
                && prev.object() == next.object()
                && prev.object() != pc.object()
            {
                connect_intervals(self, index, IntervalPosition::Start, true)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Merges an interval's classifications into topology (`MergeInterval`), over a
    /// read-only adapter: the code runs as upstream until the first B-rep edit, which
    /// defers (`SmBrep mutable owning context`, see [`MergeIntervalBrep`]);
    /// [`Self::merge_interval_mut`] makes the edits.
    ///
    /// # Errors
    /// Invalid interval indices, or [`SolidError::Unported`] for topology tolerances of
    /// opaque handles, p7 fillet handling or the first B-rep edit.
    pub fn merge_interval(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        vertices_only: bool,
    ) -> Result<MergeIntervalOutput<O>> {
        self.merge_interval_with(k, geometry, index, vertices_only, None)
    }

    /// [`Self::merge_interval`] with the classification that
    /// [`Self::other_classification`] names (`GetOtherClassification()`), which the
    /// fillet geometry step (`sm_SetDoFilletingGeometry`) reads. Without it, a
    /// classification doing fillets with an other classification defers that step.
    ///
    /// # Errors
    /// Those of [`Self::merge_interval`]; [`SolidError::Unported`] from the fillet
    /// geometry step (whose other statuses upstream ignores).
    pub fn merge_interval_with(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        vertices_only: bool,
        other: Option<&CurveClassification<O>>,
    ) -> Result<MergeIntervalOutput<O>> {
        self.merge_interval_in(
            &mut ReadOnly { k, g: geometry },
            index,
            vertices_only,
            other,
        )
    }

    /// [`Self::merge_interval_with`] editing the B-reps behind the classification's
    /// topology handles through [`MergeIntervalBrep`]: the end vertices are merged
    /// (`MergePointClass`), and the interval becomes a wire edge in a region, an edge
    /// in a face (splitting it when both ends lie on one loop) or updates the
    /// tolerance of the coincident edge.
    ///
    /// # Errors
    /// Those of [`Self::merge_interval_with`] and of the B-rep operations;
    /// [`SolidError::InvalidInput`] for a topology handle the adapter cannot resolve
    /// mutably and for the inputs upstream dereferences as null or as an object of
    /// another kind (an interval whose ends are not vertices after merging them).
    pub fn merge_interval_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        index: usize,
        vertices_only: bool,
        other: Option<&CurveClassification<O>>,
    ) -> Result<MergeIntervalOutput<O>> {
        self.merge_interval_in(&mut Mutable { k, g: geometry }, index, vertices_only, other)
    }

    pub(super) fn merge_interval_in(
        &mut self,
        a: &mut impl Imprint<O>,
        index: usize,
        vertices_only: bool,
        other: Option<&CurveClassification<O>>,
    ) -> Result<MergeIntervalOutput<O>> {
        self.merge_interval_using_peer(a, index, vertices_only, FilletPeer::Metadata(other))
    }

    #[allow(clippy::too_many_lines)] // upstream's MergeInterval, in one piece
    pub(super) fn merge_interval_using_peer(
        &mut self,
        a: &mut impl Imprint<O>,
        index: usize,
        vertices_only: bool,
        peer: FilletPeer<'_, O>,
    ) -> Result<MergeIntervalOutput<O>> {
        // init outputs
        let mut out = MergeIntervalOutput::default();

        // locals
        let src_zone_tol_3d = src_zone_tol_3d_of(a.kernel(), Some(&*self));
        let ivl = *self.at(index)?;

        // no work - skip intervals not classified inside
        if ivl.mid.point_class() == Pc::Unknown && !vertices_only {
            return Ok(out);
        }

        // low work - skip intervals already merged
        if ivl.start.point_class() == Pc::Vertex
            && ivl.mid.point_class() == Pc::Edge
            && ivl.end.point_class() == Pc::Vertex
        {
            // When DoingFillets with FilletData: make sure to use the FilletGeometry and
            // not the OriginalBrep Geometry (1 | 2 | 4: start vertex, end vertex and mid
            // edge)
            self.fillet_geometry(a, index, peer, 7)?;
            return Ok(out);
        }

        // Get the interval start and stop points and tols.
        let start_obj_zone_tol_3d = self.point_obj_zone_tol_3d(a.kernel(), a.geometry(), index)?;
        let end_obj_zone_tol_3d =
            self.point_obj_zone_tol_3d(a.kernel(), a.geometry(), index + 1)?;
        // Get SSSPoint when available, else Get SrcCurve(Ivl->EndPtParam)
        let start_pt = self
            .classify_crv_loc_by_index(a.kernel(), a.geometry(), index)?
            .0;
        let end_pt = self
            .classify_crv_loc_by_index(a.kernel(), a.geometry(), index + 1)?
            .0;
        let end_end_dist = start_pt.distance(end_pt);
        let closed_xsect_tol_3d =
            xsect_tol_3d_from_zones(a.kernel(), start_obj_zone_tol_3d, end_obj_zone_tol_3d);

        let mut closed = false;
        if end_end_dist < closed_xsect_tol_3d {
            // find out if interval is closed or degenerate
            closed = !self.is_degenerate_interval(a, index, closed_xsect_tol_3d)?;
        }

        // add end vertices when needed
        if ivl.start.point_class() != Pc::Vertex || ivl.end.point_class() != Pc::Vertex {
            // Update unclassified startPoint of interval using the mid point classification
            if self.at(index)?.start.point_class() == Pc::Unknown {
                let mid = self.at(index)?.mid;
                if mid.point_class() == Pc::Region {
                    // set StartPoint to classify to that region (don't lose SSSPoint data)
                    self.at_mut(index)?.start.copy_classification_only(&mid);
                    // set next interval's mating endPointClassification to be the same
                    let start = self.at(index)?.start;
                    self.set_mate(a, index, &start)?;
                } else if mid.point_class() == Pc::Face {
                    // Check whether StartPoint also classifies to that face
                    let face = face_object(&mid)?;
                    let mut pc = self.at(index)?.start;
                    let r = a.compute_face_point_class(start_pt, src_zone_tol_3d, face, &mut pc);
                    self.at_mut(index)?.start = pc;
                    if face_point_classified(r)? && pc.point_class() == Pc::Face {
                        // set next interval's mating endPointClassification to be the same
                        self.set_mate(a, index, &pc)?;
                    }
                    // else: point failed to project to face when it should have
                }
                // a mid EDGE with an UNKNOWN end is a classify error upstream only
                // warns about; nothing to do for an UNKNOWN interval
            }

            // Update unclassified endPoint of interval using the mid point classification
            if self.at(index)?.end.point_class() == Pc::Unknown {
                let mid = self.at(index)?.mid;
                if mid.point_class() == Pc::Region {
                    // set endPoint to classify to that region (don't lose SSSPoint data)
                    self.at_mut(index)?.end.copy_classification_only(&mid);
                    // set next interval's mating endPointClassification to be the same
                    let end = self.at(index)?.end;
                    self.set_mate(a, index + 1, &end)?;
                } else if mid.point_class() == Pc::Face {
                    // Check whether endPoint also classifies to that face
                    let face = face_object(&mid)?;
                    let mut pc = self.at(index)?.end;
                    let r = a.compute_face_point_class(end_pt, src_zone_tol_3d, face, &mut pc);
                    self.at_mut(index)?.end = pc;
                    if face_point_classified(r)? && pc.point_class() == Pc::Face {
                        // set next interval's mating endPointClassification to be the
                        // same (upstream names the mate of the interval's start here)
                        self.set_mate(a, index, &pc)?;
                    }
                }
            }

            // When StartPt and EndPt are close together -
            //  - handle closed curves so that ends share a vertex (use start vertex)
            //  - handle short edges - add only start vertex to topology graph
            if end_end_dist < closed_xsect_tol_3d {
                // find out if interval is closed or degenerate
                let degenerate = self.is_degenerate_interval(a, index, closed_xsect_tol_3d)?;
                closed = !degenerate;

                // Work from the lowest-dimensional topology: Vertex -> Edge -> Face, by
                // setting both the start and end to the same topology.
                let start_classif = self.point_class_by_index(index)?.0.point_class();
                let end_classif = self.point_class_by_index(index + 1)?.0.point_class();

                // Only do this for closed, degenerate curves.  [Fillet 2:202]
                if degenerate {
                    let start_pc = *self.point_class_by_index(index)?.0;
                    let end_pc = *self.point_class_by_index(index + 1)?.0;
                    if start_classif == Pc::Vertex && end_classif != Pc::Vertex {
                        self.update_point_class(a.kernel(), a.geometry(), index + 1, &start_pc)?;
                    } else if end_classif == Pc::Vertex && start_classif != Pc::Vertex {
                        self.update_point_class(a.kernel(), a.geometry(), index, &end_pc)?;
                    } else if start_classif == Pc::Edge && end_classif != Pc::Edge {
                        self.update_point_class(a.kernel(), a.geometry(), index + 1, &start_pc)?;
                    } else if end_classif == Pc::Edge && start_classif != Pc::Edge {
                        self.update_point_class(a.kernel(), a.geometry(), index, &end_pc)?;
                    }
                }

                // Save the original Edge.
                let orig_start_edge = edge_cast(self.point_class_by_index(index)?.0);

                // for closed or short curves - create just one vertex (the start vertex)
                // and insert it into the topology graph
                self.merge_point_class_in(
                    a,
                    index,
                    Some(&mut out.new_edges),
                    Some(&mut out.new_vertices),
                )?;

                // remember when vertex represents two distinct points within tolerance
                let start_pc = *self.point_class_by_index(index)?.0;
                if start_pc.point_class() == Pc::Vertex {
                    let v = vertex_object(&start_pc)?;
                    let scaled_zero = vertex_point(a.geometry(), v)?.scaled_zero();
                    if end_end_dist > scaled_zero {
                        a.set_vertex_exact(v, false)?;
                    }
                }

                // For degenerate curves whose end is on a different Edge: split the
                // ending Edge (a new Vertex), then combine the two Vertices.
                if degenerate && end_classif == Pc::Edge {
                    let orig_end_edge = edge_cast(self.point_class_by_index(index + 1)?.0);
                    if orig_end_edge.is_some() && orig_end_edge != orig_start_edge {
                        // Don't want this in pOptNewVertices.
                        let mut temp_new_verts = Vec::new();
                        self.merge_point_class_in(
                            a,
                            index + 1,
                            Some(&mut out.new_edges),
                            Some(&mut temp_new_verts),
                        )?;
                        let end_v = vertex_cast(self.point_class_by_index(index + 1)?.0);
                        let start_v = vertex_cast(self.point_class_by_index(index)?.0);
                        if let (Some(start_v), Some(end_v)) = (start_v, end_v) {
                            if end_v != start_v {
                                // SE: the status is ignored
                                ignore_status(a.glue_vertices(start_v, end_v))?;
                                self.point_class_by_index_mut(index + 1)?
                                    .0
                                    .set_class_object(Pc::Vertex, Some(start_v));
                            }
                        }
                    }
                }

                // When DoingFillets with FilletData (1: start vertex)
                self.fillet_geometry(a, index, peer, 1)?;

                // make nextPointClassification = thisPointClassification (cleans up
                // tolerances)
                let start_pc = *self.point_class_by_index(index)?.0;
                self.update_point_class(a.kernel(), a.geometry(), index + 1, &start_pc)?;

                // for degenerate curves - we're done: the curve segment is too small to
                // bother with
                if degenerate {
                    return Ok(out);
                }
            } else {
                // Merge StartPt into Topology graph possibly creating a new Vertex and
                // Splitting an Edge if needed.
                if matches!(
                    self.at(index)?.start.point_class(),
                    Pc::Region | Pc::Face | Pc::Edge
                ) {
                    self.merge_point_class_in(
                        a,
                        index,
                        Some(&mut out.new_edges),
                        Some(&mut out.new_vertices),
                    )?;
                }

                // Merge EndPt into Topology graph
                if matches!(
                    self.at(index)?.end.point_class(),
                    Pc::Region | Pc::Face | Pc::Edge
                ) {
                    self.merge_point_class_in(
                        a,
                        index + 1,
                        Some(&mut out.new_edges),
                        Some(&mut out.new_vertices),
                    )?;
                }

                // When doing a fillet - use FilletGeometry and not OriginalBrep Geometry
                // for tighter tols (1 | 2: start and end vertices)
                self.fillet_geometry(a, index, peer, 3)?;
            }
        }

        // State: we just merged the Start and End, so they should be Vertices.
        // Next:  merge Mid intervals as edges - may split faces

        // low work - asked to insert vertices only
        if vertices_only {
            return Ok(out);
        }

        // Get the vertices from the modified classification
        let ivl = *self.at(index)?;
        let start_v = ivl.start.object();
        let end_v = ivl.end.object();

        if start_v == end_v && !closed {
            // Curve segment is too small to bother with
            let v = vertex_object(&ivl.start)?;
            let scaled_zero = vertex_point(a.geometry(), v)?.scaled_zero();
            if end_end_dist > scaled_zero {
                a.set_vertex_exact(v, false)?;
            }
            return Ok(out);
        }

        // State: either Start and End are distinct and far enough apart, or the curve
        // is closed.  Make an Edge between them.
        let start_v = vertex_object(&ivl.start)?;
        let end_v = vertex_object(&ivl.end)?;

        // when the interval is within a region - make a wire edge
        if ivl.mid.point_class() == Pc::Region {
            let region = ivl.mid.object().ok_or_else(|| null_object("region"))?;
            // copy and trim classification->Curve (may snap the interval by tol to
            // existing knots; the status is ignored)
            let mut curve = self.curve_geometry(a.geometry())?.copy(a.kernel())?;
            let mut interval = ivl.interval;
            ignore_status(curve.trim(a.kernel(), &mut interval, TrimMode::default()))?;
            self.at_mut(index)?.interval = interval;

            // create a new wire edge in region using the new curve
            let new_e = a.make_wire_edge(region, start_v, end_v, curve, &interval)?;

            // set output
            out.new_edges.push(new_e);

            // set the edge's tolerance - never let new tolerances be less than
            // BrepZoneTol3d
            let edge_tol = vertex_brep_tolerance(a.geometry(), start_v)?.max(src_zone_tol_3d);
            a.set_edge_tolerance(new_e, edge_tol)?;

            // update the classification object
            self.at_mut(index)?
                .mid
                .set_class_object(Pc::Edge, Some(new_e));

            // When DoingFillets with FilletData (4: mid edge)
            self.fillet_geometry(a, index, peer, 4)?;
            return Ok(out);
        }

        // when merging an interval that classifies to a face
        if ivl.mid.point_class() == Pc::Face {
            return self.merge_face_interval(a, index, start_v, end_v, peer, out);
        }

        // when merging an interval that classifies to a coincident edge
        if ivl.mid.point_class() == Pc::Edge {
            self.merge_edge_interval(a, index, peer)?;
            return Ok(out);
        }

        // merge error: Interval not merged by any handled case
        Ok(out)
    }

    /// The face branch of `MergeInterval`: a trimmed copy of the interval (and of the
    /// UV curve) becomes an edge between the face's vertex sectors at its ends.
    #[allow(clippy::too_many_lines)] // upstream's branch, in one piece
    fn merge_face_interval(
        &mut self,
        a: &mut impl Imprint<O>,
        index: usize,
        start_v: O,
        end_v: O,
        peer: FilletPeer<'_, O>,
        mut out: MergeIntervalOutput<O>,
    ) -> Result<MergeIntervalOutput<O>> {
        let src_zone_tol_3d = src_zone_tol_3d_of(a.kernel(), Some(&*self));
        let face = face_object(&self.at(index)?.mid)?;

        // Create a trimmed Curve3d copy of the interval (may snap the interval by tol
        // to existing knots; the status is ignored)
        let mut curve_3d = self.curve_geometry(a.geometry())?.copy(a.kernel())?;
        let mut interval = self.at(index)?.interval;
        ignore_status(curve_3d.trim(a.kernel(), &mut interval, TrimMode::default()))?;
        self.at_mut(index)?.interval = interval;

        // when interval has a UVTrimCurve - Create trimmed CurveUV copy of the Interval
        let uv_curve = match self.uv_curve1() {
            Some(h) => {
                let uv = a
                    .geometry()
                    .curve(h)
                    .ok_or_else(|| SolidError::invalid_input(OP, "unresolved UV curve"))?;
                let mut uv = uv.copy(a.kernel())?;
                // SM_CAST_PTR(SmBSplineCurve, ...) then NER
                if !is_bspline_curve(&uv) {
                    return Err(SolidError::null_pointer(OP));
                }
                uv.trim(a.kernel(), &mut interval, TrimMode::default())?;
                self.at_mut(index)?.interval = interval;
                Some(uv)
            }
            None => None,
        };

        // locals
        let face_zone_tol_3d = object_zone_tol_3d(a.kernel(), a.geometry(), Some(face))?;
        let brep_zone_tol_3d = face_brep_tolerance(a.geometry(), face)?;
        let face_src_curve_xsect_tol_3d =
            xsect_tol_3d_from_zones(a.kernel(), face_zone_tol_3d, src_zone_tol_3d);
        let mut with_gap_zone_tol_3d = src_zone_tol_3d;

        // Compute tolerance based on distance to surface of UV curve.
        if let Some(uv) = &uv_curve {
            let surface = face_surface(a.geometry(), face)?;
            let crv_on_surf = Curve::from(CrvOnSurf::new(
                a.kernel(),
                uv.clone_for_evaluation()?,
                surface.clone_for_evaluation()?,
                None,
                0,
            )?);
            let natural = crv_on_surf.natural_interval();

            // get max dist estimate from 3DCurve to projected of UVTrimCurve
            let mut max_dist_to_surf = curve_3d
                .curve_max_distance_between(
                    a.kernel(),
                    &interval,
                    &crv_on_surf,
                    natural.min(),
                    natural.max(),
                    20,
                    None,
                )?
                .distance;

            // if estimate is large - get precise max dist measure
            if max_dist_to_surf > face_zone_tol_3d / 2.0 {
                max_dist_to_surf = curve_3d
                    .curve_max_distance_between(
                        a.kernel(),
                        &interval,
                        &crv_on_surf,
                        natural.min(),
                        natural.max(),
                        0,
                        None,
                    )?
                    .distance;

                // increase the EdgeTol value as needed
                if max_dist_to_surf > face_src_curve_xsect_tol_3d {
                    with_gap_zone_tol_3d = zone_tol3d(
                        a.kernel(),
                        src_zone_tol_3d + (max_dist_to_surf - face_src_curve_xsect_tol_3d),
                    );
                }
            }
        }

        // locals
        let t_start = interval.min();
        let t_end = interval.max();

        // classify the curve against the face at the start and end vertices
        let mut start_vu = a.find_vertex_sector(
            face,
            SectorQuery {
                vertex: start_v,
                curve: &curve_3d,
                uv_curve: uv_curve.as_ref(),
                param_at_vertex: t_start,
                param_ivl_end: t_end,
                orientation: OrientType::Same,
                zone_tol_3d: brep_zone_tol_3d,
            },
        )?;
        let mut end_vu = a.find_vertex_sector(
            face,
            SectorQuery {
                vertex: end_v,
                curve: &curve_3d,
                uv_curve: uv_curve.as_ref(),
                param_at_vertex: t_end,
                param_ivl_end: t_start,
                orientation: OrientType::Opposite,
                zone_tol_3d: brep_zone_tol_3d,
            },
        )?;

        // if either classification failed to find a sector on this face, try to find
        // another face that contains the curve at both the start and end vertices.
        //   note: When previous MergeIvl split Face, subsequent ClassifIvl Face
        //   pointer values may be stale
        if start_vu.is_none() || end_vu.is_none() {
            // get all the faces attached to the start and end vertices through the Brep
            let end_faces = vertex_faces(a.geometry(), end_v)?;
            let start_faces = vertex_faces(a.geometry(), start_v)?;

            // for every start vertex face
            'start: for &start_f in &start_faces {
                // for every end vertex face
                for &end_f in &end_faces {
                    // when the endVertex face and the startVertex face are the same
                    if start_f == end_f {
                        // classify the curve to the face at both endPoints
                        start_vu = a.find_vertex_sector(
                            start_f,
                            SectorQuery {
                                vertex: start_v,
                                curve: &curve_3d,
                                uv_curve: uv_curve.as_ref(),
                                param_at_vertex: t_start,
                                param_ivl_end: t_end,
                                orientation: OrientType::Same,
                                zone_tol_3d: brep_zone_tol_3d,
                            },
                        )?;
                        end_vu = a.find_vertex_sector(
                            start_f,
                            SectorQuery {
                                vertex: end_v,
                                curve: &curve_3d,
                                uv_curve: uv_curve.as_ref(),
                                param_at_vertex: t_end,
                                param_ivl_end: t_start,
                                orientation: OrientType::Opposite,
                                zone_tol_3d: brep_zone_tol_3d,
                            },
                        )?;

                        // when both ends classified to sectors in this face: found a
                        // keeper, exit both face iterations
                        if start_vu.is_some() && end_vu.is_some() {
                            break 'start;
                        }
                    }
                }
            }
            // when the curve does not consistently classify to some face sector:
            // upstream warns (SE_MSG) and presses on
        }

        // check state
        let (Some(start_vu), Some(end_vu)) = (start_vu, end_vu) else {
            return Err(SolidError::null_pointer(OP));
        };

        // insert the interval into the face
        let (new_e, new_f) = a.make_edge_in_face_with_vu(
            face,
            [start_vu, end_vu],
            curve_3d,
            uv_curve,
            interval,
            with_gap_zone_tol_3d,
        )?;

        // update the outputs
        if let Some(f) = new_f {
            out.new_faces.push(f);
        }
        out.new_edges.push(new_e);

        // update the edge tolerance
        a.set_edge_tolerance(new_e, with_gap_zone_tol_3d)?;

        // update the classification object
        self.at_mut(index)?
            .mid
            .set_class_object(Pc::Edge, Some(new_e));

        // When DoingFillets with FilletData (4: mid edge)
        self.fillet_geometry(a, index, peer, 4)?;

        // Update interval Mids that classify to pFace to see if they now classify to
        // pNewF
        if let Some(new_f) = new_f {
            for k in 0..self.size() {
                a.classify_face_interval(self, k, new_f, face)?;
            }
        }

        // update the newEdge's vertex tolerances as needed (FALSE: don't pass along
        // RefreshTolerance() call to connected Edges and Faces)
        let (v1, v2) = edge_vertices(a.geometry(), new_e)?;
        a.refresh_vertex_tolerance(v1)?;
        a.refresh_vertex_tolerance(v2)?;

        Ok(out)
    }

    /// The coincident-edge branch of `MergeInterval`: the edge tolerance grows to the
    /// largest gap between the interval and the edge.
    fn merge_edge_interval(
        &mut self,
        a: &mut impl Imprint<O>,
        index: usize,
        peer: FilletPeer<'_, O>,
    ) -> Result<()> {
        // When DoingFillets with FilletData (4: mid edge)
        self.fillet_geometry(a, index, peer, 4)?;

        // edge locals
        let ivl = *self.at(index)?;
        let src_zone_tol_3d = src_zone_tol_3d_of(a.kernel(), Some(&*self));
        let e = edge_object(&ivl.mid)?;
        let edge_interval = super::p3::edge_interval(a.geometry(), e)?;
        let mut d_start = edge_interval.min();
        let mut d_end = edge_interval.max();
        let edge_zone_tol_3d = object_zone_tol_3d(a.kernel(), a.geometry(), Some(e))?;
        let xsect_tol_3d = xsect_tol_3d_from_zones(a.kernel(), src_zone_tol_3d, edge_zone_tol_3d);

        let new_tolerance = {
            let k = a.kernel();
            let g = a.geometry();
            let curve = super::p3::edge_curve(g, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let this_curve = self.curve_geometry(g)?;

            // Drop classificationInterval MidPoint to Edge->curve
            let pv = this_curve.evaluate(k, ivl.interval.evaluate(0.5), 1, true, true)?;
            let p0 = *pv.first().ok_or_else(|| SolidError::sm_err(OP))?;
            let t0 = *pv.get(1).ok_or_else(|| SolidError::sm_err(OP))?;
            let drop = curve.drop_point(
                k,
                &edge_interval,
                p0,
                None,
                xsect_tol_3d,
                None,
                SolverOperation::Minimize,
            )?;

            if drop.success {
                // we have a candidate - approximate max interval/Edge->Curve distance
                let pv2 = curve.evaluate(k, drop.parameter, 1, true, true)?;
                let t2 = *pv2.get(1).ok_or_else(|| SolidError::sm_err(OP))?;
                if t0.dot(t2) < 0.0 {
                    core::mem::swap(&mut d_start, &mut d_end);
                }
                let mut max_dist_to_edge = this_curve
                    .curve_max_distance_between(k, &ivl.interval, curve, d_start, d_end, 20, None)?
                    .distance;

                // when approxMaxDist is greater than XSectTol3d
                if max_dist_to_edge > xsect_tol_3d {
                    // get precise max interval/Edge->Curve distance
                    max_dist_to_edge = this_curve
                        .curve_max_distance_between(
                            k,
                            &ivl.interval,
                            curve,
                            d_start,
                            d_end,
                            0,
                            None,
                        )?
                        .distance;
                    Some(zone_tol3d(
                        k,
                        src_zone_tol_3d + (max_dist_to_edge - xsect_tol_3d),
                    ))
                } else {
                    None
                }
            } else {
                // merge error: midPoint of merge edge classified to an EDGE failed to
                // drop to that EDGE
                None
            }
        };
        // set the new edge tolerance
        if let Some(tol) = new_tolerance {
            a.set_edge_tolerance(e, tol)?;
        }

        // (SM_USE_OLDTOL) increase coincident edge->Tol to this interval's tolerance
        let src_zone_tol_3d = src_zone_tol_3d_of(a.kernel(), Some(&*self));
        if src_zone_tol_3d > edge_zone_tol_3d {
            a.set_edge_tolerance(e, src_zone_tol_3d)?;
        }

        // update the edge's vertex tolerances as needed (SM_USE_OLDTOL: the edge's
        // vertex; FALSE: don't pass along RefreshTolerance() to connected objects)
        let v1 = edge_vertex(a.geometry(), e)?;
        a.refresh_vertex_tolerance(v1)
    }

    /// `sm_SetDoFilletingGeometry(index, *this, todo)` when doing fillets with fillet
    /// data (its status is ignored).
    fn fillet_geometry(
        &self,
        a: &mut impl Imprint<O>,
        index: usize,
        peer: FilletPeer<'_, O>,
        todo: u64,
    ) -> Result<()> {
        if !self.doing_fillets() {
            return Ok(());
        }
        match peer {
            FilletPeer::Linked(other) => {
                ignore_status(a.set_do_filleting_geometry_linked(self, index, other, todo))
            }
            FilletPeer::Metadata(other) => {
                if self.other_classification().is_none() {
                    return Ok(());
                }
                let Some(other) = other else {
                    return Err(SolidError::unported(
                        "SmCurveClassification::GetOtherClassification",
                        "intersect.curve_class_p7",
                    ));
                };
                ignore_status(a.set_do_filleting_geometry(self, index, Some(other), todo))
            }
        }
    }

    /// `SmTol::IsDegenerate(m_cpCurve, rIvl.m_vInterval, &tol)`.
    fn is_degenerate_interval(&self, a: &impl Imprint<O>, index: usize, tol: f64) -> Result<bool> {
        let curve = match self.curve() {
            Some(c) => Some(
                a.geometry()
                    .curve(c)
                    .ok_or_else(|| SolidError::invalid_input(OP, "unresolved curve handle"))?,
            ),
            None => None,
        };
        curve_is_degenerate(a.kernel(), curve, &self.at(index)?.interval, tol)
    }

    /// `if (pMate) (*pMate) = value` for `GetPointClassMateByIndex(boundary)`.
    fn set_mate(
        &mut self,
        a: &impl Imprint<O>,
        boundary: usize,
        value: &PointClassification<O>,
    ) -> Result<()> {
        if let Some(m) = self.point_class_mate_interval(a.kernel(), a.geometry(), boundary)? {
            self.at_mut(m)?.end = *value;
        }
        Ok(())
    }
}

/// `SmCurveClassification::MergePointClass`'s and `MergeInterval`'s view of the B-rep
/// behind a topology handle: the arena, the entity the handle names in it, the map from
/// its ids to the caller's handles (new vertices, edges and faces included) and the
/// notification state of its edits.
pub struct MergeContext<'a, O> {
    /// The arena holding the entity.
    pub brep: &'a mut Brep,
    /// The entity the handle names.
    pub target: EntityRef,
    /// The caller's handle of an entity of this arena.
    pub handle: Box<dyn Fn(EntityRef) -> O + 'a>,
    /// The notification records, attributes and identities of the arena's edits.
    pub notifications: edge::NotifyEnv<'a>,
    /// The identity of an edge's curve from its id and native type code (a split
    /// edge's new curve included).
    pub curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
}

/// The actual mutable merge view with checked curve-construction identity.
pub struct MergeContextWithIdentity<'a, O> {
    /// Actual arena.
    pub brep: &'a mut Brep,
    /// Target in that arena.
    pub target: EntityRef,
    /// Caller handle for each arena entity.
    pub handle: Box<dyn Fn(EntityRef) -> O + 'a>,
    /// Retained notification records, attributes and context.
    pub notifications: edge::NotifyEnv<'a>,
    /// Authority for the actual original or newly constructed Curve.
    pub curve_identity: crate::topology::brep::p9::CurveIdentity<'a>,
}
impl<'a, O> From<MergeContext<'a, O>> for MergeContextWithIdentity<'a, O> {
    fn from(context: MergeContext<'a, O>) -> Self {
        Self {
            brep: context.brep,
            target: context.target,
            handle: context.handle,
            notifications: context.notifications,
            curve_identity: crate::topology::brep::p9::CurveIdentity::Legacy(context.curve_ref),
        }
    }
}

/// Mutable access to the B-reps of a classification's topology handles, for the
/// edits of [`CurveClassification::merge_interval_mut`] and
/// [`CurveClassification::merge_point_class_mut`] (upstream edits the objects behind
/// its `SmVertex*`, `SmEdge*`, `SmFace*` and `SmRegion*` pointers).
pub trait MergeIntervalBrep<O>: ClassGeometryMut<O> {
    /// The mutable context of topology handle `object` (a vertex, edge, face or
    /// region); `None` for a handle the adapter cannot resolve (an invalid-input error
    /// for the caller).
    fn merge_context(&mut self, object: O) -> Option<MergeContext<'_, O>>;
    /// Actual-curve identity successor. Existing providers keep their old context;
    /// actual issuers override this method without manufacturing a legacy callback.
    fn merge_context_with_curve_identity(
        &mut self,
        object: O,
    ) -> Option<MergeContextWithIdentity<'_, O>> {
        self.merge_context(object).map(Into::into)
    }
}

/// `pc.MergeIntoObject(srcPoint)` on the B-rep a [`MergeIntervalBrep`] hands out for the
/// classification's object ([`PointClassification::merge_into_brep`]); a classification
/// without object or of an object that is not a vertex, edge, face or region returns
/// [`PointClassification::merge_into_object`]'s error, as upstream.
///
/// # Errors
/// Those of [`PointClassification::merge_into_brep`]; [`SolidError::InvalidInput`] for
/// an object the adapter cannot resolve mutably.
pub fn merge_into_object_mut<O: ClassObject>(
    k: &mut Kernel,
    geometry: &mut impl MergeIntervalBrep<O>,
    pc: &mut PointClassification<O>,
    src_point: Point3,
) -> Result<MergeOutputs<O>> {
    Mutable { k, g: geometry }.merge_into_object(pc, src_point)
}

/// A `FindVertexSector` call of `MergeInterval` (its other arguments are constant:
/// `bValidateSector = FALSE`, `bUVSpaceOkay` at its default `TRUE`).
pub(super) struct SectorQuery<'c, O> {
    pub(super) vertex: O,
    pub(super) curve: &'c Curve,
    pub(super) uv_curve: Option<&'c Curve>,
    pub(super) param_at_vertex: f64,
    pub(super) param_ivl_end: f64,
    pub(super) orientation: OrientType,
    pub(super) zone_tol_3d: f64,
}

/// The B-rep edits of `MergeInterval` and `MergePointClass` over either kind of
/// adapter: a read-only one defers each edit, a [`MergeIntervalBrep`] makes it.
pub(super) trait Imprint<O: ClassObject> {
    type Geometry: ClassGeometry<O>;
    fn kernel(&self) -> &Kernel;
    fn geometry(&self) -> &Self::Geometry;
    /// `SmPointClassification::MergeIntoObject(srcPoint, pOptNewEdges, pOptNewVertices)`.
    fn merge_into_object(
        &mut self,
        pc: &mut PointClassification<O>,
        src_point: Point3,
    ) -> Result<MergeOutputs<O>>;
    /// `sm_ComputeFacePointClass(point, srcZoneTol3d, pFace, rPointClass)`.
    fn compute_face_point_class(
        &mut self,
        point: Point3,
        src_zone_tol_3d: f64,
        face: O,
        pc: &mut PointClassification<O>,
    ) -> Result<()>;
    /// `sm_SetDoFilletingGeometry(index, this, todo)`.
    fn set_do_filleting_geometry(
        &mut self,
        this: &CurveClassification<O>,
        index: usize,
        other: Option<&CurveClassification<O>>,
        todo: u64,
    ) -> Result<()>;
    /// Explicitly linked actual local peer, without an opaque classification handle.
    fn set_do_filleting_geometry_linked(
        &mut self,
        _this: &CurveClassification<O>,
        _index: usize,
        _other: &CurveClassification<O>,
        _todo: u64,
    ) -> Result<()> {
        Err(SolidError::unported(
            "sm_SetDoFilletingGeometry (linked peer context)",
            "intersect.curve_class_p7",
        ))
    }
    /// `ClassifyFaceInterval(index, TRUE, pNewF, pFace)` (other arguments at their
    /// defaults).
    fn classify_face_interval(
        &mut self,
        cc: &mut CurveClassification<O>,
        index: usize,
        face: O,
        old_face: O,
    ) -> Result<()>;
    /// `pBrep->GlueVertices(pSurvivor, pToDelete)`.
    fn glue_vertices(&mut self, survivor: O, deleted: O) -> Result<()>;
    /// `pBrep->MakeWireEdge(pRegion, pStartV, pEndV, curve, interval, SM_OT_SAME)`.
    fn make_wire_edge(
        &mut self,
        region: O,
        start: O,
        end: O,
        curve: Curve,
        interval: &Extent1d,
    ) -> Result<O>;
    /// `pFace->FindVertexSector(...)`: the vertex use (of the face's B-rep) whose
    /// sector holds the curve.
    fn find_vertex_sector(
        &mut self,
        face: O,
        query: SectorQuery<'_, O>,
    ) -> Result<Option<VertexuseId>>;
    /// `pBrep->MakeEdgeInFaceWithVU(pFace, vus, curve, uv, interval, SM_OT_SAME, tol)`:
    /// the new edge and the new face of a split.
    fn make_edge_in_face_with_vu(
        &mut self,
        face: O,
        vertexuses: [VertexuseId; 2],
        curve: Curve,
        uv_curve: Option<Curve>,
        interval: Extent1d,
        curve_zone_tol_3d: f64,
    ) -> Result<(O, Option<O>)>;
    /// `pEdge->SetTolerance(tol)` (defaults: only if larger, cascade to boundaries).
    fn set_edge_tolerance(&mut self, edge: O, tolerance: f64) -> Result<()>;
    /// `pVertex->SetExact(exact)`.
    fn set_vertex_exact(&mut self, vertex: O, exact: bool) -> Result<()>;
    /// `pVertex->RefreshTolerance(FALSE)`.
    fn refresh_vertex_tolerance(&mut self, vertex: O) -> Result<()>;
    /// `target.Combine(other)`, whose propagated surf/surf/surf point moves the
    /// vertex of a vertex classification (`SmVertex::SetPoint`).
    fn combine(
        &mut self,
        target: &mut PointClassification<O>,
        other: &PointClassification<O>,
    ) -> Result<Precedence>;
}

/// A read-only adapter: every B-rep edit defers.
pub(super) struct ReadOnly<'a, G> {
    pub(super) k: &'a Kernel,
    pub(super) g: &'a G,
}

/// `Unported` for an edit a read-only adapter cannot make.
fn read_only(upstream: &'static str) -> SolidError {
    // DISPATCH GAP: SmBrep mutable owning context (read-only adapter without a Brep)
    SolidError::unported(upstream, "topology.brep_p1")
}

impl<O: ClassObject, G: ClassGeometry<O>> Imprint<O> for ReadOnly<'_, G> {
    type Geometry = G;
    fn kernel(&self) -> &Kernel {
        self.k
    }
    fn geometry(&self) -> &G {
        self.g
    }
    fn merge_into_object(
        &mut self,
        pc: &mut PointClassification<O>,
        src_point: Point3,
    ) -> Result<MergeOutputs<O>> {
        pc.merge_into_object(self.k, src_point)
    }
    fn compute_face_point_class(
        &mut self,
        point: Point3,
        src_zone_tol_3d: f64,
        face: O,
        pc: &mut PointClassification<O>,
    ) -> Result<()> {
        compute_face_point_class(point, src_zone_tol_3d, face, pc)
    }
    fn set_do_filleting_geometry(
        &mut self,
        this: &CurveClassification<O>,
        index: usize,
        other: Option<&CurveClassification<O>>,
        todo: u64,
    ) -> Result<()> {
        super::p7::set_do_filleting_geometry(self.k, self.g, index, this, other, todo)
    }
    fn set_do_filleting_geometry_linked(
        &mut self,
        this: &CurveClassification<O>,
        index: usize,
        other: &CurveClassification<O>,
        todo: u64,
    ) -> Result<()> {
        super::p7::set_do_filleting_geometry_linked(self.k, self.g, index, this, other, todo)
    }
    fn classify_face_interval(
        &mut self,
        cc: &mut CurveClassification<O>,
        index: usize,
        face: O,
        old_face: O,
    ) -> Result<()> {
        cc.classify_face_interval(
            self.k,
            self.g,
            index,
            true,
            face,
            Some(old_face),
            true,
            None,
        )
    }
    fn glue_vertices(&mut self, _survivor: O, _deleted: O) -> Result<()> {
        Err(read_only(
            "SmBrep mutable owning context (SmBrep::GlueVertices)",
        ))
    }
    fn make_wire_edge(
        &mut self,
        _region: O,
        _start: O,
        _end: O,
        _curve: Curve,
        _interval: &Extent1d,
    ) -> Result<O> {
        Err(read_only(
            "SmBrep mutable owning context (SmBrep::MakeWireEdge)",
        ))
    }
    fn find_vertex_sector(
        &mut self,
        _face: O,
        _query: SectorQuery<'_, O>,
    ) -> Result<Option<VertexuseId>> {
        Err(read_only(
            "SmBrep mutable owning context (SmFace::FindVertexSector)",
        ))
    }
    fn make_edge_in_face_with_vu(
        &mut self,
        _face: O,
        _vertexuses: [VertexuseId; 2],
        _curve: Curve,
        _uv_curve: Option<Curve>,
        _interval: Extent1d,
        _curve_zone_tol_3d: f64,
    ) -> Result<(O, Option<O>)> {
        Err(read_only(
            "SmBrep mutable owning context (SmBrep::MakeEdgeInFaceWithVU)",
        ))
    }
    fn set_edge_tolerance(&mut self, _edge: O, _tolerance: f64) -> Result<()> {
        Err(read_only(
            "SmBrep mutable owning context (SmEdge::SetTolerance)",
        ))
    }
    fn set_vertex_exact(&mut self, _vertex: O, _exact: bool) -> Result<()> {
        Err(read_only(
            "SmBrep mutable owning context (SmVertex::SetExact)",
        ))
    }
    fn refresh_vertex_tolerance(&mut self, _vertex: O) -> Result<()> {
        Err(read_only(
            "SmBrep mutable owning context (SmVertex::RefreshTolerance)",
        ))
    }
    fn combine(
        &mut self,
        target: &mut PointClassification<O>,
        other: &PointClassification<O>,
    ) -> Result<Precedence> {
        target.combine(other)
    }
}

/// A mutable adapter: the B-rep edits are made.
pub(super) struct Mutable<'a, G> {
    pub(super) k: &'a mut Kernel,
    pub(super) g: &'a mut G,
}

fn unresolved_mut() -> SolidError {
    SolidError::invalid_input(OP, "topology handle does not resolve mutably")
}

/// The context of `object`, and the arena entity of each of `members` that belongs to
/// the same B-rep (`None` for one of another B-rep).
fn context_with<'g, O: ClassObject, G: MergeIntervalBrep<O>>(
    g: &'g mut G,
    object: O,
    members: &[O],
) -> Result<(MergeContextWithIdentity<'g, O>, Vec<Option<EntityRef>>)> {
    let mut ids = Vec::new();
    for &m in members {
        let (_, e) = g.topology_entity(m).ok_or_else(unresolved_mut)?;
        ids.push(e);
    }
    let ctx = g
        .merge_context_with_curve_identity(object)
        .ok_or_else(unresolved_mut)?;
    let ids = members
        .iter()
        .zip(ids)
        .map(|(&m, e)| ((ctx.handle)(e) == m).then_some(e))
        .collect();
    Ok((ctx, ids))
}

/// The vertex of a member [`context_with`] resolved in the same B-rep.
fn member_vertex(e: Option<&Option<EntityRef>>) -> Result<VertexId> {
    match e {
        Some(Some(EntityRef::Vertex(v))) => Ok(*v),
        Some(None) => Err(SolidError::invalid_input(
            OP,
            "topology handles of different B-reps",
        )),
        _ => Err(SolidError::invalid_input(
            OP,
            "handle does not name a vertex",
        )),
    }
}

fn wrong_target(kind: &'static str) -> SolidError {
    SolidError::invalid_input(OP, kind)
}

impl<O: ClassObject, G: MergeIntervalBrep<O>> Imprint<O> for Mutable<'_, G> {
    type Geometry = G;
    fn kernel(&self) -> &Kernel {
        self.k
    }
    fn geometry(&self) -> &G {
        self.g
    }
    fn merge_into_object(
        &mut self,
        pc: &mut PointClassification<O>,
        src_point: Point3,
    ) -> Result<MergeOutputs<O>> {
        let topology = pc.object().filter(|o| {
            matches!(
                o.class_kind(),
                ClassObjectKind::Region
                    | ClassObjectKind::Face
                    | ClassObjectKind::Edge
                    | ClassObjectKind::Vertex
            )
        });
        let Some(object) = topology else {
            // no object, or not topology: upstream's error returns, before any edit
            return pc.merge_into_object(self.k, src_point);
        };
        let MergeContextWithIdentity {
            brep,
            target,
            handle,
            mut notifications,
            curve_identity,
        } = self
            .g
            .merge_context_with_curve_identity(object)
            .ok_or_else(unresolved_mut)?;
        pc.merge_into_brep_with_curve_identity(
            self.k,
            src_point,
            BrepMergeWithIdentity {
                brep,
                target,
                handle: &*handle,
                notifications: &mut notifications,
                curve_identity,
            },
        )
    }
    fn compute_face_point_class(
        &mut self,
        point: Point3,
        src_zone_tol_3d: f64,
        face: O,
        pc: &mut PointClassification<O>,
    ) -> Result<()> {
        let g = &*self.g;
        let mut ids = pc.map_objects(&mut |o| {
            g.topology_entity(o)
                .map(|(_, e)| e)
                .ok_or_else(unresolved_mut)
        })?;
        let (ctx, _) = context_with(&mut *self.g, face, &[])?;
        let EntityRef::Face(f) = ctx.target else {
            return Err(wrong_target("handle does not name a face"));
        };
        compute_face_point_class_in(self.k, ctx.brep, f, point, src_zone_tol_3d, &mut ids)?;
        *pc = ids.map_objects(&mut |e| Ok((ctx.handle)(e)))?;
        Ok(())
    }
    fn set_do_filleting_geometry(
        &mut self,
        this: &CurveClassification<O>,
        index: usize,
        other: Option<&CurveClassification<O>>,
        todo: u64,
    ) -> Result<()> {
        super::p7::set_do_filleting_geometry_mut(self.k, self.g, index, this, other, todo)
    }
    fn set_do_filleting_geometry_linked(
        &mut self,
        this: &CurveClassification<O>,
        index: usize,
        other: &CurveClassification<O>,
        todo: u64,
    ) -> Result<()> {
        super::p7::set_do_filleting_geometry_mut_linked(self.k, self.g, index, this, other, todo)
    }
    fn classify_face_interval(
        &mut self,
        cc: &mut CurveClassification<O>,
        index: usize,
        face: O,
        old_face: O,
    ) -> Result<()> {
        cc.classify_face_interval_mut(
            self.k,
            self.g,
            index,
            true,
            face,
            Some(old_face),
            true,
            None,
        )
    }
    fn glue_vertices(&mut self, survivor: O, deleted: O) -> Result<()> {
        // the B-rep of the vertex to delete (pEndPntClass->GetBrep())
        let (ctx, ids) = context_with(&mut *self.g, deleted, &[survivor])?;
        let EntityRef::Vertex(d) = ctx.target else {
            return Err(wrong_target("handle does not name a vertex"));
        };
        let s = member_vertex(ids.first())?;
        crate::topology::brep::p5::glue_vertices(self.k, ctx.brep, Some(s), Some(d), None)
    }
    fn make_wire_edge(
        &mut self,
        region: O,
        start: O,
        end: O,
        curve: Curve,
        interval: &Extent1d,
    ) -> Result<O> {
        let (mut ctx, ids) = context_with(&mut *self.g, region, &[start, end])?;
        let EntityRef::Region(r) = ctx.target else {
            return Err(wrong_target("handle does not name a region"));
        };
        let s = member_vertex(ids.first())?;
        let e = member_vertex(ids.get(1))?;
        let new_e = crate::topology::brep::p3::make_wire_edge(
            self.k,
            ctx.brep,
            &mut ctx.notifications,
            Some(r),
            s,
            e,
            curve,
            interval,
            OrientType::Same,
        )?;
        Ok((ctx.handle)(EntityRef::Edge(new_e)))
    }
    fn find_vertex_sector(
        &mut self,
        face: O,
        query: SectorQuery<'_, O>,
    ) -> Result<Option<VertexuseId>> {
        let (ctx, ids) = context_with(&mut *self.g, face, &[query.vertex])?;
        let EntityRef::Face(f) = ctx.target else {
            return Err(wrong_target("handle does not name a face"));
        };
        let v = match ids.first() {
            // a vertex of another B-rep has no vertex use on this face
            Some(None) => return Ok(None),
            e => member_vertex(e)?,
        };
        let sector = crate::topology::face::p4::find_vertex_sector(
            self.k,
            ctx.brep,
            f,
            FindVertexSectorArgs {
                vertex: v,
                curve: query.curve,
                uv_curve: query.uv_curve,
                param_at_vertex: query.param_at_vertex,
                param_ivl_end: query.param_ivl_end,
                orientation: query.orientation,
                zone_tol_3d: query.zone_tol_3d,
                validate_sector: false,
                uv_space_okay: true,
            },
        )?;
        Ok(sector.vertexuse)
    }
    fn make_edge_in_face_with_vu(
        &mut self,
        face: O,
        vertexuses: [VertexuseId; 2],
        curve: Curve,
        uv_curve: Option<Curve>,
        interval: Extent1d,
        curve_zone_tol_3d: f64,
    ) -> Result<(O, Option<O>)> {
        let (mut ctx, _) = context_with(&mut *self.g, face, &[])?;
        let EntityRef::Face(f) = ctx.target else {
            return Err(wrong_target("handle does not name a face"));
        };
        let [start, end] = vertexuses;
        let made = crate::topology::brep::p9::make_edge_in_face_with_vu_with_curve_identity(
            self.k,
            ctx.brep,
            EdgeInFaceInput {
                face: Some(f),
                vertices: [Some(start), Some(end)],
                curve: Some(curve),
                uv_trim: uv_curve,
                drop_fail: None,
                interval,
                orientation: OrientType::Same,
                curve_zone_tol_3d,
            },
            &mut ctx.notifications,
            ctx.curve_identity,
        )?;
        Ok((
            (ctx.handle)(EntityRef::Edge(made.edge)),
            made.face.map(|f| (ctx.handle)(EntityRef::Face(f))),
        ))
    }
    fn set_edge_tolerance(&mut self, edge: O, tolerance: f64) -> Result<()> {
        let (ctx, _) = context_with(&mut *self.g, edge, &[])?;
        let EntityRef::Edge(e) = ctx.target else {
            return Err(wrong_target("handle does not name an edge"));
        };
        topo_edge::set_tolerance(ctx.brep, e, tolerance, true, true)
    }
    fn set_vertex_exact(&mut self, vertex: O, exact: bool) -> Result<()> {
        let (ctx, _) = context_with(&mut *self.g, vertex, &[])?;
        let EntityRef::Vertex(v) = ctx.target else {
            return Err(wrong_target("handle does not name a vertex"));
        };
        topo_vertex::set_exact(ctx.brep, v, exact)
    }
    fn refresh_vertex_tolerance(&mut self, vertex: O) -> Result<()> {
        let (ctx, _) = context_with(&mut *self.g, vertex, &[])?;
        let EntityRef::Vertex(v) = ctx.target else {
            return Err(wrong_target("handle does not name a vertex"));
        };
        topo_vertex::refresh_tolerance(ctx.brep, v, false)
    }
    fn combine(
        &mut self,
        target: &mut PointClassification<O>,
        other: &PointClassification<O>,
    ) -> Result<Precedence> {
        let g = &mut *self.g;
        target.combine_setting_vertex_point(other, &mut |object, point| {
            // `(SmVertex *)m_pObject`: no vertex is undefined behaviour upstream
            let object = object.ok_or_else(|| null_object("classification has no vertex"))?;
            let ctx = g.merge_context(object).ok_or_else(unresolved_mut)?;
            let EntityRef::Vertex(v) = ctx.target else {
                return Err(wrong_target("handle does not name a vertex"));
            };
            let env = ctx.notifications;
            let key_of = env.key_of;
            topo_vertex::set_point(
                ctx.brep,
                v,
                point,
                env.records,
                env.attributes,
                env.context,
                key_of(EntityRef::Vertex(v)),
                &|vu| key_of(EntityRef::Vertexuse(vu)),
            )
        })
    }
}

// ---- the objects MergeInterval casts its classification objects to ------------------------

fn null_object(what: &'static str) -> SolidError {
    SolidError::invalid_input(OP, what)
}

/// The object of a classification `MergeInterval` casts to `kind` (`(SmFace*)`,
/// `(SmVertex*)`, `(SmEdge*)`): a missing object or one of another kind is undefined
/// behaviour upstream.
fn object_of<O: ClassObject>(
    pc: &PointClassification<O>,
    kind: ClassObjectKind,
    what: &'static str,
) -> Result<O> {
    pc.object()
        .filter(|o| o.class_kind() == kind)
        .ok_or_else(|| null_object(what))
}

fn face_object<O: ClassObject>(pc: &PointClassification<O>) -> Result<O> {
    object_of(
        pc,
        ClassObjectKind::Face,
        "classification object is not a face",
    )
}

fn edge_object<O: ClassObject>(pc: &PointClassification<O>) -> Result<O> {
    object_of(
        pc,
        ClassObjectKind::Edge,
        "classification object is not an edge",
    )
}

fn vertex_object<O: ClassObject>(pc: &PointClassification<O>) -> Result<O> {
    object_of(
        pc,
        ClassObjectKind::Vertex,
        "classification object is not a vertex",
    )
}

/// `SM_CAST_PTR(SmEdge, pc->GetObject())`: `None` unless the object is an edge.
fn edge_cast<O: ClassObject>(pc: &PointClassification<O>) -> Option<O> {
    pc.object()
        .filter(|o| o.class_kind() == ClassObjectKind::Edge)
}

/// `SM_CAST_PTR(SmVertex, pc->GetObject())`: `None` unless the object is a vertex.
fn vertex_cast<O: ClassObject>(pc: &PointClassification<O>) -> Option<O> {
    pc.object()
        .filter(|o| o.class_kind() == ClassObjectKind::Vertex)
}

/// `SM_CAST_PTR(SmBSplineCurve, ...)` of a curve copy (the B-spline curve and the
/// analytic curves derived from it).
fn is_bspline_curve(curve: &Curve) -> bool {
    matches!(
        curve.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

fn entity_of<'g, O: ClassObject>(
    g: &'g impl ClassGeometry<O>,
    o: O,
    upstream: &'static str,
) -> Result<(&'g Brep, EntityRef)> {
    g.topology_entity(o)
        .ok_or_else(|| SolidError::invalid_input(OP, upstream))
}

fn handle_in<O: ClassObject>(g: &impl ClassGeometry<O>, b: &Brep, e: EntityRef) -> Result<O> {
    g.topology_handle_in(b, e)
        .ok_or_else(|| SolidError::invalid_input(OP, "topology entity has no handle"))
}

/// `SmVertex::GetPoint`.
fn vertex_point<O: ClassObject>(g: &impl ClassGeometry<O>, v: O) -> Result<Point3> {
    match entity_of(g, v, "unresolved vertex (SmVertex::GetPoint)")? {
        (b, EntityRef::Vertex(v)) => topo_vertex::point(b, v),
        _ => Err(wrong_target("handle does not name a vertex")),
    }
}

/// `SmTol::GetZoneTol3d(pVertex->GetBrep())` (a null B-rep is dereferenced upstream).
fn vertex_brep_tolerance<O: ClassObject>(g: &impl ClassGeometry<O>, v: O) -> Result<f64> {
    match entity_of(g, v, "unresolved vertex (SmVertex::GetBrep)")? {
        (b, EntityRef::Vertex(v)) => Ok(topo_vertex::brep(b, v)?
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .tolerance()),
        _ => Err(wrong_target("handle does not name a vertex")),
    }
}

/// `SmTol::GetZoneTol3d(pFace->GetBrep())` (a null B-rep is dereferenced upstream).
fn face_brep_tolerance<O: ClassObject>(g: &impl ClassGeometry<O>, f: O) -> Result<f64> {
    match entity_of(g, f, "unresolved face (SmFace::GetBrep)")? {
        (b, EntityRef::Face(f)) => Ok(crate::topology::face::p4::brep(b, f)?
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .tolerance()),
        _ => Err(wrong_target("handle does not name a face")),
    }
}

/// `pFace->GetSurface()` (dereferenced upstream).
fn face_surface<O: ClassObject>(g: &impl ClassGeometry<O>, f: O) -> Result<&Surface> {
    match entity_of(g, f, "unresolved face (SmFace::GetSurface)")? {
        (b, EntityRef::Face(f)) => {
            crate::topology::face::p2::surface(b, f)?.ok_or_else(|| SolidError::null_pointer(OP))
        }
        _ => Err(wrong_target("handle does not name a face")),
    }
}

/// `pVertex->GetFaces(rFaces)`.
fn vertex_faces<O: ClassObject>(g: &impl ClassGeometry<O>, v: O) -> Result<Vec<O>> {
    match entity_of(g, v, "unresolved vertex (SmVertex::GetFaces)")? {
        (b, EntityRef::Vertex(v)) => topo_vertex::faces(b, v)?
            .into_iter()
            .map(|f| handle_in(g, b, EntityRef::Face(f)))
            .collect(),
        _ => Err(wrong_target("handle does not name a vertex")),
    }
}

/// `SM_SUCCESS == sm_ComputeFacePointClass(...)`: a failed projection is a classify
/// error upstream only warns about; unported callees and undefined behaviour
/// (invalid input) still propagate.
fn face_point_classified(r: Result<()>) -> Result<bool> {
    match r {
        Ok(()) => Ok(true),
        Err(e)
            if matches!(
                e,
                SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
            ) =>
        {
            Err(e)
        }
        Err(e) => Ok(ignored(Err::<(), _>(e))?.is_some()),
    }
}

/// `pEdge->GetVertex()` (dereferenced upstream).
fn edge_vertex<O: ClassObject>(g: &impl ClassGeometry<O>, e: O) -> Result<O> {
    match entity_of(g, e, "unresolved edge (SmEdge::GetVertex)")? {
        (b, EntityRef::Edge(e)) => {
            let v = topo_edge::vertex(b, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            handle_in(g, b, EntityRef::Vertex(v))
        }
        _ => Err(wrong_target("handle does not name an edge")),
    }
}

/// `pEdge->GetVertex()` and `pEdge->GetOtherVertex(that)` (both dereferenced
/// upstream).
fn edge_vertices<O: ClassObject>(g: &impl ClassGeometry<O>, e: O) -> Result<(O, O)> {
    match entity_of(g, e, "unresolved edge (SmEdge::GetVertex)")? {
        (b, EntityRef::Edge(e)) => {
            let v1 = topo_edge::vertex(b, e)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let v2 = topo_edge::other_vertex(b, e, Some(v1))?
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            Ok((
                handle_in(g, b, EntityRef::Vertex(v1))?,
                handle_in(g, b, EntityRef::Vertex(v2))?,
            ))
        }
        _ => Err(wrong_target("handle does not name an edge")),
    }
}

fn interval_degenerate<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
    ivl: &CurveInterval<O>,
) -> Result<bool> {
    let curve = cc
        .curve()
        .and_then(|o| geometry.curve(o))
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let surface = cc
        .surface1()
        .map(|o| {
            geometry
                .surface(o)
                .ok_or_else(|| SolidError::invalid_input(OP, "surface handle does not resolve"))
        })
        .transpose()?;
    let tol = super::xsect_tol_2d_or_3d_for_classify_pair(
        k,
        curve,
        surface,
        ivl.min(),
        ivl.start.obj_zone_tol_3d(k, geometry)?,
        ivl.max(),
        ivl.end.obj_zone_tol_3d(k, geometry)?,
    )?;
    curve_is_degenerate(k, Some(curve), &ivl.interval, tol)
}

fn homogenize_boundaries<O: ClassObject>(
    k: &Kernel,
    access: &mut impl PointParams<O>,
    src: &CurveClassification<O>,
    dst: &mut CurveClassification<O>,
) -> Result<()> {
    let count = src.size();
    let mut last = BIG_DOUBLE;
    for index in 0..=count {
        if count == 0 {
            break;
        }
        let loc = point_class_loc_by_index(src, index)?;
        let pc = src.pc_at(loc)?;
        let ivl = src.at(loc.interval)?;
        let param = if loc.end == IntervalPosition::Start {
            ivl.min()
        } else {
            ivl.max()
        };
        let geometry = access.geometry();
        let point = classify_crv_loc_by_index(k, geometry, src, index)?;
        let tol = pc.obj_zone_tol_3d(k, geometry)?;
        let found = dst.find_interval(k, geometry, param, last, tol)?;
        let mut pos = found.position;
        if pos == IntervalPosition::Outside {
            return Err(SolidError::sm_err(OP));
        }
        let mut force = false;
        if pos != IntervalPosition::Inside && found.end_dist_3d.abs() > point.scaled_zero() {
            let ivl = dst.at(found.index)?;
            let end = if pos == IntervalPosition::Start {
                &ivl.start
            } else {
                &ivl.end
            };
            let lower = if index > 0 {
                src.at(index - 1)?.mid.point_class()
            } else {
                Pc::Unknown
            };
            let upper = if index < count {
                src.at(index)?.mid.point_class()
            } else {
                Pc::Unknown
            };
            let up = (pos == IntervalPosition::Start && param < ivl.min())
                || (pos == IntervalPosition::End && param < ivl.max());
            if ivl.mid.point_class() == end.point_class()
                && ((up && upper == Pc::Unknown) || (!up && lower == Pc::Unknown))
            {
                pos = IntervalPosition::Inside;
                force = true;
            }
        }
        if pos == IntervalPosition::Inside {
            let mut new_pc = dst.at(found.index)?.mid;
            new_pc.set_pre_snap_param(param);
            new_pc.copy_sss_point(pc);
            if matches!(new_pc.point_class(), Pc::Face | Pc::Edge) {
                let point = classify_crv_loc_by_index(k, access.geometry(), src, index)?;
                // Ordinary projection failures turn the classification into UNKNOWN.
                match access.compute(k, &mut new_pc, point) {
                    Ok(()) => {}
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => new_pc.set_class_object(Pc::Unknown, None),
                }
            }
            if !dst.insert_point_class(k, access.geometry(), &new_pc, param, last, force)? {
                return Err(SolidError::sm_err(OP));
            }
        }
        last = param;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::Curve;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::geom::extent1d::Extent1d;
    use crate::intersect::point_class::ClassObjectKind;
    use crate::math::Vec3;
    use crate::surface::surface::Surface;
    use crate::surface::volume::Volume;

    #[derive(Clone, Copy, PartialEq)]
    struct H;
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Curve
        }
    }
    struct G(Curve);
    impl ClassGeometry<H> for G {
        fn curve(&self, _: H) -> Option<&Curve> {
            Some(&self.0)
        }
        fn surface(&self, _: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _: H) -> Option<Point3> {
            None
        }
    }
    fn setup() -> (Kernel, G, CurveClassification<H>) {
        let k = Kernel::default();
        let range = Extent1d::from_min_max(0.0, 1.0);
        let g = G(Curve::from(Line::bounded(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            &range,
            &BoundedLineOptions::default(),
        )));
        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(H), false);
        cc.set_size(&k, 1).unwrap();
        cc.at_mut(0).unwrap().set_interval(&range);
        (k, g, cc)
    }
    #[test]
    fn full_interval_combines_all_three_classifications() {
        let (k, g, mut cc) = setup();
        let mut input = CurveInterval::with_interval(&k, &Extent1d::from_min_max(0.0, 1.0), None);
        input.start.set_class_object(Pc::Curve, Some(H));
        input.mid = input.start;
        input.end = input.start;
        input.start.set_t_param(0.125, false);
        input.mid.set_t_param(0.5, false);
        input.end.set_t_param(0.875, false);
        cc.insert_curve_interval(&k, &g, &input).unwrap();
        let got = cc.at(0).unwrap();
        assert_eq!(got.start.t_param(), 0.125);
        assert_eq!(got.mid.t_param(), 0.5);
        assert_eq!(got.end.t_param(), 0.875);
        assert_eq!(cc.size(), 1);
    }
    #[test]
    fn collapsed_interval_combines_start_then_end_and_preserves_sss() {
        let (k, g, mut cc) = setup();
        let mut input = CurveInterval::with_interval(&k, &Extent1d::from_min_max(0.0, 0.0), None);
        input.start.set_class_object(Pc::Curve, Some(H));
        input.start.set_t_param(0.25, false);
        input.end = input.start;
        input.end.set_t_param(0.75, false);
        input.end.set_sss_point(Vec3::new(2.0, 3.0, 4.0));
        cc.insert_curve_interval(&k, &g, &input).unwrap();
        let got = cc.at(0).unwrap();
        assert_eq!(got.start.t_param(), 0.25);
        assert_eq!(got.start.sss_point(), Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(got.mid.point_class(), Pc::Unknown);
    }
    #[test]
    fn missing_curve_and_extreme_indices_return_errors() {
        let (k, g, mut cc) = setup();
        cc.set_curve(None, false);
        let input = CurveInterval::with_interval(&k, &Extent1d::from_min_max(0.25, 0.75), None);
        assert!(cc.insert_curve_interval(&k, &g, &input).is_err());
        assert!(cc.merge_interval(&k, &g, usize::MAX, false).is_err());
        assert!(matches!(
            cc.fix_end_point_class(&k, &g, usize::MAX, Vec3::ZERO),
            Err(SolidError::InvalidInput { .. })
        ));
    }
    #[test]
    fn unclassified_merge_and_empty_homogenization_use_real_api() {
        let (k, g, mut cc) = setup();
        assert!(cc.merge_interval(&k, &g, 0, false).is_ok());
        let mut a = CurveClassification::<H>::new(&k);
        let mut b = CurveClassification::<H>::new(&k);
        assert!(a.homogenize(&k, &g, &mut b).is_ok());
    }
}

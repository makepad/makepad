//! The healer's property records and problem lists, and the target management and the
//! tolerance, coincidence and degeneracy steps of the heal sequence: `SmEdgeProps`,
//! `SmVertexProps`, the `SmProbArray` class family, and the `SmHealData` members owned
//! by unit `heal.heal_data_p2` (with the `SmHealData` record itself, CO-PORTED here).
//!
//! Upstream: `source/SMLib/src/SmEdgeProps.cpp`, `source/SMLib/inc/SmEdgeProps.h`,
//! `source/SMLib/src/SmVertexProps.cpp`, `source/SMLib/inc/SmVertexProps.h`,
//! `source/SMLib/inc/SmHealProbArray.h`, `source/SMLib/src/SmHealData.cpp`,
//! `source/SMLib/inc/SmHealData.h`, `source/SMLib/inc/SmObjProps.h`.
//!
//! | Rust                                              | Upstream                                      |
//! |---------------------------------------------------|-----------------------------------------------|
//! | [`ObjProps`]                                      | `SmObjProps` (`m_bBadBackPointer`)            |
//! | [`EdgeProps`], [`EdgeProps::new`], [`EdgeProps::for_edge`] | `SmEdgeProps`, its constructors, `~SmEdgeProps` (`Drop`), `SetContext` (no counterpart) |
//! | [`EdgeProps::reset`]                              | `SmEdgeProps::ReSet`                          |
//! | [`EdgeProps::is_set_metadata`], [`EdgeProps::is_set_gaps`], [`EdgeProps::is_set`], [`EdgeProps::has_problems`] | `IsSet_Metadata`, `IsSet_Gaps`, `IsSet`, `HasProblems` |
//! | [`EdgeProps::set_props`], [`EdgeProps::set_props_metadata`] | `SmEdgeProps::SetProps`, `SetProps_Metadata` |
//! | [`VertexProps`], [`VertexProps::new`], [`VertexProps::for_vertex`] | `SmVertexProps`, its constructors, `~SmVertexProps` (`Drop`), `SetContext` (no counterpart) |
//! | [`VertexProps::reset`], [`VertexProps::set_props`], [`VertexProps::is_set`], [`VertexProps::has_problems`] | `SmVertexProps::ReSet`, `SetProps`, `IsSet`, `HasProblems` |
//! | [`ProbArray`]                                     | `SmProbArray<TYPE>` (`ReSet`, `Setup`, `IncOrigProbCount`, `Remove`, `RemoveAll`, `GetSize`, `operator[]`, `GetAt`, `Add`, `SetLabels`, `GetOrigProbCount`, `SetOrigProbCnt`) |
//! | [`PairProbArray`]                                 | `SmPairProbArray<TYPE>` (`ReSet`, `Remove`, `SetOrigCoinPairCnt`) |
//! | [`MixedProbArray`]                                | `SmMixedProbArray<TYPE>` (`ReSet`, `Setup`, `Add`) |
//! | [`TriedProbArray`]                                | `SmTriedProbArray<TYPE>` (`Setup`, `Add`)     |
//! | [`FlagLink`], [`MixedLink`], [`TriedLink`], [`EdgeFlag`], [`VertexFlag`], [`FaceFlag`], [`ObjFlag`], [`FaceMixed`], [`FaceTried`] | the pointers to members `ULONG TYPE::*`, `SmMixedLinkType TYPE::*`, `SmTriedLinkType TYPE::*` |
//! | [`HealData`]                                      | `SmHealData` (record CO-PORTED from `heal.heal_data_p1`) |
//! | [`HealData::new`], [`HealData::with_targets`]     | `SmHealData()`, `SmHealData(pBrep, rVertices, rEdges, rFaces)` (CO-PORTED: the state of `ReSet(TRUE)` on a new object) |
//! | [`HealData::load_targets`], [`HealData::free_targets`] | `LoadTargets`, `FreeTargets`             |
//! | [`HealData::setup_empty_prob_arrays`]             | `SetupEmptyProbArrays`                        |
//! | [`HealData::face_props_for_tgts`], [`HealData::edge_props_for_tgts`], [`HealData::vertex_props_for_tgts`] | `GetFaceProps_ForTgts`, `GetEdgeProps_ForTgts`, `GetVertexProps_ForTgts` |
//! | [`HealData::remove_vertex_props_from_lists`], [`HealData::remove_edge_props_from_lists`], [`HealData::remove_face_props_from_lists`] | `RemoveVertexProps_FromLists`, `RemoveEdgeProps_FromLists`, `RemoveFaceProps_FromLists` |
//! | [`HealData::cache_edge_props`], [`HealData::cache_vertex_props`], [`HealData::cache_face_props_gaps`] | `Cache_EdgeProps`, `Cache_VertexProps`, `Cache_FaceProps_Gaps` |
//! | [`HealData::fix_tol_sizes`]                       | `Fix_TolSizes`                                |
//! | [`HealData::cache_coin_vertices`], [`HealData::cache_degen_faces`] | `Cache_CoinVertices`, `Cache_DegenFaces` |
//! | [`HealData::cache_coin_edges`], [`HealData::fix_coin_edges`] | `Cache_CoinEdges`, `Fix_CoinEdges`   |
//! | [`HealData::cache_missed_edge_xsects`], [`HealData::fix_missed_edge_xsects`] | `Cache_MissedEdgeXSects`, `Fix_MissedEdgeXSects` |
//! | [`HealData::fix_uncontained_edges`], [`HealData::fix_bad_gaps`] | `Fix_UncontainedEdges`, `Fix_BadGaps` |
//! | [`edge_len_histogram_index`]                      | `SM_HISTOGRAM_EDGELEN_INDEX`                  |
//!
//! Deviations:
//! - **Object model.** The props objects live in arenas of the [`HealData`]
//!   ([`PropsArena`]) addressed by [`VertexPropsId`], [`EdgePropsId`], [`FacePropsId`]:
//!   the pointer lists (`m_sTgtVertexProps`, the problem arrays, the voxel model) hold
//!   ids, `new` is [`PropsArena::alloc`] and `delete` is [`PropsArena::free`]. An id is
//!   never reused, so pointer identity is kept. A face props record is a
//!   [`FacePropsEntry`] (its `SmObjProps` base and the [`FaceProps`] of
//!   [`crate::heal::face_props`]). The target B-rep (`m_pBrep`) is passed to each step
//!   instead of stored; there is no `SmContext` (the context is `Kernel::context`), so
//!   `SetContext` and the context arguments have no counterpart.
//! - **Gap pointers.** `m_pMaxGap3d_EdgeEdge` / `m_pMaxGap3d_EdgeFace` point at gap
//!   members of edge uses and `m_pMaxGap3d_VtxEdge` / `m_pMaxGap3d_VtxFace` at gap members
//!   of vertex uses: they are the [`EdgeuseId`] / [`VertexuseId`] owning the cached gap,
//!   whose current length is read from the B-rep when needed.
//! - **Problem arrays.** The pointers to members are link enums ([`FlagLink`] and
//!   friends) applied to the record an id names in a [`PropsStore`]; the methods that
//!   read or write a linked member take the store. `SmBooleanUL` members are
//!   [`Tri`]s (a written value above `UNSURE` reads back as `UNSURE`). Labels are
//!   `String`s. `Dump`, `DumpTgts`, the copy / assignment / equality operators and
//!   `MakeCopy` are not ported.
//! - **Statuses.** Upstream never fails these steps except through null or dangling
//!   pointers; where it would dereference null, read an array past its end
//!   (`m_sTgtEdges[ii]` when the target lists have drifted apart, the second member
//!   of an odd trailing coincident pair, a missing linked member) or pass an empty
//!   bucket list to `SmObjsInVoxels::FindCoincidentPairs`, the port returns
//!   [`SolidError::InvalidInput`]. Statuses upstream ignores (`SetProps` in the cache
//!   steps) are ignored; [`SolidError::Unported`] propagates.
//! - **`SM_HISTOGRAM_EDGELEN_INDEX`**: a NaN length (an index past the histogram
//!   upstream) is [`SolidError::InvalidInput`] in [`edge_len_histogram_index`].
//! - **`pbNewProbs` out-params** become the returned `bool`. Upstream
//!   `Cache_CoinVertices` returns early on its no-work path (no target vertices or
//!   no new ones) before writing `*pbNewProbs = FALSE`, leaving the caller's value;
//!   [`HealData::cache_coin_vertices`] returns `false` there. Its only caller,
//!   `HealTgts`, initialises that flag to `FALSE` once per call and runs the step
//!   at most once, so the caller sees the same value.
//! - **Callees.** `SmBrep::SetTolerance` (`topology.brep_p2`), `SmFace::SetTolerance`
//!   and `SmFace::IsDegenerate` (`topology.face_p3`) and the `SmBrep` overloads of
//!   `SmTol::GetModelSizeEstimate`, `SetModelSizeEstimate` and `GetZoneTol3d`
//!   (`SmBrep::GetThisModelSizeEstimate`, `SetThisModelSizeEstimate`, `GetTolerance`;
//!   `topology.brep_p1`) are wired; [`SolidError::Unported`] from their own unported
//!   callees propagates. `Cache_DegenFaces` defers where `IsDegenerate` does
//!   (`SmCurveClassification`).
//! - No recursion; no allocation from caller counts beyond the target lists given.

use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::hash_table::{
    gap_histogram_index, FindPairsOptions, ObjsInVoxels, GAP_HISTOGRAM_BUCKETS,
};
use crate::geom::tol::{
    approx_tol_3d, calc_edge_zone_tol_3d, calc_vertex_zone_tol_3d, context_model_size_estimate,
    context_zone_tol_3d, gap_gain, size_estimate_to_zone_tol_3d, zone_tol3d, SmallTopoType,
    USE_DEFAULT,
};
use crate::heal::face_props::{
    FaceProps, HealerOpType, LoopObject, MixedLinkType, PropStage, Tri, TriedLinkType,
};
use crate::kernel::Kernel;
use crate::math::{max3, max4, BIG_DOUBLE, EFF_ZERO, UNDEF_DOUBLE};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as face_p2;
use crate::topology::{
    edge, vertex, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, LoopId, RegionId, ShellId, VertexId,
    VertexuseId,
};
use crate::EntityRef;

const OP: &str = "heal::heal_data::p2";
const OP_EDGE: &str = "heal::heal_data::p2::EdgeProps::set_props";
const OP_PAIR: &str = "heal::heal_data::p2::PairProbArray::remove";
const OP_STORE: &str = "heal::heal_data::p2::PropsStore";

/// `SM_UNDEF_ULONG`: the value of an unset index.
pub const UNDEF_ULONG: u64 = 123_456_789;

/// `SM_HISTOGRAM_GAPSIZE_BUCKETCNT`.
pub const GAPSIZE_BUCKET_CNT: usize = GAP_HISTOGRAM_BUCKETS;

/// `SM_HISTOGRAM_EDGELEN_BUCKETCNT`.
pub const EDGELEN_BUCKET_CNT: usize = 11;

/// The default voxel counts per axis of the vertex coincidence model (`m_lXCnt`,
/// `m_lYCnt`, `m_lZCnt`).
pub const DEFAULT_VOXEL_CNT: usize = 12;

/// `SmTArray<SmVertex*>::Remove` and friends: removes the first `x` of `v`.
fn remove_first<T: PartialEq>(v: &mut Vec<T>, x: &T) -> bool {
    match v.iter().position(|e| e == x) {
        Some(i) => {
            v.remove(i);
            true
        }
        None => false,
    }
}

/// `SM_HISTOGRAM_EDGELEN_INDEX(a)`: `0` below `1e-5`, `10` above `1e4`, else
/// `(ULONG)(floor(log10(a)) + 6)`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a NaN length (an index past the histogram
/// upstream).
pub fn edge_len_histogram_index(a: f64) -> Result<usize> {
    if a < 1e-5 {
        Ok(0)
    } else if a > 1e4 {
        Ok(10)
    } else {
        let f = a.log10().floor();
        if f.is_nan() {
            return Err(SolidError::invalid_input(
                OP,
                "SM_HISTOGRAM_EDGELEN_INDEX of NaN indexes past the histogram",
            ));
        }
        // (ULONG)(floor(log10(a)) + 6): a value in [1, 10]
        let idx = (f + 6.0) as u64;
        usize::try_from(idx)
            .ok()
            .filter(|&i| i < EDGELEN_BUCKET_CNT)
            .ok_or_else(|| {
                SolidError::invalid_input(OP, "SM_HISTOGRAM_EDGELEN_INDEX past the histogram")
            })
    }
}

/// `(*histogram)[index]++` of an `SmTArray<ULONG>` histogram.
fn bump(h: &mut [u64], index: usize) -> Result<()> {
    let slot = h
        .get_mut(index)
        .ok_or_else(|| SolidError::invalid_input(OP, "histogram index past the end"))?;
    *slot = slot.wrapping_add(1);
    Ok(())
}

/// A tri-state from an `SmBooleanUL` value (`0` FALSE, `1` TRUE, anything else is
/// read as `UNSURE`).
fn tri_of(v: u64) -> Tri {
    match v {
        0 => Tri::False,
        1 => Tri::True,
        _ => Tri::Unsure,
    }
}

/// The `ULONG` value of a tri-state (`code()` as unsigned).
fn ul(t: Tri) -> u64 {
    t.code() as u64
}

// ---- property records -----------------------------------------------------------------

/// `SmObjProps`: the base of every props record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ObjProps {
    /// `m_bBadBackPointer`.
    pub bad_back_pointer: Tri,
}

/// `SmEdgeProps`: the cached values of one target edge.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeProps {
    /// The `SmObjProps` base.
    pub base: ObjProps,
    /// `m_pEdge`.
    pub edge: Option<EdgeId>,
    /// `m_lEdgeIndx`: the edge's index in the managing healer's edge list.
    pub edge_index: u64,
    /// `m_sEdgeInterval`.
    pub edge_interval: Extent1d,
    /// `m_sZoneTol3d`.
    pub zone_tol_3d: f64,
    /// `m_sOrigZoneTol3d`.
    pub orig_zone_tol_3d: f64,
    /// `m_dApproxEdgeLength3d`.
    pub approx_edge_length_3d: f64,
    /// `m_cpCurve != NULL`: the edge's curve has been read.
    pub has_curve: bool,
    /// `m_sNaturalInterval`: the curve's natural interval.
    pub natural_interval: Extent1d,
    /// `m_pMaxGap3d_EdgeEdge`: the edge use whose end/CCW-start gap is the largest.
    pub max_gap_3d_edge_edge: Option<EdgeuseId>,
    /// `m_pMaxGap3d_EdgeFace`: the edge use whose edge/face gap is the largest.
    pub max_gap_3d_edge_face: Option<EdgeuseId>,
    /// `m_bBadSmallZoneTol3d`.
    pub bad_small_zone_tol_3d: Tri,
    /// `m_bBadLargeZoneTol3d`.
    pub bad_large_zone_tol_3d: Tri,
    /// `m_bWarnLargeGap3d`.
    pub warn_large_gap_3d: Tri,
    /// `m_bBadDegenEdge`.
    pub bad_degen_edge: Tri,
    /// `m_bBadUncontainedEdge`.
    pub bad_uncontained_edge: Tri,
    /// `m_bBadCoincidentEdge`.
    pub bad_coincident_edge: Tri,
    /// `m_bBadMissedEdgeXSect`.
    pub bad_missed_edge_xsect: Tri,
}

impl Default for EdgeProps {
    fn default() -> Self {
        Self::new()
    }
}

/// The length of the cached end/CCW-start gap of `eu` (`SmEdgeEdgeGap::GetLength`).
fn ee_gap_length(b: &Brep, eu: EdgeuseId) -> Result<f64> {
    Ok(b.edgeuse(eu)?.edge_ccw_edge_gap.gap_3d)
}

/// The length of the cached edge/face gap of `eu` (`SmEdgeFaceGap::GetLength`).
fn ef_gap_length(b: &Brep, eu: EdgeuseId) -> Result<f64> {
    Ok(b.edgeuse(eu)?.max_edge_face_gap.gap_3d)
}

/// The length of the cached vertex/edge gap of `vu`.
fn ve_gap_length(b: &Brep, vu: VertexuseId) -> Result<f64> {
    Ok(b.vertexuse(vu)?.vertex_edge_gap.gap_3d)
}

/// The length of the cached vertex/face gap of `vu`.
fn vf_gap_length(b: &Brep, vu: VertexuseId) -> Result<f64> {
    Ok(b.vertexuse(vu)?.vertex_face_gap.gap_3d)
}

impl EdgeProps {
    /// `SmEdgeProps(cpContext)`: a record with every value unset (`ReSet()`).
    #[must_use]
    pub fn new() -> Self {
        let mut p = Self {
            base: ObjProps::default(),
            edge: None,
            edge_index: UNDEF_ULONG,
            edge_interval: Extent1d::new(),
            zone_tol_3d: UNDEF_DOUBLE,
            orig_zone_tol_3d: UNDEF_DOUBLE,
            approx_edge_length_3d: UNDEF_DOUBLE,
            has_curve: false,
            natural_interval: Extent1d::new(),
            max_gap_3d_edge_edge: None,
            max_gap_3d_edge_face: None,
            bad_small_zone_tol_3d: Tri::Unsure,
            bad_large_zone_tol_3d: Tri::Unsure,
            warn_large_gap_3d: Tri::Unsure,
            bad_degen_edge: Tri::Unsure,
            bad_uncontained_edge: Tri::Unsure,
            bad_coincident_edge: Tri::Unsure,
            bad_missed_edge_xsect: Tri::Unsure,
        };
        p.reset(true);
        p
    }

    /// `SmEdgeProps(pEdge, lEdgeIndx, cpContext)`: [`EdgeProps::new`] with the edge and
    /// its index set.
    #[must_use]
    pub fn for_edge(edge: EdgeId, edge_index: u64) -> Self {
        let mut p = Self::new();
        p.edge = Some(edge);
        p.edge_index = edge_index;
        p
    }

    /// `ReSet(bResetEdge_Metadata)`: clears the gap values, the coincidence and missed
    /// intersection flags, and with `reset_metadata` (upstream's default) also the
    /// metadata values.
    pub fn reset(&mut self, reset_metadata: bool) {
        // SetProps_MetaData()
        if reset_metadata {
            self.edge = None;
            self.edge_index = UNDEF_ULONG;

            self.edge_interval.init();
            self.zone_tol_3d = UNDEF_DOUBLE;
            self.orig_zone_tol_3d = UNDEF_DOUBLE;
            self.approx_edge_length_3d = UNDEF_DOUBLE;

            self.has_curve = false;
            self.natural_interval.init();

            self.bad_degen_edge = Tri::Unsure;
            self.bad_uncontained_edge = Tri::Unsure;
        }

        // SetProps()
        self.max_gap_3d_edge_edge = None;
        self.max_gap_3d_edge_face = None;
        self.bad_small_zone_tol_3d = Tri::Unsure;
        self.bad_large_zone_tol_3d = Tri::Unsure;
        self.warn_large_gap_3d = Tri::Unsure;

        // Other
        self.bad_coincident_edge = Tri::Unsure;
        self.bad_missed_edge_xsect = Tri::Unsure;
    }

    /// `IsSet_Metadata`: the metadata values are set (the optional index is not
    /// checked).
    #[must_use]
    pub fn is_set_metadata(&self) -> bool {
        let mut is_set = true;
        is_set &= self.edge.is_some();
        is_set &= !self.edge_interval.has_negative_length();

        is_set &= self.zone_tol_3d != UNDEF_DOUBLE;
        is_set &= self.orig_zone_tol_3d != UNDEF_DOUBLE;
        is_set &= self.approx_edge_length_3d != UNDEF_DOUBLE;

        is_set &= self.has_curve;
        is_set &= !self.natural_interval.has_negative_length();

        is_set &= self.bad_degen_edge != Tri::Unsure;
        is_set &= self.bad_uncontained_edge != Tri::Unsure;
        is_set
    }

    /// `IsSet_Gaps`: the gap values are set.
    #[must_use]
    pub fn is_set_gaps(&self) -> bool {
        let mut is_set = true;
        is_set &= self.max_gap_3d_edge_edge.is_some();
        is_set &= self.max_gap_3d_edge_face.is_some();

        is_set &= self.bad_small_zone_tol_3d != Tri::Unsure;
        is_set &= self.bad_large_zone_tol_3d != Tri::Unsure;
        is_set &= self.warn_large_gap_3d != Tri::Unsure;
        is_set
    }

    /// `IsSet`: both [`EdgeProps::is_set_metadata`] and [`EdgeProps::is_set_gaps`].
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.is_set_metadata() && self.is_set_gaps()
    }

    /// `HasProblems`: any problem flag is TRUE.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        let mut bad = false;
        bad |= self.bad_small_zone_tol_3d == Tri::True;
        bad |= self.bad_large_zone_tol_3d == Tri::True;
        bad |= self.warn_large_gap_3d == Tri::True;

        bad |= self.bad_degen_edge == Tri::True;
        bad |= self.bad_uncontained_edge == Tri::True;

        bad |= self.bad_coincident_edge == Tri::True;

        bad |= self.bad_missed_edge_xsect == Tri::True;
        bad
    }

    /// The current length of the largest end/CCW-start gap (`0` without one).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge use.
    pub fn max_edge_edge_gap_length(&self, b: &Brep) -> Result<f64> {
        self.max_gap_3d_edge_edge
            .map_or(Ok(0.0), |eu| ee_gap_length(b, eu))
    }

    /// The current length of the largest edge/face gap (`0` without one).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge use.
    pub fn max_edge_face_gap_length(&self, b: &Brep) -> Result<f64> {
        self.max_gap_3d_edge_face
            .map_or(Ok(0.0), |eu| ef_gap_length(b, eu))
    }

    /// `SetProps`: sets the metadata through [`EdgeProps::set_props_metadata`] when it
    /// is not set (with `label` as the index, as upstream passes `lOptLabel` there),
    /// clears the gap values, collects the largest end/CCW-start and edge/face gaps of
    /// the edge's uses (caching them on the edge uses and accumulating the optional
    /// histograms) and sets the tolerance and gap flags. `edge_index` is unused, as
    /// upstream.
    ///
    /// # Errors
    ///
    /// Those of [`EdgeProps::set_props_metadata`], of the edge use gap queries and
    /// of the histograms.
    #[allow(clippy::too_many_arguments)]
    pub fn set_props(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        edge_id: EdgeId,
        edge_index: u64,
        mut loop_gap_histogram: Option<&mut Vec<u64>>,
        mut edge_face_gap_histogram: Option<&mut Vec<u64>>,
        label: u64,
    ) -> Result<()> {
        let _ = edge_index;

        // Calc and Cache EdgeProps
        if !self.is_set_metadata() {
            self.set_props_metadata(k, b, edge_id, label)?;
        }

        // clear SmEdgeProps_Gap data - preserve the Metadata just computed
        self.reset(false);

        // Get MaxGap3ds from Edge->Edgeuses Gap3d
        let edgeuses = edge::edgeuses(b, edge_id, None, None)?;

        // for every Edge->Edgeuse - Save MaxGap3d seen
        for eu in edgeuses {
            let edge_ccw_edge_gap = eu1::edge_ccw_edge_gap(k, b, eu, false)?;
            let edge_face_gap = eu1::max_edge_face_gap(k, b, eu, false)?;

            // save Max EdgeEndPt/CCWEdgeStartPt Gap
            if let Some(g) = edge_ccw_edge_gap {
                let len = g.gap_3d;
                let replace = match self.max_gap_3d_edge_edge {
                    None => true,
                    Some(m) => ee_gap_length(b, m)? < len,
                };
                if replace {
                    self.max_gap_3d_edge_edge = Some(eu);
                }
                if let Some(h) = loop_gap_histogram.as_deref_mut() {
                    bump(h, gap_histogram_index(len))?;
                }
            }

            // save Max Edge/Face Gap
            if let Some(g) = edge_face_gap {
                let len = g.gap_3d;
                let replace = match self.max_gap_3d_edge_face {
                    None => true,
                    Some(m) => ef_gap_length(b, m)? < len,
                };
                if replace {
                    self.max_gap_3d_edge_face = Some(eu);
                }
                if let Some(h) = edge_face_gap_histogram.as_deref_mut() {
                    bump(h, gap_histogram_index(len))?;
                }
            }
        }

        // locals - Edge Tols and gaps
        let def_zone_tol_3d = context_zone_tol_3d(k, Some(&k.context), SmallTopoType::NormalSize);
        let edge_edge_max_gap_length_3d = self.max_edge_edge_gap_length(b)?;
        let edge_face_max_gap_length_3d = self.max_edge_face_gap_length(b)?;
        // Don't send MaxFaceZoneTol, as those Tols haven't been set yet.
        let calc_zone_tol_3d = calc_edge_zone_tol_3d(
            k,
            edge_edge_max_gap_length_3d,
            edge_face_max_gap_length_3d,
            None,
            Some(&k.context),
        );

        // check - CalcZoneTol3d should = OrigZoneTol3d
        self.bad_small_zone_tol_3d =
            Tri::from(self.orig_zone_tol_3d <= calc_zone_tol_3d - EFF_ZERO);
        self.bad_large_zone_tol_3d =
            Tri::from(self.orig_zone_tol_3d >= calc_zone_tol_3d + EFF_ZERO);

        // remember when geometry gap is bigger than DefZoneTol3d
        self.warn_large_gap_3d = Tri::from(
            edge_edge_max_gap_length_3d >= def_zone_tol_3d
                || edge_face_max_gap_length_3d >= def_zone_tol_3d,
        );
        Ok(())
    }

    /// `SetProps_Metadata`: sets the edge, its index, its tolerances, interval, curve,
    /// approximate length (5 samples), natural interval and the degenerate and
    /// uncontained flags.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge; [`SolidError::InvalidInput`] for
    /// an edge without curve (upstream dereferences null); those of the curve's
    /// `ApproximateLength` and `IsDegenerate`.
    pub fn set_props_metadata(
        &mut self,
        k: &Kernel,
        b: &Brep,
        edge_id: EdgeId,
        edge_index: u64,
    ) -> Result<()> {
        // locals
        let approx_tol_3d = approx_tol_3d(k, SmallTopoType::NormalSize);

        // set property member values
        self.edge = Some(edge_id);
        self.edge_index = edge_index;

        // Tolerance (SmTol::GetZoneTol3d(SmObject*): the edge's GetTolerance)
        self.zone_tol_3d = edge::tolerance(b, edge_id)?;
        if self.orig_zone_tol_3d == UNDEF_DOUBLE {
            self.orig_zone_tol_3d = self.zone_tol_3d;
        }

        // Edge
        self.edge_interval = edge::interval(b, edge_id)?;
        let curve = edge::curve(b, edge_id)?
            .ok_or_else(|| SolidError::invalid_input(OP_EDGE, "edge without curve"))?;
        self.has_curve = true;
        self.approx_edge_length_3d = curve
            .approximate_length(k, &self.edge_interval, 5, false)?
            .0;
        self.natural_interval = curve.natural_interval();

        // Bad Degen Edge
        self.bad_degen_edge =
            Tri::from(curve.is_degenerate(k, approx_tol_3d, Some(&self.edge_interval))?);

        // Bad UnContained Edge
        self.bad_uncontained_edge = Tri::from(
            !self
                .edge_interval
                .is_contained_by(&self.natural_interval, 0.0),
        );
        Ok(())
    }
}

/// `SmVertexProps`: the cached values of one target vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexProps {
    /// The `SmObjProps` base.
    pub base: ObjProps,
    /// `m_pVertex`.
    pub vertex: Option<VertexId>,
    /// `m_lVertexIndx`.
    pub vertex_index: u64,
    /// `m_sZoneTol3d`.
    pub zone_tol_3d: f64,
    /// `m_sOrigZoneTol3d`.
    pub orig_zone_tol_3d: f64,
    /// `m_pMaxGap3d_VtxEdge`: the vertex use whose vertex/edge gap is the largest.
    pub max_gap_3d_vtx_edge: Option<VertexuseId>,
    /// `m_pMaxGap3d_VtxFace`: the vertex use whose vertex/face gap is the largest.
    pub max_gap_3d_vtx_face: Option<VertexuseId>,
    /// `m_bBadSmallZoneTol3d`.
    pub bad_small_zone_tol_3d: Tri,
    /// `m_bBadLargeZoneTol3d`.
    pub bad_large_zone_tol_3d: Tri,
    /// `m_bWarnLargeGap3d`.
    pub warn_large_gap_3d: Tri,
    /// `m_bBadCoincidentVertex`.
    pub bad_coincident_vertex: Tri,
}

impl Default for VertexProps {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexProps {
    /// `SmVertexProps(cpContext)`: a record with every value unset.
    #[must_use]
    pub fn new() -> Self {
        let mut p = Self {
            base: ObjProps::default(),
            vertex: None,
            vertex_index: UNDEF_ULONG,
            zone_tol_3d: UNDEF_DOUBLE,
            orig_zone_tol_3d: UNDEF_DOUBLE,
            max_gap_3d_vtx_edge: None,
            max_gap_3d_vtx_face: None,
            bad_small_zone_tol_3d: Tri::Unsure,
            bad_large_zone_tol_3d: Tri::Unsure,
            warn_large_gap_3d: Tri::Unsure,
            bad_coincident_vertex: Tri::Unsure,
        };
        p.reset();
        p
    }

    /// `SmVertexProps(pVertex, lVertexIndx, cpContext)`.
    #[must_use]
    pub fn for_vertex(vertex: VertexId, vertex_index: u64) -> Self {
        let mut p = Self::new();
        p.vertex = Some(vertex);
        p.vertex_index = vertex_index;
        p
    }

    /// `ReSet`: clears every value.
    pub fn reset(&mut self) {
        self.vertex = None;
        self.vertex_index = UNDEF_ULONG;

        self.zone_tol_3d = UNDEF_DOUBLE;
        self.orig_zone_tol_3d = UNDEF_DOUBLE;
        self.max_gap_3d_vtx_edge = None;
        self.max_gap_3d_vtx_face = None;
        self.bad_small_zone_tol_3d = Tri::Unsure;
        self.bad_large_zone_tol_3d = Tri::Unsure;
        self.warn_large_gap_3d = Tri::Unsure;
        self.bad_coincident_vertex = Tri::Unsure;
    }

    /// The current length of the largest vertex/edge gap (`0` without one).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling vertex use.
    pub fn max_vtx_edge_gap_length(&self, b: &Brep) -> Result<f64> {
        self.max_gap_3d_vtx_edge
            .map_or(Ok(0.0), |vu| ve_gap_length(b, vu))
    }

    /// The current length of the largest vertex/face gap (`0` without one).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling vertex use.
    pub fn max_vtx_face_gap_length(&self, b: &Brep) -> Result<f64> {
        self.max_gap_3d_vtx_face
            .map_or(Ok(0.0), |vu| vf_gap_length(b, vu))
    }

    /// `SetProps`: resets the record, sets the vertex, its index and tolerances,
    /// collects the largest vertex/edge and vertex/face gaps of its uses (caching them
    /// on the vertex uses and accumulating the optional histograms) and sets the
    /// tolerance and gap flags. The coincidence flag is left to
    /// [`HealData::cache_coin_vertices`].
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling vertex; those of the vertex use gap
    /// queries and of the histograms.
    pub fn set_props(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        vertex_id: VertexId,
        vertex_index: u64,
        mut vert_edge_gap_histogram: Option<&mut Vec<u64>>,
        mut vert_face_gap_histogram: Option<&mut Vec<u64>>,
    ) -> Result<()> {
        // init all values except m_cpContext
        self.reset();

        // set property member values
        self.vertex = Some(vertex_id);
        self.vertex_index = vertex_index;
        self.zone_tol_3d = vertex::tolerance(b, vertex_id)?;
        if self.orig_zone_tol_3d == UNDEF_DOUBLE {
            self.orig_zone_tol_3d = self.zone_tol_3d;
        }

        // Get MaxGap3ds from Vertex->Vertexuses Gap3d
        let vertexuses = vertex::vertexuses(b, vertex_id)?;

        // for every Vertex->Vertexuse - Save MaxGap3d seen
        for vu in vertexuses {
            // Vertex/Edge Gap - Loop and Shell Vertices don't have Vertex/Edge gaps
            if let Some(g) = vertexuse::vertex_edge_gap(k, b, vu, false)? {
                let len = g.gap_3d;
                let replace = match self.max_gap_3d_vtx_edge {
                    None => true,
                    Some(m) => ve_gap_length(b, m)? < len,
                };
                if replace {
                    self.max_gap_3d_vtx_edge = Some(vu);
                }
                if let Some(h) = vert_edge_gap_histogram.as_deref_mut() {
                    bump(h, gap_histogram_index(len))?;
                }
            }

            // Vertex/Face Gap - Shell Vertices don't have Vertex/Face gaps
            if let Some(g) = vertexuse::vertex_face_gap(k, b, vu, false)? {
                let len = g.gap_3d;
                let replace = match self.max_gap_3d_vtx_face {
                    None => true,
                    Some(m) => vf_gap_length(b, m)? < len,
                };
                if replace {
                    self.max_gap_3d_vtx_face = Some(vu);
                }
                if let Some(h) = vert_face_gap_histogram.as_deref_mut() {
                    bump(h, gap_histogram_index(len))?;
                }
            }
        }

        // locals for Problem gap checks
        let def_zone_tol_3d = context_zone_tol_3d(k, Some(&k.context), SmallTopoType::NormalSize);
        let vertex_edge_max_gap_length_3d = self.max_vtx_edge_gap_length(b)?;
        let vertex_face_max_gap_length_3d = self.max_vtx_face_gap_length(b)?;
        // EdgeTol not yet set, so unreliable for this test
        let calc_zone_tol_3d = calc_vertex_zone_tol_3d(
            k,
            vertex_edge_max_gap_length_3d,
            vertex_face_max_gap_length_3d,
            None,
            Some(&k.context),
        );

        // ZoneTol3d for current Gap3d sizes
        self.bad_small_zone_tol_3d =
            Tri::from(self.orig_zone_tol_3d <= calc_zone_tol_3d - EFF_ZERO);
        self.bad_large_zone_tol_3d =
            Tri::from(self.orig_zone_tol_3d >= calc_zone_tol_3d + EFF_ZERO);

        // large gaps
        self.warn_large_gap_3d = Tri::from(
            vertex_edge_max_gap_length_3d >= def_zone_tol_3d
                || vertex_face_max_gap_length_3d >= def_zone_tol_3d,
        );
        Ok(())
    }

    /// `IsSet`: every value but the coincidence flag is set.
    #[must_use]
    pub fn is_set(&self) -> bool {
        let mut is_set = true;
        is_set &= self.vertex.is_some();
        is_set &= self.vertex_index != UNDEF_ULONG;
        is_set &= self.zone_tol_3d != UNDEF_DOUBLE;
        is_set &= self.orig_zone_tol_3d != UNDEF_DOUBLE;
        is_set &= self.max_gap_3d_vtx_edge.is_some();
        is_set &= self.max_gap_3d_vtx_face.is_some();
        is_set &= self.bad_small_zone_tol_3d != Tri::Unsure;
        is_set &= self.bad_large_zone_tol_3d != Tri::Unsure;
        is_set &= self.warn_large_gap_3d != Tri::Unsure;
        is_set
    }

    /// `HasProblems`: any problem flag is TRUE.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        self.bad_small_zone_tol_3d == Tri::True
            || self.bad_large_zone_tol_3d == Tri::True
            || self.warn_large_gap_3d == Tri::True
            || self.bad_coincident_vertex == Tri::True
    }
}

/// A face props record: the `SmObjProps` base of an `SmFaceProps` and the record.
#[derive(Debug, Clone)]
pub struct FacePropsEntry<O: LoopObject> {
    /// The `SmObjProps` base.
    pub base: ObjProps,
    /// The face properties.
    pub props: FaceProps<O>,
}

impl<O: LoopObject> FacePropsEntry<O> {
    /// `SmFaceProps(cpContext)`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: ObjProps::default(),
            props: FaceProps::new(),
        }
    }
}

impl<O: LoopObject> Default for FacePropsEntry<O> {
    fn default() -> Self {
        Self::new()
    }
}

// ---- arenas ---------------------------------------------------------------------------

macro_rules! props_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub usize);

        impl $name {
            /// The allocation index.
            #[must_use]
            pub const fn index(self) -> usize {
                self.0
            }
        }
    };
}

props_id!(
    /// A vertex props record of a [`HealData`] (an `SmVertexProps*`).
    VertexPropsId
);
props_id!(
    /// An edge props record of a [`HealData`] (an `SmEdgeProps*`).
    EdgePropsId
);
props_id!(
    /// A face props record of a [`HealData`] (an `SmFaceProps*`).
    FacePropsId
);

/// Records addressed by ids of type `I` (`new` / `delete` of upstream's props
/// objects). Freed slots stay, so an id is never reused.
#[derive(Debug, Clone)]
pub struct PropsArena<I, T> {
    slots: Vec<Option<T>>,
    _id: core::marker::PhantomData<I>,
}

impl<I, T> Default for PropsArena<I, T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            _id: core::marker::PhantomData,
        }
    }
}

/// Conversion between an arena id and its allocation index.
pub trait ArenaId: Copy {
    /// The id of allocation `i`.
    fn from_slot(i: usize) -> Self;
    /// The allocation index.
    fn slot(self) -> usize;
}

macro_rules! arena_id {
    ($name:ident) => {
        impl ArenaId for $name {
            fn from_slot(i: usize) -> Self {
                Self(i)
            }
            fn slot(self) -> usize {
                self.0
            }
        }
    };
}
arena_id!(VertexPropsId);
arena_id!(EdgePropsId);
arena_id!(FacePropsId);

impl<I: ArenaId, T> PropsArena<I, T> {
    /// `new`: stores `t` and returns its id.
    pub fn alloc(&mut self, t: T) -> I {
        self.slots.push(Some(t));
        I::from_slot(self.slots.len() - 1)
    }

    /// `delete`: drops the record; returns whether it was alive.
    pub fn free(&mut self, id: I) -> bool {
        self.slots
            .get_mut(id.slot())
            .and_then(Option::take)
            .is_some()
    }

    /// The record of `id`, if alive.
    #[must_use]
    pub fn get(&self, id: I) -> Option<&T> {
        self.slots.get(id.slot()).and_then(Option::as_ref)
    }

    /// The record of `id`, if alive.
    pub fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.slots.get_mut(id.slot()).and_then(Option::as_mut)
    }

    /// The number of records ever allocated.
    #[must_use]
    pub fn allocated(&self) -> usize {
        self.slots.len()
    }

    /// The alive records in allocation order.
    pub fn alive(&self) -> impl Iterator<Item = (I, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|t| (I::from_slot(i), t)))
    }
}

/// Access to the records the elements of a problem array name (what upstream reaches
/// by dereferencing the element pointer).
pub trait PropsStore<I> {
    /// The record type.
    type Props;
    /// The record of `id`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a freed or unknown id (a dangling pointer
    /// upstream).
    fn props(&self, id: I) -> Result<&Self::Props>;
    /// The record of `id`, mutable.
    ///
    /// # Errors
    ///
    /// As [`PropsStore::props`].
    fn props_mut(&mut self, id: I) -> Result<&mut Self::Props>;
}

fn dangling() -> SolidError {
    SolidError::invalid_input(OP_STORE, "dangling props pointer")
}

impl<I: ArenaId, T> PropsStore<I> for PropsArena<I, T> {
    type Props = T;
    fn props(&self, id: I) -> Result<&T> {
        self.get(id).ok_or_else(dangling)
    }
    fn props_mut(&mut self, id: I) -> Result<&mut T> {
        self.get_mut(id).ok_or_else(dangling)
    }
}

/// Any props record (an `SmObjProps*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropsRef {
    /// A vertex props record.
    Vertex(VertexPropsId),
    /// An edge props record.
    Edge(EdgePropsId),
    /// A face props record.
    Face(FacePropsId),
}

/// The `SmObjProps` bases of the three arenas of a [`HealData`].
#[derive(Debug)]
pub struct ObjPropsStore<'a, O: LoopObject> {
    /// The vertex props.
    pub vertex: &'a mut PropsArena<VertexPropsId, VertexProps>,
    /// The edge props.
    pub edge: &'a mut PropsArena<EdgePropsId, EdgeProps>,
    /// The face props.
    pub face: &'a mut PropsArena<FacePropsId, FacePropsEntry<O>>,
}

impl<O: LoopObject> PropsStore<PropsRef> for ObjPropsStore<'_, O> {
    type Props = ObjProps;
    fn props(&self, id: PropsRef) -> Result<&ObjProps> {
        match id {
            PropsRef::Vertex(v) => Ok(&self.vertex.props(v)?.base),
            PropsRef::Edge(e) => Ok(&self.edge.props(e)?.base),
            PropsRef::Face(f) => Ok(&self.face.props(f)?.base),
        }
    }
    fn props_mut(&mut self, id: PropsRef) -> Result<&mut ObjProps> {
        match id {
            PropsRef::Vertex(v) => Ok(&mut self.vertex.props_mut(v)?.base),
            PropsRef::Edge(e) => Ok(&mut self.edge.props_mut(e)?.base),
            PropsRef::Face(f) => Ok(&mut self.face.props_mut(f)?.base),
        }
    }
}

// ---- links (pointers to members) ------------------------------------------------------

/// A `ULONG TYPE::*` member link of a [`ProbArray`] (an `SmBooleanUL` or `ULONG`
/// member of the records).
pub trait FlagLink: Copy + PartialEq + core::fmt::Debug {
    /// The record type.
    type Props;
    /// `p->*link`.
    fn get(self, p: &Self::Props) -> u64;
    /// `p->*link = value`.
    fn set(self, p: &mut Self::Props, value: u64);
}

/// A `SmMixedLinkType TYPE::*` member link of a [`MixedProbArray`].
pub trait MixedLink: Copy + PartialEq + core::fmt::Debug {
    /// The record type.
    type Props;
    /// `p->*link = value`.
    fn set(self, p: &mut Self::Props, value: MixedLinkType);
}

/// A `SmTriedLinkType TYPE::*` member link of a [`TriedProbArray`].
pub trait TriedLink: Copy + PartialEq + core::fmt::Debug {
    /// The record type.
    type Props;
    /// `p->*link = value`.
    fn set(self, p: &mut Self::Props, value: TriedLinkType);
}

/// The flag members of [`EdgeProps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeFlag {
    /// `m_bBadSmallZoneTol3d`.
    BadSmallZoneTol3d,
    /// `m_bBadLargeZoneTol3d`.
    BadLargeZoneTol3d,
    /// `m_bWarnLargeGap3d`.
    WarnLargeGap3d,
    /// `m_bBadDegenEdge`.
    BadDegenEdge,
    /// `m_bBadUncontainedEdge`.
    BadUncontainedEdge,
    /// `m_bBadCoincidentEdge`.
    BadCoincidentEdge,
    /// `m_bBadMissedEdgeXSect`.
    BadMissedEdgeXSect,
}

impl EdgeFlag {
    fn slot(self, p: &mut EdgeProps) -> &mut Tri {
        match self {
            Self::BadSmallZoneTol3d => &mut p.bad_small_zone_tol_3d,
            Self::BadLargeZoneTol3d => &mut p.bad_large_zone_tol_3d,
            Self::WarnLargeGap3d => &mut p.warn_large_gap_3d,
            Self::BadDegenEdge => &mut p.bad_degen_edge,
            Self::BadUncontainedEdge => &mut p.bad_uncontained_edge,
            Self::BadCoincidentEdge => &mut p.bad_coincident_edge,
            Self::BadMissedEdgeXSect => &mut p.bad_missed_edge_xsect,
        }
    }
}

impl FlagLink for EdgeFlag {
    type Props = EdgeProps;
    fn get(self, p: &EdgeProps) -> u64 {
        ul(match self {
            Self::BadSmallZoneTol3d => p.bad_small_zone_tol_3d,
            Self::BadLargeZoneTol3d => p.bad_large_zone_tol_3d,
            Self::WarnLargeGap3d => p.warn_large_gap_3d,
            Self::BadDegenEdge => p.bad_degen_edge,
            Self::BadUncontainedEdge => p.bad_uncontained_edge,
            Self::BadCoincidentEdge => p.bad_coincident_edge,
            Self::BadMissedEdgeXSect => p.bad_missed_edge_xsect,
        })
    }
    fn set(self, p: &mut EdgeProps, value: u64) {
        *self.slot(p) = tri_of(value);
    }
}

/// The flag members of [`VertexProps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VertexFlag {
    /// `m_bBadSmallZoneTol3d`.
    BadSmallZoneTol3d,
    /// `m_bBadLargeZoneTol3d`.
    BadLargeZoneTol3d,
    /// `m_bWarnLargeGap3d`.
    WarnLargeGap3d,
    /// `m_bBadCoincidentVertex`.
    BadCoincidentVertex,
}

impl VertexFlag {
    fn slot(self, p: &mut VertexProps) -> &mut Tri {
        match self {
            Self::BadSmallZoneTol3d => &mut p.bad_small_zone_tol_3d,
            Self::BadLargeZoneTol3d => &mut p.bad_large_zone_tol_3d,
            Self::WarnLargeGap3d => &mut p.warn_large_gap_3d,
            Self::BadCoincidentVertex => &mut p.bad_coincident_vertex,
        }
    }
}

impl FlagLink for VertexFlag {
    type Props = VertexProps;
    fn get(self, p: &VertexProps) -> u64 {
        ul(match self {
            Self::BadSmallZoneTol3d => p.bad_small_zone_tol_3d,
            Self::BadLargeZoneTol3d => p.bad_large_zone_tol_3d,
            Self::WarnLargeGap3d => p.warn_large_gap_3d,
            Self::BadCoincidentVertex => p.bad_coincident_vertex,
        })
    }
    fn set(self, p: &mut VertexProps, value: u64) {
        *self.slot(p) = tri_of(value);
    }
}

/// The `SmObjProps` flag member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjFlag {
    /// `m_bBadBackPointer`.
    BadBackPointer,
}

impl FlagLink for ObjFlag {
    type Props = ObjProps;
    fn get(self, p: &ObjProps) -> u64 {
        ul(p.bad_back_pointer)
    }
    fn set(self, p: &mut ObjProps, value: u64) {
        p.bad_back_pointer = tri_of(value);
    }
}

/// The `ULONG` / `SmBooleanUL` members of [`FaceProps`] the healer links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceFlag {
    /// `m_bBadHasNotYetProb`.
    BadHasNotYetProb,
    /// `m_bBadSmallZoneTol3d`.
    BadSmallZoneTol3d,
    /// `m_bBadLargeZoneTol3d`.
    BadLargeZoneTol3d,
    /// `m_bBadDegenFace`.
    BadDegenFace,
    /// `m_lBadFlatCorners`.
    BadFlatCorners,
    /// `m_bBadMissingSeam`.
    BadMissingSeam,
    /// `m_bBadCrossedSeam`.
    BadCrossedSeam,
    /// `m_bBadNearMissSeam`.
    BadNearMissSeam,
    /// `m_bBadLoopProblems`.
    BadLoopProblems,
    /// `m_bBadOuterLoopOrder`.
    BadOuterLoopOrder,
    /// `m_lBadOrient_LoopCnt`.
    BadOrientLoopCnt,
    /// `m_lBadNoArea_LoopCnt`.
    BadNoAreaLoopCnt,
    /// `m_lOuterLoopCnt`.
    OuterLoopCnt,
    /// `m_lBadNested_LoopCnt`.
    BadNestedLoopCnt,
    /// `m_bBadUnpairedNoArea_Loop`.
    BadUnpairedNoAreaLoop,
    /// `m_lBadClosed3d_LoopCnt`.
    BadClosed3dLoopCnt,
    /// `m_lBadClosedPtr_LoopCnt`.
    BadClosedPtrLoopCnt,
    /// `m_lBadMissingPoles`.
    BadMissingPoles,
    /// `m_bBadCrossedPole` (listed last so the other links keep their indices).
    BadCrossedPole,
}

impl FaceFlag {
    /// Every link, in declaration order.
    pub const ALL: [Self; 19] = [
        Self::BadHasNotYetProb,
        Self::BadSmallZoneTol3d,
        Self::BadLargeZoneTol3d,
        Self::BadDegenFace,
        Self::BadFlatCorners,
        Self::BadMissingSeam,
        Self::BadCrossedSeam,
        Self::BadNearMissSeam,
        Self::BadLoopProblems,
        Self::BadOuterLoopOrder,
        Self::BadOrientLoopCnt,
        Self::BadNoAreaLoopCnt,
        Self::OuterLoopCnt,
        Self::BadNestedLoopCnt,
        Self::BadUnpairedNoAreaLoop,
        Self::BadClosed3dLoopCnt,
        Self::BadClosedPtrLoopCnt,
        Self::BadMissingPoles,
        Self::BadCrossedPole,
    ];

    /// The `SmBooleanUL` member, `None` for a counter.
    fn tri<O: LoopObject>(self, p: &mut FaceProps<O>) -> Option<&mut Tri> {
        Some(match self {
            Self::BadHasNotYetProb => &mut p.bad_has_not_yet_prob,
            Self::BadSmallZoneTol3d => &mut p.bad_small_zone_tol_3d,
            Self::BadLargeZoneTol3d => &mut p.bad_large_zone_tol_3d,
            Self::BadDegenFace => &mut p.bad_degen_face,
            Self::BadMissingSeam => &mut p.bad_missing_seam,
            Self::BadCrossedPole => &mut p.bad_crossed_pole,
            Self::BadCrossedSeam => &mut p.bad_crossed_seam,
            Self::BadNearMissSeam => &mut p.bad_near_miss_seam,
            Self::BadLoopProblems => &mut p.bad_loop_problems,
            Self::BadOuterLoopOrder => &mut p.bad_outer_loop_order,
            Self::BadUnpairedNoAreaLoop => &mut p.bad_unpaired_no_area_loop,
            _ => return None,
        })
    }

    /// The `ULONG` counter member, `None` for a tri-state.
    fn count<O: LoopObject>(self, p: &mut FaceProps<O>) -> Option<&mut u64> {
        Some(match self {
            Self::BadFlatCorners => &mut p.bad_flat_corners,
            Self::BadOrientLoopCnt => &mut p.bad_orient_loop_cnt,
            Self::BadNoAreaLoopCnt => &mut p.bad_no_area_loop_cnt,
            Self::OuterLoopCnt => &mut p.outer_loop_cnt,
            Self::BadNestedLoopCnt => &mut p.bad_nested_loop_cnt,
            Self::BadClosed3dLoopCnt => &mut p.bad_closed_3d_loop_cnt,
            Self::BadClosedPtrLoopCnt => &mut p.bad_closed_ptr_loop_cnt,
            Self::BadMissingPoles => &mut p.bad_missing_poles,
            _ => return None,
        })
    }

    /// `props->*link` of a face props record.
    #[must_use]
    pub fn value<O: LoopObject>(self, p: &FaceProps<O>) -> u64 {
        match self {
            Self::BadHasNotYetProb => ul(p.bad_has_not_yet_prob),
            Self::BadSmallZoneTol3d => ul(p.bad_small_zone_tol_3d),
            Self::BadLargeZoneTol3d => ul(p.bad_large_zone_tol_3d),
            Self::BadDegenFace => ul(p.bad_degen_face),
            Self::BadFlatCorners => p.bad_flat_corners,
            Self::BadMissingSeam => ul(p.bad_missing_seam),
            Self::BadCrossedPole => ul(p.bad_crossed_pole),
            Self::BadCrossedSeam => ul(p.bad_crossed_seam),
            Self::BadNearMissSeam => ul(p.bad_near_miss_seam),
            Self::BadLoopProblems => ul(p.bad_loop_problems),
            Self::BadOuterLoopOrder => ul(p.bad_outer_loop_order),
            Self::BadOrientLoopCnt => p.bad_orient_loop_cnt,
            Self::BadNoAreaLoopCnt => p.bad_no_area_loop_cnt,
            Self::OuterLoopCnt => p.outer_loop_cnt,
            Self::BadNestedLoopCnt => p.bad_nested_loop_cnt,
            Self::BadUnpairedNoAreaLoop => ul(p.bad_unpaired_no_area_loop),
            Self::BadClosed3dLoopCnt => p.bad_closed_3d_loop_cnt,
            Self::BadClosedPtrLoopCnt => p.bad_closed_ptr_loop_cnt,
            Self::BadMissingPoles => p.bad_missing_poles,
        }
    }

    /// `props->*link = value` of a face props record.
    pub fn assign<O: LoopObject>(self, p: &mut FaceProps<O>, value: u64) {
        if let Some(t) = self.tri(p) {
            *t = tri_of(value);
        } else if let Some(c) = self.count(p) {
            *c = value;
        }
    }
}

impl<O: LoopObject> FlagLink for FaceFlagOf<O> {
    type Props = FacePropsEntry<O>;
    fn get(self, p: &FacePropsEntry<O>) -> u64 {
        self.0.value(&p.props)
    }
    fn set(self, p: &mut FacePropsEntry<O>, value: u64) {
        self.0.assign(&mut p.props, value);
    }
}

/// A [`FaceFlag`] typed for face props records with loop handles `O`.
pub struct FaceFlagOf<O>(pub FaceFlag, core::marker::PhantomData<O>);

impl<O> core::fmt::Debug for FaceFlagOf<O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl<O> FaceFlagOf<O> {
    /// The link `f`.
    #[must_use]
    pub const fn new(f: FaceFlag) -> Self {
        Self(f, core::marker::PhantomData)
    }
}

impl<O> Clone for FaceFlagOf<O> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<O> Copy for FaceFlagOf<O> {}
impl<O> PartialEq for FaceFlagOf<O> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// The `SmMixedLinkType` members of [`FaceProps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceMixed {
    /// `m_eSheet`.
    Sheet,
    /// `m_ePoles`.
    Poles,
}

impl FaceMixed {
    /// `props->*link`.
    #[must_use]
    pub fn value<O: LoopObject>(self, p: &FaceProps<O>) -> MixedLinkType {
        match self {
            Self::Sheet => p.sheet,
            Self::Poles => p.poles_link,
        }
    }
}

/// A [`FaceMixed`] typed for face props records with loop handles `O`.
pub struct FaceMixedOf<O>(pub FaceMixed, core::marker::PhantomData<O>);

impl<O> core::fmt::Debug for FaceMixedOf<O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl<O> FaceMixedOf<O> {
    /// The link `m`.
    #[must_use]
    pub const fn new(m: FaceMixed) -> Self {
        Self(m, core::marker::PhantomData)
    }
}
impl<O> Clone for FaceMixedOf<O> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<O> Copy for FaceMixedOf<O> {}
impl<O> PartialEq for FaceMixedOf<O> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<O: LoopObject> MixedLink for FaceMixedOf<O> {
    type Props = FacePropsEntry<O>;
    fn set(self, p: &mut FacePropsEntry<O>, value: MixedLinkType) {
        match self.0 {
            FaceMixed::Sheet => p.props.sheet = value,
            FaceMixed::Poles => p.props.poles_link = value,
        }
    }
}

/// The `SmTriedLinkType` members of [`FaceProps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceTried {
    /// `m_eBeenThroughMoveSeam`.
    MoveSeam,
    /// `m_eBeenThroughSplitEdgesAtSeam`.
    SplitEdgesAtSeam,
    /// `m_eBeenThroughSplitFacesAtSeam`.
    SplitFacesAtSeam,
}

impl FaceTried {
    /// `props->*link`.
    #[must_use]
    pub fn value<O: LoopObject>(self, p: &FaceProps<O>) -> TriedLinkType {
        match self {
            Self::MoveSeam => p.been_through_move_seam,
            Self::SplitEdgesAtSeam => p.been_through_split_edges_at_seam,
            Self::SplitFacesAtSeam => p.been_through_split_faces_at_seam,
        }
    }
}

/// A [`FaceTried`] typed for face props records with loop handles `O`.
pub struct FaceTriedOf<O>(pub FaceTried, core::marker::PhantomData<O>);

impl<O> core::fmt::Debug for FaceTriedOf<O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl<O> FaceTriedOf<O> {
    /// The link `t`.
    #[must_use]
    pub const fn new(t: FaceTried) -> Self {
        Self(t, core::marker::PhantomData)
    }
}
impl<O> Clone for FaceTriedOf<O> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<O> Copy for FaceTriedOf<O> {}
impl<O> PartialEq for FaceTriedOf<O> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<O: LoopObject> TriedLink for FaceTriedOf<O> {
    type Props = FacePropsEntry<O>;
    fn set(self, p: &mut FacePropsEntry<O>, value: TriedLinkType) {
        match self.0 {
            FaceTried::MoveSeam => p.props.been_through_move_seam = value,
            FaceTried::SplitEdgesAtSeam => p.props.been_through_split_edges_at_seam = value,
            FaceTried::SplitFacesAtSeam => p.props.been_through_split_faces_at_seam = value,
        }
    }
}

// ---- problem arrays -------------------------------------------------------------------

/// `SmProbArray<TYPE>`: a list of props records with a problem, the member flag
/// marking it on each record, the original problem count and report labels.
#[derive(Debug, Clone)]
pub struct ProbArray<I, L> {
    /// `m_sProbArray`.
    pub prob_array: Vec<I>,
    /// `m_plObjProbFlag` (`None`: not set up).
    pub obj_prob_flag: Option<L>,
    /// `m_lOrigProbCount`.
    pub orig_prob_count: u64,
    /// `m_eGetHealerOp`: the step that builds the list.
    pub get_healer_op: HealerOpType,
    /// `m_sOutputLabel`.
    pub output_label: String,
    /// `m_sDataLabel`.
    pub data_label: String,
    /// `m_sProbLabel`.
    pub prob_label: String,
}

impl<I, L> Default for ProbArray<I, L> {
    fn default() -> Self {
        Self::new(HealerOpType::None)
    }
}

impl<I, L> ProbArray<I, L> {
    /// `SmProbArray(eGetHealerOp)`.
    #[must_use]
    pub const fn new(get_healer_op: HealerOpType) -> Self {
        Self {
            prob_array: Vec::new(),
            obj_prob_flag: None,
            orig_prob_count: 0,
            get_healer_op,
            output_label: String::new(),
            data_label: String::new(),
            prob_label: String::new(),
        }
    }

    /// `ReSet`: empties the list (the original count is kept).
    pub fn reset(&mut self) {
        self.prob_array.clear();
    }

    /// `Setup`: the building step and the member flag.
    pub fn setup(&mut self, get_healer_op: HealerOpType, obj_prob_flag: Option<L>) {
        self.get_healer_op = get_healer_op;
        self.obj_prob_flag = obj_prob_flag;
    }

    /// `GetOrigProbCount`.
    #[must_use]
    pub const fn orig_prob_count(&self) -> u64 {
        self.orig_prob_count
    }

    /// `IncOrigProbCount`: the count before incrementing it.
    pub fn inc_orig_prob_count(&mut self) -> u64 {
        let old = self.orig_prob_count;
        self.orig_prob_count = old.wrapping_add(1);
        old
    }

    /// `SetOrigProbCnt`.
    pub fn set_orig_prob_cnt(&mut self, count: u64) {
        self.orig_prob_count = count;
    }

    /// `RemoveAll`.
    pub fn remove_all(&mut self) {
        self.prob_array.clear();
    }

    /// `GetSize`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.prob_array.len()
    }

    /// `GetSize() == 0`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prob_array.is_empty()
    }

    /// `SetLabels`.
    pub fn set_labels(&mut self, output_label: &str, data_label: &str, prob_label: &str) {
        output_label.clone_into(&mut self.output_label);
        data_label.clone_into(&mut self.data_label);
        prob_label.clone_into(&mut self.prob_label);
    }
}

impl<I: Copy + PartialEq, L> ProbArray<I, L> {
    /// `Remove`: removes the first occurrence of `e`; whether it was listed.
    pub fn remove(&mut self, e: I) -> bool {
        remove_first(&mut self.prob_array, &e)
    }

    /// `GetAt` / `operator[]`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<I> {
        self.prob_array.get(index).copied()
    }
}

impl<I: Copy + PartialEq, L: FlagLink> ProbArray<I, L> {
    /// `Add`: appends `e`, sets the original count to the new size and, with
    /// `set_obj_prob_flag` (upstream's default) and a member flag, marks the record
    /// TRUE. Returns the new element's index.
    ///
    /// # Errors
    ///
    /// Those of the store (a dangling element).
    pub fn add<S: PropsStore<I, Props = L::Props>>(
        &mut self,
        store: &mut S,
        e: I,
        set_obj_prob_flag: bool,
    ) -> Result<usize> {
        self.orig_prob_count = self.prob_array.len() as u64 + 1;
        if set_obj_prob_flag {
            if let Some(link) = self.obj_prob_flag {
                link.set(store.props_mut(e)?, 1);
            }
        }
        self.prob_array.push(e);
        Ok(self.prob_array.len() - 1)
    }
}

/// `SmPairProbArray<TYPE>`: a [`ProbArray`] of coincident pairs (the current pairs
/// first, then members whose pair was fixed) with the original pair count.
#[derive(Debug, Clone)]
pub struct PairProbArray<I, L> {
    /// The `SmProbArray` base.
    pub base: ProbArray<I, L>,
    /// `m_lOrigCoinPairCnt`.
    pub orig_coin_pair_cnt: u64,
}

impl<I, L> Default for PairProbArray<I, L> {
    fn default() -> Self {
        Self::new(HealerOpType::None)
    }
}

impl<I, L> PairProbArray<I, L> {
    /// `SmPairProbArray(eGetHealerOp)`.
    #[must_use]
    pub const fn new(get_healer_op: HealerOpType) -> Self {
        Self {
            base: ProbArray::new(get_healer_op),
            orig_coin_pair_cnt: 0,
        }
    }

    /// `ReSet`: empties the list and clears the pair count.
    pub fn reset(&mut self) {
        self.base.reset();
        self.orig_coin_pair_cnt = 0;
    }

    /// `SetOrigCoinPairCnt`.
    pub fn set_orig_coin_pair_cnt(&mut self, count: u64) {
        self.orig_coin_pair_cnt = count;
    }
}

impl<I: Copy + PartialEq, L: FlagLink> PairProbArray<I, L> {
    /// `Remove`: removes every current pair holding `e` (walking the leading pairs whose
    /// two members are both flagged TRUE), then the occurrences of `e` among the
    /// following single members (skipping the element after each removal, as
    /// upstream). Whether anything was removed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream reads a member flag without a
    /// member link or reads past the end of the list (a TRUE last member of an odd
    /// list); those of the store.
    pub fn remove<S: PropsStore<I, Props = L::Props>>(&mut self, store: &S, e: I) -> Result<bool> {
        let flag = |store: &S, x: Option<I>| -> Result<u64> {
            let x = x.ok_or_else(|| SolidError::invalid_input(OP_PAIR, "read past the end"))?;
            let link = self
                .base
                .obj_prob_flag
                .ok_or_else(|| SolidError::invalid_input(OP_PAIR, "no member flag"))?;
            Ok(link.get(store.props(x)?))
        };

        // manage coin pairs
        let mut curr_cnt: usize = 0;
        let mut removed_pair = false;
        let mut rtn = false;
        let mut ii: usize = 0;
        while ii < self.base.prob_array.len() + if removed_pair { 2 } else { 0 } {
            if removed_pair {
                ii -= 2;
                removed_pair = false;
            }
            // when past the list of current problems
            let size = self.base.prob_array.len();
            if ii + 1 > size
                || flag(store, self.base.prob_array.get(ii).copied())? != 1
                || flag(store, self.base.prob_array.get(ii + 1).copied())? != 1
            {
                break;
            }
            curr_cnt += 1;

            // when this coincident pair contains the TgtElement
            if self.base.prob_array.get(ii) == Some(&e)
                || self.base.prob_array.get(ii + 1) == Some(&e)
            {
                self.base.prob_array.drain(ii..ii + 2);
                curr_cnt -= 1;
                removed_pair = true;
                rtn = true;
            }
            ii += 2;
        }

        // manage fixed single entries
        let mut ii = 2 * curr_cnt;
        while ii < self.base.prob_array.len() {
            // when this is the TgtElement
            if self.base.prob_array.get(ii) == Some(&e) {
                self.base.prob_array.remove(ii);
                rtn = true;
            }
            ii += 1;
        }
        Ok(rtn)
    }
}

/// `SmMixedProbArray<TYPE>`: the records with a property that may or may not be a
/// problem, each marked through a `SmMixedLinkType` member.
#[derive(Debug, Clone)]
pub struct MixedProbArray<I, M> {
    /// The `SmProbArray` base (its member flag is unused).
    pub base: ProbArray<I, ObjFlag>,
    /// `m_peObjProp`.
    pub obj_prop: Option<M>,
}

impl<I, M> Default for MixedProbArray<I, M> {
    fn default() -> Self {
        Self::new(HealerOpType::None)
    }
}

impl<I, M> MixedProbArray<I, M> {
    /// `SmMixedProbArray(eGetHealerOp)`.
    #[must_use]
    pub const fn new(get_healer_op: HealerOpType) -> Self {
        Self {
            base: ProbArray::new(get_healer_op),
            obj_prop: None,
        }
    }

    /// `ReSet`: empties the list and clears the original count.
    pub fn reset(&mut self) {
        self.base.reset();
        self.base.orig_prob_count = 0;
    }

    /// `Setup`.
    pub fn setup(&mut self, get_healer_op: HealerOpType, obj_prop: Option<M>) {
        self.base.get_healer_op = get_healer_op;
        self.obj_prop = obj_prop;
    }
}

impl<I: Copy, M: MixedLink> MixedProbArray<I, M> {
    /// `Add`: appends `e` and sets its linked member to `link_type`.
    ///
    /// # Errors
    ///
    /// Those of the store.
    pub fn add<S: PropsStore<I, Props = M::Props>>(
        &mut self,
        store: &mut S,
        e: I,
        link_type: MixedLinkType,
    ) -> Result<usize> {
        self.base.orig_prob_count = self.base.prob_array.len() as u64 + 1;
        if let Some(link) = self.obj_prop {
            link.set(store.props_mut(e)?, link_type);
        }
        self.base.prob_array.push(e);
        Ok(self.base.prob_array.len() - 1)
    }
}

/// `SmTriedProbArray<TYPE>`: the records run through a fix function, each marked
/// through a `SmTriedLinkType` member.
#[derive(Debug, Clone)]
pub struct TriedProbArray<I, T> {
    /// The `SmProbArray` base (its member flag is unused).
    pub base: ProbArray<I, ObjFlag>,
    /// `m_peBeenThroughFix`.
    pub been_through_fix: Option<T>,
}

impl<I, T> Default for TriedProbArray<I, T> {
    fn default() -> Self {
        Self::new(HealerOpType::None)
    }
}

impl<I, T> TriedProbArray<I, T> {
    /// `SmTriedProbArray(eGetHealerOp)`.
    #[must_use]
    pub const fn new(get_healer_op: HealerOpType) -> Self {
        Self {
            base: ProbArray::new(get_healer_op),
            been_through_fix: None,
        }
    }

    /// `ReSet` (the base's: the original count is kept).
    pub fn reset(&mut self) {
        self.base.reset();
    }

    /// `Setup`.
    pub fn setup(&mut self, get_healer_op: HealerOpType, been_through_fix: Option<T>) {
        self.base.get_healer_op = get_healer_op;
        self.been_through_fix = been_through_fix;
    }
}

impl<I: Copy, T: TriedLink> TriedProbArray<I, T> {
    /// `Add`: appends `e` and sets its linked member to `link_type`.
    ///
    /// # Errors
    ///
    /// Those of the store.
    pub fn add<S: PropsStore<I, Props = T::Props>>(
        &mut self,
        store: &mut S,
        e: I,
        link_type: TriedLinkType,
    ) -> Result<usize> {
        self.base.orig_prob_count = self.base.prob_array.len() as u64 + 1;
        if let Some(link) = self.been_through_fix {
            link.set(store.props_mut(e)?, link_type);
        }
        self.base.prob_array.push(e);
        Ok(self.base.prob_array.len() - 1)
    }
}

// ---- SmHealData -------------------------------------------------------------------------

/// A plain problem array of vertex props.
pub type VertexProbArray = ProbArray<VertexPropsId, VertexFlag>;
/// A plain problem array of edge props.
pub type EdgeProbArray = ProbArray<EdgePropsId, EdgeFlag>;
/// A plain problem array of face props.
pub type FaceProbArray<O> = ProbArray<FacePropsId, FaceFlagOf<O>>;
/// A mixed problem array of face props.
pub type FaceMixedProbArray<O> = MixedProbArray<FacePropsId, FaceMixedOf<O>>;
/// A tried problem array of face props.
pub type FaceTriedProbArray<O> = TriedProbArray<FacePropsId, FaceTriedOf<O>>;

/// `SmHealData`: the targets, props records, problem lists and statistics of one heal
/// run over a B-rep. `O` is the loop handle of the face records' seam classifications.
#[derive(Debug, Clone)]
pub struct HealData<O: LoopObject> {
    /// `m_eStopHealOp`.
    pub stop_heal_op: HealerOpType,
    /// `m_eDoneHealOp`.
    pub done_heal_op: HealerOpType,
    /// `m_lRecursionCount`.
    pub recursion_count: u64,

    /// The vertex props records (`new SmVertexProps`).
    pub vertex_props: PropsArena<VertexPropsId, VertexProps>,
    /// The edge props records.
    pub edge_props: PropsArena<EdgePropsId, EdgeProps>,
    /// The face props records.
    pub face_props: PropsArena<FacePropsId, FacePropsEntry<O>>,

    /// `m_sTgtVertices`.
    pub tgt_vertices: Vec<VertexId>,
    /// `m_sTgtEdges`.
    pub tgt_edges: Vec<EdgeId>,
    /// `m_sTgtFaces`.
    pub tgt_faces: Vec<FaceId>,
    /// `m_sTgtVertexProps`.
    pub tgt_vertex_props: Vec<VertexPropsId>,
    /// `m_sTgtEdgeProps`.
    pub tgt_edge_props: Vec<EdgePropsId>,
    /// `m_sTgtFaceProps`.
    pub tgt_face_props: Vec<FacePropsId>,
    /// `m_lOldVertexCnt`: `0`, or the count already processed when re-running on new
    /// topology.
    pub old_vertex_cnt: usize,
    /// `m_lOldEdgeCnt`.
    pub old_edge_cnt: usize,
    /// `m_lOldFaceCnt`.
    pub old_face_cnt: usize,

    /// `m_sNotYetFaceProps`.
    pub not_yet_face_props: FaceProbArray<O>,

    /// `m_sBadObjProps_BackPointers`.
    pub bad_obj_props_back_pointers: ProbArray<PropsRef, ObjFlag>,
    /// `m_sBadEdgeProps_Uncontained`.
    pub bad_edge_props_uncontained: EdgeProbArray,
    /// `m_sBadEdgeProps_SmallZoneTol3d`.
    pub bad_edge_props_small_zone_tol_3d: EdgeProbArray,
    /// `m_sBadEdgeProps_LargeZoneTol3d`.
    pub bad_edge_props_large_zone_tol_3d: EdgeProbArray,
    /// `m_sWarnEdgeProps_LargeGap3d`.
    pub warn_edge_props_large_gap_3d: EdgeProbArray,
    /// `m_sBadEdgeProps_DegenEdges`.
    pub bad_edge_props_degen_edges: EdgeProbArray,
    /// `m_sLoopGap3d_Histogram`.
    pub loop_gap_3d_histogram: Vec<u64>,
    /// `m_sEdgeFaceGap3d_Histogram`.
    pub edge_face_gap_3d_histogram: Vec<u64>,
    /// `m_sEdgeLen3d_Histogram`.
    pub edge_len_3d_histogram: Vec<u64>,
    /// `m_dMaxEdgeLength`.
    pub max_edge_length: f64,
    /// `m_dAvgEdgeLength`.
    pub avg_edge_length: f64,
    /// `m_dNonDegen_MinEdgeLength`.
    pub non_degen_min_edge_length: f64,
    /// `m_lNonDegenEdgeCnt`.
    pub non_degen_edge_cnt: u64,

    /// `m_sBadVertexProps_SmallZoneTol3d`.
    pub bad_vertex_props_small_zone_tol_3d: VertexProbArray,
    /// `m_sBadVertexProps_LargeZoneTol3d`.
    pub bad_vertex_props_large_zone_tol_3d: VertexProbArray,
    /// `m_sWarnVertexProps_LargeGap3d`.
    pub warn_vertex_props_large_gap_3d: VertexProbArray,
    /// `m_sVertEdgeGap3d_Histogram`.
    pub vert_edge_gap_3d_histogram: Vec<u64>,
    /// `m_sVertFaceGap3d_Histogram`.
    pub vert_face_gap_3d_histogram: Vec<u64>,

    /// `m_sBadFaceProps_SmallZoneTol3d`.
    pub bad_face_props_small_zone_tol_3d: FaceProbArray<O>,
    /// `m_sBadFaceProps_LargeZoneTol3d`.
    pub bad_face_props_large_zone_tol_3d: FaceProbArray<O>,

    /// `m_lXCnt`.
    pub x_cnt: usize,
    /// `m_lYCnt`.
    pub y_cnt: usize,
    /// `m_lZCnt`.
    pub z_cnt: usize,
    /// `m_sVertexBBox`.
    pub vertex_bbox: Extent3d,
    /// `m_pVerticesInVoxels`.
    pub vertices_in_voxels: ObjsInVoxels<VertexPropsId>,
    /// `m_sBadVertexProps_CoinVertices`.
    pub bad_vertex_props_coin_vertices: PairProbArray<VertexPropsId, VertexFlag>,
    /// `m_sVertVertGap3d_Histogram`.
    pub vert_vert_gap_3d_histogram: Vec<u64>,

    /// `m_sBadFaceProps_DegenFaces`.
    pub bad_face_props_degen_faces: FaceProbArray<O>,
    /// `m_sBadEdgeProps_CoinEdges`.
    pub bad_edge_props_coin_edges: PairProbArray<EdgePropsId, EdgeFlag>,
    /// `m_sBadEdgeProps_MissedEdgeXSects`.
    pub bad_edge_props_missed_edge_xsects: PairProbArray<EdgePropsId, EdgeFlag>,

    /// `m_sFaceProps_Sheets`.
    pub face_props_sheets: FaceMixedProbArray<O>,
    /// `m_sFaceProps_NoVertexPole`.
    pub face_props_no_vertex_pole: FaceMixedProbArray<O>,
    /// `m_sBadFaceProps_FlatCorner`.
    pub bad_face_props_flat_corner: FaceProbArray<O>,
    /// `m_sBadFaceProps_MissingSeam`.
    pub bad_face_props_missing_seam: FaceProbArray<O>,
    /// `m_sBadFaceProps_CrossedPole`: boundary edges crossing an interior surface pole.
    pub bad_face_props_crossed_pole: FaceProbArray<O>,
    /// `m_sBadFaceProps_CrossedSeam`.
    pub bad_face_props_crossed_seam: FaceProbArray<O>,
    /// `m_sBadFaceProps_NearMissSeam`.
    pub bad_face_props_near_miss_seam: FaceProbArray<O>,
    /// `m_sRanFaceProps_MoveSeam`.
    pub ran_face_props_move_seam: FaceTriedProbArray<O>,
    /// `m_sRanFaceProps_SplitEdgesAtSeam`.
    pub ran_face_props_split_edges_at_seam: FaceTriedProbArray<O>,
    /// `m_lRanRegion_TestClosure_Cnt`.
    pub ran_region_test_closure_cnt: u64,
    /// `m_lFixRegion_SplitRegion_Cnt`.
    pub fix_region_split_region_cnt: u64,

    /// `m_sBadFaceProps_LoopProblems`.
    pub bad_face_props_loop_problems: FaceProbArray<O>,
    /// `m_sBadFaceProps_OuterLoopOrder`.
    pub bad_face_props_outer_loop_order: FaceProbArray<O>,
    /// `m_sBadFaceProps_OrientLoop`.
    pub bad_face_props_orient_loop: FaceProbArray<O>,
    /// `m_sBadFaceProps_NoArea_Loop`.
    pub bad_face_props_no_area_loop: FaceProbArray<O>,
    /// `m_sBadFaceProps_MultiOuterLoops`.
    pub bad_face_props_multi_outer_loops: FaceProbArray<O>,
    /// `m_sBadFaceProps_NestedOuterLoops`.
    pub bad_face_props_nested_outer_loops: FaceProbArray<O>,
    /// `m_sBadFaceProps_UnpairedNoAreaLoop`.
    pub bad_face_props_unpaired_no_area_loop: FaceProbArray<O>,
    /// `m_sBadFaceProps_Closed3dLoop`.
    pub bad_face_props_closed_3d_loop: FaceProbArray<O>,
    /// `m_sBadFaceProps_ClosedPtrLoop`.
    pub bad_face_props_closed_ptr_loop: FaceProbArray<O>,
    /// `m_sBadFaceProps_MissingPole`.
    pub bad_face_props_missing_pole: FaceProbArray<O>,
    /// `m_sRanFaceProps_SplitFaceAtSeam`.
    pub ran_face_props_split_face_at_seam: FaceTriedProbArray<O>,

    /// `m_sNewFaces_FromFixDegenFaces`.
    pub new_faces_from_fix_degen_faces: Vec<FaceId>,
    /// `m_sNewEdges_FromSplitEdgesAtSeams`.
    pub new_edges_from_split_edges_at_seams: Vec<EdgeId>,
    /// `m_sNewVertices_FromSplitEdgesAtSeams`.
    pub new_vertices_from_split_edges_at_seams: Vec<VertexId>,
    /// `m_sNewVerticesOnPoles_FromFixBadLoops`.
    pub new_vertices_on_poles_from_fix_bad_loops: Vec<VertexId>,
    /// `m_sNewLoopsOnPoles_FromFixBadLoops`.
    pub new_loops_on_poles_from_fix_bad_loops: Vec<LoopId>,
    /// `m_sNewFaces_FromExtraLoops`.
    pub new_faces_from_extra_loops: Vec<FaceId>,
    /// `m_sNewFaces_FromSplitFaceAtSeams`.
    pub new_faces_from_split_face_at_seams: Vec<FaceId>,
    /// `m_sNewEdgesOnSeams_FromSplitFaceAtSeams`.
    pub new_edges_on_seams_from_split_face_at_seams: Vec<EdgeId>,
    /// `m_sNewVertices_FromSplitFaceAtSeams`.
    pub new_vertices_from_split_face_at_seams: Vec<VertexId>,
    /// `m_sNewRegions_FromClosedSheets`.
    pub new_regions_from_closed_sheets: Vec<RegionId>,
    /// `m_sNewShells_FromClosedSheets`.
    pub new_shells_from_closed_sheets: Vec<ShellId>,
}

/// `SmZoneTol3d sZoneTol3d = SmTol::GetZoneTol3d(pBrep)` and the other `SmBrep`
/// overloads of `SmTol` (`SmBrep::GetTolerance`).
fn brep_zone_tol_3d(b: &Brep) -> f64 {
    b.tolerance()
}

/// `SmTol::GetModelSizeEstimate(const SmBrep&)` (`SmBrep::GetThisModelSizeEstimate`).
fn brep_model_size_estimate(k: &Kernel, b: &Brep) -> f64 {
    b.this_model_size_estimate(k, true)
}

/// `SmTol::SetModelSizeEstimate(SmBrep&, double)` (`SmBrep::SetThisModelSizeEstimate`;
/// the returned value is not used upstream).
fn set_brep_model_size_estimate(k: &mut Kernel, b: &mut Brep, size: f64) -> f64 {
    b.set_this_model_size_estimate(k, size)
}

/// `SmBrep::SetTolerance(sZoneTol3d)` (`SM_OLDTOL_LINE`, default arguments).
fn set_brep_tolerance(k: &Kernel, b: &mut Brep, zone_tol_3d: f64) -> Result<()> {
    crate::topology::brep::p2::set_tolerance(k, b, zone_tol_3d, true, true)
}

/// `SmFace::SetTolerance(sNewZoneTol3d, bUpdateOnlyIfLarger)` (`bCascadeToBndries`
/// default `TRUE`).
fn set_face_tolerance(
    b: &mut Brep,
    f: FaceId,
    zone_tol_3d: f64,
    only_if_larger: bool,
) -> Result<()> {
    crate::topology::face::p3::set_tolerance(b, f, zone_tol_3d, only_if_larger, true)
}

/// `SmFace::IsDegenerate(bSkipQuickCheck)` (`pOptZoneTol3d` default `NULL`).
fn face_is_degenerate(k: &Kernel, b: &mut Brep, f: FaceId, skip_quick_check: bool) -> Result<bool> {
    crate::topology::face::p3::is_degenerate(k, b, f, skip_quick_check, None)
}

/// `SmTol::CalcEdgeZoneTol3d` / `CalcVertexZoneTol3d` with an `SmBrep`: the context
/// formula over the B-rep's zone tolerance `def`.
fn calc_brep_gap_zone_tol(
    k: &Kernel,
    def: f64,
    gap_1d: f64,
    gap_2d: f64,
    max_up: Option<f64>,
) -> f64 {
    let gain = gap_gain(k);
    let r = match max_up {
        Some(up) => max4(def, up, gain * gap_1d / 2.0, gain * gap_2d - def),
        None => max3(def, gain * gap_1d / 2.0, gain * gap_2d - def),
    };
    zone_tol3d(k, r)
}

/// A fresh gap-size histogram.
fn gap_histogram() -> Vec<u64> {
    vec![0; GAPSIZE_BUCKET_CNT]
}

/// Drops an upstream status the caller ignores; [`SolidError::Unported`],
/// [`SolidError::InvalidInput`] (null pointers upstream dereferences) and
/// [`SolidError::NotFound`] propagate.
fn ignore_status(r: Result<()>) -> Result<()> {
    match r {
        Err(SolidError::Upstream { .. } | SolidError::Nlib { .. }) | Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The element `i` of a target list, or [`SolidError::InvalidInput`] (upstream reads
/// an `SmTArray` past its end).
fn tgt<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP, "target list read past its end"))
}

impl<O: LoopObject> Default for HealData<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: LoopObject> HealData<O> {
    /// `SmHealData()`: an empty healer in the state `ReSet(TRUE)` leaves (CO-PORTED from
    /// `heal.heal_data_p1`): empty targets and problem lists (set up by
    /// [`HealData::setup_empty_prob_arrays`]), zeroed histograms, `12` voxels per axis
    /// and an empty voxel model.
    #[must_use]
    pub fn new() -> Self {
        let mut h = Self {
            stop_heal_op: HealerOpType::None,
            done_heal_op: HealerOpType::None,
            recursion_count: 0,
            vertex_props: PropsArena::default(),
            edge_props: PropsArena::default(),
            face_props: PropsArena::default(),
            tgt_vertices: Vec::new(),
            tgt_edges: Vec::new(),
            tgt_faces: Vec::new(),
            tgt_vertex_props: Vec::new(),
            tgt_edge_props: Vec::new(),
            tgt_face_props: Vec::new(),
            old_vertex_cnt: 0,
            old_edge_cnt: 0,
            old_face_cnt: 0,
            not_yet_face_props: ProbArray::default(),
            bad_obj_props_back_pointers: ProbArray::default(),
            bad_edge_props_uncontained: ProbArray::default(),
            bad_edge_props_small_zone_tol_3d: ProbArray::default(),
            bad_edge_props_large_zone_tol_3d: ProbArray::default(),
            warn_edge_props_large_gap_3d: ProbArray::default(),
            bad_edge_props_degen_edges: ProbArray::default(),
            loop_gap_3d_histogram: gap_histogram(),
            edge_face_gap_3d_histogram: gap_histogram(),
            edge_len_3d_histogram: vec![0; EDGELEN_BUCKET_CNT],
            max_edge_length: 0.0,
            avg_edge_length: 0.0,
            non_degen_min_edge_length: BIG_DOUBLE,
            non_degen_edge_cnt: 0,
            bad_vertex_props_small_zone_tol_3d: ProbArray::default(),
            bad_vertex_props_large_zone_tol_3d: ProbArray::default(),
            warn_vertex_props_large_gap_3d: ProbArray::default(),
            vert_edge_gap_3d_histogram: gap_histogram(),
            vert_face_gap_3d_histogram: gap_histogram(),
            bad_face_props_small_zone_tol_3d: ProbArray::default(),
            bad_face_props_large_zone_tol_3d: ProbArray::default(),
            x_cnt: DEFAULT_VOXEL_CNT,
            y_cnt: DEFAULT_VOXEL_CNT,
            z_cnt: DEFAULT_VOXEL_CNT,
            vertex_bbox: Extent3d::new(),
            vertices_in_voxels: empty_voxels(),
            bad_vertex_props_coin_vertices: PairProbArray::default(),
            vert_vert_gap_3d_histogram: gap_histogram(),
            bad_face_props_degen_faces: ProbArray::default(),
            bad_edge_props_coin_edges: PairProbArray::default(),
            bad_edge_props_missed_edge_xsects: PairProbArray::default(),
            face_props_sheets: MixedProbArray::default(),
            face_props_no_vertex_pole: MixedProbArray::default(),
            bad_face_props_flat_corner: ProbArray::default(),
            bad_face_props_missing_seam: ProbArray::default(),
            bad_face_props_crossed_pole: ProbArray::default(),
            bad_face_props_crossed_seam: ProbArray::default(),
            bad_face_props_near_miss_seam: ProbArray::default(),
            ran_face_props_move_seam: TriedProbArray::default(),
            ran_face_props_split_edges_at_seam: TriedProbArray::default(),
            ran_region_test_closure_cnt: 0,
            fix_region_split_region_cnt: 0,
            bad_face_props_loop_problems: ProbArray::default(),
            bad_face_props_outer_loop_order: ProbArray::default(),
            bad_face_props_orient_loop: ProbArray::default(),
            bad_face_props_no_area_loop: ProbArray::default(),
            bad_face_props_multi_outer_loops: ProbArray::default(),
            bad_face_props_nested_outer_loops: ProbArray::default(),
            bad_face_props_unpaired_no_area_loop: ProbArray::default(),
            bad_face_props_closed_3d_loop: ProbArray::default(),
            bad_face_props_closed_ptr_loop: ProbArray::default(),
            bad_face_props_missing_pole: ProbArray::default(),
            ran_face_props_split_face_at_seam: TriedProbArray::default(),
            new_faces_from_fix_degen_faces: Vec::new(),
            new_edges_from_split_edges_at_seams: Vec::new(),
            new_vertices_from_split_edges_at_seams: Vec::new(),
            new_vertices_on_poles_from_fix_bad_loops: Vec::new(),
            new_loops_on_poles_from_fix_bad_loops: Vec::new(),
            new_faces_from_extra_loops: Vec::new(),
            new_faces_from_split_face_at_seams: Vec::new(),
            new_edges_on_seams_from_split_face_at_seams: Vec::new(),
            new_vertices_from_split_face_at_seams: Vec::new(),
            new_regions_from_closed_sheets: Vec::new(),
            new_shells_from_closed_sheets: Vec::new(),
        };
        // ReSet(TRUE): FreeTargets() (nothing yet) and SetupEmptyProbArrays()
        h.setup_empty_prob_arrays();
        h
    }

    /// `SmHealData(pBrep, rVertices, rEdges, rFaces)`: [`HealData::new`], then
    /// [`HealData::load_targets`].
    #[must_use]
    pub fn with_targets(vertices: &[VertexId], edges: &[EdgeId], faces: &[FaceId]) -> Self {
        let mut h = Self::new();
        h.load_targets(vertices, edges, faces);
        h
    }

    /// The `SmObjProps` view of the three props arenas.
    pub fn obj_props_store(&mut self) -> ObjPropsStore<'_, O> {
        ObjPropsStore {
            vertex: &mut self.vertex_props,
            edge: &mut self.edge_props,
            face: &mut self.face_props,
        }
    }

    /// `LoadTargets`: copies the target lists and gives each target a new, reset props
    /// record. (The B-rep is passed to the steps instead of stored.)
    pub fn load_targets(&mut self, vertices: &[VertexId], edges: &[EdgeId], faces: &[FaceId]) {
        // Copy target arrays
        self.tgt_vertices = vertices.to_vec();
        self.tgt_edges = edges.to_vec();
        self.tgt_faces = faces.to_vec();

        // Size PropsArrays - allocate ObjProps array members (ReSet by the
        // constructors, as the explicit ReSet calls after them)
        self.tgt_vertex_props = vertices
            .iter()
            .map(|_| self.vertex_props.alloc(VertexProps::new()))
            .collect();
        self.tgt_edge_props = edges
            .iter()
            .map(|_| self.edge_props.alloc(EdgeProps::new()))
            .collect();
        self.tgt_face_props = faces
            .iter()
            .map(|_| self.face_props.alloc(FacePropsEntry::new()))
            .collect();

        // Init PropArray elems
        for &id in &self.tgt_vertex_props {
            if let Some(p) = self.vertex_props.get_mut(id) {
                p.reset();
            }
        }
        for &id in &self.tgt_edge_props {
            if let Some(p) = self.edge_props.get_mut(id) {
                p.reset(true);
            }
        }
        for &id in &self.tgt_face_props {
            if let Some(p) = self.face_props.get_mut(id) {
                p.props.reset(PropStage::Stage0);
            }
        }
    }

    /// `FreeTargets`: frees the target props records and those of the not-yet list,
    /// and empties those four lists (the target topology lists are kept).
    pub fn free_targets(&mut self) {
        for &id in &self.tgt_vertex_props {
            self.vertex_props.free(id);
        }
        for &id in &self.tgt_edge_props {
            self.edge_props.free(id);
        }
        for &id in &self.tgt_face_props {
            self.face_props.free(id);
        }

        // m_sNotYetFaceProps.m_sProbArray[ii] pts point to objs that are owned here
        for &id in &self.not_yet_face_props.prob_array {
            self.face_props.free(id);
        }

        // Clear the array memory
        self.tgt_vertex_props.clear();
        self.tgt_edge_props.clear();
        self.tgt_face_props.clear();
        self.not_yet_face_props.remove_all();
    }

    /// `SetupEmptyProbArrays`: sets each problem list's building step and member link,
    /// its labels, and empties it.
    #[allow(clippy::too_many_lines)]
    pub fn setup_empty_prob_arrays(&mut self) {
        use FaceFlag as F;
        use HealerOpType as H;
        let ff = |f: FaceFlag| Some(FaceFlagOf::<O>::new(f));

        // Section 1 Scope: Set ProbArray Links
        self.not_yet_face_props
            .setup(H::None, ff(F::BadHasNotYetProb));

        self.bad_obj_props_back_pointers
            .setup(H::FixBackpointers, Some(ObjFlag::BadBackPointer));
        self.bad_edge_props_uncontained
            .setup(H::CacheEdgeprops, Some(EdgeFlag::BadUncontainedEdge));
        self.bad_edge_props_small_zone_tol_3d
            .setup(H::CacheEdgeprops, Some(EdgeFlag::BadSmallZoneTol3d));
        self.bad_edge_props_large_zone_tol_3d
            .setup(H::CacheEdgeprops, Some(EdgeFlag::BadLargeZoneTol3d));
        self.warn_edge_props_large_gap_3d
            .setup(H::CacheEdgeprops, Some(EdgeFlag::WarnLargeGap3d));
        self.bad_edge_props_degen_edges
            .setup(H::CacheEdgeprops, Some(EdgeFlag::BadDegenEdge));

        self.bad_vertex_props_small_zone_tol_3d
            .setup(H::CacheVertexprops, Some(VertexFlag::BadSmallZoneTol3d));
        self.bad_vertex_props_large_zone_tol_3d
            .setup(H::CacheVertexprops, Some(VertexFlag::BadLargeZoneTol3d));
        self.warn_vertex_props_large_gap_3d
            .setup(H::CacheVertexprops, Some(VertexFlag::WarnLargeGap3d));

        self.bad_face_props_small_zone_tol_3d
            .setup(H::CacheFacepropsGaps, ff(F::BadSmallZoneTol3d));
        self.bad_face_props_large_zone_tol_3d
            .setup(H::CacheFacepropsGaps, ff(F::BadLargeZoneTol3d));

        self.bad_vertex_props_coin_vertices
            .base
            .setup(H::CacheCoinVertices, Some(VertexFlag::BadCoincidentVertex));

        self.bad_face_props_degen_faces
            .setup(H::CacheDegenFaces, ff(F::BadDegenFace));

        self.bad_edge_props_coin_edges
            .base
            .setup(H::CacheCoinEdges, Some(EdgeFlag::BadCoincidentEdge));

        self.bad_edge_props_missed_edge_xsects
            .base
            .setup(H::CacheMissedEdgexsects, Some(EdgeFlag::BadMissedEdgeXSect));

        self.face_props_sheets
            .setup(H::CacheFaceprops2, Some(FaceMixedOf::new(FaceMixed::Sheet)));
        self.face_props_no_vertex_pole
            .setup(H::CacheFaceprops2, Some(FaceMixedOf::new(FaceMixed::Poles)));
        self.bad_face_props_flat_corner
            .setup(H::CacheFaceprops2, ff(F::BadFlatCorners));

        self.bad_face_props_missing_seam
            .setup(H::CacheFaceprops2, ff(F::BadMissingSeam));
        self.bad_face_props_crossed_pole
            .setup(H::CacheFaceprops2, ff(F::BadCrossedPole));
        self.bad_face_props_crossed_seam
            .setup(H::CacheFaceprops2, ff(F::BadCrossedSeam));
        self.bad_face_props_near_miss_seam
            .setup(H::CacheFaceprops2, ff(F::BadNearMissSeam));

        self.ran_face_props_move_seam
            .setup(H::FixMoveseam, Some(FaceTriedOf::new(FaceTried::MoveSeam)));

        self.ran_face_props_split_edges_at_seam.setup(
            H::FixSplitedgeAtseam,
            Some(FaceTriedOf::new(FaceTried::SplitEdgesAtSeam)),
        );

        self.bad_face_props_loop_problems
            .setup(H::CacheFaceprops3, ff(F::BadLoopProblems));
        self.bad_face_props_outer_loop_order
            .setup(H::CacheFaceprops3, ff(F::BadOuterLoopOrder));
        self.bad_face_props_orient_loop
            .setup(H::CacheFaceprops3, ff(F::BadOrientLoopCnt));
        self.bad_face_props_no_area_loop
            .setup(H::CacheFaceprops3, ff(F::BadNoAreaLoopCnt));

        self.bad_face_props_multi_outer_loops
            .setup(H::CacheFaceprops3, ff(F::OuterLoopCnt));
        self.bad_face_props_nested_outer_loops
            .setup(H::CacheFaceprops3, ff(F::BadNestedLoopCnt));
        self.bad_face_props_unpaired_no_area_loop
            .setup(H::CacheFaceprops3, ff(F::BadUnpairedNoAreaLoop));
        self.bad_face_props_closed_3d_loop
            .setup(H::CacheFaceprops3, ff(F::BadClosed3dLoopCnt));
        self.bad_face_props_closed_ptr_loop
            .setup(H::CacheFaceprops3, ff(F::BadClosedPtrLoopCnt));
        self.bad_face_props_missing_pole
            .setup(H::CacheFaceprops3, ff(F::BadMissingPoles));

        self.ran_face_props_split_face_at_seam.setup(
            H::FixSplitfaceAtseams,
            Some(FaceTriedOf::new(FaceTried::SplitFacesAtSeam)),
        );

        // begin Section 2 Scope: Set ProbArray Labels
        let bad = " - Bad ";
        let warn = " - Warn ";

        self.not_yet_face_props.set_labels(
            "Faces with NotYet Problems              ",
            "Unfixable Face Cnt ",
            "Warn - faces with probs not yet fixable",
        );

        self.bad_obj_props_back_pointers.set_labels(
            "Obj BackPointer Not Set",
            "BadBackPointer Cnt",
            bad,
        );

        self.bad_edge_props_uncontained.set_labels(
            "Edge ParamLength exceeds Crv ParamDomain",
            "Uncontained Cnt    ",
            bad,
        );
        self.bad_edge_props_small_zone_tol_3d.set_labels(
            "Edge ZoneTol3d too small                ",
            "SmallTol Edge Cnt  ",
            bad,
        );

        self.bad_edge_props_large_zone_tol_3d.set_labels(
            "Edge ZoneTol3d too large                ",
            "LargeTol Edge Cnt  ",
            bad,
        );
        self.warn_edge_props_large_gap_3d.set_labels(
            "Edge Gaps larger than def XSectTol3d    ",
            "LargeGap Edge Cnt  ",
            warn,
        );
        self.bad_edge_props_degen_edges.set_labels(
            "Edges too short                         ",
            "Degen Edge Cnt     ",
            bad,
        );

        self.bad_vertex_props_small_zone_tol_3d.set_labels(
            "Vertex ZoneTol3d too small              ",
            "SmallTol Vert Cnt  ",
            bad,
        );
        self.bad_vertex_props_large_zone_tol_3d.set_labels(
            "Vertex ZoneTol3d too large              ",
            "LargeTol Vert Cnt  ",
            bad,
        );
        self.warn_vertex_props_large_gap_3d.set_labels(
            "Vertex Gaps larger than def XSectTol3d  ",
            "LargeGap Vert Cnt  ",
            warn,
        );

        self.bad_face_props_small_zone_tol_3d.set_labels(
            "Face ZoneTol3d too small                ",
            "SmallTol Face Cnt  ",
            bad,
        );
        self.bad_face_props_large_zone_tol_3d.set_labels(
            "Face ZoneTol3d too large                ",
            "LargeTol Face Cnt  ",
            bad,
        );

        self.bad_vertex_props_coin_vertices.base.set_labels(
            "Vertices too close to one another       ",
            "Coincident PairCnt ",
            bad,
        );

        self.bad_face_props_degen_faces.set_labels(
            "Faces too small                         ",
            "Degen Face Cnt     ",
            bad,
        );

        self.bad_edge_props_coin_edges.base.set_labels(
            "Edges too close to one another          ",
            "Coincident PairCnt ",
            bad,
        );

        self.bad_edge_props_missed_edge_xsects.base.set_labels(
            "Edge XSects missing Vertices            ",
            "Missed EdgeXSectCnt",
            bad,
        );

        self.face_props_sheets.base.set_labels(
            "Sheet Face counts                       ",
            "                   ",
            "",
        );
        self.face_props_no_vertex_pole.base.set_labels(
            "Surface NoVertex Poles                  ",
            "                   ",
            "",
        );
        self.bad_face_props_flat_corner.set_labels(
            "Surface Flat Corners (Su==Sv at corner) ",
            "Flat Corner Cnt    ",
            bad,
        );

        self.bad_face_props_missing_seam.set_labels(
            "Face on closed srf missing a seam       ",
            "MissingSeam Cnt    ",
            bad,
        );
        self.bad_face_props_crossed_pole.set_labels(
            "Face boundary crosses a pole            ",
            "CrossedPole Cnt    ",
            bad,
        );
        self.bad_face_props_crossed_seam.set_labels(
            "Face on closed srf crosses a seam       ",
            "CrossedSeam Cnt    ",
            bad,
        );
        self.bad_face_props_near_miss_seam.set_labels(
            "Face on closed srf comes close to a seam",
            "NearMissSeam Cnt   ",
            bad,
        );

        self.ran_face_props_move_seam.base.set_labels(
            "Faces sent through Fix_MoveSeam         ",
            "                   ",
            "",
        );

        self.ran_face_props_split_edges_at_seam.base.set_labels(
            "Faces sent through Fix_SplitEdgesAtSeam ",
            "                   ",
            "",
        );

        self.bad_face_props_loop_problems.set_labels(
            "All Faces with any Loop Problems        ",
            "ProbLoop FaceCnt   ",
            bad,
        );

        self.bad_face_props_outer_loop_order.set_labels(
            "OuterLoop not 1st on Face LoopList      ",
            "Bad OuterLoop Face cnt           ",
            bad,
        );
        self.bad_face_props_orient_loop.set_labels(
            "LoopOrient-outer/inner Loop conflict    ",
            "Bad OrientedLoop Face cnt        ",
            bad,
        );
        self.bad_face_props_no_area_loop.set_labels(
            "Loop fails to enclose an area in UVSpace",
            "Bad NoAreaLoop Face cnt          ",
            bad,
        );
        self.bad_face_props_multi_outer_loops.set_labels(
            "Multiple Outer Loops on Face            ",
            "Bad multiple OuterLoop Face cnt  ",
            bad,
        );
        self.bad_face_props_nested_outer_loops.set_labels(
            "Nested OuterLoops on Face               ",
            "Bad nested OuterLoop Face cnt    ",
            bad,
        );

        self.bad_face_props_unpaired_no_area_loop.set_labels(
            "odd NoAreaLoopCnt on Face: no AddSeamFix",
            "Bad unpaired NoAreaLoop Face Cnt ",
            bad,
        );
        self.bad_face_props_closed_3d_loop.set_labels(
            "FaceLoop Edge EndPts not closed in 3d   ",
            "Bad 3d UnclosedLoop Face Cnt     ",
            bad,
        );
        self.bad_face_props_closed_ptr_loop.set_labels(
            "FaceLoop Edge EndPtrs not a closed loop ",
            "Bad ptr UnclosedLoop Face Cnt    ",
            bad,
        );
        self.bad_face_props_missing_pole.set_labels(
            "Face contains an unmarked srf pole      ",
            "Bad Missing Vert on Pole Face Cnt",
            bad,
        );

        self.ran_face_props_split_face_at_seam.base.set_labels(
            "Faces sent through Fix_SplitFaceAtSeam  ",
            "                   ",
            "",
        );

        // begin Section 3 Scope: Clear ProbArray ProbLists
        self.not_yet_face_props.reset();

        self.bad_obj_props_back_pointers.reset();

        self.bad_edge_props_uncontained.reset();
        self.bad_edge_props_small_zone_tol_3d.reset();
        self.bad_edge_props_large_zone_tol_3d.reset();
        self.warn_edge_props_large_gap_3d.reset();
        self.bad_edge_props_degen_edges.reset();

        self.bad_vertex_props_small_zone_tol_3d.reset();
        self.bad_vertex_props_large_zone_tol_3d.reset();
        self.warn_vertex_props_large_gap_3d.reset();

        self.bad_face_props_small_zone_tol_3d.reset();
        self.bad_face_props_large_zone_tol_3d.reset();

        self.bad_vertex_props_coin_vertices.reset();

        self.bad_face_props_degen_faces.reset();

        self.bad_edge_props_coin_edges.reset();

        self.bad_edge_props_missed_edge_xsects.reset();

        self.face_props_sheets.reset();
        self.face_props_no_vertex_pole.reset();
        self.bad_face_props_flat_corner.reset();

        self.bad_face_props_missing_seam.reset();
        self.bad_face_props_crossed_pole.reset();
        self.bad_face_props_crossed_seam.reset();
        self.bad_face_props_near_miss_seam.reset();

        self.ran_face_props_move_seam.reset();

        self.ran_face_props_split_edges_at_seam.reset();

        self.bad_face_props_loop_problems.reset();

        self.bad_face_props_outer_loop_order.reset();
        self.bad_face_props_orient_loop.reset();
        self.bad_face_props_no_area_loop.reset();

        self.bad_face_props_multi_outer_loops.reset();
        self.bad_face_props_nested_outer_loops.reset();

        self.bad_face_props_unpaired_no_area_loop.reset();
        self.bad_face_props_closed_3d_loop.reset();
        self.bad_face_props_closed_ptr_loop.reset();
        self.bad_face_props_missing_pole.reset();

        self.ran_face_props_split_face_at_seam.reset();
    }

    /// `GetFaceProps_ForTgts`: the face props of the target faces among `tgts`, in
    /// `tgts` order. With `stale_tgts` (upstream's default) the entries are only
    /// compared, never queried; either way only faces can match.
    #[must_use]
    pub fn face_props_for_tgts(&self, tgts: &[EntityRef], stale_tgts: bool) -> Vec<FacePropsId> {
        let _ = stale_tgts;
        let mut out = Vec::new();
        for t in tgts {
            if let EntityRef::Face(f) = t {
                if let Some(i) = self.tgt_faces.iter().position(|x| x == f) {
                    if let Some(&p) = self.tgt_face_props.get(i) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    /// `GetEdgeProps_ForTgts`.
    #[must_use]
    pub fn edge_props_for_tgts(&self, tgts: &[EntityRef], stale_tgts: bool) -> Vec<EdgePropsId> {
        let _ = stale_tgts;
        let mut out = Vec::new();
        for t in tgts {
            if let EntityRef::Edge(e) = t {
                if let Some(i) = self.tgt_edges.iter().position(|x| x == e) {
                    if let Some(&p) = self.tgt_edge_props.get(i) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    /// `GetVertexProps_ForTgts`.
    #[must_use]
    pub fn vertex_props_for_tgts(
        &self,
        tgts: &[EntityRef],
        stale_tgts: bool,
    ) -> Vec<VertexPropsId> {
        let _ = stale_tgts;
        let mut out = Vec::new();
        for t in tgts {
            if let EntityRef::Vertex(v) = t {
                if let Some(i) = self.tgt_vertices.iter().position(|x| x == v) {
                    if let Some(&p) = self.tgt_vertex_props.get(i) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    /// `RemoveVertexProps_FromLists`: takes `props` (and its vertex) off every list
    /// without freeing it. Returns whether the vertex was on the target vertex list
    /// and whether the props were on the target props list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a freed record (upstream dereferences it) and
    /// from the coincident pair list.
    pub fn remove_vertex_props_from_lists(&mut self, props: VertexPropsId) -> Result<(bool, bool)> {
        let vertex = self.vertex_props.props(props)?.vertex;

        // remove Tgt Vertex and VertexProps from the Target lists (a null m_pVertex
        // matches only a null entry, which the lists never hold)
        let rm_from_tgt_vertices = vertex.is_some_and(|v| remove_first(&mut self.tgt_vertices, &v));
        let rm_from_tgt_vertex_props = remove_first(&mut self.tgt_vertex_props, &props);

        // Base class
        self.bad_obj_props_back_pointers
            .remove(PropsRef::Vertex(props));

        // remove Tgt VertexProps Ptr from all the various Heal VertexProps Ptr lists
        self.bad_vertex_props_small_zone_tol_3d.remove(props);
        self.bad_vertex_props_large_zone_tol_3d.remove(props);
        self.warn_vertex_props_large_gap_3d.remove(props);
        self.bad_vertex_props_coin_vertices
            .remove(&self.vertex_props, props)?;
        self.vertices_in_voxels.remove(props);

        Ok((rm_from_tgt_vertices, rm_from_tgt_vertex_props))
    }

    /// `RemoveEdgeProps_FromLists`.
    ///
    /// # Errors
    ///
    /// As [`HealData::remove_vertex_props_from_lists`].
    pub fn remove_edge_props_from_lists(&mut self, props: EdgePropsId) -> Result<(bool, bool)> {
        let edge_id = self.edge_props.props(props)?.edge;

        // remove Tgt Edge and EdgeProps from the Target lists
        let rm_from_tgt_edges = edge_id.is_some_and(|e| remove_first(&mut self.tgt_edges, &e));
        let rm_from_tgt_edge_props = remove_first(&mut self.tgt_edge_props, &props);

        // Base class
        self.bad_obj_props_back_pointers
            .remove(PropsRef::Edge(props));

        // remove Tgt EdgeProps Ptr from all the various Heal EdgeProps Ptr lists
        self.bad_edge_props_uncontained.remove(props);
        self.bad_edge_props_small_zone_tol_3d.remove(props);
        self.bad_edge_props_large_zone_tol_3d.remove(props);
        self.warn_edge_props_large_gap_3d.remove(props);
        self.bad_edge_props_degen_edges.remove(props);
        self.bad_edge_props_coin_edges
            .remove(&self.edge_props, props)?;
        self.bad_edge_props_missed_edge_xsects
            .remove(&self.edge_props, props)?;

        Ok((rm_from_tgt_edges, rm_from_tgt_edge_props))
    }

    /// `RemoveFaceProps_FromLists`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a freed record.
    pub fn remove_face_props_from_lists(&mut self, props: FacePropsId) -> Result<(bool, bool)> {
        let face = self.face_props.props(props)?.props.face;

        // remove Tgt Face and FaceProps from the Target lists
        let rm_from_tgt_faces = face.is_some_and(|f| remove_first(&mut self.tgt_faces, &f));
        let rm_from_tgt_face_props = remove_first(&mut self.tgt_face_props, &props);

        // Base class
        self.bad_obj_props_back_pointers
            .remove(PropsRef::Face(props));

        // remove Tgt FaceProps Ptr from all the various Heal FaceProps Ptr lists
        self.not_yet_face_props.remove(props);
        self.bad_face_props_small_zone_tol_3d.remove(props);
        self.bad_face_props_large_zone_tol_3d.remove(props);
        self.bad_face_props_degen_faces.remove(props);
        self.face_props_sheets.base.remove(props);
        self.face_props_no_vertex_pole.base.remove(props);
        self.bad_face_props_flat_corner.remove(props);
        self.bad_face_props_missing_seam.remove(props);
        self.bad_face_props_crossed_pole.remove(props);
        self.bad_face_props_crossed_seam.remove(props);
        self.bad_face_props_near_miss_seam.remove(props);
        self.ran_face_props_move_seam.base.remove(props);
        self.ran_face_props_split_edges_at_seam.base.remove(props);
        self.bad_face_props_loop_problems.remove(props);
        self.bad_face_props_outer_loop_order.remove(props);
        self.bad_face_props_orient_loop.remove(props);
        self.bad_face_props_no_area_loop.remove(props);
        self.bad_face_props_multi_outer_loops.remove(props);
        self.bad_face_props_nested_outer_loops.remove(props);
        self.bad_face_props_unpaired_no_area_loop.remove(props);
        self.bad_face_props_closed_3d_loop.remove(props);
        self.bad_face_props_closed_ptr_loop.remove(props);
        self.bad_face_props_missing_pole.remove(props);
        self.ran_face_props_split_face_at_seam.base.remove(props);

        Ok((rm_from_tgt_faces, rm_from_tgt_face_props))
    }

    /// `Cache_EdgeProps`: sets the props of the (new) target edges whose gap values are
    /// not set, lists their problems and accumulates the edge length statistics and
    /// histograms. Returns whether new problems were found.
    ///
    /// # Errors
    ///
    /// Those of [`EdgeProps::set_props`] that upstream would not survive (unported
    /// callees, null and dangling pointers); [`SolidError::InvalidInput`] when the
    /// target edge list is shorter than the props list where it is read.
    pub fn cache_edge_props(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        this_healer_op: HealerOpType,
    ) -> Result<bool> {
        let _ = this_healer_op;
        let mut new_probs = false;

        // no work - no Edges or no new Edges
        if self.tgt_edge_props.is_empty() || self.tgt_edge_props.len() == self.old_edge_cnt {
            return Ok(new_probs);
        }

        // reset histograms when processing all ObjProps: skip reset when processing new ObjProps
        if self.old_edge_cnt == 0 {
            self.max_edge_length = 0.0;
            self.avg_edge_length = 0.0;
            self.non_degen_min_edge_length = BIG_DOUBLE;
            self.non_degen_edge_cnt = 0;

            self.loop_gap_3d_histogram = gap_histogram();
            self.edge_face_gap_3d_histogram = gap_histogram();
            self.edge_len_3d_histogram = vec![0; EDGELEN_BUCKET_CNT];
        } else {
            // change AvgEdgeLength back to a total
            if self.non_degen_edge_cnt > 0 {
                self.avg_edge_length *= self.non_degen_edge_cnt as f64;
            }
        }

        // for every EdgeProps - (m_lOldEdgeCnt != 0 when updating TgtLists with new objs)
        let mut ii = self.old_edge_cnt;
        while ii < self.tgt_edge_props.len() {
            let id = tgt(&self.tgt_edge_props, ii)?;

            // set EdgeProps_Gaps
            if !self.edge_props.props(id)?.is_set_gaps() {
                let edge_id = tgt(&self.tgt_edges, ii)?;
                let mut p = self.edge_props.props(id)?.clone();
                let r = p.set_props(
                    k,
                    b,
                    edge_id,
                    ii as u64,
                    Some(&mut self.loop_gap_3d_histogram),
                    Some(&mut self.edge_face_gap_3d_histogram),
                    ii as u64,
                );
                *self.edge_props.props_mut(id)? = p;
                ignore_status(r)?;
            }
            let p = self.edge_props.props(id)?.clone();

            // m_sBadEdgeProps_Uncontained
            if p.bad_uncontained_edge == Tri::True {
                self.bad_edge_props_uncontained
                    .add(&mut self.edge_props, id, true)?;
                new_probs = true;
            }

            // m_sBadEdgeProps_SmallZoneTol3d
            if p.bad_small_zone_tol_3d == Tri::True {
                self.bad_edge_props_small_zone_tol_3d
                    .add(&mut self.edge_props, id, true)?;
                new_probs = true;
            }

            // m_sBadEdgeProps_LargeZoneTol3d
            if p.bad_large_zone_tol_3d == Tri::True {
                self.bad_edge_props_large_zone_tol_3d
                    .add(&mut self.edge_props, id, true)?;
                new_probs = true;
            }

            // m_sWarnEdgeProps_LargeGap3d
            if p.warn_large_gap_3d == Tri::True {
                self.warn_edge_props_large_gap_3d
                    .add(&mut self.edge_props, id, true)?;
            }

            // EdgeLength Histogram
            bump(
                &mut self.edge_len_3d_histogram,
                edge_len_histogram_index(p.approx_edge_length_3d)?,
            )?;

            // BadDegen EdgeList
            if p.bad_degen_edge == Tri::True {
                self.bad_edge_props_degen_edges
                    .add(&mut self.edge_props, id, true)?;
                new_probs = true;
            } else {
                // accumulate m_dMaxEdgeLength statistics
                if p.approx_edge_length_3d > self.max_edge_length {
                    self.max_edge_length = p.approx_edge_length_3d;
                }

                // accumulate m_dNonDegen_MinEdgeLength statistics
                if p.approx_edge_length_3d < self.non_degen_min_edge_length {
                    self.non_degen_min_edge_length = p.approx_edge_length_3d;
                }

                // accumulate m_dAvgEdgeLength
                self.avg_edge_length += p.approx_edge_length_3d;
                self.non_degen_edge_cnt += 1;
            }
            ii += 1;
        }

        // finish getting AvgEdgeLength
        if self.non_degen_edge_cnt > 0 {
            self.avg_edge_length /= self.non_degen_edge_cnt as f64;
        }
        Ok(new_probs)
    }

    /// `Cache_VertexProps`: sets the props of the (new) target vertices that are not
    /// set and lists their problems. Returns whether new problems were found.
    ///
    /// # Errors
    ///
    /// As [`HealData::cache_edge_props`].
    pub fn cache_vertex_props(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        this_healer_op: HealerOpType,
    ) -> Result<bool> {
        let _ = this_healer_op;
        let mut new_probs = false;

        // no work - no vertices or no new vertices
        if self.tgt_vertex_props.is_empty() || self.tgt_vertex_props.len() == self.old_vertex_cnt {
            return Ok(new_probs);
        }

        // reset histograms when processing all ObjProps
        if self.old_vertex_cnt == 0 {
            self.vert_edge_gap_3d_histogram = gap_histogram();
            self.vert_face_gap_3d_histogram = gap_histogram();
        }

        // for every VertexProp obj - Get VertexProperties
        let mut ii = self.old_vertex_cnt;
        while ii < self.tgt_vertex_props.len() {
            let id = tgt(&self.tgt_vertex_props, ii)?;

            // set VertexProps
            if !self.vertex_props.props(id)?.is_set() {
                let vertex_id = tgt(&self.tgt_vertices, ii)?;
                let mut p = self.vertex_props.props(id)?.clone();
                let r = p.set_props(
                    k,
                    b,
                    vertex_id,
                    ii as u64,
                    Some(&mut self.vert_edge_gap_3d_histogram),
                    Some(&mut self.vert_face_gap_3d_histogram),
                );
                *self.vertex_props.props_mut(id)? = p;
                ignore_status(r)?;
            }
            let p = self.vertex_props.props(id)?.clone();

            // m_bBadSmallZoneTol3d
            if p.bad_small_zone_tol_3d == Tri::True {
                self.bad_vertex_props_small_zone_tol_3d
                    .add(&mut self.vertex_props, id, true)?;
                new_probs = true;
            }

            // m_bBadLargeZoneTol3d
            if p.bad_large_zone_tol_3d == Tri::True {
                self.bad_vertex_props_large_zone_tol_3d
                    .add(&mut self.vertex_props, id, true)?;
                new_probs = true;
            }

            // m_bWarnLargeGap3d
            if p.warn_large_gap_3d == Tri::True {
                self.warn_vertex_props_large_gap_3d
                    .add(&mut self.vertex_props, id, true)?;
            }
            ii += 1;
        }
        Ok(new_probs)
    }

    /// `Cache_FaceProps_Gaps`: sets the gap stage of the (new) target face props that
    /// do not have it and lists their tolerance problems. Returns whether new problems
    /// were found.
    ///
    /// # Errors
    ///
    /// As [`HealData::cache_edge_props`].
    pub fn cache_face_props_gaps(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        this_healer_op: HealerOpType,
    ) -> Result<bool> {
        let _ = this_healer_op;
        let mut new_probs = false;

        // no work - no Faces or no new Faces
        if self.tgt_face_props.is_empty() || self.tgt_face_props.len() == self.old_face_cnt {
            return Ok(new_probs);
        }

        // for every FaceProp obj
        let mut ii = self.old_face_cnt;
        while ii < self.tgt_face_props.len() {
            let id = tgt(&self.tgt_face_props, ii)?;

            // set FaceProps_Stage1
            let e = self.face_props.props(id)?;
            if !e.props.has_props(PropStage::Gaps, e.props.face) {
                let face = tgt(&self.tgt_faces, ii)?;
                let done = self.done_heal_op;
                let e = self.face_props.props_mut(id)?;
                ignore_status(
                    e.props
                        .set_props(k, b, PropStage::Gaps, face, ii as u64, done),
                )?;
            }
            let p = &self.face_props.props(id)?.props;
            let (small, large) = (p.bad_small_zone_tol_3d, p.bad_large_zone_tol_3d);

            // m_sBadFaceProps_SmallZoneTol3d
            if small == Tri::True {
                self.bad_face_props_small_zone_tol_3d
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }

            // m_sBadFaceProps_LargeZoneTol3d
            if large == Tri::True {
                self.bad_face_props_large_zone_tol_3d
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }
            ii += 1;
        }
        Ok(new_probs)
    }

    /// `Fix_TolSizes`: picks the model size from the longest edge, sets the B-rep's
    /// model size estimate and tolerance from it, then gives every face, edge and
    /// vertex with a bad tolerance the one its gaps call for.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from what `SmBrep::SetTolerance` and the face tolerance
    /// setter reach; [`SolidError::InvalidInput`] for a dangling props record
    /// or one without its topology.
    #[allow(clippy::too_many_lines)]
    pub fn fix_tol_sizes(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        this_healer_op: HealerOpType,
    ) -> Result<()> {
        let _ = this_healer_op;

        // Single list of problem Vertex objs
        let mut bad_tol_vertex_props = self.bad_vertex_props_small_zone_tol_3d.prob_array.clone();
        bad_tol_vertex_props.extend_from_slice(&self.bad_vertex_props_large_zone_tol_3d.prob_array);

        // Single list of problem Edge objs
        let mut bad_tol_edge_props = self.bad_edge_props_small_zone_tol_3d.prob_array.clone();
        bad_tol_edge_props.extend_from_slice(&self.bad_edge_props_large_zone_tol_3d.prob_array);

        // Single list of problem Face objs
        let mut bad_tol_face_props = self.bad_face_props_small_zone_tol_3d.prob_array.clone();
        bad_tol_face_props.extend_from_slice(&self.bad_face_props_large_zone_tol_3d.prob_array);

        // Set Context default ZoneTol3d based on Part size
        let mut this_model_size = if self.tgt_edges.is_empty() {
            50.0
        } else {
            self.max_edge_length
        };

        // ModelSize picks Working tolerance values
        let s = this_model_size;
        this_model_size = if s <= 500.0 && s > 0.5 {
            50.0
        } else if s <= 50000.0 && s > 500.0 {
            5000.0
        } else if s <= 0.5 && s > 0.005 {
            0.05
        } else if s <= 5e6 && s > 50000.0 {
            500_000.0
        } else if s <= 0.005 && s > 0.00005 {
            0.0005
        } else {
            s
        };
        let brep_model_size = brep_model_size_estimate(k, b);
        let context_model_size = context_model_size_estimate(k, &k.context);

        let this_est_zone_tol_3d = size_estimate_to_zone_tol_3d(this_model_size);
        let brep_est_zone_tol_3d = size_estimate_to_zone_tol_3d(brep_model_size);
        let context_est_zone_tol_3d = size_estimate_to_zone_tol_3d(context_model_size);

        // When The BrepTolerance needs to be set - set it based on ModelSizeEstimate
        if this_est_zone_tol_3d != brep_est_zone_tol_3d {
            if this_est_zone_tol_3d == context_est_zone_tol_3d {
                set_brep_model_size_estimate(k, b, USE_DEFAULT);
            } else {
                set_brep_model_size_estimate(k, b, this_model_size);
            }
        }
        set_brep_tolerance(k, b, this_est_zone_tol_3d)?;

        // for every problem Face - Calc and Store a new ZoneTol3d
        for &id in &bad_tol_face_props {
            let p = &self.face_props.props(id)?.props;

            // skip faces already fixed
            if p.bad_small_zone_tol_3d == Tri::False && p.bad_large_zone_tol_3d == Tri::False {
                continue;
            }
            let face = p.face.ok_or_else(|| {
                SolidError::invalid_input(OP, "Fix_TolSizes: face props without face")
            })?;

            // update the tolerance (SmTol::CalcFaceZoneTol3d(pBrep))
            let zone_tol_3d = brep_zone_tol_3d(b);

            if self.face_props.props(id)?.props.orig_zone_tol_3d == UNDEF_DOUBLE {
                let t = face_p2::tolerance(b, face)?;
                self.face_props.props_mut(id)?.props.orig_zone_tol_3d = t;
            }
            set_face_tolerance(b, face, zone_tol_3d, false)?;
            let p = &mut self.face_props.props_mut(id)?.props;
            p.zone_tol_3d = zone_tol_3d;

            // remember the tol has been fixed on this Face
            p.bad_small_zone_tol_3d = Tri::False;
            p.bad_large_zone_tol_3d = Tri::False;
        }

        // Edge: for every problem edge - Calc and Store a new ZoneTol3d
        for &id in &bad_tol_edge_props {
            let p = self.edge_props.props(id)?.clone();

            // skip edges already fixed
            if p.bad_small_zone_tol_3d == Tri::False && p.bad_large_zone_tol_3d == Tri::False {
                continue;
            }
            let bad_tol_edge = p.edge.ok_or_else(|| {
                SolidError::invalid_input(OP, "Fix_TolSizes: edge props without edge")
            })?;

            // Get the largest FaceTol connected to this Edge
            let mut max_face_tol = zone_tol3d(k, 0.0);
            for f in edge::faces(b, bad_tol_edge, None)? {
                let face_zone_tol_3d = face_p2::tolerance(b, f)?;
                max_face_tol = if face_zone_tol_3d > max_face_tol {
                    face_zone_tol_3d
                } else {
                    max_face_tol
                };
            }

            // update the tolerance
            let max_edge_edge_gap_3d_length = p.max_edge_edge_gap_length(b)?;
            let max_edge_face_gap_3d_length = p.max_edge_face_gap_length(b)?;
            let def = brep_zone_tol_3d(b);
            let zone_tol_3d = calc_brep_gap_zone_tol(
                k,
                def,
                max_edge_edge_gap_3d_length,
                max_edge_face_gap_3d_length,
                Some(max_face_tol),
            );
            if p.orig_zone_tol_3d == UNDEF_DOUBLE {
                let t = edge::tolerance(b, bad_tol_edge)?;
                self.edge_props.props_mut(id)?.orig_zone_tol_3d = t;
            }
            edge::set_tolerance(b, bad_tol_edge, zone_tol_3d, false, false)?;
            let p = self.edge_props.props_mut(id)?;
            p.zone_tol_3d = zone_tol_3d;

            // remember the tol has been fixed on this Edge
            p.bad_small_zone_tol_3d = Tri::False;
            p.bad_large_zone_tol_3d = Tri::False;
        }

        // for every problem Vertex - Calc and Store a new ZoneTol3d
        for &id in &bad_tol_vertex_props {
            let p = self.vertex_props.props(id)?.clone();

            // skip vertices already fixed
            if p.bad_small_zone_tol_3d == Tri::False && p.bad_large_zone_tol_3d == Tri::False {
                continue;
            }
            let bad_tol_vertex = p.vertex.ok_or_else(|| {
                SolidError::invalid_input(OP, "Fix_TolSizes: vertex props without vertex")
            })?;

            // Get the largest EdgeTol connected to this vertex
            let mut max_edge_tol_3d = zone_tol3d(k, 0.0);
            for e in vertex::edges(b, bad_tol_vertex, None)? {
                let edge_zone_tol_3d = edge::tolerance(b, e)?;
                max_edge_tol_3d = if edge_zone_tol_3d > max_edge_tol_3d {
                    edge_zone_tol_3d
                } else {
                    max_edge_tol_3d
                };
            }

            // update the tolerance
            let max_vertex_edge_gap_3d_length = p.max_vtx_edge_gap_length(b)?;
            let max_vertex_face_gap_3d_length = p.max_vtx_face_gap_length(b)?;
            let def = brep_zone_tol_3d(b);
            let zone_tol_3d = calc_brep_gap_zone_tol(
                k,
                def,
                max_vertex_edge_gap_3d_length,
                max_vertex_face_gap_3d_length,
                Some(max_edge_tol_3d),
            );
            if p.orig_zone_tol_3d == UNDEF_DOUBLE {
                let t = vertex::tolerance(b, bad_tol_vertex)?;
                self.vertex_props.props_mut(id)?.orig_zone_tol_3d = t;
            }
            // FALSE = allow shrinking tolerances (bCascadeToBndries: default TRUE)
            vertex::set_tolerance(b, bad_tol_vertex, zone_tol_3d, false, true)?;
            let p = self.vertex_props.props_mut(id)?;
            p.zone_tol_3d = zone_tol_3d;

            // remember the tol has been fixed on this Vertex
            p.bad_small_zone_tol_3d = Tri::False;
            p.bad_large_zone_tol_3d = Tri::False;
        }
        Ok(())
    }

    /// `Cache_CoinVertices`: places the (new) target vertices into the voxel model
    /// (sized from their bounding box on the first call) and lists the coincident
    /// pairs it finds, marking both members and counting the pair. Returns whether new
    /// problems were found.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when re-running on new vertices puts none of them
    /// into a voxel (upstream reads the first bucket index of the empty list), for a
    /// dangling record or one without vertex, and from the voxel model;
    /// [`SolidError::NotFound`] for a dangling vertex.
    pub fn cache_coin_vertices(
        &mut self,
        k: &Kernel,
        b: &Brep,
        this_healer_op: HealerOpType,
    ) -> Result<bool> {
        let _ = this_healer_op;

        // no work - no vertices or no new vertices
        if self.tgt_vertex_props.is_empty() || self.tgt_vertex_props.len() == self.old_vertex_cnt {
            return Ok(false);
        }

        // init output
        let mut new_probs = false;

        let vertex_of = |store: &PropsArena<VertexPropsId, VertexProps>,
                         id: VertexPropsId|
         -> Result<VertexId> {
            store.props(id)?.vertex.ok_or_else(|| {
                SolidError::invalid_input(OP, "Cache_CoinVertices: vertex props without vertex")
            })
        };

        // On the first call or when asked to recheck all the vertices
        if self.old_vertex_cnt == 0 {
            // always reset histograms when processing all vertices
            self.vert_vert_gap_3d_histogram = gap_histogram();

            // find TgtVertex BoundingBox
            self.vertex_bbox.init();

            // for every TgtVertex - grow BoundingBox
            for &id in &self.tgt_vertex_props {
                let v = vertex_of(&self.vertex_props, id)?;
                self.vertex_bbox.add_point(vertex::point(b, v)?);
            }

            // set the voxel model size
            let estimate = self.tgt_vertex_props.len().checked_mul(7).ok_or_else(|| {
                SolidError::invalid_input(OP, "Cache_CoinVertices: too many vertices")
            })?;
            self.vertices_in_voxels.set_size(
                self.x_cnt,
                self.y_cnt,
                self.z_cnt,
                Some(&self.vertex_bbox),
                estimate,
            )?;
        }

        // When adding more points to the voxels - keep track of the updated buckets
        let mut updated_buckets: Vec<usize> = Vec::new();
        let track = self.old_vertex_cnt != 0;

        // add the incremental tgt vertices and positions into the voxel model
        let mut ii = self.old_vertex_cnt;
        while ii < self.tgt_vertex_props.len() {
            let id = tgt(&self.tgt_vertex_props, ii)?;
            let v = vertex_of(&self.vertex_props, id)?;
            let point = vertex::point(b, v)?;
            let zone_tol_3d = vertex::tolerance(b, v)?;

            // Create and Add SmPointItem obj to m_pVerticesInVoxels
            let _voxel = self.vertices_in_voxels.add(
                id,
                point,
                zone_tol_3d,
                if track {
                    Some(&mut updated_buckets)
                } else {
                    None
                },
            )?;
            ii += 1;
        }

        // next - check for coincidence
        if track && updated_buckets.is_empty() {
            return Err(SolidError::invalid_input(
                OP,
                "Cache_CoinVertices: no updated bucket (upstream reads past the empty list)",
            ));
        }
        let mut hist = [0u64; GAPSIZE_BUCKET_CNT];
        for (h, &x) in hist.iter_mut().zip(&self.vert_vert_gap_3d_histogram) {
            *h = x;
        }
        let limit = self.old_vertex_cnt;
        let pairs = self.vertices_in_voxels.find_coincident_pairs(
            k,
            FindPairsOptions {
                limit_index: if track { Some(limit) } else { None },
                buckets: if track { Some(&updated_buckets) } else { None },
                histogram: Some(&mut hist),
            },
        )?;
        if self.vert_vert_gap_3d_histogram.len() != GAPSIZE_BUCKET_CNT {
            return Err(SolidError::invalid_input(
                OP,
                "Cache_CoinVertices: histogram size",
            ));
        }
        self.vert_vert_gap_3d_histogram.copy_from_slice(&hist);

        // for every coincident pair - add entry pair to the problem array
        let items = self.vertices_in_voxels.point_items();
        let item = |i: usize| -> Result<VertexPropsId> {
            items
                .get(i)
                .map(|p| p.item)
                .ok_or_else(|| SolidError::invalid_input(OP, "coincident pair item"))
        };
        let mut found: Vec<(VertexPropsId, VertexPropsId)> = Vec::new();
        for (&a, &c) in pairs.first.iter().zip(&pairs.second) {
            found.push((item(a)?, item(c)?));
        }
        for (p1, p2) in found {
            // defensive: skip pairs referencing stale/freed vertex props
            if !self.tgt_vertex_props.contains(&p1) || !self.tgt_vertex_props.contains(&p2) {
                continue;
            }

            // add coincident pair Props to prob array and set the linked flags
            self.bad_vertex_props_coin_vertices
                .base
                .add(&mut self.vertex_props, p1, true)?;
            self.bad_vertex_props_coin_vertices
                .base
                .add(&mut self.vertex_props, p2, true)?;

            self.bad_vertex_props_coin_vertices.orig_coin_pair_cnt = self
                .bad_vertex_props_coin_vertices
                .orig_coin_pair_cnt
                .wrapping_add(1);

            // set output
            new_probs = true;
        }
        Ok(new_probs)
    }

    /// `Cache_DegenFaces`: lists the target faces that are degenerate. Returns whether
    /// any was found.
    ///
    /// # Errors
    ///
    /// Those of `SmFace::IsDegenerate` (`topology.face_p3`);
    /// [`SolidError::InvalidInput`] when the target face list is shorter than the
    /// props list or for a dangling record.
    pub fn cache_degen_faces(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        this_healer_op: HealerOpType,
    ) -> Result<bool> {
        let _ = this_healer_op;
        let mut new_probs = false;

        // for every FaceProps - determine if face is degenerate.
        let num_faces = self.tgt_face_props.len();
        for ii in 0..num_faces {
            let id = tgt(&self.tgt_face_props, ii)?;
            let face = tgt(&self.tgt_faces, ii)?;
            self.face_props.props_mut(id)?.props.face = Some(face);

            // Can't use UV with these invalid faces
            let skip_quick_check = true;

            if face_is_degenerate(k, b, face, skip_quick_check)? {
                // side effects: marks the flag, counts orig problems
                self.bad_face_props_degen_faces
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }
        }
        Ok(new_probs)
    }

    /// `Cache_CoinEdges` (not implemented upstream: visits the new edge props only).
    #[must_use]
    pub fn cache_coin_edges(&self, this_healer_op: HealerOpType) -> bool {
        let _ = this_healer_op;
        // no work - no Edges or no new Edges; otherwise the TODO loop does nothing
        false
    }

    /// `Fix_CoinEdges` (not implemented upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) when coincident edges are listed.
    pub fn fix_coin_edges(&self, this_healer_op: HealerOpType) -> Result<()> {
        let _ = this_healer_op;
        if !self.bad_edge_props_coin_edges.base.is_empty() {
            return Err(SolidError::sm_err(
                "heal::heal_data::p2::HealData::fix_coin_edges",
            ));
        }
        Ok(())
    }

    /// `Cache_MissedEdgeXSects` (not implemented upstream).
    #[must_use]
    pub fn cache_missed_edge_xsects(&self, this_healer_op: HealerOpType) -> bool {
        let _ = this_healer_op;
        false
    }

    /// `Fix_MissedEdgeXSects` (not implemented upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) when missed intersections are listed.
    pub fn fix_missed_edge_xsects(&self, this_healer_op: HealerOpType) -> Result<()> {
        let _ = this_healer_op;
        if !self.bad_edge_props_missed_edge_xsects.base.is_empty() {
            return Err(SolidError::sm_err(
                "heal::heal_data::p2::HealData::fix_missed_edge_xsects",
            ));
        }
        Ok(())
    }

    /// `Fix_UncontainedEdges`: clamps the cached interval of every uncontained edge to
    /// its curve's natural interval (upstream's placeholder fix; the B-rep is not
    /// changed) and clears its flag.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a dangling record.
    pub fn fix_uncontained_edges(&mut self, this_healer_op: HealerOpType) -> Result<()> {
        let _ = this_healer_op;
        for ii in 0..self.bad_edge_props_uncontained.len() {
            let id = tgt(&self.bad_edge_props_uncontained.prob_array, ii)?;
            let p = self.edge_props.props_mut(id)?;

            // SetMin / SetMax refuse a negative interval; their status is ignored
            if p.edge_interval.min() < p.natural_interval.min() {
                let m = p.natural_interval.min();
                let _ = p.edge_interval.set_min(m);
            }

            if p.edge_interval.max() > p.natural_interval.max() {
                let m = p.natural_interval.max();
                let _ = p.edge_interval.set_max(m);
            }

            // remember the fix
            p.bad_uncontained_edge = Tri::False;
        }
        Ok(())
    }

    /// `Fix_BadGaps` (not implemented upstream: does nothing).
    pub fn fix_bad_gaps(&self, this_healer_op: HealerOpType) {
        let _ = this_healer_op;
    }
}

/// An unsized voxel model (`SetSize(0, 0, 0, NULL, 0)`).
fn empty_voxels() -> ObjsInVoxels<VertexPropsId> {
    ObjsInVoxels::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intersect::point_class::{ClassObject, ClassObjectKind};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H;
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Other
        }
    }
    impl LoopObject for H {
        fn from_loop(_l: LoopId) -> Self {
            H
        }
    }

    #[test]
    fn edge_len_index_buckets_by_decade() {
        assert_eq!(edge_len_histogram_index(1e-6).unwrap(), 0);
        assert_eq!(edge_len_histogram_index(5e-5).unwrap(), 1);
        assert_eq!(edge_len_histogram_index(0.5).unwrap(), 5);
        assert_eq!(edge_len_histogram_index(5.0).unwrap(), 6);
        assert_eq!(edge_len_histogram_index(9999.0).unwrap(), 9);
        assert_eq!(edge_len_histogram_index(1e5).unwrap(), 10);
        assert!(edge_len_histogram_index(f64::NAN).is_err());
    }

    #[test]
    fn pair_remove_keeps_upstream_skip() {
        let mut arena: PropsArena<VertexPropsId, VertexProps> = PropsArena::default();
        let ids: Vec<_> = (0..4).map(|_| arena.alloc(VertexProps::new())).collect();
        let mut a: PairProbArray<VertexPropsId, VertexFlag> =
            PairProbArray::new(HealerOpType::None);
        a.base.setup(
            HealerOpType::CacheCoinVertices,
            Some(VertexFlag::BadCoincidentVertex),
        );
        for &i in &ids {
            a.base.add(&mut arena, i, true).unwrap();
        }
        assert!(a.remove(&arena, ids[2]).unwrap());
        assert_eq!(a.base.prob_array, vec![ids[0], ids[1]]);
        // a single trailing TRUE member of an odd list reads past the end upstream
        let mut b: PairProbArray<VertexPropsId, VertexFlag> =
            PairProbArray::new(HealerOpType::None);
        b.base
            .setup(HealerOpType::None, Some(VertexFlag::BadCoincidentVertex));
        b.base.add(&mut arena, ids[0], true).unwrap();
        assert!(b.remove(&arena, ids[1]).is_err());
    }

    #[test]
    fn fresh_heal_data_is_set_up() {
        let h: HealData<H> = HealData::with_targets(&[], &[], &[]);
        assert_eq!(
            h.bad_edge_props_degen_edges.get_healer_op,
            HealerOpType::CacheEdgeprops
        );
        assert_eq!(h.non_degen_min_edge_length, BIG_DOUBLE);
        assert_eq!(h.edge_len_3d_histogram.len(), EDGELEN_BUCKET_CNT);
        assert_eq!(h.bad_face_props_missing_pole.prob_label, " - Bad ");
    }
}

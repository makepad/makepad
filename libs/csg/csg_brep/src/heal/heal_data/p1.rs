//! The healer's entry points and its stage 2 / stage 3 face, sheet, trim curve and
//! infinite region steps: the `SmHealData` members owned by unit `heal.heal_data_p1`
//! and `SmApiHealBrep`.
//!
//! Upstream: `source/SMLib/src/SmHealData.cpp`, `source/SMLib/inc/SmHealData.h`,
//! `source/SM_API/src/SmApiHeal.cpp`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`HealData::new`] (in [`super::p2`])         | `SmHealData::SmHealData()` (CO-PORTED by `heal.heal_data_p2`) |
//! | `Drop` (implicit)                            | `SmHealData::~SmHealData`                      |
//! | [`HealData::reset`]                          | `SmHealData::ReSet`                            |
//! | [`HealData::move_to_not_yet_problems`]       | `SmHealData::MoveToNotYetProblems`             |
//! | [`HealData::heal_brep`], [`HealBrepTargets`] | `SmHealData::HealBrep`                         |
//! | [`HealData::cache_face_props_stage2`]        | `SmHealData::Cache_FaceProps_Stage2`           |
//! | [`HealData::fix_bad_sheets`]                 | `SmHealData::Fix_BadSheets`                    |
//! | [`HealData::cache_face_props_stage3`]        | `SmHealData::Cache_FaceProps_Stage3`           |
//! | [`HealData::make_uv_trim_curves`]            | `SmHealData::Make_UVTrimCurves`                |
//! | [`HealData::fix_infinite_region`]            | `SmHealData::Fix_InfiniteRegion`               |
//! | [`api_heal_brep`]                            | `SmApiHealBrep`                                |
//!
//! Deviations:
//! - **Object model** as in [`super::p2`]: the props records are arena ids, the
//!   B-rep is passed to each step (`m_pBrep` is not stored), a null B-rep pointer is
//!   `None`, and the destructor is the implicit `Drop` of the arenas (`FreeTargets`
//!   and the voxel model's `SetSize(0, ..)` have nothing left to release).
//! - **Marks.** The topology marks `Fix_BadSheets` reads and writes are the caller's
//!   [`Marks`] record of the B-rep, as in [`crate::topology::topology_traverser`]; the
//!   mark locks are [`MarkLock`]s of `Kernel::context` (the caller's `rMarkLock` is
//!   passed in, the step's own `sSheetMarkLock` is claimed and released inside).
//! - **Statuses.** Upstream ignores the `SmFaceProps::SetProps` statuses of
//!   `Cache_FaceProps_Stage2` and every `CreateUVTrimCurves` failure (a `WARN`), and
//!   `Cache_FaceProps_Stage3` reacts only to `SM_ERR_NOTYET_HEAL_FACE`. Those
//!   statuses ([`SolidError::Upstream`], [`SolidError::Nlib`]) are ignored here too;
//!   [`SolidError::Unported`], [`SolidError::InvalidInput`] (a null pointer upstream
//!   dereferences) and [`SolidError::NotFound`] (a dangling id) propagate.
//!   Stage two takes `&mut Brep` to retain the gap-cache writes of its seam queries,
//!   including sibling invalidations made before an ignored error.
//! - **Props without a face.** `Cache_FaceProps_Stage2/3` call `SetProps` with the
//!   props' `m_pFace`, which is null only when `Cache_FaceProps_Gaps` has not run on
//!   them (never in the upstream heal sequence). Upstream then runs `SetProps_Gaps`
//!   on the null face and either fails the stage or dereferences it; the port
//!   returns [`SolidError::InvalidInput`] for such props in both cases.
//! - **`pbNewProbs` out-params** become the returned `bool`.
//! - **`Make_UVTrimCurves`** does not accumulate the mean and maximum gap statistics
//!   of `CreateUVTrimCurves`: upstream never reads them.
//! - **Region list.** `Fix_InfiniteRegion` walks the B-rep's circular region list
//!   from the infinite region (`GetInfiniteRegion`, `GetNext`): the port takes
//!   [`crate::topology::brep::p1::regions`] rotated to start at
//!   [`crate::topology::Brep::infinite_region`] (empty without one).
//! - **Unported callees** (`UNPORTED` stubs): `SmFace::CreateUVTrimCurves`
//!   (`topology.face_p3`), so `Make_UVTrimCurves` stops at a stub when asked to make
//!   curves. `SmBrep::FindAndSetInfiniteRegion` (`topology.brep_p5`) is wired with its
//!   declared default `SM_VS_INFINITEREGION_SWAP`, on a fresh
//!   [`crate::query::brep::BrepQuery`] of the B-rep.
//!   `Fix_BadSheets` calls `brep_p6` when it finds a sheet face between two closed
//!   shells; `HealBrep` and `SmApiHealBrep` run the heal sequence of [`super::p3`].
//! - No recursion; no allocation from caller counts beyond the target lists given.

use crate::error::{Result, SolidError};
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::heal::face_props::{
    HealerOpType, LoopObject, MixedLinkType, PropStage, SM_ERR_NOTYET_HEAL_FACE, SS_NONE,
};
use crate::kernel::Kernel;
use crate::math::BIG_DOUBLE;
use crate::topology::edge::NotifyEnv;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as face_p2;
use crate::topology::face::p3 as face_p3;
use crate::topology::topology_traverser::{closure_traversal, Entity, Marks};
use crate::topology::{Brep, EdgeId, FaceId, FaceuseId, RegionId, VertexId};

use super::p2::{FacePropsId, HealData, PropsStore, EDGELEN_BUCKET_CNT, GAPSIZE_BUCKET_CNT};

const OP: &str = "heal::heal_data::p1";

/// Drops an upstream status the caller ignores; [`SolidError::Unported`],
/// [`SolidError::InvalidInput`] (null pointers upstream dereferences) and
/// [`SolidError::NotFound`] propagate.
fn ignore_status(r: Result<()>) -> Result<()> {
    match r {
        Err(SolidError::Upstream { .. } | SolidError::Nlib { .. }) | Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `pBrep->GetVertices(sTgtVertices)`.
fn brep_vertices(b: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p1::vertices(b, None)
}

/// `pBrep->GetEdges(sTgtEdges)`.
fn brep_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(b, None)
}

/// `pBrep->GetFaces(sTgtFaces)`.
fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(b, None)
}

/// The B-rep's regions in list order from the infinite region
/// (`pBrep->GetInfiniteRegion()`, then `GetNext()` until the list wraps around):
/// the region list rotated to start at the infinite region; empty without one
/// (upstream dereferences the null pointer there).
fn brep_regions_from_infinite(b: &Brep) -> Result<Vec<RegionId>> {
    let Some(infinite) = b.infinite_region() else {
        return Ok(Vec::new());
    };
    let mut regions = crate::topology::brep::p1::regions(b, None)?;
    match regions.iter().position(|&r| r == infinite) {
        Some(at) => {
            regions.rotate_left(at);
            Ok(regions)
        }
        // an infinite region that is not on the region list: nothing to walk
        None => Ok(Vec::new()),
    }
}

/// `pBrep->FindAndSetInfiniteRegion()`: ray casts for the infinite region and moves
/// it to the head of the region list.
fn brep_find_and_set_infinite_region(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    let query = crate::query::brep::BrepQuery::of_brep(b);
    crate::topology::brep::p5::find_and_set_infinite_region(
        k,
        b,
        &query,
        None,
        // SM_VS_INFINITEREGION_SWAP: SmBrep.h's native default.
        crate::topology::brep::p1::SetRegionVoidType::InfiniteRegionSwap,
        None,
    )
}

/// `pRegion->IsVoid()` as a C++ condition (`UNSURE` is non-zero, so true).
fn region_is_void(b: &Brep, r: RegionId) -> Result<bool> {
    Ok(b.region(r)?.is_void() != 0)
}

/// `pRegion->SetIsVoid(bIsVoid)` (`TRUE` = 1, `FALSE` = 0).
fn set_region_is_void(b: &mut Brep, r: RegionId, is_void: bool) -> Result<()> {
    b.region_mut(r)?.set_is_void(i32::from(is_void));
    Ok(())
}

/// `pFace->CreateUVTrimCurves(TRUE, NULL, NULL, ..)`: makes the face's UV trim curves
/// and adjusts the edge and vertex tolerances to the measured gaps.
fn face_create_uv_trim_curves(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    // `CreateUVTrimCurves(TRUE, NULL, NULL, ..)`: the gap statistics are not read.
    face_p3::create_uv_trim_curves(k, b, f, true, None).map(|_| ())
}

/// `pBrep->HealBrep(SM_HO_ALL)` (default targets: every vertex, edge and face; no UV
/// trim curves).
fn brep_heal_brep(k: &mut Kernel, b: &mut Brep, healer_op: HealerOpType) -> Result<()> {
    crate::topology::brep::p1::heal_brep(k, b, healer_op, None, None, None, false)
}

/// The optional target lists of [`HealData::heal_brep`] (`pOptVertices`,
/// `pOptEdges`, `pOptFaces`): `None` targets every vertex, edge or face of the B-rep.
#[derive(Debug, Clone, Copy, Default)]
pub struct HealBrepTargets<'a> {
    /// `pOptVertices`.
    pub vertices: Option<&'a [VertexId]>,
    /// `pOptEdges`.
    pub edges: Option<&'a [EdgeId]>,
    /// `pOptFaces`.
    pub faces: Option<&'a [FaceId]>,
}

/// `SmApiHealBrep`: runs the whole heal sequence (`SM_HO_ALL`) on `b`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) without a B-rep; otherwise
/// those of [`HealData::heal_brep`] (`SmBrep::HealBrep` ignores every status but `Unported`).
pub fn api_heal_brep(k: &mut Kernel, b: Option<&mut Brep>) -> Result<()> {
    let Some(b) = b else {
        return Err(SolidError::invalid_input(
            "heal::heal_data::p1::api_heal_brep",
            "no B-rep",
        ));
    };
    brep_heal_brep(k, b, HealerOpType::All)?;
    // (not upstream: the port's cached topology fields, true for the caller)
    b.refresh_cached_topology();
    Ok(())
}

impl<O: LoopObject> HealData<O> {
    /// `ReSet(bReSetNewObjArrays)`: frees the target props, empties the target and
    /// problem lists, zeroes the step statistics and histograms and empties the voxel
    /// model; with `reset_new_obj_arrays` (upstream's default) also empties the lists
    /// of objects the fix steps made. The voxel counts and the vertex box are kept.
    ///
    /// # Errors
    ///
    /// Those of emptying the voxel model (none for zero voxels).
    pub fn reset(&mut self, reset_new_obj_arrays: bool) -> Result<()> {
        // Free any existing *Props before clearing arrays to prevent leaks.
        self.free_targets();

        // clear all SmProbArray<SmObjType> lists: refresh prob array links, labels,
        // and sizes
        self.setup_empty_prob_arrays();

        // m_pBrep: leave pointer alone

        // largest HealerOp value passed to HealBrep for this HealData obj
        self.stop_heal_op = HealerOpType::None;
        self.done_heal_op = HealerOpType::None;
        self.recursion_count = 0;

        // TgtObjects and TgtProps
        self.tgt_vertices.clear();
        self.tgt_edges.clear();
        self.tgt_faces.clear();

        self.tgt_vertex_props.clear();
        self.tgt_edge_props.clear();
        self.tgt_face_props.clear();

        self.old_vertex_cnt = 0;
        self.old_edge_cnt = 0;
        self.old_face_cnt = 0;

        // Step: SmHealData::Cache_EdgeProps()
        reset_histogram(&mut self.loop_gap_3d_histogram, GAPSIZE_BUCKET_CNT);
        reset_histogram(&mut self.edge_face_gap_3d_histogram, GAPSIZE_BUCKET_CNT);
        reset_histogram(&mut self.edge_len_3d_histogram, EDGELEN_BUCKET_CNT);
        self.max_edge_length = 0.0;
        self.avg_edge_length = 0.0;
        self.non_degen_min_edge_length = BIG_DOUBLE;
        self.non_degen_edge_cnt = 0;

        // Step: SmHealData::Cache_VertexProps()
        reset_histogram(&mut self.vert_edge_gap_3d_histogram, GAPSIZE_BUCKET_CNT);
        reset_histogram(&mut self.vert_face_gap_3d_histogram, GAPSIZE_BUCKET_CNT);

        // Step: Cache_CoinVertices()
        self.vertices_in_voxels.set_size(0, 0, 0, None, 0)?;

        // Step: SmHealData::Cache_CoinEdges()
        reset_histogram(&mut self.vert_vert_gap_3d_histogram, GAPSIZE_BUCKET_CNT);

        // Step: SmHealData::Fix_BadSheets()
        self.ran_region_test_closure_cnt = 0;
        self.fix_region_split_region_cnt = 0;

        // When asked - clear NewObjArray lists
        if reset_new_obj_arrays {
            self.new_faces_from_fix_degen_faces.clear();
            self.new_edges_from_split_edges_at_seams.clear();
            self.new_vertices_from_split_edges_at_seams.clear();
            self.new_vertices_on_poles_from_fix_bad_loops.clear();
            self.new_loops_on_poles_from_fix_bad_loops.clear();
            self.new_faces_from_extra_loops.clear();
            self.new_faces_from_split_face_at_seams.clear();
            self.new_edges_on_seams_from_split_face_at_seams.clear();
            self.new_vertices_from_split_face_at_seams.clear();
            self.new_regions_from_closed_sheets.clear();
            self.new_shells_from_closed_sheets.clear();
        }
        Ok(())
    }

    /// `MoveToNotYetProblems`: takes `props` (and its face) off the target and
    /// problem lists and onto the not-yet list, which then owns it.
    ///
    /// # Errors
    ///
    /// Those of [`HealData::remove_face_props_from_lists`] and of the not-yet list's
    /// `Add` (a dangling id).
    pub fn move_to_not_yet_problems(&mut self, props: FacePropsId) -> Result<()> {
        // remove Tgt FaceProps Ptr from all the various Heal FaceProps Ptr lists
        // (includes m_sNotYetFaceProps)
        self.remove_face_props_from_lists(props)?;

        // Add FaceProps to NotYetFaceProps List - memory ownership moves with this
        // pointer
        self.not_yet_face_props
            .add(&mut self.face_props, props, true)?;
        Ok(())
    }

    /// `HealBrep`: targets the given (or every) vertex, edge and face of `b`, resets
    /// the healer, loads the targets and runs the heal sequence through `healer_op`
    /// (`HealTgts`). Without a B-rep there is nothing to do.
    ///
    /// # Errors
    ///
    /// Those of the B-rep list queries ([`crate::topology::brep::p1`]), of
    /// [`HealData::reset`] and of [`HealData::heal_targets`].
    pub fn heal_brep(
        &mut self,
        k: &mut Kernel,
        b: Option<&mut Brep>,
        healer_op: HealerOpType,
        targets: HealBrepTargets<'_>,
        make_uv_trim_curves: bool,
    ) -> Result<()> {
        // no work - no Brep
        let Some(b) = b else {
            return Ok(());
        };

        // TgtVertices - When pOptVertices is NULL Tgt all Vertices
        let tgt_vertices = match targets.vertices {
            None => brep_vertices(b)?,
            Some(v) => v.to_vec(),
        };

        // TgtEdges - When pOptEdges is NULL Tgt all Edges
        let tgt_edges = match targets.edges {
            None => brep_edges(b)?,
            Some(e) => e.to_vec(),
        };

        // TgtFaces - When pOptFaces is NULL Tgt all Faces
        let tgt_faces = match targets.faces {
            None => brep_faces(b)?,
            Some(f) => f.to_vec(),
        };

        // clear this HealData object
        self.reset(true)?;

        // Gather Tgt Arrays - allocate TgtProps memory
        self.load_targets(&tgt_vertices, &tgt_edges, &tgt_faces);

        // Heal the tgts in m_sTgtObjs and m_sTgtObjProps
        self.heal_targets(k, b, healer_op, make_uv_trim_curves)
    }

    /// `Cache_FaceProps_Stage2`: sets the stage 2 props (surface, sheet, closure,
    /// poles, flat corners, seams) of the (new) target faces that lack them and lists
    /// the sheets, the faces with poles and the flat corner and seam problems.
    /// Returns whether new problems were found.
    /// Lazy seam-query gap writes and invalidations are retained in `b`, including
    /// those made before an ignored stage-two error.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for props without a face (see the module
    /// deviations) or where upstream dereferences null; [`SolidError::Unported`] from
    /// the stage 2 props (`SmFace::HasSeamProblem` for a closed surface).
    pub fn cache_face_props_stage2(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        _this_healer_op: HealerOpType,
    ) -> Result<bool> {
        // init output
        let mut new_probs = false;

        // no work - no Faces or no new Faces
        let n = self.tgt_face_props.len();
        if n == 0 || n == self.old_face_cnt {
            return Ok(false);
        }

        // for every FaceProp obj - Get FaceProperties
        for ii in self.old_face_cnt..n {
            let id = tgt(&self.tgt_face_props, ii)?;
            let done_heal_op = self.done_heal_op;
            let entry = self.face_props.props_mut(id)?;
            let p = &mut entry.props;

            // set FaceProps_Stage2 - (possibly done when pOptFaceProps != NULL)
            if !p.has_props(PropStage::Stage2, p.face) {
                let Some(face) = p.face else {
                    return Err(no_face());
                };
                ignore_status(p.set_props_with_query_gaps(
                    k,
                    b,
                    PropStage::Stage2,
                    face,
                    ii as u64,
                    done_heal_op,
                ))?;
            }

            // count the kinds of faces that force healing checks later on
            let (sheet, poles_link, flat, missing, crossed_pole, crossed, near_miss) = (
                p.sheet,
                p.poles_link,
                p.bad_flat_corners,
                p.bad_missing_seam,
                p.bad_crossed_pole,
                p.bad_crossed_seam,
                p.bad_near_miss_seam,
            );

            // SheetFace List
            if sheet != MixedLinkType::NoProp {
                self.face_props_sheets
                    .add(&mut self.face_props, id, sheet)?;
            }

            // NoVertex_Poles Face List - not a problem when pole is not 'in' or 'on'
            // Face
            if poles_link != MixedLinkType::NoProp {
                self.face_props_no_vertex_pole
                    .add(&mut self.face_props, id, poles_link)?;
            }

            // FlatCorners Face List
            if flat != 0 {
                self.bad_face_props_flat_corner
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }

            // Missing Seam List
            if missing.code() != 0 {
                self.bad_face_props_missing_seam
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }

            // Crossed Pole List
            if crossed_pole.code() != 0 {
                self.bad_face_props_crossed_pole
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }

            // Crossed Seam List
            if crossed.code() != 0 {
                self.bad_face_props_crossed_seam
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }

            // Near Miss Seam List
            if near_miss.code() != 0 {
                self.bad_face_props_near_miss_seam
                    .add(&mut self.face_props, id, true)?;
                new_probs = true;
            }
        }

        // all done
        Ok(new_probs)
    }

    /// `Fix_BadSheets`: for each listed sheet face whose two face uses lie in the
    /// same closed shell without sharing a face use, splits the region on either side
    /// (`SmBrep::TestClosureSplitRegion`), records the new shells and regions and
    /// marks the face's sheet property okay or fixed. `mark_lock` is the caller's
    /// claimed mark (`rMarkLock`), `marks` the B-rep's topology marks, `env` the
    /// notification state the region split notifies through (`SmBrep::Notify`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream dereferences null (a listed props
    /// without a face, a face without face uses); those of the closure traversals and
    /// the mark lock; [`SolidError::Unported`] from the region-splitting dependencies
    /// (`query.brep`).
    pub fn fix_bad_sheets(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        env: &mut NotifyEnv<'_>,
        mark_lock: &MarkLock,
        _this_healer_op: HealerOpType,
    ) -> Result<()> {
        // Heal_2: Find and Split Shells with SheetFaces that link two closed Shells
        // together by mistake
        if !self.face_props_sheets.base.is_empty() {
            // locals
            let mut sheet_lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
            let r = self.fix_bad_sheets_with(k, b, marks, env, mark_lock, &sheet_lock);
            sheet_lock.release(std::slice::from_mut(&mut k.context))?;
            r?;
        }

        // all done
        Ok(())
    }

    /// The sheet loop of [`HealData::fix_bad_sheets`] under the claimed
    /// `sheet_lock`.
    fn fix_bad_sheets_with(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        env: &mut NotifyEnv<'_>,
        mark_lock: &MarkLock,
        sheet_lock: &MarkLock,
    ) -> Result<()> {
        let sheet_mark_type = sheet_lock.mark_type();
        let mut up_faceuses: Vec<FaceuseId> = Vec::new();
        let mut down_faceuses: Vec<FaceuseId> = Vec::new();

        // check every SheetFace to see if a SheetFace is really a boundary between two
        // shells (in linear time - use marks)
        let mut ii = 0;
        while ii < self.face_props_sheets.base.len() {
            let id = tgt(&self.face_props_sheets.base.prob_array, ii)?;
            ii += 1;
            let Some(loop_face) = self.face_props.props(id)?.props.face else {
                return Err(no_face());
            };
            let (up, down) = face_p2::faceuses(b, loop_face)?;
            let (Some(up_faceuse), Some(down_faceuse)) = (up, down) else {
                return Err(SolidError::invalid_input(OP, "face without face uses"));
            };

            // skip faces (after counting them) whose faceuses are already marked
            if marks.is_marked(k, Entity::Faceuse(up_faceuse), sheet_mark_type)
                || marks.is_marked(k, Entity::Faceuse(down_faceuse), sheet_mark_type)
            {
                // mark this face as a bad FaceSheet
                let sheet = sheet_value(b, up_faceuse, down_faceuse)?;
                self.face_props.props_mut(id)?.props.sheet = sheet;
                continue;
            }

            // ClosureTraversal has no native error return on a consistent
            // graph. Its Rust link/lock refusals must still propagate.
            // Check up and down faceuse shell closures.
            let up_closed =
                closure_traversal(k, b, marks, up_faceuse, mark_lock, Some(&mut up_faceuses))?;
            let down_closed = closure_traversal(
                k,
                b,
                marks,
                down_faceuse,
                mark_lock,
                Some(&mut down_faceuses),
            )?;

            // locals
            let up_shell = eu1::faceuse_shell(b, up_faceuse)?;
            let down_shell = eu1::faceuse_shell(b, down_faceuse)?;

            // iter UpFaceuses looking for any faces also in DownFaceuses (in linear
            // time)
            let mark_type = mark_lock.mark_type();
            let has_common_elements = up_faceuses
                .iter()
                .any(|&fu| marks.is_marked(k, Entity::Faceuse(fu), mark_type));

            // when a SheetFace is actually a boundary between different shells (a
            // database error to fix) - fix it: make sure the Shells and Regions are
            // properly sorted
            if up_closed && down_closed && !has_common_elements && up_shell == down_shell {
                // fix - Split the Region on either side of the BadSheet into two
                // different regions - increments mark again
                let split = crate::topology::brep::p6::test_closure_split_region_attempt(
                    k, b, marks, env, true, up_faceuse,
                )?;
                // Bare call in SmHealData.cpp:4832: ignore the native SmStatus
                // and retain its partial outputs. The outer Result still
                // propagates Unported and Rust topology/resolver refusals.
                let (new_shell, new_region) = (split.shell, split.region);

                // count the TestClosureSplitRegion runs
                self.ran_region_test_closure_cnt += 1;

                // Mark the FaceProp Sheet problem
                let sheet = sheet_value(b, up_faceuse, down_faceuse)?;
                self.face_props.props_mut(id)?.props.sheet = sheet;

                // mark the Up and Down faceuses as already classified so that they can
                // be skipped in the next iteration
                for &fu in &up_faceuses {
                    marks.mark(k, Entity::Faceuse(fu), sheet_mark_type);
                }
                for &fu in &down_faceuses {
                    marks.mark(k, Entity::Faceuse(fu), sheet_mark_type);
                }

                // count the number of fixes due to the TestClosureSplitRegion runs
                if let Some(s) = new_shell {
                    self.fix_region_split_region_cnt += 1;
                    self.new_shells_from_closed_sheets.push(s);
                }
                if let Some(r) = new_region {
                    self.new_regions_from_closed_sheets.push(r);
                }
            }
        }
        Ok(())
    }

    /// `Cache_FaceProps_Stage3`: sets the stage 3 (loop) props of the (new) target
    /// faces that lack them, last face first (skipping a face missing a seam whose
    /// closed surface has only vertex loops: it stays active for seam insertion),
    /// moves the faces with a not-yet supported problem to the not-yet list and lists
    /// the loop problems. Returns whether new problems were found (always `false`, as
    /// upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for props without a face (see the module
    /// deviations) or where upstream dereferences null; [`SolidError::Unported`] from
    /// the stage 3 props (`SmFace::ClassifyLoops`).
    pub fn cache_face_props_stage3(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        _this_healer_op: HealerOpType,
    ) -> Result<bool> {
        // init output
        let new_probs = false;

        // no work - no Faces or no new Faces
        let n = self.tgt_face_props.len();
        if n == 0 || n == self.old_face_cnt {
            return Ok(new_probs);
        }

        // Get AfterSplitEdges FaceProperties - run iter backwards because a face might
        // be removed from list
        let mut ii = n;
        while ii > self.old_face_cnt {
            let id = tgt(&self.tgt_face_props, ii - 1)?;
            ii -= 1;
            let done_heal_op = self.done_heal_op;
            let p = &mut self.face_props.props_mut(id)?.props;

            // Vertex loops alone cannot form a loop tree. Keep this face active for
            // seam insertion; its completed edge boundary can then be classified.
            if p.bad_missing_seam.is_true() {
                let Some(face) = p.face else {
                    return Err(no_face());
                };
                if crate::topology::face::p3::is_closed_surface_with_vertex_loops(
                    k,
                    b,
                    face,
                    Some(&*p),
                )? {
                    continue;
                }
            }

            // when needed - compute FaceProps After Fix_SplitEdgesAtSeam() -
            // SM_PROPSTAGE_3
            if !p.has_props(PropStage::Stage3, p.face) {
                let Some(face) = p.face else {
                    return Err(no_face());
                };
                let face_index = p.face_index;
                let rtn = p.set_props_with_query_gaps(
                    k,
                    b,
                    PropStage::Stage3,
                    face,
                    face_index,
                    done_heal_op,
                );

                match rtn {
                    // When Face has NotYet supported problems
                    Err(SolidError::Upstream { code, .. }) if code == SM_ERR_NOTYET_HEAL_FACE => {
                        // isolate the face from the heal sequence
                        self.move_to_not_yet_problems(id)?;

                        // skip any further processing on this FaceProps
                        continue;
                    }
                    r => ignore_status(r)?,
                }
            }

            // count the kinds of faces that force healing checks later on
            let p = &self.face_props.props(id)?.props;
            let outer_loop_order = p.bad_outer_loop_order.code() > 0;
            let orient = p.bad_orient_loop_cnt > 0;
            let no_area = p.bad_no_area_loop_cnt > 0;
            let multi_outer = p.outer_loop_cnt > 1;
            let nested = p.bad_nested_loop_cnt > 0;
            let missing_pole = p.bad_missing_poles != SS_NONE;
            let unpaired = p.bad_unpaired_no_area_loop.is_true();
            let closed_3d = p.bad_closed_3d_loop_cnt > 0;
            let closed_ptr = p.bad_closed_ptr_loop_cnt > 0;
            let mut has_loop_prob = false;
            let fp = &mut self.face_props;

            // BadOuterLoopOrder Face List
            if outer_loop_order {
                self.bad_face_props_outer_loop_order.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadOrient_Loop Face List
            if orient {
                self.bad_face_props_orient_loop.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadNoArea_Loop Face List
            if no_area {
                self.bad_face_props_no_area_loop.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadMultiOuterLoop and/or BadNested_Loop Face List
            if multi_outer {
                self.bad_face_props_multi_outer_loops.add(fp, id, true)?;
                has_loop_prob = true;
            }

            if nested {
                self.bad_face_props_nested_outer_loops.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadMissingPole Face List
            if missing_pole {
                self.bad_face_props_missing_pole.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadUnpairedNoArea_Loop Face List
            if unpaired {
                self.bad_face_props_unpaired_no_area_loop
                    .add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadClosed3d_Loop - EdgeEnd/EdgeEnd Gaps too big
            if closed_3d {
                self.bad_face_props_closed_3d_loop.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // BadClosedPtr_Loop - Edgeuse->CCW Linked Ptr list is not closed when all
            // EdgeEnd/EdgeEnd gaps are within tol
            if closed_ptr {
                self.bad_face_props_closed_ptr_loop.add(fp, id, true)?;
                has_loop_prob = true;
            }

            // accumulate list of All FaceProps with Any LoopProblems
            if has_loop_prob {
                self.bad_face_props_loop_problems.add(fp, id, true)?;
            }
        }

        // all done
        Ok(new_probs)
    }

    /// `Make_UVTrimCurves`: with `make_uv_trim_curves`, makes the UV trim curves of
    /// every (new) target face and adjusts the edge and vertex tolerances to the
    /// measured gaps (a failure on one face is only a warning upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a target props without a face (upstream
    /// dereferences null); those of `SmFace::CreateUVTrimCurves`
    /// (`topology::face::p3::create_uv_trim_curves`) except `SmStatus` failures, which are ignored.
    pub fn make_uv_trim_curves(
        &self,
        k: &Kernel,
        b: &mut Brep,
        make_uv_trim_curves: bool,
        _this_healer_op: HealerOpType,
    ) -> Result<()> {
        // no work - no Faces or no new Faces
        let n = self.tgt_face_props.len();
        if n == 0 || n == self.old_face_cnt {
            return Ok(());
        }

        // When asked - Create UVTrimCurves and set tolerances
        if make_uv_trim_curves {
            // for every face
            for ii in self.old_face_cnt..n {
                let id = tgt(&self.tgt_face_props, ii)?;
                let Some(loop_face) = self.face_props.props(id)?.props.face else {
                    return Err(no_face());
                };

                // given previous healing, expect all CreateUVTrimCurve calls to
                // succeed (a failure is a WARN upstream)
                ignore_status(face_create_uv_trim_curves(k, b, loop_face))?;
            }
        }

        // all done
        Ok(())
    }

    /// `Fix_InfiniteRegion`: a B-rep with one region (a sheet model) gets a void
    /// region; with two, ray casting finds the infinite region, which is set void and
    /// the other solid; with more, a non-void infinite region is fixed by ray casting
    /// (the other regions are left).
    ///
    /// # Errors
    ///
    /// Those of `SmBrep::FindAndSetInfiniteRegion` (`topology.brep_p5`);
    /// [`SolidError::InvalidInput`] for a B-rep without regions (upstream dereferences
    /// null).
    pub fn fix_infinite_region(
        &self,
        k: &mut Kernel,
        b: &mut Brep,
        _this_healer_op: HealerOpType,
    ) -> Result<()> {
        // locals
        let regions = brep_regions_from_infinite(b)?;
        let Some(&infinite_region) = regions.first() else {
            return Err(SolidError::invalid_input(OP, "B-rep without regions"));
        };

        match regions.len() {
            // case: Sheet models - 1 Void Region
            1 => set_region_is_void(b, infinite_region, true)?,

            // case: 2 regions - 1 Void InfiniteRegion, 1 solid Region
            2 => {
                // Plausible void flags do not establish which region is geometrically
                // infinite.
                brep_find_and_set_infinite_region(k, b)?;

                // Refresh region pointers after classification, then set Void/Solid data.
                let regions = brep_regions_from_infinite(b)?;
                let infinite_region = tgt(&regions, 0)?;
                let next_region = tgt(&regions, 1)?;
                set_region_is_void(b, infinite_region, true)?;
                set_region_is_void(b, next_region, false)?;
            }

            // case: more than 2 regions
            _ => {
                // only check obviously broken Regions
                if !region_is_void(b, infinite_region)? {
                    // use ray casting to find and set the InfiniteRegion
                    brep_find_and_set_infinite_region(k, b)?;

                    // Set infinite region Void/Solid data (on the region found before
                    // the ray casting, as upstream)
                    set_region_is_void(b, infinite_region, true)?;
                }
            }
        }

        // all done
        Ok(())
    }
}

/// `pUpFaceuse->GetRegion() == pDownFaceuse->GetRegion() ? SM_MP_OKAY : SM_MP_FIXED`.
fn sheet_value(b: &Brep, up: FaceuseId, down: FaceuseId) -> Result<MixedLinkType> {
    Ok(
        if eu1::faceuse_region(b, up)? == eu1::faceuse_region(b, down)? {
            MixedLinkType::Okay
        } else {
            MixedLinkType::Fixed
        },
    )
}

/// Resizes a histogram to `n` buckets and zeroes it (`SetSize(n)`, `SetAll(0)`).
fn reset_histogram(h: &mut Vec<u64>, n: usize) {
    h.clear();
    h.resize(n, 0);
}

/// The element `i` of a list, or [`SolidError::InvalidInput`] (upstream reads an
/// `SmTArray` past its end).
fn tgt<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP, "list read past its end"))
}

/// The error for face props without a face where upstream would use it.
fn no_face() -> SolidError {
    SolidError::invalid_input(OP, "face props without a face")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intersect::point_class::{ClassObject, ClassObjectKind};
    use crate::topology::{Face, LoopId, ShellId};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H;
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Loop
        }
    }
    impl LoopObject for H {
        fn from_loop(_l: LoopId) -> Self {
            Self
        }
    }

    fn ids() -> (Vec<VertexId>, Vec<EdgeId>, Vec<FaceId>) {
        (
            (0..3).map(VertexId::from_index).collect(),
            (0..2).map(EdgeId::from_index).collect(),
            (0..4).map(FaceId::from_index).collect(),
        )
    }

    #[test]
    fn reset_frees_targets_and_restores_the_fresh_state() {
        let (v, e, f) = ids();
        let mut h: HealData<H> = HealData::with_targets(&v, &e, &f);
        h.done_heal_op = HealerOpType::FixBadsheets;
        h.old_face_cnt = 2;
        h.max_edge_length = 3.0;
        h.non_degen_edge_cnt = 7;
        h.edge_len_3d_histogram[3] = 5;
        h.ran_region_test_closure_cnt = 4;
        h.new_faces_from_extra_loops.push(FaceId::from_index(1));
        h.x_cnt = 3;
        h.reset(false).unwrap();
        assert!(h.tgt_vertices.is_empty() && h.tgt_face_props.is_empty());
        assert_eq!(h.face_props.alive().count(), 0);
        assert_eq!(h.done_heal_op, HealerOpType::None);
        assert_eq!(h.old_face_cnt, 0);
        assert_eq!(h.max_edge_length, 0.0);
        assert_eq!(h.non_degen_min_edge_length, BIG_DOUBLE);
        assert_eq!(h.non_degen_edge_cnt, 0);
        assert!(h.edge_len_3d_histogram.iter().all(|&x| x == 0));
        assert_eq!(h.edge_len_3d_histogram.len(), EDGELEN_BUCKET_CNT);
        assert_eq!(h.ran_region_test_closure_cnt, 0);
        // kept without the flag, as are the voxel counts
        assert_eq!(h.new_faces_from_extra_loops.len(), 1);
        assert_eq!(h.x_cnt, 3);
        h.reset(true).unwrap();
        assert!(h.new_faces_from_extra_loops.is_empty());
    }

    #[test]
    fn move_to_not_yet_takes_props_off_the_targets() {
        let (v, e, f) = ids();
        let mut h: HealData<H> = HealData::with_targets(&v, &e, &f);
        let id = h.tgt_face_props[1];
        h.face_props.props_mut(id).unwrap().props.face = Some(FaceId::from_index(1));
        h.move_to_not_yet_problems(id).unwrap();
        assert_eq!(h.tgt_face_props.len(), 3);
        assert!(!h.tgt_face_props.contains(&id));
        assert_eq!(h.tgt_faces.len(), 3);
        assert_eq!(h.not_yet_face_props.prob_array, vec![id]);
        assert!(h
            .face_props
            .props(id)
            .unwrap()
            .props
            .bad_has_not_yet_prob
            .is_true());
        // the not-yet list owns it: freeing the targets frees it
        h.free_targets();
        assert!(h.face_props.get(id).is_none());
    }

    #[test]
    fn stage_steps_without_new_faces_do_nothing() {
        let (v, e, f) = ids();
        let k = Kernel::default();
        let mut b = Brep::default();
        let mut h: HealData<H> = HealData::with_targets(&v, &e, &f);
        h.old_face_cnt = 4;
        assert!(!h
            .cache_face_props_stage2(&k, &mut b, HealerOpType::CacheFaceprops2)
            .unwrap());
        assert!(!h
            .cache_face_props_stage3(&k, &mut b, HealerOpType::CacheFaceprops3)
            .unwrap());
        let mut b = Brep::default();
        h.make_uv_trim_curves(&k, &mut b, true, HealerOpType::MakeUvtrimcurves)
            .unwrap();
    }

    #[test]
    fn stage2_needs_the_face_set_by_the_gap_step() {
        let (v, e, f) = ids();
        let k = Kernel::default();
        let mut b = Brep::default();
        let mut h: HealData<H> = HealData::with_targets(&v, &e, &f);
        let r = h.cache_face_props_stage2(&k, &mut b, HealerOpType::CacheFaceprops2);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        let r = h.cache_face_props_stage3(&k, &mut b, HealerOpType::CacheFaceprops3);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn make_uv_trim_curves_only_when_asked() {
        let (v, e, f) = ids();
        let k = Kernel::default();
        let mut b = Brep::default();
        let mut h: HealData<H> = HealData::with_targets(&v, &e, &f);
        h.make_uv_trim_curves(&k, &mut b, false, HealerOpType::MakeUvtrimcurves)
            .unwrap();
        for (i, &id) in h.tgt_face_props.clone().iter().enumerate() {
            h.face_props.props_mut(id).unwrap().props.face = Some(FaceId::from_index(i as u32));
        }
        // `CreateUVTrimCurves` is called for every target face; a dangling face id
        // propagates (only the SmStatus failures are ignored)
        let r = h.make_uv_trim_curves(&k, &mut b, true, HealerOpType::MakeUvtrimcurves);
        assert!(matches!(r, Err(SolidError::NotFound { .. })), "{r:?}");
    }

    #[test]
    fn heal_brep_and_api_entry_points() {
        let mut k = Kernel::default();
        let mut h: HealData<H> = HealData::new();
        h.heal_brep(
            &mut k,
            None,
            HealerOpType::All,
            HealBrepTargets::default(),
            false,
        )
        .unwrap();
        assert!(matches!(
            api_heal_brep(&mut k, None),
            Err(SolidError::InvalidInput { .. })
        ));
        // on a B-rep the whole-B-rep target lists are gathered (its one face) and the
        // heal sequence of heal_data_p3 runs past Fix_TolSizes (SmBrep::SetTolerance
        // raises the face's tolerance through SmFace::SetTolerance) past
        // Cache_DegenFaces (SmFace::GetEdges, topology.face_p4) and the SmLockBrepBox
        // (query.brep) up to the SmStitch of heal.stitch_p1 (SmBrep::HealBrep passes
        // Unported through, ignoring every other status)
        let mut b = Brep::default();
        let face = b
            .add_face_record(Face::new(ShellId::from_index(0)))
            .unwrap();
        assert!(api_heal_brep(&mut k, Some(&mut b)).is_ok());
        let r = h.heal_brep(
            &mut k,
            Some(&mut b),
            HealerOpType::All,
            HealBrepTargets::default(),
            false,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        assert_eq!(b.face(face).unwrap().tolerance, 1e-5);
        assert_eq!(h.tgt_faces, vec![face]);
        let (v, e, f) = ids();
        let t = HealBrepTargets {
            vertices: Some(&v),
            edges: Some(&e),
            faces: Some(&f),
        };
        let r = h.heal_brep(&mut k, Some(&mut b), HealerOpType::All, t, false);
        // The sequence now runs: these synthetic target ids do not exist in b.
        assert!(matches!(r, Err(SolidError::NotFound { .. })));
        assert_eq!(h.done_heal_op, HealerOpType::None);
        // the targets were loaded before the heal sequence
        assert_eq!(h.tgt_faces, f);
        assert_eq!(h.tgt_face_props.len(), 4);
    }

    #[test]
    fn fix_bad_sheets_without_sheets_does_nothing() {
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let mut h: HealData<H> = HealData::new();
        let mut marks = Marks::default();
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        let mut state = crate::topology::brep::p2::RegionNotifyState::new(&b);
        h.fix_bad_sheets(
            &mut k,
            &mut b,
            &mut marks,
            &mut state.env(),
            &lock,
            HealerOpType::FixBadsheets,
        )
        .unwrap();
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
        // a B-rep without regions: upstream dereferences the null infinite region
        let r = h.fix_infinite_region(&mut k, &mut b, HealerOpType::FixInfiniteRegions);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }
}

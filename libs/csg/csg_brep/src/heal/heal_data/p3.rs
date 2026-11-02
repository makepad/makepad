//! Healer target updates, ordered healing and coincidence, degeneracy and seam repairs.
//!
//! Upstream: `source/SMLib/src/SmHealData.cpp`, `source/SMLib/src/SmBrep.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`HealData::update_target_lists`] | `SmHealData::UpdateTargetLists(array overload)` |
//! | [`HealData::update_tracked_targets`] | `SmHealData::UpdateTargetLists(tracker overload)` |
//! | [`HealData::heal_targets`] | `SmHealData::HealTgts` |
//! | [`HealData::fix_coin_vertices`] | `SmHealData::Fix_CoinVertices` |
//! | [`HealData::fix_degen_faces`] | `SmHealData::Fix_DegenFaces` |
//! | [`HealData::fix_degen_edges`] | `SmHealData::Fix_DegenEdges` |
//! | [`HealData::fix_move_seam`] | `SmHealData::Fix_MoveSeam` |
//! | [`HealData::fix_split_edges_at_seam`] | `SmHealData::Fix_SplitEdgesAtSeam` |
//! | [`HealData::fix_split_face_at_seams`] | `SmHealData::Fix_SplitFaceAtSeams` |
//! | calls `topology::brep::p1::is_live_topology_member` (unit `topology.brep_p1`; it replaced the private co-port `live`) | `SmBrep::IsLiveTopologyMember` |
//!
//! Deviations: topology and props are arena ids as in [`super::p2`]. The B-rep and
//! kernel are explicit arguments. Tracker object keys are resolved by the caller,
//! because `SmTopology` notifications use caller-chosen keys. `None` add entries
//! represent null tracked pointers; stale additions are skipped by arena identity.
//! `heal_targets`, `fix_coin_vertices`, `fix_degen_faces` and `fix_degen_edges`
//! temporarily enable `Brep::editing_enabled` (`SmTemporaryChangeValue(m_bEditingEnabled,
//! TRUE)`) and restore it on every exit.
//! Mutating callees not yet ported return `Unported`, including when C++ ignores
//! their status; no successful mutation is simulated. Removed/added identities
//! for p5 edits are captured by the actual context callback (`capture_changes`).
//! Nonempty captures still require the owning notification adapter's key-to-id map;
//! they defer rather than guessing identities from arena indices. The ported p3/p4/face edits
//! (`SqueezeEdge`, `DeleteEdge`, `DeleteVertex`, `MakeVertexSplitEdge`, `DeleteFace`, `CreateFaceInRegionFromSurface`, `MoveSeam`,
//! `SplitAtSeam` and `Fix_SplitEdgesAtSeam`'s vertex-only `MergeIntervals`) run under a
//! real `SmTrackTopologyChanges` on a context of their own (`tracked_edit`), one per edit
//! (upstream keeps one per heal loop; the lists merge to the same result with `Changes::append`); the kernel
//! context's callbacks and identity domain are lent to that context, and the edits
//! name every object by its kernel-issued identity (`topology::brep::p3`'s
//! Deviations), which the tracker's lists are decoded by. It has no geometry-owner
//! metadata, so a geometry owner walk is a capture failure that leaves only the Chg
//! list short, which `Changes` does not read. Recursive target catch-up is limited to 32 nested updates (upstream
//! has no bound); temporary counters and healer state are restored on every exit.
//! Errors from nested `HealTgts` propagate after the healer state is restored.
//! Bare/SE mutation statuses are
//! ignored except `Unported`; failed calls with no output skip uses of that output.
//! `Fix_BadLoops` stays disabled. Classification handles of the seam splits
//! (`FindEdgeObjects`, `MergeIntervals` outputs, `SplitAtSeam`'s vertex lookups) name
//! B-rep topology through `LoopObject::as_topology`; a handle that names none is
//! `InvalidInput`.
//! Partial lists from completed merge intervals survive ignored failures.
//! `Fix_SplitEdgesAtSeam` does not format the diagnostic of a pole split refused at
//! an edge end (`SE_MSG`); another refused split returns `SM_ERR`.

use crate::error::{ignored as ignore_soft, EntityRef, Result, SolidError, SM_ERR};
use crate::geom::attribute::ObjectKey;
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::heal::face_props::{HealerOpType, LoopObject};
use crate::kernel::Kernel;
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p4;
use crate::topology::topology::TrackTopologyChanges;
use crate::topology::topology_traverser::{Entity, Marks};
use crate::topology::{edge, Brep, EdgeId, FaceId, VertexId};

use super::p2::{EdgeProps, FacePropsEntry, FacePropsId, HealData, PropsStore, VertexProps};

use crate::curve::curve::Curve;
use crate::geom::object::ObjectRef;
use crate::heal::face_props::{FaceProps, PropStage, TriedLinkType};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::ClassGeometry;
use crate::math::{Point3, EFF_ZERO_PARAM};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::edge::NotifyEnv;
use crate::topology::face::p1::{self as fp1, SeamProblem};
use crate::topology::vertex;

const OP: &str = "heal::heal_data::p3";
/// Maximum number of nested `UpdateTargetLists` catch-up passes.
pub const MAX_TARGET_UPDATE_DEPTH: u64 = 32;

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `SmTemporaryChangeValue<SmBoolean>(pBrep->m_bEditingEnabled, TRUE)`.
fn with_editing_enabled<T>(b: &mut Brep, op: impl FnOnce(&mut Brep) -> Result<T>) -> Result<T> {
    let saved = b.editing_enabled();
    b.set_editing_enabled(true);
    let r = op(b);
    b.set_editing_enabled(saved);
    r
}

/// Per-operation topology notifications, in the same order as the native tracker.
#[derive(Default)]
struct Changes {
    removed: Vec<EntityRef>,
    added: Vec<Option<EntityRef>>,
}

impl Changes {
    fn append(&mut self, other: Self) {
        // Native notification lists use AddUnique; a later removal also
        // cancels an earlier add. A survivor re-added by a merge goes last.
        for e in other.removed {
            if !self.removed.contains(&e) {
                self.removed.push(e);
            }
            self.added.retain(|x| *x != Some(e));
        }
        for e in other.added {
            if !self.added.contains(&e) {
                self.added.push(e);
            }
        }
    }
}

impl<O: LoopObject> HealData<O> {
    /// Applies a stopped topology tracker's remove/add lists. `resolve` maps its
    /// opaque keys to typed ids, including ids of removed objects; an unresolved
    /// remove entry cannot match a target, an unresolved add is skipped.
    /// Returns `SM_ERR` while that tracker is active.
    pub fn update_tracked_targets(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        tracker: &TrackTopologyChanges,
        resolve: impl Fn(ObjectKey) -> Option<EntityRef>,
        through: HealerOpType,
    ) -> Result<()> {
        if tracker.is_tracking(Some(&k.context)) {
            return Err(SolidError::Upstream {
                operation: OP,
                code: SM_ERR,
                entity: None,
            });
        }
        if let Some(error) = tracker.failure() {
            return Err(error);
        }
        let removed: Vec<_> = tracker
            .rm_list()
            .iter()
            .filter_map(|&x| resolve(x))
            .collect();
        let added: Vec<_> = tracker.add_list().iter().map(|&x| resolve(x)).collect();
        self.update_target_lists(k, b, &removed, &added, through)
    }

    /// Removes stale targets and their props from every problem list, adds live
    /// new targets and heals them through `min(through, done_heal_op)`, returning the
    /// nested healing's error.
    /// The temporary old counts, recursion count and stop/done state are restored
    /// even when nested healing returns an error.
    pub fn update_target_lists(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        removed: &[EntityRef],
        added: &[Option<EntityRef>],
        through: HealerOpType,
    ) -> Result<()> {
        // Resolve every stale props identity before removing any of them.
        let faces = self.face_props_for_tgts(removed, true);
        let edges = self.edge_props_for_tgts(removed, true);
        let vertices = self.vertex_props_for_tgts(removed, true);
        for id in faces {
            self.remove_face_props_from_lists(id)?;
            self.face_props.free(id);
        }
        for id in edges {
            self.remove_edge_props_from_lists(id)?;
            self.edge_props.free(id);
        }
        for id in vertices {
            self.remove_vertex_props_from_lists(id)?;
            self.vertex_props.free(id);
        }
        if added.is_empty() {
            return Ok(());
        }
        if self.recursion_count >= MAX_TARGET_UPDATE_DEPTH {
            return Err(invalid("target-update recursion exceeds 32 nested passes"));
        }
        let saved_counts = (self.old_face_cnt, self.old_edge_cnt, self.old_vertex_cnt);
        self.old_face_cnt = self.tgt_faces.len();
        self.old_edge_cnt = self.tgt_edges.len();
        self.old_vertex_cnt = self.tgt_vertices.len();
        for &e in added.iter().flatten() {
            if !bp1::is_live_topology_member(b, Some(e)) {
                continue;
            }
            match e {
                EntityRef::Face(f) => {
                    self.tgt_faces.push(f);
                    self.tgt_face_props
                        .push(self.face_props.alloc(FacePropsEntry::new()));
                }
                EntityRef::Edge(e) => {
                    self.tgt_edges.push(e);
                    self.tgt_edge_props
                        .push(self.edge_props.alloc(EdgeProps::new()));
                }
                EntityRef::Vertex(v) => {
                    self.tgt_vertices.push(v);
                    self.tgt_vertex_props
                        .push(self.vertex_props.alloc(VertexProps::new()));
                }
                _ => (),
            }
        }
        let saved_state = (self.stop_heal_op, self.done_heal_op);
        let through = if through.code() > self.done_heal_op.code() {
            self.done_heal_op
        } else {
            through
        };
        self.stop_heal_op = through;
        self.done_heal_op = HealerOpType::None;
        self.recursion_count += 1;
        let result = self.heal_targets(k, b, through, false);
        if self.recursion_count > 0 {
            self.recursion_count -= 1;
        }
        (self.stop_heal_op, self.done_heal_op) = saved_state;
        (self.old_face_cnt, self.old_edge_cnt, self.old_vertex_cnt) = saved_counts;
        result
    }

    /// Runs the native heal sequence through `healer_op`, resetting stop/done state
    /// on entry. Step 1 (`Fix_BackPointers`) and the disabled `Fix_BadLoops` run no
    /// repair: requesting either records it as done and stops there.
    /// `make_uv_trim_curves` corresponds to the upstream default `false`.
    pub fn heal_targets(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        healer_op: HealerOpType,
        make_uv_trim_curves: bool,
    ) -> Result<()> {
        self.stop_heal_op = healer_op;
        self.done_heal_op = HealerOpType::None;
        with_editing_enabled(b, |b| {
            let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
            let mut marks = Marks::default();
            let result =
                self.heal_targets_with(k, b, &mut marks, &lock, healer_op, make_uv_trim_curves);
            lock.release(std::slice::from_mut(&mut k.context))?;
            result
        })
    }

    fn heal_targets_with(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        lock: &MarkLock,
        through: HealerOpType,
        make: bool,
    ) -> Result<()> {
        // The condition is Done < requested, not Step <= requested. Preserve the
        // gaps in the sequence (Fix_BackPointers and disabled Fix_BadLoops).
        macro_rules! step {
            ($op:ident, $call:expr) => {
                if self.done_heal_op.code() < through.code() {
                    $call?;
                    self.done_heal_op = HealerOpType::$op;
                }
            };
        }
        // Backpointer repair is unimplemented. Honor its requested boundary without
        // advancing into property caching. This records progress, not a repair.
        if self.done_heal_op.code() < through.code() && through == HealerOpType::FixBackpointers {
            self.done_heal_op = HealerOpType::FixBackpointers;
        }

        step!(
            CacheEdgeprops,
            self.cache_edge_props(k, b, HealerOpType::CacheEdgeprops)
        );
        step!(
            CacheVertexprops,
            self.cache_vertex_props(k, b, HealerOpType::CacheVertexprops)
        );
        step!(
            CacheFacepropsGaps,
            self.cache_face_props_gaps(k, b, HealerOpType::CacheFacepropsGaps)
        );
        step!(
            FixTolsizes,
            self.fix_tol_sizes(k, b, HealerOpType::FixTolsizes)
        );
        step!(
            CacheCoinVertices,
            self.cache_coin_vertices(k, b, HealerOpType::CacheCoinVertices)
        );
        step!(
            FixCoinVertices,
            self.fix_coin_vertices(k, b, HealerOpType::FixCoinVertices)
        );
        step!(
            CacheDegenFaces,
            self.cache_degen_faces(k, b, HealerOpType::CacheDegenFaces)
        );
        step!(
            FixDegenFaces,
            self.fix_degen_faces(k, b, HealerOpType::FixDegenFaces)
        );
        step!(
            FixDegenEdges,
            self.fix_degen_edges(k, b, HealerOpType::FixDegenEdges)
        );
        step!(
            CacheCoinEdges,
            Ok::<_, SolidError>(self.cache_coin_edges(HealerOpType::CacheCoinEdges))
        );
        step!(
            FixCoinEdges,
            self.fix_coin_edges(HealerOpType::FixCoinEdges)
        );
        step!(
            CacheMissedEdgexsects,
            Ok::<_, SolidError>(self.cache_missed_edge_xsects(HealerOpType::CacheMissedEdgexsects))
        );
        step!(
            FixMissedEdgexsects,
            self.fix_missed_edge_xsects(HealerOpType::FixMissedEdgexsects)
        );
        step!(
            FixUncontainedEdges,
            self.fix_uncontained_edges(HealerOpType::FixUncontainedEdges)
        );
        step!(FixBadgaps, {
            self.fix_bad_gaps(HealerOpType::FixBadgaps);
            Ok::<_, SolidError>(())
        });
        step!(
            CacheFaceprops2,
            self.cache_face_props_stage2(k, b, HealerOpType::CacheFaceprops2)
        );
        step!(
            FixMoveseam,
            self.fix_move_seam(k, b, marks, lock, HealerOpType::FixMoveseam)
        );
        step!(
            FixSplitedgeAtseam,
            self.fix_split_edges_at_seam(k, b, marks, lock, HealerOpType::FixSplitedgeAtseam)
        );
        step!(FixBadsheets, {
            // The heal path carries no attribute state at any level (no AttributeStore,
            // no SmTopology records), so SmSAGObject attribute propagation during
            // healing is not modelled anywhere in heal: the region split's
            // SmBrep::Notify runs on default region records and an empty store. Thread
            // an env from heal_targets once heal gains an AttributeStore
            // (plan/findings.md, heal.heal_data_p3).
            let mut state = crate::topology::brep::p2::RegionNotifyState::new(b);
            self.fix_bad_sheets(
                k,
                b,
                marks,
                &mut state.env(),
                lock,
                HealerOpType::FixBadsheets,
            )
        });
        step!(
            CacheFaceprops3,
            self.cache_face_props_stage3(k, b, HealerOpType::CacheFaceprops3)
        );
        step!(
            FixSplitfaceAtseams,
            self.fix_split_face_at_seams(k, b, HealerOpType::FixSplitfaceAtseams)
        );
        // Honor this requested boundary without running the disabled repair or
        // advancing into trim generation. This records progress, not a loop fix.
        if self.done_heal_op.code() < through.code() && through == HealerOpType::FixBadloops {
            self.done_heal_op = HealerOpType::FixBadloops;
        }
        // Fix_BadLoops is disabled upstream.
        step!(
            MakeUvtrimcurves,
            self.make_uv_trim_curves(k, b, make, HealerOpType::MakeUvtrimcurves)
        );
        step!(
            FixInfiniteRegions,
            self.fix_infinite_region(k, b, HealerOpType::FixInfiniteRegions)
        );
        Ok(())
    }

    /// Merges the live coincident vertex pairs in snapshot order, rewriting later
    /// pairs from the removed props to the survivor after each tracked merge.
    pub fn fix_coin_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        _op: HealerOpType,
    ) -> Result<()> {
        with_editing_enabled(b, |b| {
            let mut pairs = self.bad_vertex_props_coin_vertices.base.prob_array.clone();
            let n = pairs.len() / 2; // upstream intentionally ignores an odd tail
            for i in 0..n {
                let (Some(&survivor), Some(&deleted)) = (pairs.get(2 * i), pairs.get(2 * i + 1))
                else {
                    return Err(invalid("coincident pair list changed"));
                };
                if survivor == deleted
                    || !self.tgt_vertex_props.contains(&survivor)
                    || !self.tgt_vertex_props.contains(&deleted)
                {
                    continue;
                }
                let a = self
                    .vertex_props
                    .props(survivor)?
                    .vertex
                    .ok_or_else(|| invalid("survivor props without vertex"))?;
                let d = self
                    .vertex_props
                    .props(deleted)?
                    .vertex
                    .ok_or_else(|| invalid("deleted props without vertex"))?;
                let changes = combine_vertices(k, b, a, d)?;
                self.update_target_lists(
                    k,
                    b,
                    &changes.removed,
                    &changes.added,
                    HealerOpType::FixCoinVertices,
                )?;
                for id in pairs.iter_mut().take(2 * n).skip(2 * (i + 1)) {
                    if *id == deleted {
                        *id = survivor;
                    }
                }
            }
            Ok(())
        })
    }

    /// Squeezes degenerate edges in reverse problem-list order, deleting closed
    /// single-vertex edges instead, then updates all target and problem lists.
    pub fn fix_degen_edges(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        _op: HealerOpType,
    ) -> Result<()> {
        let queue = self.bad_edge_props_degen_edges.prob_array.clone();
        if queue.is_empty() {
            return Ok(());
        }
        with_editing_enabled(b, |b| {
            let mut changes = Changes::default();
            for id in queue.into_iter().rev() {
                let p = self.edge_props.props(id)?;
                let e = p.edge.ok_or(SolidError::Upstream {
                    operation: OP,
                    code: SM_ERR,
                    entity: None,
                })?;
                if p.bad_degen_edge.is_false() {
                    continue;
                }
                let (a, d) = edge::start_end_vertices(b, e)?;
                let edits = if a == d {
                    delete_edge(k, b, e)?
                } else {
                    squeeze_edge(
                        k,
                        b,
                        e,
                        a.ok_or_else(|| invalid("edge without start vertex"))?,
                    )?
                };
                changes.append(edits);
            }
            self.update_target_lists(
                k,
                b,
                &changes.removed,
                &changes.added,
                HealerOpType::FixDegenEdges,
            )
        })
    }
}

// The context callback, not a mutation's nominal inputs/outputs, owns these lists
// (`SmTrackTopologyChanges`, then `UpdateTargetLists(GetRmList(), GetAddList(), ..)`).
// The edits run without a notification environment, so part 5 names `b`'s objects by
// their kernel identities (its plain entry points' `plain_edit`:
// `topology::brep::p3::topology_ref` with `b`'s serial in the kernel context's domain),
// which `topology::brep::p3::topology_entity` inverts with that serial. A key it did not
// issue is an error, never a guessed identity.
fn capture_changes<T>(
    k: &mut Kernel,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep) -> Result<T>,
) -> Result<(T, Changes)> {
    use crate::geom::context::NotifyCallback;
    use crate::topology::topology::TopologyChangeCallback;
    if k.context.sys_notify_callback().is_some() {
        // Native StartTracking preserves a preexisting callback, leaving this
        // tracker's own arrays empty. StopTracking preserves that callback too.
        // These are the actual empty native arrays, not guessed edit identities.
        let value = edit(k, b)?;
        return Ok((value, Changes::default()));
    }
    k.context
        .set_sys_notify_callback(Some(Box::new(TopologyChangeCallback::new(1))));
    let result = edit(k, b);
    let captured = k
        .context
        .sys_notify_callback()
        .and_then(NotifyCallback::as_any)
        .and_then(|a| a.downcast_ref::<TopologyChangeCallback>())
        .map(|callback| (callback.lists(), callback.failure()));
    k.context.set_sys_notify_callback(None);
    let value = result?;
    let (mut lists, failure) =
        captured.ok_or_else(|| invalid("edit replaced its topology tracker"))?;
    if let Some(error) = failure {
        return Err(error);
    }
    let serial = b.notify_serial();
    // StopTracking: the connected edges and faces of the added vertices and edges
    // (`SmTopologyChangeCallback::RecordDependencies`), once the edit is complete.
    // Without a serial no object of `b` has a kernel identity to record.
    if let Some(serial) = serial {
        lists.record_dependencies(
            b,
            &|key| crate::topology::brep::p3::topology_entity(serial, key),
            &|e| crate::topology::brep::p3::topology_ref(serial, e).key,
        )?;
    }
    let decode = |key| {
        serial
            .and_then(|s| crate::topology::brep::p3::topology_entity(s, key))
            .ok_or_else(|| invalid("tracked object without a kernel identity of this B-rep"))
    };
    let removed = lists
        .rm_list
        .iter()
        .map(|&key| decode(key))
        .collect::<Result<Vec<_>>>()?;
    let added = lists
        .add_list
        .iter()
        .map(|&key| decode(key).map(Some))
        .collect::<Result<Vec<_>>>()?;
    Ok((value, Changes { removed, added }))
}

fn combine_vertices(k: &mut Kernel, b: &mut Brep, a: VertexId, d: VertexId) -> Result<Changes> {
    let ((), changes) = capture_changes(k, b, |k, b| {
        let mut deleted_edge = None;
        crate::topology::brep::p5::combine_coincident_vertices(k, b, a, d, &mut deleted_edge)
    })?;
    Ok(changes)
}

/// Tracker key of the edit trackers (one at a time).
const EDIT_TRACKER: u64 = 0x4845_414c; // "HEAL"

/// Runs the B-rep edit `edit` under a topology tracker, as the healer's loops do
/// (`SmTrackTopologyChanges sTopologyChanges(m_pBrep, TRUE)`, SmHealData.cpp:3341 and
/// the later loops), and returns its result with what the tracker recorded
/// (`SmTopologyChangeCallback::Execute`, `topology::topology`): the removed and the
/// added faces, edges and vertices. The edit also borrows the kernel (mark locks),
/// so the kernel context's notify callbacks are lent to a context of their own for
/// the edit's notifications and given back afterwards
/// (`topology::brep::p3::with_lent_context`); a callback already installed there (an
/// outer tracker) keeps the edit's notifications, and the tracker then records
/// nothing, as upstream. The healer keeps no notification identities of its own, so
/// `b`'s objects are named by their kernel identities
/// (`topology::brep::p3::kernel_named_edit`), and the tracker's lists are decoded with
/// `b`'s serial. Notification records and attributes are scratch: nothing in the
/// healer reads them. A failed edit still reports what was tracked before it failed
/// (callers that ignore its status keep those changes). Not nestable: a tracked edit
/// (or a `topology::brep::p3::with_kernel_callbacks`) inside `edit` is refused, before
/// anything changes.
fn tracked_edit<R>(
    k: &mut Kernel,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut NotifyEnv<'_>) -> Result<R>,
) -> (Result<R>, Changes) {
    use crate::topology::brep::p3 as brep_p3;
    let lent = brep_p3::with_lent_context(k, |k, ctx| {
        // (the tracker reads its B-rep only to tell it from null)
        let mut tracker = TrackTopologyChanges::new(
            Some(crate::topology::BrepId::from_index(0)),
            EDIT_TRACKER,
            true,
            Some(&mut *ctx),
        );
        let r = brep_p3::kernel_named_edit(k, ctx, b, edit);
        let serial = b.notify_serial();
        // StopTracking, with its dependency recording (`RecordDependencies`); without a
        // serial no object of `b` has a kernel identity to record
        let stopped = match serial {
            Some(serial) => tracker.stop_tracking_with_brep(
                Some(&mut *ctx),
                b,
                &|key| brep_p3::topology_entity(serial, key),
                &|e| brep_p3::topology_ref(serial, e).key,
            ),
            None => Ok(()),
        };
        let r = match (r, stopped) {
            (Ok(_), Err(e)) => Err(e),
            (r, _) => r,
        };
        let edit_entity = |key: ObjectKey| {
            serial
                .and_then(|s| brep_p3::topology_entity(s, key))
                .filter(|e| {
                    matches!(
                        e,
                        EntityRef::Face(_) | EntityRef::Edge(_) | EntityRef::Vertex(_)
                    )
                })
        };
        let changes = Changes {
            removed: tracker
                .rm_list()
                .into_iter()
                .filter_map(edit_entity)
                .collect(),
            added: tracker.add_list().into_iter().map(edit_entity).collect(),
        };
        tracker.finish(Some(ctx));
        (r, changes)
    });
    lent.unwrap_or_else(|e| (Err(e), Changes::default()))
}
/// `SqueezeEdge(e, survivor)` under the edit tracker ([`tracked_edit`]).
fn squeeze_edge(k: &mut Kernel, b: &mut Brep, e: EdgeId, survivor: VertexId) -> Result<Changes> {
    let (r, changes) = tracked_edit(k, b, |k, b, env| {
        crate::topology::brep::p3::squeeze_edge(k, b, env, e, survivor)
    });
    r.map(|()| changes)
}
/// `SmBrep::DeleteEdge(edge)` (default `pOptSurfaceToKeep = NULL`) under the edit
/// tracker ([`tracked_edit`]).
fn delete_edge(k: &mut Kernel, b: &mut Brep, e: EdgeId) -> Result<Changes> {
    let (r, changes) = tracked_edit(k, b, |k, b, env| {
        p4::delete_edge(k, b, &mut p4::DeleteEnv::from_notify(env), e, None)
    });
    r.map(|()| changes)
}

impl<O: LoopObject> HealData<O> {
    /// Squeezes every degenerate face (vertex-only loops included), then removes the
    /// wire edges left behind and the face's vertices that became shell vertices.
    pub fn fix_degen_faces(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        _op: HealerOpType,
    ) -> Result<()> {
        let queue = self.bad_face_props_degen_faces.prob_array.clone();
        if queue.is_empty() {
            return Ok(());
        }
        with_editing_enabled(b, |b| {
            let mut changes = Changes::default();
            for id in queue {
                if !self.tgt_face_props.contains(&id) {
                    continue;
                }
                let p = &self.face_props.props(id)?.props;
                if p.bad_degen_face.is_false() {
                    continue;
                }
                let Some(f) = p.face else {
                    continue; // IsLiveTopologyMember(NULL) is false upstream.
                };
                if !bp1::is_live_topology_member(b, Some(EntityRef::Face(f))) {
                    continue;
                }
                let mut edges = face_edges(b, f)?;
                let mut vertices = face_vertices(b, f)?;

                // Squeeze every degenerate boundary, including vertex-only loops.
                let (new_edges, deleted_edges, deleted_vertices, edits) =
                    ignore_soft(squeeze_face(k, b, f))?.unwrap_or_default();
                changes.append(edits);
                for e in new_edges {
                    if !edges.contains(&e) {
                        edges.push(e);
                    }
                }
                edges.retain(|e| !deleted_edges.contains(e));
                vertices.retain(|v| !deleted_vertices.contains(v));

                // Remove wire edges left by squeezing, then their isolated vertices.
                for &e in &edges {
                    if edge::is_wire(b, e)? {
                        changes.append(ignore_soft(delete_edge(k, b, e))?.unwrap_or_default());
                    }
                }
                for &v in &vertices {
                    // Keep vertices that still belong to another face or wire.
                    if vertex::is_shell_vertex(b, v)? {
                        changes.append(ignore_soft(delete_vertex(k, b, v))?.unwrap_or_default());
                    }
                }
            }
            self.update_target_lists(
                k,
                b,
                &changes.removed,
                &changes.added,
                HealerOpType::FixDegenFaces,
            )
        })
    }

    /// Tries moving each seam-problem face once, in reverse problem-list order.
    /// A tail update selects problems directly from the new props instead.
    pub fn fix_move_seam(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        lock: &MarkLock,
        _op: HealerOpType,
    ) -> Result<()> {
        let mut queue = Vec::new();
        if self.old_face_cnt != 0 {
            for &id in self.tgt_face_props.iter().skip(self.old_face_cnt) {
                let p = &self.face_props.props(id)?.props;
                // SmBooleanUL tests nonzero (UNSURE is nonzero too).
                if seam_problem(p) {
                    queue.push(id);
                }
            }
        } else {
            queue.clone_from(&self.bad_face_props_missing_seam.prob_array);
            for &id in self
                .bad_face_props_crossed_seam
                .prob_array
                .iter()
                .chain(&self.bad_face_props_near_miss_seam.prob_array)
            {
                if !queue.contains(&id) {
                    queue.push(id);
                }
            }
        }
        if queue.is_empty() {
            return Ok(());
        }
        lock.new_mark(std::slice::from_mut(&mut k.context))?;
        let mut changes = Changes::default();
        for id in queue.into_iter().rev() {
            let p = &mut self.face_props.props_mut(id)?.props;
            let f = p.face.ok_or_else(|| invalid("seam props without face"))?;
            if marks.is_marked(k, Entity::Face(f), lock.mark_type()) {
                continue;
            }
            marks.mark(k, Entity::Face(f), lock.mark_type());
            // MoveSeam(TRUE /* move inside */, FALSE /* do not split */, props).
            let (res, edits) = move_seam(k, b, f, p, true, false);
            changes.append(edits);
            ignore_soft(res)?;
        }
        self.update_target_lists(
            k,
            b,
            &changes.removed,
            &changes.added,
            HealerOpType::FixMoveseam,
        )?;
        for &id in &self.tgt_face_props {
            let p = &self.face_props.props(id)?.props;
            let f = p.face.ok_or_else(|| invalid("seam props without face"))?;
            if marks.is_marked(k, Entity::Face(f), lock.mark_type()) {
                let tried = if seam_problem(p) {
                    TriedLinkType::NoFix
                } else {
                    TriedLinkType::Fixed
                };
                self.ran_face_props_move_seam
                    .add(&mut self.face_props, id, tried)?;
            }
        }
        Ok(())
    }

    /// Splits the boundary edges crossing interior poles, then the edges crossing U
    /// and then V seams, and records one split-edge tried outcome per attempted face.
    /// Native marking deliberately visits the last direction's `sThisSplitEdges`,
    /// not their combined list.
    pub fn fix_split_edges_at_seam(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        lock: &MarkLock,
        _op: HealerOpType,
    ) -> Result<()> {
        // Face marks survive property replacement; record results after the final
        // refresh.
        lock.new_mark(std::slice::from_mut(&mut k.context))?;

        // Consume Stage 2 pole crossings before using the cached seam classifications.
        self.split_edges_at_poles(k, b, marks, lock)?;

        let queue = self.bad_face_props_crossed_seam.prob_array.clone();
        if !queue.is_empty() {
            self.split_edges_at_seams(k, b, marks, lock, queue)?;
        }

        // Reacquire current properties, as in Fix_MoveSeam. One result per attempted
        // face includes pole failures even when that face's seam crossings were
        // repaired.
        for &id in &self.tgt_face_props {
            let p = &self.face_props.props(id)?.props;
            let f = p.face.ok_or_else(|| invalid("seam props without face"))?;
            if !marks.is_marked(k, Entity::Face(f), lock.mark_type()) {
                continue;
            }
            let tried = if p.bad_crossed_pole.code() != 0 || p.bad_crossed_seam.code() != 0 {
                TriedLinkType::NoFix
            } else {
                TriedLinkType::Fixed
            };
            self.ran_face_props_split_edges_at_seam.base.remove(id);
            self.ran_face_props_split_edges_at_seam
                .add(&mut self.face_props, id, tried)?;
        }

        // all done
        Ok(())
    }

    /// The pole-crossing loop of `Fix_SplitEdgesAtSeam`: splits the cached crossing
    /// edge of the first listed face whose split has not failed, refreshing the
    /// targets after each split (an edge can cross several poles or bound several
    /// faces, and the original edge survives with a shorter interval). A split refused
    /// at an edge end is remembered outside the props, because another successful
    /// edit can replace the props and list the same unresolved candidate again.
    fn split_edges_at_poles(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        lock: &MarkLock,
    ) -> Result<()> {
        // (face, edge, parameter) of each split refused at an edge end
        let mut failed_pole_splits: Vec<(Option<FaceId>, Option<EdgeId>, f64)> = Vec::new();
        loop {
            let mut next = None;
            for &id in &self.bad_face_props_crossed_pole.prob_array {
                let candidate = &self.face_props.props(id)?.props;
                if candidate.bad_crossed_pole.code() == 0 {
                    continue;
                }
                let failed = failed_pole_splits.iter().any(|&(face, edge, parameter)| {
                    face == candidate.face
                        && edge == candidate.crossed_pole_edge
                        && (parameter - candidate.crossed_pole_parameter).abs() < EFF_ZERO_PARAM
                });
                if !failed {
                    next = Some(id);
                    break;
                }
            }
            let Some(id) = next else {
                break;
            };

            let p = &self.face_props.props(id)?.props;
            let face = p.face;
            let parameter = p.crossed_pole_parameter;
            let edge = p
                .crossed_pole_edge
                .ok_or_else(|| invalid("crossed pole without edge"))?;
            let f = face.ok_or_else(|| invalid("crossed pole props without face"))?;
            marks.mark(k, Entity::Face(f), lock.mark_type());
            let (split, edits) = tracked_edit(k, b, |k, b, env| {
                crate::topology::brep::p9::make_vertex_split_edge_with_curve_identity(
                    k,
                    b,
                    edge,
                    parameter,
                    env,
                    crate::topology::brep::p9::CurveIdentity::Actual(
                        &crate::topology::brep::p9::issue_curve_identity,
                    ),
                )
            });
            let split = split?;
            if !split.split && split.edges.is_none() {
                if let Some(v) = split.vertex {
                    let (start, end) = edge::start_end_vertices(b, edge)?;
                    if Some(v) == start || Some(v) == end {
                        // Pole-crossing classification disagrees with
                        // MakeVertexSplitEdge: the split was rejected near an endpoint.
                        // Another face can have a valid crossing on this same edge.
                        failed_pole_splits.push((face, Some(edge), parameter));
                        continue;
                    }
                }
            }
            // Only endpoint rejection is known to leave the edge unsplit. Other errors
            // can follow interval/curve edits, so do not continue with stale properties.
            if !split.split {
                return Err(SolidError::sm_err(OP));
            }
            // UpdateTargetLists deletes old properties. Reacquire the next candidate.
            self.update_target_lists(
                k,
                b,
                &edits.removed,
                &edits.added,
                HealerOpType::FixMoveseam,
            )?;
        }
        Ok(())
    }

    /// The seam-crossing loop of `Fix_SplitEdgesAtSeam` over `queue` (the crossed
    /// seam list), last face first.
    fn split_edges_at_seams(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        marks: &mut Marks,
        lock: &MarkLock,
        queue: Vec<FacePropsId>,
    ) -> Result<()> {
        let mut changes = Changes::default();
        // Upstream keeps both statuses and the last direction's edges across faces.
        let mut merge_ok = [true, true];
        let mut this_split_edges = Vec::new();
        for id in queue.into_iter().rev() {
            if !self.tgt_face_props.contains(&id) {
                continue;
            }
            let p = &mut self.face_props.props_mut(id)?.props;
            let Some(f) = p.face else {
                continue; // IsLiveTopologyMember(NULL) is false upstream.
            };
            if !bp1::is_live_topology_member(b, Some(EntityRef::Face(f)))
                || p.bad_crossed_seam.is_false()
            {
                continue;
            }
            if classification_has_dead_topology(b, p.crv_class_u.as_ref())?
                || classification_has_dead_topology(b, p.crv_class_v.as_ref())?
            {
                refresh_stage2(k, b, p, self.done_heal_op)?;
                if p.bad_crossed_seam.is_false() {
                    continue;
                }
            }
            if has_u(p.bad_crossed_seam_dir) {
                this_split_edges = classified_edges(p.crv_class_u.as_ref())?;
            }
            if has_v(p.bad_crossed_seam_dir) {
                this_split_edges = classified_edges(p.crv_class_v.as_ref())?;
            }
            let mut rebuild = false;
            for &e in &this_split_edges {
                if marks.is_marked(k, Entity::Edge(e), lock.mark_type()) {
                    rebuild = true;
                }
                marks.mark(k, Entity::Edge(e), lock.mark_type());
            }
            if rebuild {
                refresh_stage2(k, b, p, self.done_heal_op)?;
            }
            let dirs = [
                (has_u(p.bad_crossed_seam_dir), true),
                (has_v(p.bad_crossed_seam_dir), false),
            ];
            for (run, u) in dirs {
                if !run {
                    continue;
                }
                marks.mark(k, Entity::Face(f), lock.mark_type());
                let p = &mut self.face_props.props_mut(id)?.props;
                let class = if u {
                    p.crv_class_u.as_mut()
                } else {
                    p.crv_class_v.as_mut()
                }
                .ok_or_else(|| invalid("crossed seam without classification"))?;
                let dir = if u {
                    SurfParamType::U
                } else {
                    SurfParamType::V
                };
                // MergeIntervals(&faces, &edges, &vertices, pThisFace, TRUE /* vertices
                // only */) edits the face's B-rep under the loop's topology tracker.
                let (merge, edits) = tracked_edit(k, b, |k, b, env| {
                    crate::topology::face::p4::merge_seam_intervals(
                        k,
                        b,
                        Some(env),
                        f,
                        dir,
                        class,
                        true,
                    )
                });
                changes.append(edits);
                let (merged, success) = match merge? {
                    Ok(merged) => (merged, true),
                    Err(failure) => {
                        ignore_soft::<()>(Err(failure.error))?;
                        (*failure.merged, false)
                    }
                };
                if let Some(status) = merge_ok.get_mut(usize::from(!u)) {
                    *status = success;
                }
                // Native retains lists appended before an ignored merge failure.
                for o in &merged.edges {
                    match o.as_topology() {
                        Some(EntityRef::Edge(x)) => {
                            self.new_edges_from_split_edges_at_seams.push(x);
                        }
                        _ => return Err(invalid("MergeIntervals made an edge without B-rep edge")),
                    }
                }
                for o in &merged.vertices {
                    match o.as_topology() {
                        Some(EntityRef::Vertex(x)) => {
                            self.new_vertices_from_split_edges_at_seams.push(x);
                        }
                        _ => {
                            return Err(invalid(
                                "MergeIntervals made a vertex without B-rep vertex",
                            ))
                        }
                    }
                }
                self.ran_face_props_split_edges_at_seam.add(
                    &mut self.face_props,
                    id,
                    TriedLinkType::Ran,
                )?;
                // End the arena borrow before adding the tried record.
                // The next direction obtains the classification again below.
            }
            if merge_ok == [true, true] {
                let p = &mut self.face_props.props_mut(id)?.props;
                p.bad_crossed_seam_dir = SurfParamType::Neither;
                p.bad_crossed_seam = crate::heal::face_props::Tri::False;
            }
        }
        // Stop before this stage even when called directly after later healing.
        // Reentering it here would retry failed pole splits with a fresh skip list.
        self.update_target_lists(
            k,
            b,
            &changes.removed,
            &changes.added,
            HealerOpType::FixMoveseam,
        )
    }

    /// Inserts missing seam edges and records the topology created by splitting.
    /// Native code updates targets through `FixSplitedgeAtseam` and records tried
    /// outcomes in the split-face list.
    pub fn fix_split_face_at_seams(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        _op: HealerOpType,
    ) -> Result<()> {
        let queue = self.bad_face_props_missing_seam.prob_array.clone();
        if queue.is_empty() {
            return Ok(());
        }
        let mut changes = Changes::default();
        for id in queue.into_iter().rev() {
            if !self.tgt_face_props.contains(&id) {
                continue;
            }
            let p = &mut self.face_props.props_mut(id)?.props;
            let Some(f) = p.face else {
                continue; // IsLiveTopologyMember(NULL) is false upstream.
            };
            if !bp1::is_live_topology_member(b, Some(EntityRef::Face(f)))
                || p.bad_missing_seam.is_false()
            {
                continue;
            }
            if classification_has_dead_topology(b, p.crv_class_u.as_ref())?
                || classification_has_dead_topology(b, p.crv_class_v.as_ref())?
            {
                refresh_stage2(k, b, p, self.done_heal_op)?;
                if p.bad_missing_seam.is_false() {
                    continue;
                }
            }
            let (split, edits) = split_face_at_seam(k, b, f, p);
            // (status ignored upstream; the tracker keeps what was edited before a failure)
            changes.append(edits);
            let new_faces = ignore_soft(split)?.unwrap_or_default();
            if new_faces.is_empty() {
                self.ran_face_props_split_face_at_seam.add(
                    &mut self.face_props,
                    id,
                    TriedLinkType::NoFix,
                )?;
            } else {
                self.ran_face_props_split_face_at_seam
                    .base
                    .inc_orig_prob_count();
            }
        }
        for e in changes.added.iter().flatten() {
            match *e {
                EntityRef::Face(x) => self.new_faces_from_split_face_at_seams.push(x),
                EntityRef::Edge(x) => self.new_edges_on_seams_from_split_face_at_seams.push(x),
                EntityRef::Vertex(x) => self.new_vertices_from_split_face_at_seams.push(x),
                _ => (),
            }
        }
        self.update_target_lists(
            k,
            b,
            &changes.removed,
            &changes.added,
            HealerOpType::FixSplitedgeAtseam,
        )
    }
}

fn seam_problem<O: LoopObject>(p: &FaceProps<O>) -> bool {
    !p.bad_missing_seam.is_false()
        || !p.bad_crossed_seam.is_false()
        || !p.bad_near_miss_seam.is_false()
}
fn has_u(d: SurfParamType) -> bool {
    matches!(d, SurfParamType::U | SurfParamType::Both)
}
fn has_v(d: SurfParamType) -> bool {
    matches!(d, SurfParamType::V | SurfParamType::Both)
}
fn refresh_stage2<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    p: &mut FaceProps<O>,
    done: HealerOpType,
) -> Result<()> {
    p.reset(PropStage::Stage2);
    crate::error::ignore_status(p.set_props(
        k,
        b,
        PropStage::Stage2,
        p.face.ok_or_else(|| invalid("props without face"))?,
        p.face_index,
        done,
    ))
}
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    crate::topology::face::p4::vertices(b, f, None, false, None)
}
#[cfg(test)]
fn glue_vertices(k: &mut Kernel, b: &mut Brep, a: VertexId, d: VertexId) -> Result<Changes> {
    let ((), changes) = capture_changes(k, b, |k, b| {
        crate::topology::brep::p5::glue_vertices(k, b, Some(a), Some(d), None)
    })?;
    Ok(changes)
}
/// [`squeeze_face`]'s new edges, deleted edges, deleted vertices and tracked changes.
type SqueezedFaceParts = (Vec<EdgeId>, Vec<EdgeId>, Vec<VertexId>, Changes);

fn squeeze_face(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<SqueezedFaceParts> {
    let (output, changes) = capture_changes(k, b, |k, b| {
        let mut output = crate::topology::brep::p5::SqueezedFace::default();
        // The enclosing healer edit session enables editing upstream; NULL uses the
        // B-rep stitching tolerance.
        crate::topology::brep::p5::squeeze_face(k, b, true, f, None, &mut output)?;
        Ok(output)
    })?;
    Ok((
        output.new_edges,
        output.deleted_edges,
        output.deleted_vertices,
        changes,
    ))
}

/// `SmBrep::DeleteVertex(vertex)` under the edit tracker ([`tracked_edit`]).
fn delete_vertex(k: &mut Kernel, b: &mut Brep, v: VertexId) -> Result<Changes> {
    let (r, changes) = tracked_edit(k, b, |k, b, env| {
        p4::delete_vertex(k, b, &mut p4::DeleteEnv::from_notify(env), v)
    });
    r.map(|()| changes)
}
/// The face as the owner of its surface (`this` of the face's members): the surface's
/// owner, or the face's arena identity when it has none.
fn face_ref(b: &Brep, f: FaceId) -> Result<ObjectRef> {
    Ok(b.face(f)?
        .surface
        .as_ref()
        .and_then(Surface::owner)
        .unwrap_or_else(|| arena_identity(CacheObject::Face(f))))
}

/// The seam problem of the face properties (`m_eBadCrossedSeam`, `m_eBadMissingSeam`,
/// `m_eBadNearMissSeam`).
fn props_seam_problem<O: LoopObject>(p: &FaceProps<O>) -> SeamProblem {
    SeamProblem {
        crossed: p.bad_crossed_seam_dir,
        missing: p.bad_missing_seam_dir,
        near_miss: p.bad_near_miss_seam_dir,
    }
}

/// Classification geometry that resolves nothing: the seam members read it only without
/// face properties, and the healer always passes them.
struct NoGeometry;

impl<O> ClassGeometry<O> for NoGeometry {
    fn curve(&self, _: O) -> Option<&Curve> {
        None
    }
    fn surface(&self, _: O) -> Option<&Surface> {
        None
    }
    fn volume(&self, _: O) -> Option<&Volume> {
        None
    }
    fn point(&self, _: O) -> Option<Point3> {
        None
    }
}

/// Runs `op` on the face properties' two classifications (`*m_pCrvClassU`,
/// `*m_pCrvClassV`, dereferenced unchecked upstream), lent out of `p` for the call.
fn with_classes<O: LoopObject, T>(
    p: &mut FaceProps<O>,
    op: impl FnOnce(
        &mut CurveClassification<O>,
        &mut CurveClassification<O>,
        &FaceProps<O>,
    ) -> Result<T>,
) -> Result<T> {
    let (Some(mut cu), Some(mut cv)) = (p.crv_class_u.take(), p.crv_class_v.take()) else {
        return Err(invalid("seam props without classifications"));
    };
    let r = op(&mut cu, &mut cv, p);
    p.crv_class_u = Some(cu);
    p.crv_class_v = Some(cv);
    r
}

/// `pThisFace->MoveSeam(eU, eV, *m_pCrvClassU, *m_pCrvClassV, m_eBadCrossedSeam,
/// m_eBadMissingSeam, m_eBadNearMissSeam, bOkToMoveToCenter, bOkToSplit, NULL, NULL, NULL,
/// pThisFaceProps)` under [`tracked_edit`]; upstream ignores the plan results (debug
/// output only) and keeps no new split topology, while `ReplaceSurface`'s
/// `SM_NO_POST_EDIT` marks the face removed and re-added in the tracker.
fn move_seam<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    p: &mut FaceProps<O>,
    move_inside: bool,
    split: bool,
) -> (Result<()>, Changes) {
    let this = match face_ref(b, f) {
        Ok(r) => r,
        Err(e) => return (Err(e), Changes::default()),
    };
    let problem = props_seam_problem(p);
    let (res, changes) = tracked_edit(k, b, |k, b, env| {
        with_classes(p, |cu, cv, p| {
            fp1::move_seam_notified(
                k,
                b,
                Some(env),
                f,
                this,
                cu,
                cv,
                problem,
                fp1::MoveSeamOptions {
                    ok_to_move_to_center: move_inside,
                    ok_to_split: split,
                },
                Some(p),
                // (no new topology without split; a handle would be InvalidInput)
                &|_| None,
            )
        })
    });
    (res.map(|_| ()), changes)
}
/// Classification geometry that resolves topology handles to the healed B-rep and
/// nothing else: `HasDeadTopologyPointObject` only asks where a vertex, edge or face
/// point classification's object lives (`SmBrep::IsLiveTopologyMember`).
struct BrepTopology<'a>(&'a Brep);

impl<O: LoopObject> ClassGeometry<O> for BrepTopology<'_> {
    fn curve(&self, _: O) -> Option<&Curve> {
        None
    }
    fn surface(&self, _: O) -> Option<&Surface> {
        None
    }
    fn volume(&self, _: O) -> Option<&Volume> {
        None
    }
    fn point(&self, _: O) -> Option<Point3> {
        None
    }
    fn topology_entity(&self, o: O) -> Option<(&Brep, EntityRef)> {
        o.as_topology().map(|e| (self.0, e))
    }
    /// The healed B-rep is the only one the handles reach: a constant id (the first);
    /// any other B-rep has none.
    fn brep_id(&self, brep: &Brep) -> Option<crate::topology::BrepId> {
        core::ptr::eq(brep, self.0).then_some(crate::topology::BrepId::from_index(0))
    }
}

fn classification_has_dead_topology<O: LoopObject>(
    b: &Brep,
    class: Option<&CurveClassification<O>>,
) -> Result<bool> {
    match class {
        None => Ok(false),
        Some(class) => class.has_dead_topology_point_object(&BrepTopology(b), Some(b)),
    }
}
fn classified_edges<O: LoopObject>(class: Option<&CurveClassification<O>>) -> Result<Vec<EdgeId>> {
    let class = class.ok_or_else(|| invalid("crossed seam without classification"))?;
    // FindEdgeObjects(sThisSplitEdges, NULL, 2): the handles name the B-rep's edges
    class
        .find_edge_objects(&NoGeometry, None, 2)?
        .into_iter()
        .map(|o| match o.as_topology() {
            Some(EntityRef::Edge(e)) => Ok(e),
            _ => Err(invalid("classified edge object is not a B-rep edge")),
        })
        .collect()
}
/// `pThisFace->SplitAtSeam(*m_pCrvClassU, *m_pCrvClassV, m_eBadCrossedSeam,
/// m_eBadMissingSeam, m_eBadNearMissSeam, &sThisNewFaces, NULL, NULL, pThisFaceProps)`
/// under the loop's topology tracker ([`tracked_edit`]): the new faces, and what the
/// tracker recorded. The classification handles name the B-rep's own topology
/// ([`LoopObject::as_topology`]).
fn split_face_at_seam<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    p: &mut FaceProps<O>,
) -> (Result<Vec<FaceId>>, Changes) {
    let problem = props_seam_problem(p);
    let (split, changes) = tracked_edit(k, b, |k, b, env| {
        with_classes(p, |cu, cv, p| {
            fp1::split_at_seam_notified(k, b, Some(env), f, cu, cv, problem, Some(p), &|o: O| {
                o.as_topology()
            })
        })
    });
    (split.map(|s| s.faces), changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heal::face_props::Tri;
    use crate::intersect::point_class::{ClassObject, ClassObjectKind};
    use crate::topology::LoopId;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct L(LoopId);
    impl ClassObject for L {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Loop
        }
    }
    impl LoopObject for L {
        fn from_loop(id: LoopId) -> Self {
            Self(id)
        }
    }

    /// A handle naming a vertex or a loop.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum T {
        Vertex(VertexId),
        Loop(LoopId),
    }
    impl ClassObject for T {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Vertex(_) => ClassObjectKind::Vertex,
                Self::Loop(_) => ClassObjectKind::Loop,
            }
        }
    }
    impl LoopObject for T {
        fn from_loop(id: LoopId) -> Self {
            Self::Loop(id)
        }
        fn as_topology(&self) -> Option<EntityRef> {
            match self {
                Self::Vertex(v) => Some(EntityRef::Vertex(*v)),
                Self::Loop(l) => Some(EntityRef::Loop(*l)),
            }
        }
    }

    #[test]
    fn brep_topology_names_the_healed_brep() {
        // `PointClassification::GetBrep` of a vertex point reaches the healed
        // B-rep's (constant) id; the skipped B-rep gives none
        use crate::intersect::point_class::{PointClassType, PointClassification};
        let mut k = Kernel::default();
        let b = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Vec3::new(0.0, 0.0, 0.0),
            1.0,
            1.0,
            1.0,
        )
        .unwrap();
        let v = bp1::vertices(&b, None).unwrap()[0];
        let topology = BrepTopology(&b);
        let mut pc = PointClassification::<T>::new(1e-6);
        pc.set_class_object(PointClassType::Vertex, Some(T::Vertex(v)));
        let id = crate::topology::BrepId::from_index(0);
        assert_eq!(pc.brep(&topology, None).unwrap(), Some(id));
        assert_eq!(pc.brep(&topology, Some(id)).unwrap(), None);
        let other = b.clone();
        assert_eq!(ClassGeometry::<T>::brep_id(&topology, &other), None);
    }

    #[test]
    fn catch_up_restores_state_after_failed_callee() {
        let mut k = Kernel::default();
        let mut b = Brep::default();
        // a face whose tolerance SmBrep::SetTolerance raises (SmFace::SetTolerance):
        // B-rep and face tolerances below the estimate
        let f = b
            .add_face_record(crate::topology::Face::new(
                crate::topology::ShellId::from_index(0),
            ))
            .unwrap();
        b.face_mut(f).unwrap().tolerance = 0.0;
        b.set_tolerance(0.0);
        let mut h = HealData::<L>::new();
        h.old_face_cnt = 7;
        h.old_edge_cnt = 8;
        h.old_vertex_cnt = 9;
        h.stop_heal_op = HealerOpType::All;
        h.done_heal_op = HealerOpType::FixTolsizes;
        h.recursion_count = 3;
        // Even a null add entry enters native catch-up. Fix_TolSizes still sets the
        // B-rep and face tolerances, despite empty target arrays.
        h.update_target_lists(&mut k, &mut b, &[], &[None], HealerOpType::All)
            .unwrap();
        assert_eq!((b.tolerance(), b.face(f).unwrap().tolerance), (1e-5, 1e-5));
        assert_eq!(
            (h.old_face_cnt, h.old_edge_cnt, h.old_vertex_cnt),
            (7, 8, 9)
        );
        assert_eq!(
            (h.stop_heal_op, h.done_heal_op),
            (HealerOpType::All, HealerOpType::FixTolsizes)
        );
        assert_eq!(h.recursion_count, 3);
        // A catch-up through Fix_InfiniteRegions fails on this region-less B-rep and
        // restores the same state.
        h.done_heal_op = HealerOpType::FixInfiniteRegions;
        let e = h
            .update_target_lists(&mut k, &mut b, &[], &[None], HealerOpType::All)
            .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));
        assert_eq!(
            (h.old_face_cnt, h.old_edge_cnt, h.old_vertex_cnt),
            (7, 8, 9)
        );
        assert_eq!(
            (h.stop_heal_op, h.done_heal_op),
            (HealerOpType::All, HealerOpType::FixInfiniteRegions)
        );
        assert_eq!(h.recursion_count, 3);
        // Both nested and outer mark locks are released on a failed pass.
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        assert_eq!(lock.mark_type(), crate::geom::context::MarkType::Mark);
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
    }

    #[test]
    fn recursion_limit_handles_extreme_counts_without_overflow() {
        for depth in [MAX_TARGET_UPDATE_DEPTH, u64::MAX] {
            let mut h = HealData::<L>::new();
            h.recursion_count = depth;
            let r = h.update_target_lists(
                &mut Kernel::default(),
                &mut Brep::default(),
                &[],
                &[None],
                HealerOpType::None,
            );
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
            assert_eq!(h.recursion_count, depth);
        }
    }

    #[test]
    fn additions_skip_null_stale_and_unrelated_topology() {
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let v = b
            .add_vertex_record(crate::topology::Vertex::default())
            .unwrap();
        let mut h = HealData::<L>::new();
        h.done_heal_op = HealerOpType::None;
        h.update_target_lists(
            &mut k,
            &mut b,
            &[],
            &[
                None,
                Some(EntityRef::Vertex(VertexId::from_index(u32::MAX))),
                Some(EntityRef::Vertex(v)),
            ],
            HealerOpType::All,
        )
        .unwrap();
        assert_eq!(h.tgt_vertices, vec![v]);
        assert_eq!(h.tgt_vertex_props.len(), 1);
        assert_eq!(
            h.vertex_props.get(h.tgt_vertex_props[0]).unwrap().vertex,
            None
        );
        assert_eq!(h.recursion_count, 0);
    }

    #[test]
    fn removal_frees_props_and_cleans_problem_lists() {
        let mut b = Brep::default();
        let v = b
            .add_vertex_record(crate::topology::Vertex::default())
            .unwrap();
        let mut h = HealData::<L>::with_targets(&[v], &[], &[]);
        let id = h.tgt_vertex_props[0];
        h.vertex_props.get_mut(id).unwrap().vertex = Some(v);
        h.bad_vertex_props_small_zone_tol_3d
            .add(&mut h.vertex_props, id, true)
            .unwrap();
        h.update_target_lists(
            &mut Kernel::default(),
            &mut b,
            &[EntityRef::Vertex(v)],
            &[],
            HealerOpType::All,
        )
        .unwrap();
        assert!(h.tgt_vertices.is_empty());
        assert!(h.tgt_vertex_props.is_empty());
        assert!(h.bad_vertex_props_small_zone_tol_3d.is_empty());
        assert!(h.vertex_props.get(id).is_none());
    }

    #[test]
    fn coincident_snapshot_skips_self_pairs_and_retired_props() {
        let mut h = HealData::<L>::new();
        let a = h.vertex_props.alloc(VertexProps::new());
        let d = h.vertex_props.alloc(VertexProps::new());
        h.tgt_vertex_props.push(a);
        h.bad_vertex_props_coin_vertices.base.prob_array = vec![a, a, a, d];
        h.fix_coin_vertices(
            &mut Kernel::default(),
            &mut Brep::default(),
            HealerOpType::FixCoinVertices,
        )
        .unwrap();
    }

    #[test]
    fn false_problem_flags_do_not_trigger_topology_mutations() {
        let mut b = Brep::default();
        let e = b
            .add_edge_record(crate::topology::Edge::new([VertexId::from_index(0); 2]))
            .unwrap();
        let mut h = HealData::<L>::new();
        let mut p = EdgeProps::new();
        p.edge = Some(e);
        p.bad_degen_edge = Tri::False;
        let id = h.edge_props.alloc(p);
        h.bad_edge_props_degen_edges.prob_array.push(id);
        h.fix_degen_edges(&mut Kernel::default(), &mut b, HealerOpType::FixDegenEdges)
            .unwrap();
    }
    #[test]
    fn null_faces_are_skipped_in_all_live_member_guarded_repairs() {
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let mut h = HealData::<L>::new();
        let id = h.face_props.alloc(FacePropsEntry::new());
        h.tgt_face_props.push(id);
        h.bad_face_props_degen_faces
            .add(&mut h.face_props, id, true)
            .unwrap();
        h.bad_face_props_crossed_seam
            .add(&mut h.face_props, id, true)
            .unwrap();
        h.bad_face_props_missing_seam
            .add(&mut h.face_props, id, true)
            .unwrap();
        h.fix_degen_faces(&mut k, &mut b, HealerOpType::FixDegenFaces)
            .unwrap();
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        // The seam loop skips the null face; the tried-outcome pass over every target
        // then reads its mark, which upstream dereferences.
        let r = h.fix_split_edges_at_seam(
            &mut k,
            &mut b,
            &mut Marks::default(),
            &lock,
            HealerOpType::FixSplitedgeAtseam,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
        h.fix_split_face_at_seams(&mut k, &mut b, HealerOpType::FixSplitfaceAtseams)
            .unwrap();
        assert_eq!(
            h.face_props.props(id).unwrap().props.bad_crossed_seam,
            Tri::True
        );
    }

    #[test]
    fn neither_direction_still_updates_crossed_seam_flags_in_native_order() {
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let f = b
            .add_face_record(crate::topology::Face::new(
                crate::topology::ShellId::from_index(0),
            ))
            .unwrap();
        let mut h = HealData::<L>::with_targets(&[], &[], &[f]);
        let id = h.tgt_face_props[0];
        let p = &mut h.face_props.props_mut(id).unwrap().props;
        p.face = Some(f);
        p.bad_crossed_seam_dir = SurfParamType::Neither;
        p.bad_missing_seam = Tri::False;
        p.bad_near_miss_seam = Tri::False;
        h.bad_face_props_crossed_seam
            .add(&mut h.face_props, id, true)
            .unwrap();
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        h.fix_split_edges_at_seam(
            &mut k,
            &mut b,
            &mut Marks::default(),
            &lock,
            HealerOpType::FixSplitedgeAtseam,
        )
        .unwrap();
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
        let p = &h.face_props.props(id).unwrap().props;
        assert_eq!(p.bad_crossed_seam_dir, SurfParamType::Neither);
        assert_eq!(p.bad_crossed_seam, Tri::False);
        // No direction ran, so the face was never marked and gets no tried outcome.
        assert_eq!(p.been_through_split_edges_at_seam, TriedLinkType::None);
        assert!(h
            .ran_face_props_split_edges_at_seam
            .base
            .prob_array
            .is_empty());
    }

    #[test]
    fn requested_disabled_steps_record_their_boundary_without_running_later_steps() {
        // Fix_BackPointers: Cache_EdgeProps does not run.
        let mut h = HealData::<L>::with_targets(&[], &[EdgeId::from_index(5)], &[]);
        h.heal_targets(
            &mut Kernel::default(),
            &mut Brep::default(),
            HealerOpType::FixBackpointers,
            false,
        )
        .unwrap();
        assert_eq!(h.done_heal_op, HealerOpType::FixBackpointers);
        // Fix_BadLoops: Make_UVTrimCurves does not run.
        let mut h = HealData::<L>::new();
        h.done_heal_op = HealerOpType::FixSplitfaceAtseams;
        let mut k = Kernel::default();
        let mut b = Brep::default();
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        h.heal_targets_with(
            &mut k,
            &mut b,
            &mut Marks::default(),
            &lock,
            HealerOpType::FixBadloops,
            false,
        )
        .unwrap();
        lock.release(std::slice::from_mut(&mut k.context)).unwrap();
        assert_eq!(h.done_heal_op, HealerOpType::FixBadloops);
    }

    #[test]
    fn missing_edge_has_native_sm_err_status() {
        let mut h = HealData::<L>::new();
        let id = h.edge_props.alloc(EdgeProps::new());
        h.bad_edge_props_degen_edges.prob_array.push(id);
        assert!(matches!(
            h.fix_degen_edges(
                &mut Kernel::default(),
                &mut Brep::default(),
                HealerOpType::FixDegenEdges
            ),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
    }

    /// The wire chain A--B--C without curves (`topology::brep::p3`'s test fixture).
    fn wire_chain() -> (Brep, [VertexId; 3], [EdgeId; 2]) {
        crate::topology::brep::p3::test_support::wire_chain(false)
    }

    /// The healer's `SqueezeEdge` completes on an ordinary wire chain: its tracked
    /// edit seeds every object's `SmTopology` record, so `SM_NO_MERGE_IN_BREP` (which
    /// reads both vertices') succeeds, and the tracker reports the edit
    /// (`SmTopologyChangeCallback::Execute`).
    #[test]
    fn healer_squeeze_of_a_wire_chain_completes_under_its_tracker() {
        let (mut b, [a, bv, _], [e0, e1]) = wire_chain();
        let mut k = Kernel::default();
        let changes = squeeze_edge(&mut k, &mut b, e0, a).unwrap();
        // StopTracking's RecordDependencies adds the survivor's remaining edge to both
        // lists (a wire has no faces)
        assert_eq!(
            changes.removed,
            vec![
                EntityRef::Edge(e0),
                EntityRef::Vertex(a),
                EntityRef::Vertex(bv),
                EntityRef::Edge(e1)
            ]
        );
        assert_eq!(
            changes.added,
            vec![Some(EntityRef::Vertex(a)), Some(EntityRef::Edge(e1))]
        );
        // the squeezed edge's uses and their vertex uses are gone; the deleted vertex's
        // other use (of the second edge) moved to the survivor
        assert!(b.edge(e0).unwrap().edgeuses.is_empty());
        assert_eq!(b.vertex(a).unwrap().vertexuses.len(), 1);
    }

    /// A tracked edit nested in a lent edit is refused before anything changes
    /// (`topology::brep::p3::with_lent_context`), and tracks nothing; once the outer
    /// lend is over, a tracked edit runs.
    #[test]
    fn nested_tracked_edits_are_refused() {
        use crate::topology::brep::p3 as brep_p3;
        let (mut b, [a, _, _], [e0, _]) = wire_chain();
        let mut k = Kernel::default();
        let r = brep_p3::with_kernel_callbacks(&mut k, &mut b, |k, b, _| {
            let (r, changes) =
                tracked_edit(k, b, |k, b, env| brep_p3::squeeze_edge(k, b, env, e0, a));
            assert!(changes.removed.is_empty() && changes.added.is_empty());
            r
        });
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        assert!(!b.edge(e0).unwrap().edgeuses.is_empty());
        assert!(squeeze_edge(&mut k, &mut b, e0, a).is_ok());
    }

    /// The A--B--C chain of [`wire_chain`] with line curves and AB's uses reversed (its
    /// first edge use at B, so `GetVertex`, the vertex `FixDegenEdges` keeps, is B), a
    /// kernel with a recording user callback, and healers queueing AB and BC.
    #[allow(clippy::type_complexity)]
    fn two_healer_setup() -> (
        Brep,
        Kernel,
        std::sync::Arc<std::sync::Mutex<Vec<ObjectKey>>>,
        HealData<L>,
        HealData<L>,
    ) {
        use crate::curve::curve::Curve;
        use crate::curve::line::Line;
        use crate::geom::context::{NotifyCallback, GEOMETRY_KEY_BASE};
        use crate::geom::object::{NotifyOperation, ObjectRef};
        use crate::heal::face_props::Tri;
        use crate::heal::heal_data::p2::EdgeProps;
        use crate::topology::VertexuseId;
        use std::sync::{Arc, Mutex};

        #[derive(Debug)]
        struct Rec(Arc<Mutex<Vec<ObjectKey>>>);
        impl NotifyCallback for Rec {
            fn execute(
                &mut self,
                _operation: NotifyOperation,
                caller: ObjectRef,
                data1: Option<ObjectRef>,
                data2: Option<ObjectRef>,
                data3: Option<ObjectRef>,
            ) -> Result<()> {
                let mut seen = self.0.lock().unwrap();
                for x in [Some(caller), data1, data2, data3].into_iter().flatten() {
                    if x.key.0 >= GEOMETRY_KEY_BASE {
                        seen.push(x.key);
                    }
                }
                Ok(())
            }
        }
        fn queue(e: EdgeId) -> HealData<L> {
            let mut h = HealData::<L>::new();
            let mut p = EdgeProps::for_edge(e, u64::from(e.index()));
            p.bad_degen_edge = Tri::True;
            let id = h.edge_props.alloc(p);
            h.bad_edge_props_degen_edges.prob_array = vec![id];
            h
        }

        let (mut b, [a, bv, c], [e0, e1]) = wire_chain();
        // reverse AB: edge use 0 (SAME) at B, edge use 1 at A
        let (vu0, vu1) = (VertexuseId::from_index(0), VertexuseId::from_index(1));
        b.vertexuse_mut(vu0).unwrap().vertex = Some(bv);
        b.vertexuse_mut(vu1).unwrap().vertex = Some(a);
        b.vertex_mut(a).unwrap().vertexuses = vec![vu1];
        b.vertex_mut(bv).unwrap().vertexuses = vec![vu0, VertexuseId::from_index(2)];
        b.edge_mut(e0).unwrap().vertices = [bv, a];
        assert_eq!(edge::start_end_vertices(&b, e0).unwrap().0, Some(bv));
        for (e, p, q) in [(e0, bv, a), (e1, bv, c)] {
            let (pp, qq) = (b.vertex(p).unwrap().point, b.vertex(q).unwrap().point);
            b.edge_mut(e).unwrap().curve = Some(Curve::from(Line::from_points(pp, qq, 3)));
        }
        let mut k = Kernel::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Rec(Arc::clone(&seen)))));
        (b, k, seen, queue(e0), queue(e1))
    }

    /// The two edits' curve keys, as the callback saw them.
    fn keys_of_two_edits(
        seen: &std::sync::Mutex<Vec<ObjectKey>>,
        first_len: usize,
    ) -> (Vec<ObjectKey>, Vec<ObjectKey>) {
        let all = seen.lock().unwrap().clone();
        (all[..first_len].to_vec(), all[first_len..].to_vec())
    }

    /// Curve identities outlive every healer: through the public `fix_degen_edges`, H1
    /// squeezing AB while keeping B (refused with `SM_ERR` after its removal
    /// notification, both curves still live), then a fresh H2 squeezing BC (keeping B,
    /// succeeds), notify the two curves under distinct keys.
    #[test]
    fn curve_identities_survive_a_fresh_healer() {
        let (mut b, mut k, seen, mut h1, mut h2) = two_healer_setup();
        let r1 = h1.fix_degen_edges(&mut k, &mut b, HealerOpType::FixDegenEdges);
        assert!(
            matches!(r1, Err(SolidError::Upstream { code: SM_ERR, .. })),
            "{r1:?}"
        );
        let first = seen.lock().unwrap().len();
        h2.fix_degen_edges(&mut k, &mut b, HealerOpType::FixDegenEdges)
            .unwrap();
        let (one, two) = keys_of_two_edits(&seen, first);
        assert!(!one.is_empty() && !two.is_empty());
        assert!(two.iter().all(|x| !one.contains(x)), "{one:?} {two:?}");
    }

    /// The same with a clone of H1 made before H1's edit (re-queued on BC): cloned
    /// healer state cannot restart the identities either.
    #[test]
    fn curve_identities_survive_a_cloned_healer() {
        let (mut b, mut k, seen, mut h1, h2) = two_healer_setup();
        let mut h1_clone = h1.clone();
        h1_clone.edge_props = h2.edge_props.clone();
        h1_clone.bad_edge_props_degen_edges = h2.bad_edge_props_degen_edges.clone();
        let r1 = h1.fix_degen_edges(&mut k, &mut b, HealerOpType::FixDegenEdges);
        assert!(
            matches!(r1, Err(SolidError::Upstream { code: SM_ERR, .. })),
            "{r1:?}"
        );
        let first = seen.lock().unwrap().len();
        h1_clone
            .fix_degen_edges(&mut k, &mut b, HealerOpType::FixDegenEdges)
            .unwrap();
        let (one, two) = keys_of_two_edits(&seen, first);
        assert!(!one.is_empty() && !two.is_empty());
        assert!(two.iter().all(|x| !one.contains(x)), "{one:?} {two:?}");
    }

    #[test]
    fn aliased_glue_preserves_a_preexisting_tracker_without_invented_changes() {
        use crate::geom::context::NotifyCallback;
        use crate::topology::topology::TopologyChangeCallback;
        let mut k = Kernel::default();
        let mut b = Brep::new();
        k.context
            .set_sys_notify_callback(Some(Box::new(TopologyChangeCallback::new(27))));
        // Native GlueVertices compares identities before using the pointer.
        let v = VertexId::from_index(0);
        let changes = glue_vertices(&mut k, &mut b, v, v).unwrap();
        assert!(changes.removed.is_empty() && changes.added.is_empty());
        let callback = k
            .context
            .sys_notify_callback()
            .and_then(NotifyCallback::as_any)
            .and_then(|a| a.downcast_ref::<TopologyChangeCallback>())
            .unwrap();
        assert_eq!(callback.tracker(), 27);
        assert!(callback.lists().notify_events.is_empty());
    }
}

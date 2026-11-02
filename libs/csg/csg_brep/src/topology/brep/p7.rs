//! `SmBrep`, part 7: reading a B-rep from a database file, making faces from edge uses
//! or from curves, gluing edges, copying faces between B-reps and merging the
//! self-intersection curves of a surface.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`read_from_file`]                           | `SmBrep::ReadFromFile`                       |
//! | [`make_face`], [`MakeFaceOptions`], [`NewFace`] | `SmBrep::MakeFace`                        |
//! | [`make_face_with_curves`], [`make_face_with_curves_with_kernel_context`], [`make_face_with_curve_owners`], [`FaceCurvesInput`] | `SmBrep::MakeFaceWithCurves` |
//! | [`glue_edges_general`]                       | `SmBrep::GlueEdgesGeneral`                   |
//! | [`copy_faces`], [`copy_faces_with_merge`]    | `SmBrep::CopyFaces`                          |
//! | [`merge_self_intersection_curves`], [`SelfIntersectionCurve`] | `SmBrep::MergeSelfIntersectionCurves` |
//!
//! Deviations:
//! - **Object model.** The B-rep is a [`Brep`] of arenas; the methods are free functions
//!   taking it. Pointer arguments are `Option`s of ids (a null region is the infinite
//!   region), out-parameters are return values. Where upstream dereferences null the port
//!   returns [`SolidError::InvalidInput`] (`NER` is `SM_ERR_NULL_POINTER`).
//! - **`ReadFromFile`** takes the file's contents (the library reads no files: it must
//!   build for `wasm32-unknown-unknown`); the caller reads the file. The reader is
//!   [`BrepData::read_from_file`] (unit `topology.brep_data`), which bounds every count it
//!   reads and the nesting of its polymorphic readers (AGENTS.md section 1).
//! - **Arena records.** `GlueEdgesGeneral` destroys the deleted edge's stable arena
//!   record after its last edgeuse pair moves, and the shell it merges away
//!   (`delete pDelShell`). Other plain `delete`s of edge uses, vertex uses and regions
//!   unlink the record (it stays as an orphan).
//!   `m_pEdgeListHead->Remove` takes the edge off [`Brep::edge_list`] before the
//!   transfers begin. The owning-list edits
//!   `SmEdge::Remove(eu)`, `SmVertex::Remove(vu)`, `SmRegion::Remove(shell)` and
//!   `SmBrep::Remove(region)` (inline `SmOwningTopology` operations) edit the records'
//!   use lists and back links.
//! - **Curves and surfaces are values.** `MakeFaceWithCurves` consumes its curves and
//!   surface (upstream: "all inputs are consumed"), so "two copies, never two references"
//!   holds by construction. `make_face_with_curve_owners` exposes curve-slot owners
//!   for callers that read the consumed curves' native owner pointers; discarded
//!   curves retain `None`, and completed slots survive a later failure. `MakeFace` takes the surface by value (`None` is a null
//!   pointer). `CopyFaces` copies from one B-rep into another one: upstream also allows
//!   the destination to be the source B-rep, which a shared and a mutable borrow cannot
//!   express (copy within one B-rep through a clone).
//! - **Index checks.** Loop counts that add up to more curves, orientations or edge uses
//!   than given, and self-intersection UV curve arrays shorter than the 3D curves, are
//!   [`SolidError::InvalidInput`] (upstream reads past its arrays).
//! - **Flags.** `IsDegenerate`, `IsDegenerateCurve` and `IsDegeneratePoint` return flags
//!   upstream; a Rust error of those tests (other than [`SolidError::Unported`]) counts as
//!   "not degenerate".
//! - **Gap targets on curves on surfaces.** The optional-surface deformation path
//!   retains the actual surface kind and adjusts the UV curve using surface drops,
//!   preserving upstream's default orientation and tangent choices.
//! - **Statuses ignored upstream** (`LiftCurve`, `DeformToEndPointTargets`, `ConvertTo2D`
//!   of the UV curves, `GetEnds`, `CopyAndAddAnalytics`, `FindRadialSector`,
//!   `SetUVTrimCurve`, `TestClosureSplitRegion`, `sm_CheckAndFixUVTrimCurves`,
//!   `RebuildUVTrimCurves`, `MakeFaceuse`) are ignored; [`SolidError::Unported`] always
//!   propagates. A failed `LiftCurve` leaves no 3D curve (a degenerate one), as upstream;
//!   a failed `GetEnds` keeps the previous end point. A failed `CopyAndAddAnalytics` is
//!   [`SolidError::InvalidInput`] (upstream then dereferences the null copy), as is
//!   `pbOptCrvOnSurf` without UV curves (upstream loops over a null array).
//! - **Wire edges.** `MakeWireEdge` / `MakeWireEdgeVertex` (unit `topology.brep_p3`, wired
//!   by its PR #204) notify through the caller's notification environment, as the
//!   other edits here do.
//! - **Unported callees** return [`SolidError::Unported`]: `DeleteEdge`
//!   (`topology.brep_p4`). `GlueVertices` calls `topology.brep_p5` with the caller's
//!   notification environment. `CopyFaces` with the merge builds `boolean.merge_p2`'s
//!   merge and calls its `MakeFaceuse` (so it takes the kernel mutably, for the
//!   context's `DoingBoolean`); its copies are notified in `env`'s one identity space.
//!   `MergeSelfIntersectionCurves` classifies each UV curve with
//!   `intersect.curve_class_p4`'s constructor and `query.face_p2`'s `CurveOnClassify`,
//!   then `intersect.curve_class_p1`'s `SetCurve` and `intersect.curve_class_p5`'s
//!   `MergeIntervals`, over `topology.face_p3`'s [`IntersectObject`] handles (the merge's
//!   curve `i` is `Curve(i)`, its UV curves `UvCurve(i, false / true)`, the surface
//!   `Surface(face)`); their unported callees defer the call. The merges run
//!   `merge_intervals_mut` over the B-rep (upstream edits it through the
//!   classification's topology pointers), so `MergeIntoObject` makes its vertex loops,
//!   edge splits and edges in `b`; the caller's `curve_ref` names the curves of split
//!   edges in the notifications. The 3D curves are the call's copies of the caller's
//!   (`SetXSectCurveThroughPoint` edits those copies, not the caller's values, which
//!   are borrowed shared). The classification's
//!   borrowed interval array is not modelled (`lOptDataSize = 16` is passed through). The
//!   upstream interval loop around them is ported (`GetSize() != 1` skips before the
//!   second merge, `SM_ERR` after it, `SE(SM_ERR)` for a middle not on an edge).
//!   `SmShell::IsInnerShell` (merged shells) calls `topology.shell`'s port;
//!   `MakeFaceTopology` and `TestClosureSplitRegion(faceuse)` call `topology.brep_p6`'s;
//!   `MakeTopologyFromData` and `TestClosureSplitRegion(edge)` call
//!   `topology.brep_p10`'s (`ReadFromFile` takes that unit's [`TopologyEnv`]).
//!   `GetInfiniteRegion` is the inline read [`Brep::infinite_region`], and the glue's
//!   removal of an emptied region walks `topology.brep_p1`'s region list.
//!   `MakeFaceTopology` names the face by the notification environment's identity
//!   and builds it on a copy of the surface (with no surface identity: its owner
//!   changes are not notified), which `MakeFace` then replaces with its own
//!   (upstream: one shared surface whose owner `MakeFace` restores). The copy shares
//!   the original's cache slot, so a surface cache built while making the face stays
//!   with the installed surface as upstream's does. It claims its
//!   marks through a fresh [`Marks`] record per call, as each call takes a new mark.
//! - **Original-surface transfer ownership (#894).** Deliberately correcting pinned
//!   C++ `SmBrep.cpp:18859-18889`, the original-transfer path never restores the old
//!   owner after installation, even on a late failure. The installed original keeps
//!   its identity and partial topology; its owner, geometry-owner registration and
//!   cached tolerance agree with the installed face before returning the failure.
//!   The legacy-copy path retains upstream's owner-restoration behavior.
//! - **`MakeFace`** returns nothing when `MakeFaceTopology` fails; upstream still hands
//!   back the region and shell it may have made (`MakeFaceWithCurves` warns about it).
//! - **`GlueEdgesGeneral`**: the arena cannot share one curve between two edges, so
//!   upstream's `pSurvCurve == pDelCurve` short cut (no `CurveMaxDistanceBetween`) is taken
//!   only for two edges without curve (`NULL == NULL`). The glued-away edge leaves the
//!   owning list before transfers and is destroyed after its last edgeuse pair moves,
//!   as upstream does; unrelated typed ids remain valid.
//! - **Notification context.** Face-from-curves entrypoints share one algorithm.
//!   The kernel-context variant binds caller-owned stores to the actual kernel only
//!   during construction notifications; the external-context API keeps its
//!   explicit external context, including null.
//! - **Bounds.** No recursion; every loop is over input arrays or B-rep lists.

use crate::boolean::merge::p1::{MergeState, VertexCopy};
use crate::boolean::merge::p2::{make_faceuse_with, MergeContext, MergeOperand, TopologyCopy};
use crate::boolean::topology_intersector::p1::{Relations, TopologyIntersector};
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol::zone_tol3d;
use crate::geom::tools::{
    find_coincident_edge, find_first_vertex_within_tol_to_pnt, CoincidentEdgeQuery,
};
use crate::intersect::curve_class::p1::{CurveClassification, CurveInterval};
use crate::intersect::curve_class::p4::ClassificationInit;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::curve_class::p6::{MergeContext as ClassMergeContext, MergeIntervalBrep};
use crate::intersect::point_class::{ClassGeometry, PointClassType};
use crate::kernel::Kernel;
use crate::math::{Point3, EFF_ZERO, EFF_ZERO_DEG, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::InValueType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p2 as qf2;
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::brep::p1;
use crate::topology::brep::p10::{self, MakeTopologyOptions, TopologyEnv};
use crate::topology::brep::p2;
use crate::topology::brep::p4;
use crate::topology::brep::p6;
use crate::topology::brep_data::BrepData;
use crate::topology::database_io::FileType;
use crate::topology::edge as ed;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as fp2;
use crate::topology::face::p3::{IntersectObject, IntersectObjects};
use crate::topology::topology_traverser::{self as tt, Marks};
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, LoopId, RegionId, ShellId, VertexId, VertexuseId,
};

const OP: &str = "topology::brep::p7";

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `SM_ZONE_TOL_3D / 10.0`: the default point tolerance of the surface degeneracy tests.
const DEGENERATE_TOL_3D: f64 = crate::geom::tol::ZONE_TOL_3D / 10.0;

/// `SmBrep::Remove(region)` (the inline `SmOwningTopology::Remove`): takes `region` off
/// the B-rep's region list (`topology.brep_p1`'s [`p1::regions`] order); removing the
/// list head passes the head to the next region, or `NULL` when the list empties.
///
/// # Errors
///
/// `SM_ERR` when `region` is not on the list.
fn brep_remove_region(b: &mut Brep, region: RegionId) -> Result<()> {
    let list = p1::regions(b, None)?;
    if !list.contains(&region) {
        return Err(SolidError::sm_err(OP));
    }
    let head = list.first().copied();
    let new_head = if head == Some(region) {
        list.get(1).copied()
    } else {
        head
    };
    b.region_mut(region)?.in_brep = false;
    b.set_region_list_head(new_head);
    Ok(())
}

// ---- unported callees ---------------------------------------------------------------------

/// `SmBrep::Notify(op, data1, data2, data3)` ([`p2::notify`] through the caller's `env`;
/// no Rust caller holds the B-rep's `SM_OC_BREP` cache). Every call site ignores the
/// status (`SmBrep.cpp:9840`, `:9854`): only [`SolidError::Unported`] and the port's
/// representation failures propagate.
fn brep_notify(
    b: &mut Brep,
    env: &mut ed::NotifyEnv<'_>,
    op: NotifyOperation,
    data: [Option<p2::NotifyArg>; 3],
) -> Result<()> {
    ignore_status(p2::notify(b, env, None, op, data))
}

/// The B-rep itself as a notification datum (`this`).
fn this_brep(b: &Brep) -> p2::NotifyArg {
    p2::NotifyArg::Object(p2::brep_ref(b))
}

/// `SmBrep::SetTolerance(tol, bUpdateOnlyIfLarger, bCascadeToBndries)` (`SM_USE_OLDTOL`;
/// [`p2::set_tolerance`]); the status is ignored (`SmBrep.cpp:2014`).
fn brep_set_tolerance(
    k: &Kernel,
    b: &mut Brep,
    tol: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    ignore_status(p2::set_tolerance(
        k,
        b,
        tol,
        update_only_if_larger,
        cascade_to_boundaries,
    ))
}

/// `sm_CheckAndFixUVTrimCurves(this)` ([`p2::check_and_fix_uv_trim_curves`]).
fn check_and_fix_uv_trim_curves(k: &Kernel, b: &mut Brep) -> Result<()> {
    p2::check_and_fix_uv_trim_curves(k, b)
}

/// `RebuildUVTrimCurves()` ([`p2::rebuild_uv_trim_curves`]).
fn rebuild_uv_trim_curves(k: &Kernel, b: &mut Brep) -> Result<()> {
    p2::rebuild_uv_trim_curves(k, b)
}

/// `MakeWireEdge(region, start, end, curve, ivl, orient, rpNewEdge)`
/// ([`crate::topology::brep::p3::make_wire_edge`], unit `topology.brep_p3`), notifying
/// through `env`.
#[allow(clippy::too_many_arguments)]
fn make_wire_edge(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    start: VertexId,
    end: VertexId,
    curve: Curve,
    ivl: Extent1d,
    orient: OrientType,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<EdgeId> {
    crate::topology::brep::p3::make_wire_edge(k, b, env, region, start, end, curve, &ivl, orient)
}

/// `MakeWireEdgeVertex(region, start, curve, ivl, orient, endPt, rpNewEdge, rpEndVertex)`
/// ([`crate::topology::brep::p3::make_wire_edge_vertex`], unit `topology.brep_p3`), as
/// [`make_wire_edge`].
#[allow(clippy::too_many_arguments)]
fn make_wire_edge_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    start: VertexId,
    curve: Curve,
    ivl: Extent1d,
    orient: OrientType,
    end_pt: Point3,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<(EdgeId, VertexId)> {
    crate::topology::brep::p3::make_wire_edge_vertex(
        k, b, env, region, start, curve, &ivl, orient, end_pt,
    )
}

/// `DeleteEdge(edge)` (default `pOptSurfaceToKeep = NULL`), with the caller's
/// notification records, attributes and identities (and context, else the kernel's).
fn delete_edge(k: &mut Kernel, b: &mut Brep, e: EdgeId, env: &mut ed::NotifyEnv<'_>) -> Result<()> {
    let mut d = p4::DeleteEnv::from_notify(env);
    p4::delete_edge(k, b, &mut d, e, None)
}

/// `GlueVertices(survivor, other)` (default `pOptWireEdgesInBrep = NULL`), with the
/// caller's notification environment.
fn glue_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    v1: VertexId,
    v2: VertexId,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<()> {
    super::p5::glue_with_notifications(k, b, Some(v1), Some(v2), None, Some(env))
}

/// `TestClosureSplitRegion(TRUE, edge, aNewShells, aNewRegions)` (unit
/// `topology.brep_p10`; status ignored by the caller).
fn test_closure_split_region_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut ed::NotifyEnv<'_>,
    e: EdgeId,
) -> Result<()> {
    p10::test_closure_split_region(k, b, env, true, e).map(|_| ())
}

/// `MakeTopologyFromData(pBrepData, sAttributes, bMakeUVTrimCurves, bAddAnalytics,
/// bForceSameTol)` (unit `topology.brep_p10`) with the defaults `bCheckUVCurves =
/// FALSE`, `bAlwaysHeal = FALSE`, `bHealerIsEnabled = TRUE`. `ReadFromFile` passes an
/// empty local `sAttributes`, so the records' attribute indices refer to nothing.
fn make_topology_from_data(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    make_uv_trim_curves: bool,
    add_analytics: bool,
    force_same_tol: bool,
) -> Result<()> {
    // SmTArray<SmAttribute*> sAttributes; (empty)
    let saved = std::mem::take(&mut env.all_attributes);
    let r = p10::make_topology_from_data(
        k,
        b,
        data,
        env,
        MakeTopologyOptions {
            make_uv_trim_curves,
            add_analytics,
            force_same_tol,
            check_uv_trim_curves: false,
            always_heal: false,
            healer_enabled: true,
        },
    );
    env.all_attributes = saved;
    r
}

/// `SmFace::SetTolerance(tol, bUpdateOnlyIfLarger, bCascadeToBndries)` (`SM_USE_OLDTOL`).
fn face_set_tolerance(
    b: &mut Brep,
    f: FaceId,
    tol: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    crate::topology::face::p3::set_tolerance(
        b,
        f,
        tol,
        update_only_if_larger,
        cascade_to_boundaries,
    )
}

/// `pNewFace->CreateUVTrimCurves(bAdjustTolerances, &uvCurves, &orients, ..)`.
fn face_create_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    adjust_tolerances: bool,
    uv: Vec<Curve>,
    orients: &[OrientType],
) -> Result<()> {
    let mut curves: Vec<Option<Curve>> = uv.into_iter().map(Some).collect();
    // the gap statistics (the NULL out-parameters) are not read
    crate::topology::face::p3::create_uv_trim_curves(
        k,
        b,
        f,
        adjust_tolerances,
        Some((&mut curves, orients)),
    )
    .map(|_| ())
}

/// `pFace->GetBrep() == this` (unit `topology.face_p4`): a face of this arena whose
/// shell's region is in the B-rep.
fn face_in_brep(b: &Brep, f: FaceId) -> Result<bool> {
    Ok(crate::topology::face::p4::brep(b, f)?.is_some())
}

/// `pFace->GetEdges(sEdges)` (unit `topology.face_p4`; default arguments).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

/// `SmMerge sMerge(ctx, pDestinationBrep, this, pDestinationBrep->GetTolerance(), 20 deg)`,
/// then per face the `GetBrep() == this` check (`SM_ERR`) and its
/// `MakeFaceuse(pFace->GetUpwardFaceuse(), .., NULL, &sNewFaceList)` with the context's
/// `DoingBoolean` set (`SmTemporaryChangeValue`); `MakeFaceuse`'s status is ignored (`SE`).
/// The copy notifications name source and destination entities through the one identity
/// space of `env` (`this_face` for the new faces): entities whose arena ids coincide share
/// an identity.
fn merge_copy_faces(
    mut context: MergeContext<'_>,
    src: &mut Brep,
    dest: &mut Brep,
    faces: &[FaceId],
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
) -> Result<Vec<FaceId>> {
    let mut new_faces = Vec::new();
    for &f in faces {
        if !face_in_brep(src, f)? {
            return Err(SolidError::sm_err(OP));
        }
        let faceuse = fp2::upward_faceuse(src, f)?
            .ok_or(SolidError::invalid_input(OP, "face without face use"))?;
        let doing_boolean = context.kernel().context.doing_boolean();
        context.kernel_mut().context.set_doing_boolean(true);
        let mut list = Vec::new();
        let made = make_faceuse_with(
            context.reborrow(),
            dest,
            src,
            faceuse,
            None,
            relations,
            copy,
            Some(&mut list),
        );
        context
            .kernel_mut()
            .context
            .set_doing_boolean(doing_boolean);
        ignored(made)?;
        new_faces.extend(list);
    }
    Ok(new_faces)
}

/// The classification of a self-intersection curve (`SmCurveClassification`), over
/// [`IntersectObject`] handles: the curve at index `i` of the merge's curves is
/// [`IntersectObject::Curve`] `(i)`, its UV curves [`IntersectObject::UvCurve`].
type SelfClassification = CurveClassification<IntersectObject>;

/// `SmCurveClassification sCC(uvCurve, ivl, uvCurve, tol, 16, data, FALSE, TRUE,
/// &surface)` with `surface` the surface of `face`, then `CurveOnClassify(TRUE, sCC)` on
/// each of `faces` (the remaining arguments default).
fn classify_on_faces(
    k: &Kernel,
    objects: &mut SelfMergeObjects<'_, '_>,
    faces: &[FaceId],
    face: FaceId,
    uv_curve: IntersectObject,
    ivl: Extent1d,
    tol: f64,
) -> Result<SelfClassification> {
    let mut cc = CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(uv_curve),
            interval: ivl,
            uv_curve: Some(uv_curve),
            src_zone_tol_3d: zone_tol3d(k, tol),
            opt_data_size: 16,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: Some(IntersectObject::Surface(face, false)),
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )?;
    for &f in faces {
        qf2::curve_on_classify_mut(k, f, objects, true, &mut cc, false, false, true)?;
    }
    Ok(cc)
}

/// The objects of [`merge_self_intersection_curves`]' merges over the B-rep they edit:
/// upstream's `MergeIntervals` edits the B-rep through the classification's `SmFace*`,
/// `SmEdge*` and `SmVertex*` pointers (a vertex loop, an edge split, a new edge), and
/// `SetXSectCurveThroughPoint` edits the merged 3D curve. The handles are those of
/// [`IntersectObjects`] over one B-rep (side `false`).
struct SelfMergeObjects<'a, 'e> {
    b: &'a mut Brep,
    curves_3d: &'a mut [Curve],
    uv1: &'a [Option<Curve>],
    uv2: &'a [Option<Curve>],
    env: &'a mut ed::NotifyEnv<'e>,
    curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
}

impl SelfMergeObjects<'_, '_> {
    /// The read-only objects over the same B-rep and curves.
    fn view(&self) -> IntersectObjects<'_> {
        IntersectObjects {
            b: &*self.b,
            other: &*self.b,
            side: false,
            curves_3d: &*self.curves_3d,
            uv1: self.uv1,
            uv2: self.uv2,
        }
    }
}

impl ClassGeometry<IntersectObject> for SelfMergeObjects<'_, '_> {
    fn classification_vertex_point(&self, o: IntersectObject) -> Option<Point3> {
        self.view().classification_vertex_point(o)
    }
    fn topology_zone_tol_3d(&self, o: IntersectObject) -> Option<f64> {
        self.view().topology_zone_tol_3d(o)
    }
    fn curve(&self, o: IntersectObject) -> Option<&Curve> {
        self.view().object_curve(o)
    }
    fn surface(&self, o: IntersectObject) -> Option<&Surface> {
        self.view().object_surface(o)
    }
    fn volume(&self, _o: IntersectObject) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: IntersectObject) -> Option<Point3> {
        None
    }
    fn topology_entity(&self, o: IntersectObject) -> Option<(&Brep, EntityRef)> {
        self.view().object_entity(o)
    }
    fn topology_handle(&self, e: EntityRef) -> Option<IntersectObject> {
        self.view().topology_handle(e)
    }
    fn topology_handle_in(&self, brep: &Brep, e: EntityRef) -> Option<IntersectObject> {
        self.view().topology_handle_in(brep, e)
    }
}

impl ClassGeometryMut<IntersectObject> for SelfMergeObjects<'_, '_> {
    fn curve_mut(&mut self, o: IntersectObject) -> Option<&mut Curve> {
        match o {
            IntersectObject::Curve(i) => self.curves_3d.get_mut(i),
            _ => None,
        }
    }
    fn topology_entity_mut(&mut self, o: IntersectObject) -> Option<(&mut Brep, EntityRef)> {
        match o {
            // (both sides name this one B-rep)
            IntersectObject::Topology(e, _) => Some((&mut *self.b, e)),
            _ => None,
        }
    }
}

impl qf2::CurveOnClassifyObjects<IntersectObject> for SelfMergeObjects<'_, '_> {
    fn face(&self, f: FaceId) -> IntersectObject {
        self.view().face(f)
    }
    fn vertex(&self, v: VertexId) -> IntersectObject {
        self.view().vertex(v)
    }
    fn edge(&self, e: EdgeId) -> IntersectObject {
        self.view().edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> IntersectObject {
        self.view().vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> IntersectObject {
        self.view().edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> IntersectObject {
        self.view().edge_curve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> IntersectObject {
        self.view().trim_curve(eu)
    }
    fn face_surface(&self, f: FaceId) -> Option<IntersectObject> {
        self.view().face_surface(f)
    }
    fn loop_id(&self, tgt_loop: IntersectObject) -> Option<LoopId> {
        self.view().loop_id(tgt_loop)
    }
}

impl qf2::FaceBrep for SelfMergeObjects<'_, '_> {
    fn brep(&self) -> &Brep {
        self.b
    }
}

impl qf2::CurveOnClassifyBrep<IntersectObject> for SelfMergeObjects<'_, '_> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.b
    }
}

impl MergeIntervalBrep<IntersectObject> for SelfMergeObjects<'_, '_> {
    fn merge_context(
        &mut self,
        o: IntersectObject,
    ) -> Option<ClassMergeContext<'_, IntersectObject>> {
        let IntersectObject::Topology(target, side) = o else {
            return None;
        };
        if !matches!(
            target,
            EntityRef::Vertex(_) | EntityRef::Edge(_) | EntityRef::Face(_) | EntityRef::Region(_)
        ) {
            return None;
        }
        Some(ClassMergeContext {
            brep: &mut *self.b,
            target,
            handle: Box::new(move |e| IntersectObject::Topology(e, side)),
            notifications: ed::NotifyEnv {
                records: &mut *self.env.records,
                attributes: &mut *self.env.attributes,
                context: self.env.context.as_deref_mut(),
                key_of: self.env.key_of,
            },
            curve_ref: self.curve_ref,
        })
    }
}

/// `sCC.SetCurve(curve_3d)` and `sCC.MergeIntervals(&newFaces, NULL, NULL, face)`: the
/// faces the merge made.
fn set_curve_and_merge(
    k: &mut Kernel,
    objects: &mut SelfMergeObjects<'_, '_>,
    cc: &mut SelfClassification,
    curve_3d: IntersectObject,
) -> Result<Vec<FaceId>> {
    cc.set_curve(Some(curve_3d), false);
    let merged = cc.merge_intervals_mut(k, objects, false)?;
    merged
        .faces
        .into_iter()
        .map(|o| match o {
            IntersectObject::Topology(EntityRef::Face(f), _) => Ok(f),
            _ => Err(null("merged face is not a face")),
        })
        .collect()
}

/// `SM_CAST_PTR(SmEdge, rCIvl.m_vMid.GetObject())` of an interval whose middle is on an
/// edge (upstream dereferences the result).
fn mid_edge(ivl: &CurveInterval<IntersectObject>) -> Result<EdgeId> {
    match ivl.mid.object() {
        Some(IntersectObject::Topology(EntityRef::Edge(e), _)) => Ok(e),
        _ => Err(null("interval middle on an edge without edge")),
    }
}

// ---- owning-list edits ----------------------------------------------------------------------

/// `SmEdge::Remove(eu)`: unlinks the edge use from the edge.
fn edge_remove(b: &mut Brep, e: EdgeId, eu: EdgeuseId) -> Result<()> {
    let list = &mut b.edge_mut(e)?.edgeuses;
    let Some(at) = list.iter().position(|&x| x == eu) else {
        return Err(SolidError::sm_err(OP));
    };
    list.remove(at);
    b.edgeuse_mut(eu)?.edge = None;
    Ok(())
}

/// Merges `del_shell` into `keep_shell`, deletes it (`delete pDelShell`: its destruction
/// notification under `env`, its record and attribute uses) and drops the emptied
/// region (the shell-merging tails of `GlueEdgesGeneral`).
fn merge_shells(
    k: &mut Kernel,
    b: &mut Brep,
    mut del: ShellId,
    mut keep: ShellId,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<()> {
    // m_pInfiniteRegion (NULL when unset; pointer equality, NULL == NULL)
    let inf = b.infinite_region();
    let keep_region = eu1::shell_region(b, keep)?;
    let del_region = eu1::shell_region(b, del)?;
    if keep_region == inf {
        // ok.
    } else if del_region == inf
        // pKeepShell->IsInnerShell(bIsInner): swap when it succeeds with TRUE (asked
        // only when neither region is the infinite one)
        || ignored(crate::topology::Shell::is_inner_shell(k, b, keep))? == Some(true)
    {
        core::mem::swap(&mut del, &mut keep);
    }

    tt::remove_shell(k, b, del, keep)?;

    // remove it from its region, then the region when empty
    let reg = eu1::shell_region(b, del)?.ok_or_else(|| null("shell without region"))?;
    // pReg->Remove(pDelShell); delete pDelShell (SmBrep.cpp:10200, 10429: ~SmShell, ported
    // as shell::destroy); then pReg->GetSize()
    crate::topology::shell::region_remove_shell(b, reg, del)?;
    crate::topology::shell::destroy(k, b, del, env)?;
    if b.region(reg)?.shells.is_empty() {
        remove_emptied_region(b, reg)?;
    }
    Ok(())
}

/// The tail of the shell merge once `reg` lost its last shell: `this->Remove(pReg)`
/// (status ignored), then `m_pInfiniteRegion = m_pList`.
fn remove_emptied_region(b: &mut Brep, reg: RegionId) -> Result<()> {
    // Upstream deletes pReg even when Remove fails; a failed removal here leaves the
    // region (and its in_brep) as it was. Unreachable: a merged shell's region is listed.
    ignore_status(brep_remove_region(b, reg))?;
    // make sure infinite region points where it should: the first region of the list
    // (a stored head of None starts at the first region on the list; it is not an
    // empty list)
    let head = p1::regions(b, None)?.first().copied();
    b.set_infinite_region(head);
    Ok(())
}

// ---- ReadFromFile ---------------------------------------------------------------------------

/// `ReadFromFile`: reads one B-rep from the contents of a database file of `file_type`
/// into `b` (a B-rep as `SmBrep()` makes it: see
/// [`Brep::new_with_infinite_region`]), then checks and fixes its UV trim curves, or
/// rebuilds them with `rebuild_uv_trim_curves`. `env` names the new topology objects and
/// receives their records and the attributes of the geometry copies (its
/// `all_attributes` is not used: upstream passes an empty array).
///
/// # Errors
///
/// Those of [`BrepData::read_from_file`] and
/// [`p10::make_topology_from_data`]; [`SolidError::Unported`]
/// (`sm_CheckAndFixUVTrimCurves`, `RebuildUVTrimCurves`, and the callees of
/// `MakeTopologyFromData`).
pub fn read_from_file(
    k: &mut Kernel,
    b: &mut Brep,
    data: Vec<u8>,
    file_type: FileType,
    rebuild_uv_trim_curves_flag: bool,
    env: &mut TopologyEnv<'_>,
) -> Result<()> {
    let mut brep_data = BrepData::read_from_file(k, data, file_type)?;
    // MakeTopologyFromData(pBrepData, sAttributes, FALSE, TRUE, TRUE)
    make_topology_from_data(k, b, &mut brep_data, env, false, true, true)?;
    drop(brep_data);

    // check the UV trim curves unless they are rebuilt (statuses ignored)
    if !rebuild_uv_trim_curves_flag {
        ignore_status(check_and_fix_uv_trim_curves(k, b))?;
    }
    if rebuild_uv_trim_curves_flag {
        ignore_status(rebuild_uv_trim_curves(k, b))?;
    }
    Ok(())
}

// ---- MakeFace -------------------------------------------------------------------------------

/// The optional arguments of [`make_face`] (upstream defaults: no radial sector check, no
/// UV trim curves, none given).
#[derive(Debug, Clone, Copy, Default)]
pub struct MakeFaceOptions<'a> {
    /// `bVerifyRadialSectors`: check the face goes into the right radial sectors
    /// (non-manifold models).
    pub verify_radial_sectors: bool,
    /// `bCalculateTrims`: attach or make a UV trim curve on every edge use.
    pub calculate_trims: bool,
    /// `cpOptUVCurves`: UV trim curves to assign to the new edge uses (`None` entries
    /// are null).
    pub uv_curves: Option<&'a [Option<BSplineCurve>]>,
}

/// The topology [`make_face`] and [`make_face_with_curves`] made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NewFace {
    /// `rpNewRegion`: the region split off, if the face split one.
    pub region: Option<RegionId>,
    /// `rpNewShell`: the new shell, if any.
    pub shell: Option<ShellId>,
    /// `rpNewFace`: the new face.
    pub face: Option<FaceId>,
}

/// What [`make_face_attempt`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct MakeFaceAttempt {
    /// The status of the build: `Ok(())` on success, or the upstream failure.
    pub status: Result<()>,
    /// `rpNewRegion`: the region split off, if the face split one.
    pub region: Option<RegionId>,
    /// `rpNewShell`: the new shell, if any.
    pub shell: Option<ShellId>,
    /// `rpNewFace`: the new face.
    pub face: Option<FaceId>,
}

/// `MakeFace` attempt variant preserving partial outputs (such as `region` on split).
#[allow(clippy::too_many_arguments)]
pub fn make_face_attempt(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    edgeuses: &[EdgeuseId],
    loop_vertices: &[VertexId],
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    orientation: OrientType,
    opts: &MakeFaceOptions<'_>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<MakeFaceAttempt> {
    make_face_attempt_with_state(
        k,
        b,
        region,
        loops,
        edgeuses,
        loop_vertices,
        surface,
        uv_domain,
        orientation,
        opts,
        this_face,
        &mut ed::NotifyState::from_env(env),
    )
}

#[allow(clippy::too_many_arguments)]
fn make_face_attempt_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    edgeuses: &[EdgeuseId],
    loop_vertices: &[VertexId],
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    orientation: OrientType,
    opts: &MakeFaceOptions<'_>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyState<'_>,
) -> Result<MakeFaceAttempt> {
    let Some(surface) = surface else {
        return Err(SolidError::null_pointer(OP));
    };
    let mut transfer = p6::FaceSurfaceTransfer::new(surface);
    let mut attempt = MakeFaceAttempt {
        status: Ok(()),
        region: None,
        shell: None,
        face: None,
    };
    make_face_transfer_attempt_with_state(
        k,
        b,
        region,
        loops,
        edgeuses,
        loop_vertices,
        &mut transfer,
        uv_domain,
        orientation,
        opts,
        this_face,
        env,
        true,
        &mut attempt,
    )?;
    Ok(attempt)
}

#[allow(clippy::too_many_arguments)]
fn make_face_transfer_attempt_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    edgeuses: &[EdgeuseId],
    loop_vertices: &[VertexId],
    transfer: &mut p6::FaceSurfaceTransfer,
    uv_domain: &Extent2d,
    orientation: OrientType,
    opts: &MakeFaceOptions<'_>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyState<'_>,
    legacy: bool,
    out: &mut MakeFaceAttempt,
) -> Result<()> {
    *out = MakeFaceAttempt {
        status: Ok(()),
        region: None,
        shell: None,
        face: None,
    };
    transfer.owned_surface()?;
    let region = region.or(b.infinite_region());
    let old_owner = transfer.owned_surface()?.owner();
    let options = p6::FaceTopologyOptions {
        region,
        edge_loops: loops,
        edgeuses,
        loop_vertices,
        verify_radial_sectors: opts.verify_radial_sectors,
        calculate_trims: opts.calculate_trims,
        uv_curves: opts.uv_curves,
        surface_key: None,
    };
    let attempt = if legacy {
        // The copy shares the original's caches: upstream builds the face on the one
        // surface `MakeFace` keeps, so a cache made while building it stays with it.
        let mut copy =
            p6::FaceSurfaceTransfer::new(transfer.owned_surface()?.clone_sharing_cache());
        p6::make_face_topology_transfer_attempt_with_state(
            k,
            b,
            &mut Marks::default(),
            env,
            &mut copy,
            orientation,
            &options,
        )
    } else {
        p6::make_face_topology_transfer_attempt_with_state(
            k,
            b,
            &mut Marks::default(),
            env,
            transfer,
            orientation,
            &options,
        )
    };
    // Only the legacy copy leaves the original outside the installed face.
    if legacy {
        transfer.surface_mut(b)?.set_owner(old_owner);
    }
    out.region = attempt.region;
    out.shell = attempt.shell;
    out.face = attempt.face;
    let result = (|| {
        if let Err(error) = attempt.status {
            if matches!(error, SolidError::Upstream { .. }) {
                out.status = Err(error);
                return Ok(());
            }
            return Err(error);
        }
        let new_face = attempt.face.ok_or_else(|| null("MakeFace made no face"))?;
        b.face_mut(new_face)?.uv_domain = *uv_domain;
        let tol = b.tolerance();
        face_set_tolerance(b, new_face, tol, true, true)?;
        let owner = this_face(new_face);
        if legacy {
            transfer.install(b, new_face)?;
        }
        transfer.surface_mut(b)?.set_owner(Some(owner));
        b.geometry_owner_keys
            .insert(EntityRef::Face(new_face), owner);
        b.sync_face_surface_tolerance(new_face)?;
        Ok(())
    })();
    if !legacy && (result.is_err() || out.status.is_err()) {
        if let Some(face) = transfer.installed_face() {
            // Topology may fail after installing the original. Keep that partial
            // result canonical without changing its status or construction order.
            let owner = this_face(face);
            transfer.surface_mut(b)?.set_owner(Some(owner));
            b.geometry_owner_keys.insert(EntityRef::Face(face), owner);
            b.sync_face_surface_tolerance(face)?;
        }
    }
    result
}

/// `MakeFace`: makes a face on `surface` (over `uv_domain`) in `region` (the infinite
/// region for `None`) from existing edge uses, `loops` giving the number of edge uses of
/// each loop (outer loop first, counter-clockwise when `orientation` is
/// [`OrientType::Same`]), and loop vertices. The face owns the surface and takes the
/// B-rep's tolerance. `this_face` names the new face for its surface's owner.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without surface; those of the topology build and tolerance
/// propagation, including [`SolidError::Unported`] from their unfinished callees.
#[allow(clippy::too_many_arguments)]
pub fn make_face(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    edgeuses: &[EdgeuseId],
    loop_vertices: &[VertexId],
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    orientation: OrientType,
    opts: &MakeFaceOptions<'_>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<NewFace> {
    make_face_with_state(
        k,
        b,
        region,
        loops,
        edgeuses,
        loop_vertices,
        surface,
        uv_domain,
        orientation,
        opts,
        this_face,
        &mut ed::NotifyState::from_env(env),
    )
}

#[allow(clippy::too_many_arguments)]
fn make_face_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    edgeuses: &[EdgeuseId],
    loop_vertices: &[VertexId],
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    orientation: OrientType,
    opts: &MakeFaceOptions<'_>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyState<'_>,
) -> Result<NewFace> {
    let attempt = make_face_attempt_with_state(
        k,
        b,
        region,
        loops,
        edgeuses,
        loop_vertices,
        surface,
        uv_domain,
        orientation,
        opts,
        this_face,
        env,
    )?;
    attempt.status?;
    Ok(NewFace {
        region: attempt.region,
        shell: attempt.shell,
        face: attempt.face,
    })
}

// ---- MakeFaceWithCurves ---------------------------------------------------------------------

/// The curves of [`make_face_with_curves`]: one entry of `loops` per loop (its curve count,
/// outer loop first) over the curves in loop order, each with its orientation; 3D curves
/// (`None` entries are degenerate), UV curves (B-spline kinds), or both.
#[derive(Debug, Clone, Default)]
pub struct FaceCurvesInput {
    /// `crCurveLoops`.
    pub loops: Vec<usize>,
    /// `cpOpt3DCurves`.
    pub curves_3d: Option<Vec<Option<Curve>>>,
    /// `cpOptUVCurves`.
    pub uv_curves: Option<Vec<Curve>>,
    /// `crCurveOrientations`.
    pub orientations: Vec<OrientType>,
    /// `crLoopPoints`: the points of vertex loops.
    pub loop_points: Vec<Point3>,
}

/// The curve of `curves[i]`, which upstream reads unchecked.
fn at<T>(v: &[T], i: usize) -> Result<&T> {
    v.get(i)
        .ok_or_else(|| null("loop counts exceed the curves"))
}

/// The start of `c` as oriented by `orient` (`GetEnds`, status ignored: `prev` kept).
fn oriented_end(
    k: &Kernel,
    c: &Curve,
    orient: OrientType,
    start: bool,
    prev: Point3,
) -> Result<Point3> {
    Ok(match ignored(c.ends(k))? {
        Some(e) => {
            if (orient == OrientType::Same) == start {
                e.start
            } else {
                e.end
            }
        }
        None => prev,
    })
}

/// `MakeFaceWithCurves`: makes a face on `surface` (over `uv_domain`, `orientation` as in
/// [`make_face`]) from loops of curves: with UV curves only, each is lifted to 3D (or made
/// a curve on surface when `crv_on_surf`) and gaps larger than the B-rep tolerance are
/// closed by moving the curve ends to the gap middles; then wire edges and vertices are
/// made per curve (degenerate curves dropped, coincident ones shared) and the face is
/// made from their edge uses. `this_face` names the new face for its surface's owner.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` without curves or loops, for a degenerate surface, or when a
/// loop does not close; `SM_ERR` for an edge shorter than the tolerance that closes;
/// [`SolidError::InvalidInput`] for loop counts exceeding the curves; those of the
/// evaluations; [`SolidError::Unported`] (see the module docs).
pub fn make_face_with_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<NewFace> {
    make_face_with_curves_impl(
        k,
        b,
        region,
        input,
        surface,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        &mut Vec::new(),
        &mut ed::NotifyState::from_env(env),
    )
}

/// [`make_face_with_curves`] retaining each input curve slot's owning edge.
/// Degenerate and coincident curves discarded by construction have no owner;
/// output slots preserve their original input positions. Completed owners remain
/// in `curve_owners` if a later construction step fails.
///
/// # Errors
/// As [`make_face_with_curves`].
pub fn make_face_with_curve_owners(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    curve_owners: &mut Vec<Option<EdgeId>>,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<NewFace> {
    make_face_with_curves_impl(
        k,
        b,
        region,
        input,
        surface,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        curve_owners,
        &mut ed::NotifyState::from_env(env),
    )
}

/// [`make_face_with_curves`] using caller-owned notification stores and the actual
/// kernel context. Geometry and construction order are identical to the legacy
/// entrypoint, including its incomplete topology barriers. `this_face` still
/// identifies the new face as the surface's owner; `stores.key_of` names entities
/// for topology notifications.
pub fn make_face_with_curves_with_kernel_context(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    stores: &mut ed::NotificationStores<'_>,
) -> Result<NewFace> {
    make_face_with_curves_with_state(
        k,
        b,
        region,
        input,
        surface,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        &mut ed::NotifyState::from_kernel_stores(stores),
    )
}

pub(crate) fn make_face_with_curves_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    state: &mut ed::NotifyState<'_>,
) -> Result<NewFace> {
    make_face_with_curves_impl(
        k,
        b,
        region,
        input,
        surface,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        &mut Vec::new(),
        state,
    )
}

#[allow(clippy::too_many_lines)] // one upstream routine
fn make_face_with_curves_impl(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    curve_owners: &mut Vec<Option<EdgeId>>,
    state: &mut ed::NotifyState<'_>,
) -> Result<NewFace> {
    let mut transfer = p6::FaceSurfaceTransfer::new(surface);
    let mut out = NewFace::default();
    make_face_with_curves_transfer_impl(
        k,
        b,
        region,
        input,
        &mut transfer,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        curve_owners,
        state,
        &mut out,
        true,
    )?;
    Ok(out)
}

/// Constructs with the original surface, retaining its location and partial outputs on error.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_face_with_curves_transfer(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    transfer: &mut p6::FaceSurfaceTransfer,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyEnv<'_>,
    out: &mut NewFace,
) -> Result<()> {
    make_face_with_curves_transfer_impl(
        k,
        b,
        region,
        input,
        transfer,
        uv_domain,
        orientation,
        crv_on_surf,
        this_face,
        &mut Vec::new(),
        &mut ed::NotifyState::from_env(env),
        out,
        false,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn make_face_with_curves_transfer_impl(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    transfer: &mut p6::FaceSurfaceTransfer,
    uv_domain: &Extent2d,
    orientation: OrientType,
    crv_on_surf: Option<bool>,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    curve_owners: &mut Vec<Option<EdgeId>>,
    state: &mut ed::NotifyState<'_>,
    out: &mut NewFace,
    legacy: bool,
) -> Result<()> {
    curve_owners.clear();
    let FaceCurvesInput {
        loops,
        curves_3d,
        mut uv_curves,
        orientations,
        loop_points,
    } = input;
    *out = NewFace::default();
    let surface = transfer.owned_surface()?;

    // check input
    if curves_3d.is_none() && uv_curves.is_none() {
        return Err(SolidError::invalid_input(OP, "no curves"));
    }
    if loops.is_empty() {
        return Err(SolidError::invalid_input(OP, "no loops"));
    }
    // GetInfiniteRegion() (an inline field read; NULL when unset)
    let region = region.or(b.infinite_region());
    // (the surface tests return flags upstream: a failed test is "no")
    if ignored(surface.is_degenerate_curve(k, EFF_ZERO_DEG, None, DEGENERATE_TOL_3D))?
        .unwrap_or(false)
    {
        return Err(SolidError::invalid_input(
            OP,
            "surface degenerates to a curve",
        ));
    }
    if ignored(surface.is_degenerate_point(k, DEGENERATE_TOL_3D, None))?.unwrap_or(false) {
        return Err(SolidError::invalid_input(
            OP,
            "surface degenerates to a point",
        ));
    }

    // locals
    let total: usize = loops
        .iter()
        .try_fold(0usize, |a, &n| a.checked_add(n))
        .ok_or_else(|| null("loop counts overflow"))?;
    let zone_tol = b.tolerance();
    let crv_on_surf = crv_on_surf == Some(true);

    // only UV curves: lift them to 3D, fixing gaps larger than tolerance
    let mut curves_3d: Vec<Option<Curve>> = if let Some(c) = curves_3d {
        c
    } else {
        {
            let uv = uv_curves.as_deref_mut().ok_or_else(|| null("no curves"))?;
            if total > uv.len() || total > orientations.len() {
                return Err(null("loop counts exceed the curves"));
            }
            let mut s3d: Vec<Option<Curve>> = Vec::new();
            let mut index = 0usize;
            for &n in &loops {
                // (index into s3d, curve) of the non-degenerate 3D curves of this loop
                let mut to_check: Vec<usize> = Vec::new();
                for jj in 0..n {
                    // (upstream converts the caller's curve: the copies made for the UV
                    // trim curves later are the converted ones)
                    let entry = uv
                        .get_mut(index + jj)
                        .ok_or_else(|| null("loop counts exceed the curves"))?;
                    ignore_status(entry.convert_to_2d(k))?;
                    let uv_curve = entry.clone_for_evaluation()?;
                    let new3d = if crv_on_surf {
                        let c = CrvOnSurf::new(
                            k,
                            uv_curve,
                            surface.clone_for_evaluation()?,
                            Some(*uv_domain),
                            2,
                        )?;
                        let c = Curve::from(c);
                        // (IsDegenerate returns a flag upstream: a failed test is "no")
                        if ignored(c.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false) {
                            None
                        } else {
                            Some(c)
                        }
                    } else {
                        let ivl = uv_curve.natural_interval();
                        ignored(surface.lift_curve(
                            k,
                            uv_domain,
                            &uv_curve,
                            &ivl,
                            zone_tol / 4.0,
                            false,
                        ))?
                        .map(|l| l.curve)
                    };
                    if new3d.is_some() {
                        to_check.push(s3d.len());
                    }
                    s3d.push(new3d);
                }

                // the gap middles of gaps larger than tolerance (at each curve's start)
                let tol2 = zone_tol * zone_tol;
                let nc = to_check.len();
                let mut new_points: Vec<Option<Point3>> = vec![None; nc];
                // (sCurveStart / sCWCurveEnd keep the previous iteration's value when
                // GetEnds fails)
                let mut start = Point3::UNDEF;
                let mut cw_end = Point3::UNDEF;
                for jj in 0..nc {
                    let cw_jj = if jj == 0 { nc - 1 } else { jj - 1 };
                    let ci = *at(&to_check, jj)?;
                    let cwi = *at(&to_check, cw_jj)?;
                    // (upstream indexes the orientations by the position among the
                    // non-degenerate curves)
                    let cw_index = if jj == 0 {
                        index + n - 1
                    } else {
                        index + jj - 1
                    };
                    let o = *at(&orientations, index + jj)?;
                    let cwo = *at(&orientations, cw_index)?;
                    let c = at(&s3d, ci)?
                        .as_ref()
                        .ok_or_else(|| null("missing curve"))?;
                    let cw = at(&s3d, cwi)?
                        .as_ref()
                        .ok_or_else(|| null("missing curve"))?;
                    start = oriented_end(k, c, o, true, start)?;
                    cw_end = oriented_end(k, cw, cwo, false, cw_end)?;
                    if start.distance_squared(cw_end) > tol2 {
                        if let Some(slot) = new_points.get_mut(jj) {
                            *slot = Some((start + cw_end) / 2.0);
                        }
                    }
                }

                // deform the curves to the gap middles (statuses ignored)
                for jj in 0..nc {
                    let new_start = *at(&new_points, jj)?;
                    let new_end = *at(&new_points, if jj + 1 == nc { 0 } else { jj + 1 })?;
                    let ci = *at(&to_check, jj)?;
                    let Some(Some(curve)) = s3d.get_mut(ci) else {
                        return Err(null("missing curve"));
                    };
                    let deformed = ignored(curve.deform_to_end_point_targets_on_surface(
                        k,
                        new_start,
                        new_end,
                        true,
                        None,
                        InValueType::Unconstrained,
                        None,
                        InValueType::Unconstrained,
                        Some((surface, None)),
                    ))?
                    .flatten();
                    if let Some(d) = deformed {
                        if let Some(slot) = s3d.get_mut(ci) {
                            *slot = Some(d);
                        }
                    }
                }
                index += n;
            }
            s3d
        }
    };
    if total > curves_3d.len() || total > orientations.len() {
        return Err(null("loop counts exceed the curves"));
    }

    // make the wire edges and vertices of every loop
    let mut sorted_loops: Vec<usize> = Vec::new();
    let mut edges: Vec<EdgeId> = Vec::new();
    let mut edge_orients: Vec<u8> = Vec::new();
    let mut curve_total = 0usize;
    for &n in &loops {
        let mut degenerate = 0usize;
        let mut vertices: Vec<VertexId> = Vec::new();
        for jj in 0..n {
            let slot = curve_total + jj;
            curve_owners.push(None);
            let Some(curve) = curves_3d.get_mut(slot).and_then(Option::take) else {
                degenerate += 1;
                continue;
            };
            let ivl = curve.natural_interval();
            let is_degen = ignored(curve.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false);
            let approx_len = curve.approximate_length(k, &ivl, 5, false)?.0;
            if is_degen || approx_len < zone_tol {
                degenerate += 1;
                continue;
            }

            let (start_param, end_param, orient, mut l_orient) =
                if *at(&orientations, slot)? == OrientType::Same {
                    (ivl.min(), ivl.max(), OrientType::Same, 0u8)
                } else {
                    (ivl.max(), ivl.min(), OrientType::Opposite, 1u8)
                };

            // find or make the start vertex
            let start_pt = curve.evaluate_point(k, start_param)?;
            let mut m = find_first_vertex_within_tol_to_pnt(b, &vertices, start_pt, zone_tol)?;
            if !vertices.is_empty() && !m.found {
                if m.distance < zone_tol * 100.0 {
                    m = find_first_vertex_within_tol_to_pnt(
                        b,
                        &vertices,
                        start_pt,
                        zone_tol + m.distance,
                    )?;
                }
                if !m.found {
                    return Err(SolidError::invalid_input(
                        OP,
                        "loop gap larger than tolerance",
                    ));
                }
            }
            let found_start = m.found;
            let start_vertex = if vertices.is_empty() {
                let (_shell, v) = super::p9::make_shell_vertex_with_context(
                    Some(&mut k.context),
                    b,
                    region,
                    start_pt,
                    state,
                )?;
                vertices.push(v);
                // (upstream assigns the new shell only when it already has one)
                v
            } else {
                *at(&vertices, m.index)?
            };

            // the end vertex
            let end_pt = curve.evaluate_point(k, end_param)?;
            if end_pt.distance_squared(start_pt) < zone_tol * zone_tol && approx_len < zone_tol {
                return Err(SolidError::sm_err(OP));
            }
            let mut me = find_first_vertex_within_tol_to_pnt(b, &vertices, end_pt, zone_tol)?;
            if jj + 1 == n && !me.found {
                if me.distance < zone_tol * 100.0 {
                    me = find_first_vertex_within_tol_to_pnt(
                        b,
                        &vertices,
                        end_pt,
                        zone_tol + me.distance,
                    )?;
                }
                if !me.found {
                    return Err(SolidError::invalid_input(OP, "loop does not close"));
                }
            }

            if me.found {
                let end_vertex = *at(&vertices, me.index)?;
                if found_start {
                    let query = CoincidentEdgeQuery {
                        start_vertex,
                        end_vertex,
                        curve: &curve,
                        // (a new input curve: no edge owns it)
                        curve_owner: None,
                        interval: ivl,
                        tolerance: zone_tol,
                    };
                    if let Some(c) = find_coincident_edge(k, b, &query, &edges, 0)? {
                        if c.max_deviation < zone_tol * 1.0e-3 {
                            // share the coincident edge (the curve is dropped)
                            let coin = *at(&edges, c.index)?;
                            edges.push(coin);
                            if !c.same_orientation {
                                l_orient = 1 - l_orient;
                            }
                            let coin_orient = *at(&edge_orients, c.index)?;
                            edge_orients.push(if coin_orient == l_orient {
                                coin_orient
                            } else {
                                1 - coin_orient
                            });
                            continue;
                        }
                    }
                }
                let e = state.with_kernel_env(k, |k, env| {
                    make_wire_edge(
                        k,
                        b,
                        region,
                        start_vertex,
                        end_vertex,
                        curve,
                        ivl,
                        orient,
                        env,
                    )
                })?;
                edges.push(e);
                if let Some(owner) = curve_owners.get_mut(slot) {
                    *owner = Some(e);
                }
                edge_orients.push(l_orient);
            } else {
                let (e, v) = state.with_kernel_env(k, |k, env| {
                    make_wire_edge_vertex(
                        k,
                        b,
                        region,
                        start_vertex,
                        curve,
                        ivl,
                        orient,
                        end_pt,
                        env,
                    )
                })?;
                edges.push(e);
                if let Some(owner) = curve_owners.get_mut(slot) {
                    *owner = Some(e);
                }
                edge_orients.push(l_orient);
                vertices.push(v);
            }
        }
        curve_total += n;
        if n > degenerate {
            sorted_loops.push(n - degenerate);
        }
    }

    // vertex loops
    let mut loop_vertices: Vec<VertexId> = Vec::new();
    for &p in &loop_points {
        let (_shell, v) =
            super::p9::make_shell_vertex_with_context(Some(&mut k.context), b, region, p, state)?;
        loop_vertices.push(v);
    }

    // the edge use of each edge with the wanted orientation
    let mut face_edgeuses: Vec<EdgeuseId> = Vec::new();
    for (i, &e) in edges.iter().enumerate() {
        let o = *at(&edge_orients, i)?;
        for eu in ed::edgeuses(b, e, None, None)? {
            let eo = eu1::orientation(b, eu)?;
            if (eo == OrientType::Same && o == 0) || (eo == OrientType::Opposite && o == 1) {
                face_edgeuses.push(eu);
                break;
            }
        }
    }

    let mut made = MakeFaceAttempt {
        status: Ok(()),
        region: None,
        shell: None,
        face: None,
    };
    let result = make_face_transfer_attempt_with_state(
        k,
        b,
        region,
        &sorted_loops,
        &face_edgeuses,
        &loop_vertices,
        transfer,
        uv_domain,
        orientation,
        &MakeFaceOptions::default(),
        this_face,
        state,
        legacy,
        &mut made,
    );
    out.region = made.region;
    if made.shell.is_some() && out.shell.is_some() {
        out.shell = made.shell;
    }
    out.face = made.face;
    result?;
    made.status?;

    // with curves on surface: make the UV trim curves from the given UV curves
    if crv_on_surf {
        let face = out.face.ok_or_else(|| null("no new face"))?;
        // (upstream loops over cpOptUVCurves->GetSize(): a null array is dereferenced)
        let uv: Vec<Curve> = uv_curves
            .ok_or_else(|| null("crv_on_surf without UV curves (upstream dereferences null)"))?;
        // CreateUVTrimCurves(FALSE, &sNewUVCurves, &crCurveOrientations, ..)
        let r = face_create_uv_trim_curves(k, b, face, false, uv, &orientations);
        match r {
            Err(e) if e.is_unported() => return Err(e),
            Err(SolidError::Upstream { code, .. }) if code == crate::error::SM_ERR => {
                // delete the edge uses of the face's edges
                for e in face_edges(b, face)? {
                    for eu in ed::edgeuses(b, e, None, None)? {
                        edge_remove(b, e, eu)?;
                    }
                }
                return Err(SolidError::sm_err(OP));
            }
            Ok(()) | Err(_) => {}
        }
    }
    Ok(())
}

// ---- GlueEdgesGeneral -----------------------------------------------------------------------

/// `GlueEdgesGeneral`: merges the edge uses of `to_delete` into `surviving` (edges with
/// common vertices; `relative_orientation` says how their curves run), drops the deleted
/// edge, widens the surviving edge's tolerance by `edge_gap`, merges the shells that now
/// touch and, with `do_region_nesting`, places the edge uses in their radial sectors and
/// tests for split regions. A wire `to_delete` is deleted. Nothing happens without both
/// edges or for one edge.
///
/// # Errors
///
/// `SM_ERR` when the edges do not share both vertices; those of the radial sector, edge
/// use and shell edits and of `SmBrep::Notify`'s representation checks;
/// [`SolidError::Unported`] (see the module docs).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream routine
pub fn glue_edges_general(
    k: &mut Kernel,
    b: &mut Brep,
    surviving: Option<EdgeId>,
    relative_orientation: OrientType,
    edge_gap: f64,
    do_region_nesting: bool,
    to_delete: Option<EdgeId>,
    inside_faceuses: Option<&[FaceuseId]>,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<()> {
    let (Some(surv), Some(del)) = (surviving, to_delete) else {
        return Ok(());
    };
    if surv == del {
        return Ok(());
    }

    // Notify(SM_NO_PRE_EDIT, this, NULL, NULL)
    let this = this_brep(b);
    brep_notify(b, env, NotifyOperation::PreEdit, [Some(this), None, None])?;

    // a wire edge is just deleted
    if ed::is_wire(b, del)? {
        // status ignored upstream
        ignore_status(delete_edge(k, b, del, env))?;
        return Ok(());
    }

    let surv_ref = (env.key_of)(EntityRef::Edge(surv));
    let brep_ref = p2::brep_ref(b);
    ed::notify(
        b,
        surv,
        env,
        NotifyOperation::PreEdit,
        [Some(surv_ref), Some(brep_ref), None],
    )?;
    // Notify(SM_NO_MERGE_IN_BREP, pSurvivingEdge, pEdgeToDelete, this)
    brep_notify(
        b,
        env,
        NotifyOperation::MergeInBrep,
        [
            Some(p2::NotifyArg::Entity(EntityRef::Edge(surv))),
            Some(p2::NotifyArg::Entity(EntityRef::Edge(del))),
            Some(p2::NotifyArg::Object(brep_ref)),
        ],
    )?;
    let brep_ref = p2::brep_ref(b);
    for f in ed::faces(b, del, None)? {
        // SmFace::Notify(SM_NO_PRE_EDIT, pFace, this, NULL)
        let face_ref = (env.key_of)(EntityRef::Face(f));
        crate::topology::face::p3::notify(
            b,
            f,
            env,
            None,
            NotifyOperation::PreEdit,
            [Some(face_ref), Some(brep_ref), None],
        )?;
    }

    // the same geometry: the same curve over the same interval, or curves within a
    // thousandth of the edge tolerance whose middles agree
    let mut same_geometry = false;
    if relative_orientation == OrientType::Same {
        let surv_ivl = ed::interval(b, surv)?;
        let del_ivl = ed::interval(b, del)?;
        if crate::geom::tol::are_same(surv_ivl.min(), del_ivl.min())
            && crate::geom::tol::are_same(surv_ivl.max(), del_ivl.max())
        {
            match (&b.edge(surv)?.curve, &b.edge(del)?.curve) {
                // when curves are the same (upstream's pointer test; in the arena only two
                // edges without curve)
                (None, None) => same_geometry = true,
                // test the geometry
                (Some(sc), Some(dc)) => {
                    let surv_tol = b.edge(surv)?.tolerance;
                    let tol = surv_tol / 100.0;
                    let dn = dc.natural_interval();
                    let md = sc.curve_max_distance_between(
                        k,
                        &sc.natural_interval(),
                        dc,
                        dn.min(),
                        dn.max(),
                        4,
                        Some(tol),
                    )?;
                    if md.distance < surv_tol / 1000.00 {
                        let t_mid = surv_ivl.evaluate(0.45678);
                        let sp = sc.evaluate_point(k, t_mid)?;
                        let dp = dc.evaluate_point(k, t_mid)?;
                        let tolerance = EFF_ZERO * (1.0 + sp.max_dimension());
                        if sp.distance(dp) < tolerance {
                            same_geometry = true;
                        }
                    }
                }
                // (upstream dereferences the null curve)
                _ => return Err(null("edge without curve")),
            }
        }
    }

    // both vertices must be shared
    let sv_st = ed::start_vertex(b, surv)?;
    let sv_end = ed::other_vertex(b, surv, sv_st)?;
    let dv_st = ed::start_vertex(b, del)?;
    let dv_end = ed::other_vertex(b, del, dv_st)?;
    let st_glued = sv_st == dv_st || sv_st == dv_end;
    let end_glued = sv_end == dv_st || sv_end == dv_end;
    if !st_glued || !end_glued {
        return Err(SolidError::sm_err(OP));
    }

    let mut surv_wire = ed::is_wire(b, surv)?;
    let to_delete_eus = ed::edgeuses(b, del, None, None)?;

    // Remove Edge from the Brep list - the edge record is not deleted
    b.remove_from_edge_list(del);

    let mut test_split: Vec<FaceuseId> = Vec::new();

    // every mated pair, from the end of the list
    let mut jj = to_delete_eus.len();
    while jj >= 2 {
        let deu1 = *at(&to_delete_eus, jj - 2)?;
        let deu2 = *at(&to_delete_eus, jj - 1)?;

        // the radial sector the pair goes into
        let mut sector_eu: Option<EdgeuseId> = None;
        let mut sector_fu: Option<FaceuseId> = None;
        if do_region_nesting {
            if let Some(s) = ignored(ed::find_radial_sector(
                k,
                b,
                surv,
                deu1,
                None,
                OrientType::Same,
                inside_faceuses,
            ))? {
                sector_eu = s.radial_edgeuse;
                sector_fu = s.coincident_faceuse;
            }
        }
        let mut sector = match sector_eu {
            Some(s) => s,
            None => eu1::primary_edgeuse(b, surv)?.ok_or_else(|| null("edge without edge use"))?,
        };

        // coincident faces: take the side whose regions match
        if sector_fu.is_some() {
            let reg = |b: &Brep, eu: Option<EdgeuseId>| -> Result<Option<RegionId>> {
                let eu = eu.ok_or_else(|| null("missing edge use"))?;
                match eu1::shell(b, eu)? {
                    Some(s) => eu1::shell_region(b, s),
                    None => Err(null("edge use without shell")),
                }
            };
            let mate = eu1::mate(b, sector)?;
            let r_del1 = reg(b, Some(deu1))?;
            let r_del2 = reg(b, Some(deu2))?;
            let r_sector = reg(b, Some(sector))?;
            let r_mate = reg(b, mate)?;
            let r_sec_rad = reg(b, eu1::radial(b, sector)?)?;
            let mate_rad = match mate {
                Some(m) => eu1::radial(b, m)?,
                None => None,
            };
            let r_mate_rad = reg(b, mate_rad)?;
            let (m1, m2, s1, s2) = if eu1::orientation(b, deu1)? == eu1::orientation(b, sector)? {
                (
                    r_del1 == r_sec_rad,
                    r_del2 == r_sector,
                    r_del1 == r_mate,
                    r_del2 == r_mate_rad,
                )
            } else {
                (
                    r_del1 == r_sector,
                    r_del2 == r_sec_rad,
                    r_del1 == r_mate_rad,
                    r_del2 == r_mate,
                )
            };
            let switch = if m1 && m2 {
                Some(false)
            } else if s1 && s2 {
                Some(true)
            } else if m1 || m2 {
                Some(false)
            } else if s1 || s2 {
                Some(true)
            } else {
                None
            };
            if switch == Some(true) {
                sector = mate_rad.ok_or_else(|| null("mate without radial"))?;
            }
        }

        // (sbCheckDEU1 is FALSE upstream: the region check of the pair is dead code)

        // different geometry: the pair's UV trim curve no longer fits (status ignored)
        if !same_geometry {
            ignore_status(eu1::set_uv_trim_curve(
                b,
                deu1,
                eu1::UvTrimCurveArg::Null,
                0.0,
                true,
                None,
            ))?;
        }

        edge_remove(b, del, deu1)?;
        edge_remove(b, del, deu2)?;

        // a face use to test for a split region
        let shell_of = |b: &Brep, eu: EdgeuseId| eu1::shell(b, eu);
        let mut temp_fu: Option<FaceuseId> = None;
        let sector_mate = eu1::mate(b, sector)?.ok_or_else(|| null("edge use without mate"))?;
        if eu1::is_loop_edgeuse(b, sector)? && shell_of(b, sector)? == shell_of(b, sector_mate)? {
            temp_fu = eu1::faceuse(b, sector)?;
        } else if eu1::is_loop_edgeuse(b, deu1)? && shell_of(b, deu1)? == shell_of(b, deu2)? {
            temp_fu = eu1::faceuse(b, deu1)?;
        }
        if let Some(fu) = temp_fu {
            if !test_split.contains(&fu) {
                test_split.push(fu);
            }
        }

        if relative_orientation == OrientType::Opposite {
            let o1 = eu1::orientation(b, deu1)?;
            let o2 = eu1::orientation(b, deu2)?;
            eu1::set_orientation(b, deu1, o2)?;
            eu1::set_orientation(b, deu2, o1)?;
        }

        let surv_eus = ed::edgeuses(b, surv, None, None)?;
        ed::add_oriented_eu_pair(b, surv, deu1, deu2, Some(sector))?;

        // the surviving edge was a wire: drop its wire edge uses and their vertex uses
        if surv_wire {
            for eu in surv_eus {
                let vu =
                    eu1::vertexuse(b, eu)?.ok_or_else(|| null("edge use without vertex use"))?;
                edge_remove(b, surv, eu)?;
                let v = b
                    .vertexuse(vu)?
                    .vertex
                    .ok_or_else(|| null("vertex use without vertex"))?;
                let list = &mut b.vertex_mut(v)?.vertexuses;
                if let Some(at) = list.iter().position(|&x| x == vu) {
                    list.remove(at);
                }
                b.vertexuse_mut(vu)?.vertex = None;
            }
            surv_wire = false;
        }

        // widen the surviving edge's tolerance to enclose the gap
        let del_tol = b.edge(del)?.tolerance;
        let surv_tol = b.edge(surv)?.tolerance;
        // (smos_Max: a > b ? a : b)
        let max_tol = if del_tol > surv_tol {
            del_tol
        } else {
            surv_tol
        };
        let mut radius = edge_gap + max_tol;
        if radius < edge_gap * 2.0 {
            radius = edge_gap * 2.0;
        }
        if radius > surv_tol * 1.1 {
            ed::set_tolerance(b, surv, radius, true, true)?;
        }

        // Upstream destroys the deleted edge once its last pair has moved
        // (`delete pDeleteEdge`: its destruction notification and attribute cleanup).
        if b.edge(del)?.edgeuses.is_empty() {
            ed::destroy(k, b, del, env)?;
        }
        jj -= 2;
    }

    // merge the shells the surviving edge's edge uses now connect
    for eu in ed::edgeuses(b, surv, None, None)? {
        let radial = eu1::radial(b, eu)?.ok_or_else(|| null("edge use without radial"))?;
        let del_shell = eu1::shell(b, eu)?.ok_or_else(|| null("edge use without shell"))?;
        let keep_shell = eu1::shell(b, radial)?.ok_or_else(|| null("edge use without shell"))?;
        if del_shell != keep_shell {
            merge_shells(k, b, del_shell, keep_shell, env)?;
        }
    }

    if do_region_nesting {
        let mut marks = Marks::default();
        let mut new_region = None;
        for fu in test_split {
            // status ignored; each call first resets rpNewRegion to NULL
            let attempt = p6::test_closure_split_region_attempt(k, b, &mut marks, env, true, fu)?;
            new_region = attempt.region;
        }
        if ed::is_spine(b, surv)? && new_region.is_none() {
            ignore_status(test_closure_split_region_edge(k, b, env, surv))?;
        }
    }
    Ok(())
}

// ---- CopyFaces ------------------------------------------------------------------------------

/// `CopyFaces`: copies `faces` of `src` into `dest`: with `use_merge` through the merge
/// (keeping the topology between copied faces), else face by face from copies of their
/// curves and surface (a face that cannot be copied makes the result `SM_ERR` but the
/// others are still copied). The destination's tolerance grows to the source's (it is
/// never lowered). Returns the
/// new faces of `dest`, and whether every face was copied.
///
/// # Errors
///
/// `SM_ERR` for a face of another B-rep; [`SolidError::Unported`] (see the module docs).
pub fn copy_faces(
    k: &mut Kernel,
    src: &mut Brep,
    faces: &[FaceId],
    dest: &mut Brep,
    use_merge: bool,
    this_face: &dyn Fn(FaceId) -> ObjectRef,
    env: &mut ed::NotifyEnv<'_>,
) -> Result<(Vec<FaceId>, bool)> {
    // pDestinationBrep->SetTolerance(GetTolerance()) = (tol, TRUE, TRUE): grow only
    brep_set_tolerance(k, dest, src.tolerance(), true, true)?;
    let mut new_faces = Vec::new();
    let mut all_ok = true;
    // (upstream reads the destination's infinite region before both branches; an
    // inline field read, NULL when unset)
    let inf = dest.infinite_region();

    if use_merge {
        // (the merge object is made before the face loop; MakeFaceuse's status is
        // ignored)
        let _merge = MergeState::<TopologyIntersector<MergeOperand>>::with_breps(
            k,
            (&*dest, MergeOperand::Primary),
            Some((&*src, MergeOperand::Other)),
            dest.tolerance(),
            20.0 * PI / 180.0,
            None,
        )?;
        let source_key = env.key_of;
        let mut copy = TopologyCopy {
            vertices: VertexCopy {
                records: env.records,
                attributes: env.attributes,
                source_key: &|v| source_key(EntityRef::Vertex(v)),
                destination_key: source_key,
            },
            edge_source_key: &|e| source_key(EntityRef::Edge(e)),
            edge_destination_key: &|e| source_key(EntityRef::Edge(e)),
            face_source_key: &|f| source_key(EntityRef::Face(f)),
            face_destination_key: this_face,
        };
        let context = match env.context.as_deref_mut() {
            Some(context) => MergeContext::Lent(k, context),
            None => MergeContext::Split(k, None),
        };
        if let Some(made) = ignored(merge_copy_faces(
            context,
            src,
            dest,
            faces,
            &mut Relations::default(),
            &mut copy,
        ))? {
            new_faces = made;
        }
        return Ok((new_faces, all_ok));
    }

    for &f in faces {
        if !face_in_brep(src, f)? {
            return Err(SolidError::sm_err(OP));
        }
        let Some(curves) = ignored(fp2::create_curves_from_face(
            k,
            src,
            f,
            OrientType::Same,
            true,
            true,
        ))?
        else {
            all_ok = false;
            continue;
        };
        let s1 = fp2::surface(src, f)?.ok_or_else(|| null("face without surface"))?;
        // (status ignored upstream, which then dereferences the null copy)
        let s2 = ignored(s1.copy_and_add_analytics(k))?
            .ok_or_else(|| null("CopyAndAddAnalytics failed (upstream dereferences null)"))?;
        let dom = s2.natural_uv_domain_with_nurb(k)?;
        let input = FaceCurvesInput {
            loops: curves
                .curve_loops
                .iter()
                .map(|&n| usize::try_from(n).map_err(|_| null("loop count")))
                .collect::<Result<Vec<_>>>()?,
            curves_3d: curves.curves_3d.map(|v| v.into_iter().map(Some).collect()),
            uv_curves: curves.uv_curves,
            orientations: curves.curve_orients,
            loop_points: Vec::new(),
        };
        match make_face_with_curves(
            k,
            dest,
            inf,
            input,
            s2,
            &dom,
            OrientType::Same,
            None,
            this_face,
            env,
        ) {
            Ok(made) => {
                if let Some(nf) = made.face {
                    new_faces.push(nf);
                }
                // SmFace::Notify(SM_NO_COPY, pNewFace, pDestinationBrep, this), through
                // the caller's records and attributes (the B-reps have no identity
                // here). Both faces are SmTopology objects upstream, the new one with
                // the default record its constructor gives it, so a face the caller
                // has no record for gets one (#1628: a fresh empty map failed as soon
                // as the new face's identity differed from the source face's).
                let this = (env.key_of)(EntityRef::Face(f));
                let new_face = made.face.map(this_face);
                env.records.or_default(this.key);
                if let Some(nf) = new_face {
                    env.records.or_default(nf.key);
                }
                crate::topology::face::p3::notify(
                    src,
                    f,
                    env,
                    None,
                    NotifyOperation::Copy,
                    [new_face, None, None],
                )?;
            }
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => all_ok = false,
        }
    }
    Ok((new_faces, all_ok))
}

/// `CopyFaces` with a caller-retained merge's relationships and distinct source
/// identities. It uses the same face-copy loop as [`copy_faces`]; mate records are
/// retained even when a later face fails. Ordinary `MakeFaceuse` errors are ignored,
/// as in C++; dependency and invalid-input errors propagate.
pub fn copy_faces_with_merge(
    context: MergeContext<'_>,
    src: &mut Brep,
    faces: &[FaceId],
    dest: &mut Brep,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
) -> Result<Vec<FaceId>> {
    brep_set_tolerance(context.kernel(), dest, src.tolerance(), true, true)?;
    // C++ constructs this local even when the caller supplies a merge object.
    let _local_merge = MergeState::<TopologyIntersector<MergeOperand>>::with_breps(
        context.kernel(),
        (&*dest, MergeOperand::Primary),
        Some((&*src, MergeOperand::Other)),
        dest.tolerance(),
        20.0 * PI / 180.0,
        None,
    )?;
    merge_copy_faces(context, src, dest, faces, relations, copy)
}

// ---- MergeSelfIntersectionCurves ------------------------------------------------------------

/// One self-intersection curve of a surface: its 3D curve and its UV curves on the two
/// sheets that meet there.
#[derive(Debug, Clone)]
pub struct SelfIntersectionCurve {
    /// `cr3DCurves[k]`.
    pub curve_3d: Curve,
    /// `crUVCurves1[k]`.
    pub uv_curve_1: Curve,
    /// `crUVCurves2[k]`.
    pub uv_curve_2: Curve,
}

/// `MergeSelfIntersectionCurves`: imprints each self-intersection curve of the surface of
/// `face` on the face (through both UV curves) and glues the two edges made for it,
/// giving non-manifold edges with four face uses. Returns the glued edges. The merges
/// edit `b` (vertex loops, edge splits and new edges, notified in `env`); `curve_ref`
/// names an edge's curve by the edge and the curve's type code, the curve of an edge a
/// split makes included.
///
/// # Errors
///
/// `SM_ERR` when the second classification does not give one interval; those of the
/// classifications, their B-rep edits, vertex and edge gluing; [`SolidError::Unported`]
/// (the classifications' callees, `GlueVertices`, `GlueEdgesGeneral`'s callees).
pub fn merge_self_intersection_curves(
    k: &mut Kernel,
    b: &mut Brep,
    face: Option<FaceId>,
    tol_3d: f64,
    curves: &[SelfIntersectionCurve],
    env: &mut ed::NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
) -> Result<Vec<EdgeId>> {
    let mut new_edges = Vec::new();
    let mut all_faces: Vec<FaceId> = face.into_iter().collect();
    let face = match face {
        Some(f) => f,
        None if curves.is_empty() => return Ok(new_edges),
        None => return Err(null("surface owner is not a face")),
    };
    // the classifications' curves: curve i is IntersectObject::Curve(i), its UV curves
    // IntersectObject::UvCurve(i, false / true)
    let mut curves_3d: Vec<Curve> = curves.iter().map(|c| c.curve_3d.clone()).collect();
    let uv1: Vec<Option<Curve>> = curves.iter().map(|c| Some(c.uv_curve_1.clone())).collect();
    let uv2: Vec<Option<Curve>> = curves.iter().map(|c| Some(c.uv_curve_2.clone())).collect();
    for (i, c) in curves.iter().enumerate() {
        let tol = tol_3d;
        let curve_3d = IntersectObject::Curve(i);
        let mut merge_objects = SelfMergeObjects {
            b: &mut *b,
            curves_3d: &mut curves_3d,
            uv1: &uv1,
            uv2: &uv2,
            env: &mut *env,
            curve_ref,
        };
        let mut cc1 = classify_on_faces(
            k,
            &mut merge_objects,
            &all_faces,
            face,
            IntersectObject::UvCurve(i, false),
            c.uv_curve_1.natural_interval(),
            tol,
        )?;
        let new_faces = set_curve_and_merge(k, &mut merge_objects, &mut cc1, curve_3d)?;
        all_faces.extend(new_faces);
        // (the intervals are copied: the gluing below edits the B-rep the
        // classification's objects borrow)
        let intervals = cc1.intervals().to_vec();
        for ivl in &intervals {
            if ivl.mid.point_class() != PointClassType::Edge {
                continue;
            }
            let e1 = mid_edge(ivl)?;
            let mut merge_objects = SelfMergeObjects {
                b: &mut *b,
                curves_3d: &mut curves_3d,
                uv1: &uv1,
                uv2: &uv2,
                env: &mut *env,
                curve_ref,
            };
            let mut cc2 = classify_on_faces(
                k,
                &mut merge_objects,
                &all_faces,
                face,
                IntersectObject::UvCurve(i, true),
                ivl.interval,
                tol,
            )?;
            if cc2.size() != 1 {
                continue;
            }
            let new_faces = set_curve_and_merge(k, &mut merge_objects, &mut cc2, curve_3d)?;
            all_faces.extend(new_faces);
            let [ivl2] = cc2.intervals() else {
                return Err(SolidError::sm_err(OP));
            };
            if ivl2.mid.point_class() != PointClassType::Edge {
                // (SE(SM_ERR): the error is signalled, not returned)
                continue;
            }
            let e2 = mid_edge(ivl2)?;
            let st1 = ed::start_vertex(b, e1)?;
            let end1 = ed::other_vertex(b, e1, st1)?;
            let st2 = ed::start_vertex(b, e2)?;
            let end2 = ed::other_vertex(b, e2, st2)?;
            let v = |x: Option<VertexId>| x.ok_or_else(|| null("edge without vertex"));
            if st1 != st2 {
                glue_vertices(k, b, v(st1)?, v(st2)?, env)?;
            }
            if st1 != end1 && end1 != end2 {
                glue_vertices(k, b, v(end1)?, v(end2)?, env)?;
            }
            if e1 != e2 {
                glue_edges_general(
                    k,
                    b,
                    Some(e1),
                    OrientType::Same,
                    tol,
                    false,
                    Some(e2),
                    None,
                    env,
                )?;
            }
            new_edges.push(e1);
        }
    }
    Ok(new_edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::geom::vector2d::Point2;
    use crate::math::Vec3;
    use crate::surface::plane::Plane;
    use crate::topology::Region;

    fn key_of(_e: EntityRef) -> ObjectRef {
        ObjectRef {
            type_code: 0,
            key: ObjectKey(0),
        }
    }

    fn on_list() -> Region {
        Region {
            in_brep: true,
            ..Region::default()
        }
    }

    #[test]
    fn emptied_region_resets_the_infinite_region_to_the_list_head() {
        // the region is not on the list (Remove fails, ignored): the head stays, and a
        // stored head of None is the first region on the list
        let mut b = Brep::new();
        let r0 = b.add_region_record(on_list()).unwrap();
        let r1 = b.add_region_record(Region::default()).unwrap();
        assert_eq!(b.region_list_head(), None);
        remove_emptied_region(&mut b, r1).unwrap();
        assert_eq!(b.infinite_region(), Some(r0));
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r0]);

        // removing the head passes it on; removing the last region empties the list
        let mut b = Brep::new();
        let r0 = b.add_region_record(on_list()).unwrap();
        let r1 = b.add_region_record(on_list()).unwrap();
        b.set_infinite_region(Some(r0));
        remove_emptied_region(&mut b, r0).unwrap();
        assert_eq!(b.infinite_region(), Some(r1));
        assert_eq!(b.region_list_head(), Some(r1));
        remove_emptied_region(&mut b, r1).unwrap();
        assert_eq!(b.infinite_region(), None);
        assert!(p1::regions(&b, None).unwrap().is_empty());

        // removing a region behind the head keeps the head
        let mut b = Brep::new();
        let r0 = b.add_region_record(on_list()).unwrap();
        let r1 = b.add_region_record(on_list()).unwrap();
        remove_emptied_region(&mut b, r1).unwrap();
        assert_eq!(b.infinite_region(), Some(r0));
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r0]);
    }

    fn face_ref(f: FaceId) -> ObjectRef {
        ObjectRef {
            type_code: fp2::TYPE,
            key: ObjectKey(1 + u64::from(f.index())),
        }
    }

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z))
    }

    fn dom() -> Extent2d {
        Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))
    }

    #[test]
    fn curve_owner_slots_retain_discarded_inputs_before_a_later_failure() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let region = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        b.set_infinite_region(Some(region));
        let mut owners = vec![Some(EdgeId::from_index(99))];
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        let result = make_face_with_curve_owners(
            &mut k,
            &mut b,
            // All curves are discarded first; the later MakeFace region lookup
            // must fail independently of whether brep_p6 has been ported.
            Some(RegionId::from_index(12345)),
            FaceCurvesInput {
                loops: vec![3],
                curves_3d: Some(vec![None, None, None]),
                orientations: vec![OrientType::Same; 3],
                ..FaceCurvesInput::default()
            },
            plane(),
            &dom(),
            OrientType::Same,
            None,
            &face_ref,
            &mut owners,
            &mut env,
        );
        assert!(matches!(result, Err(error) if !error.is_unported()));
        assert_eq!(owners, vec![None, None, None]);
    }

    #[test]
    fn glue_no_ops() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let v0 = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let v1 = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let e = b.add_edge(v0, v1).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        for (s, d) in [
            (None, Some(e)),
            (Some(e), None),
            (None, None),
            (Some(e), Some(e)),
        ] {
            glue_edges_general(
                &mut k,
                &mut b,
                s,
                OrientType::Same,
                0.0,
                true,
                d,
                None,
                &mut env,
            )
            .unwrap();
        }
        let e2 = b.add_edge(v0, v1).unwrap();
        let r = glue_edges_general(
            &mut k,
            &mut b,
            Some(e),
            OrientType::Same,
            0.0,
            true,
            Some(e2),
            None,
            &mut env,
        );
        // two bare edges (no edge uses): past the B-rep notifications the edge use walk
        // dereferences the missing primary edge use
        assert!(
            matches!(
                r,
                Err(SolidError::InvalidInput {
                    reason: "edge without primary edge use",
                    ..
                })
            ),
            "glue: {r:?}"
        );
    }

    // A triangular sheet and a separate wire: deleting the sheet's first edge is
    // non-wire, but gluing to the wire fails the real shared-endpoint check after
    // the B-rep, edge and incident-face notification prefix.
    fn glue_notification_fixture() -> (Brep, EdgeId, EdgeId, FaceId) {
        use crate::topology::shell::{ShellMembers, ShellType};
        use crate::topology::{
            Edgeuse, EdgeuseOwner, Faceuse, LoopEdge, Loopuse, LoopuseStart, Vertexuse,
            VertexuseOwner,
        };

        let mut b = Brep::new_with_infinite_region().unwrap();
        b.set_key(ObjectKey(9_876_543_210));
        let region = b.infinite_region().unwrap();
        let sheet = b.add_shell().unwrap();
        let wire = b.add_shell().unwrap();
        b.region_mut(region).unwrap().shells = vec![sheet, wire];
        for shell in [sheet, wire] {
            b.shell_mut(shell).unwrap().region = Some(region);
        }
        let face = b.add_face(sheet).unwrap();
        b.face_mut(face).unwrap().surface = Some(plane());
        b.face_mut(face).unwrap().uv_domain = dom();
        b.face_mut(face).unwrap().rectangular_trim = 0;
        let faceuses = [OrientType::Same, OrientType::Opposite].map(|orientation| {
            b.add_faceuse_record(Faceuse {
                face: Some(face),
                shell: Some(sheet),
                orientation,
                ..Faceuse::default()
            })
            .unwrap()
        });
        b.face_mut(face).unwrap().faceuse = Some(faceuses[0]);
        b.shell_mut(sheet).unwrap().shell_type = ShellType::Faceuse;
        b.shell_mut(sheet).unwrap().members = ShellMembers::Faceuses(faceuses.to_vec());
        let vertices = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::Z + Vec3::X]
            .map(|point| b.add_vertex(point).unwrap());
        let edges = [(0, 1), (1, 2), (2, 0), (3, 4)].map(|(start, end)| {
            let edge = b.add_edge(vertices[start], vertices[end]).unwrap();
            let curve = Curve::from(crate::curve::line::Line::from_points(
                b.vertex(vertices[start]).unwrap().point,
                b.vertex(vertices[end]).unwrap().point,
                3,
            ));
            b.edge_mut(edge).unwrap().interval = curve.natural_interval();
            b.edge_mut(edge).unwrap().curve = Some(curve);
            edge
        });
        let loop_ = b
            .add_loop(
                face,
                edges[..3]
                    .iter()
                    .map(|&edge| LoopEdge {
                        edge,
                        forward: true,
                    })
                    .collect(),
            )
            .unwrap();
        let loopuses = faceuses.map(|fu| {
            b.add_loopuse_record(Loopuse {
                faceuse: Some(fu),
                loop_: Some(loop_),
                orientation: OrientType::Same,
                ..Loopuse::default()
            })
            .unwrap()
        });
        b.loop_mut(loop_).unwrap().loopuse = Some(loopuses[0]);
        for side in 0..2 {
            b.faceuse_mut(faceuses[side]).unwrap().mate = Some(faceuses[1 - side]);
            b.faceuse_mut(faceuses[side]).unwrap().loopuses = vec![loopuses[side]];
            b.loopuse_mut(loopuses[side]).unwrap().mate = Some(loopuses[1 - side]);
        }
        for (index, edge) in edges.into_iter().enumerate() {
            let ends = b.edge(edge).unwrap().vertices;
            for (side, orientation) in [OrientType::Same, OrientType::Opposite]
                .into_iter()
                .enumerate()
            {
                let vertex = ends[side];
                let vu = b
                    .add_vertexuse_record(Vertexuse {
                        vertex: Some(vertex),
                        ..Vertexuse::default()
                    })
                    .unwrap();
                let eu = b
                    .add_edgeuse_record(Edgeuse {
                        edge: Some(edge),
                        owner: if index < 3 {
                            EdgeuseOwner::Loopuse(Some(loopuses[side]))
                        } else {
                            EdgeuseOwner::Shell(Some(wire))
                        },
                        vertexuse: Some(vu),
                        orientation,
                        ..Edgeuse::default()
                    })
                    .unwrap();
                b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
                b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
                b.edge_mut(edge).unwrap().edgeuses.push(eu);
            }
        }
        for (side, &loopuse) in loopuses.iter().enumerate() {
            let uses = edges[..3]
                .iter()
                .map(|&edge| b.edge(edge).unwrap().edgeuses[side])
                .collect::<Vec<_>>();
            b.loopuse_mut(loopuse).unwrap().start = LoopuseStart::Edgeuse(Some(uses[0]));
            for index in 0..3 {
                let (next, previous) = if side == 0 { (1, 2) } else { (2, 1) };
                let eu = b.edgeuse_mut(uses[index]).unwrap();
                eu.ccw = Some(uses[(index + next) % 3]);
                eu.cw = Some(uses[(index + previous) % 3]);
            }
        }
        b.shell_mut(wire).unwrap().shell_type = ShellType::Wire;
        b.shell_mut(wire).unwrap().members =
            ShellMembers::Edgeuse(b.edge(edges[3]).unwrap().edgeuses[0]);
        (b, edges[3], edges[0], face)
    }

    #[test]
    fn glue_destroys_transferred_edge_after_last_pair() {
        let mut k = Kernel::default();
        let (mut b, surviving, deleted, _) = glue_notification_fixture();
        let ends = b.edge(deleted).unwrap().vertices;
        let shared_ends = [ends[1], ends[0]];
        // Make the wire coincide with the sheet edge, with opposite direction.
        // Both edges share vertex identities, as GlueEdgesGeneral requires.
        let wire_uses = ed::edgeuses(&b, surviving, None, None).unwrap();
        for (eu, vertex) in wire_uses.into_iter().zip(shared_ends) {
            let vu = eu1::vertexuse(&b, eu).unwrap().unwrap();
            let old_vertex = b.vertexuse(vu).unwrap().vertex.unwrap();
            b.vertex_mut(old_vertex)
                .unwrap()
                .vertexuses
                .retain(|&candidate| candidate != vu);
            b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
            b.vertexuse_mut(vu).unwrap().vertex = Some(vertex);
        }
        b.edge_mut(surviving).unwrap().vertices = shared_ends;
        b.edge_mut(surviving).unwrap().curve =
            Some(Curve::from(crate::curve::line::Line::from_points(
                b.vertex(shared_ends[0]).unwrap().point,
                b.vertex(shared_ends[1]).unwrap().point,
                3,
            )));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        glue_edges_general(
            &mut k,
            &mut b,
            Some(surviving),
            OrientType::Opposite,
            0.0,
            false,
            Some(deleted),
            None,
            &mut ed::NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &key_of,
            },
        )
        .unwrap();
        assert!(!p1::edges(&b, None).unwrap().contains(&deleted));
        assert!(b.edge(deleted).is_err());
        assert_eq!(b.edges().len(), 3);
        assert_eq!(ed::edgeuses(&b, surviving, None, None).unwrap().len(), 2);
    }

    #[test]
    fn glue_destruction_notifies_and_clears_the_deleted_edges_attribute_users() {
        // `delete pDeleteEdge`: ~SmEdge notifies SM_NO_DESTRUCTION with DoingBoolean set,
        // ~SmAObject removes the edge from its attributes (issue #556).
        use crate::geom::attribute::{Attribute, Behavior};
        use crate::topology::edge::notification_test_support as callbacks;
        let mut k = Kernel::default();
        let events = callbacks::install(&mut k.context);
        let (mut b, surviving, deleted, _) = glue_notification_fixture();
        let ends = b.edge(deleted).unwrap().vertices;
        let shared_ends = [ends[1], ends[0]];
        let wire_uses = ed::edgeuses(&b, surviving, None, None).unwrap();
        for (eu, vertex) in wire_uses.into_iter().zip(shared_ends) {
            let vu = eu1::vertexuse(&b, eu).unwrap().unwrap();
            let old_vertex = b.vertexuse(vu).unwrap().vertex.unwrap();
            b.vertex_mut(old_vertex)
                .unwrap()
                .vertexuses
                .retain(|&candidate| candidate != vu);
            b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
            b.vertexuse_mut(vu).unwrap().vertex = Some(vertex);
        }
        b.edge_mut(surviving).unwrap().vertices = shared_ends;
        b.edge_mut(surviving).unwrap().curve =
            Some(Curve::from(crate::curve::line::Line::from_points(
                b.vertex(shared_ends[0]).unwrap().point,
                b.vertex(shared_ends[1]).unwrap().point,
                3,
            )));
        // distinct identities per entity (the module's `key_of` names every edge alike)
        let key_of = crate::topology::loopuse::notify_ref;
        let surviving_ref = key_of(EntityRef::Edge(surviving));
        let deleted_ref = key_of(EntityRef::Edge(deleted));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let shared = attributes
            .insert(Attribute::long(81, 912, Behavior::Reference))
            .unwrap();
        attributes.add_attribute(surviving_ref.key, shared).unwrap();
        attributes.add_attribute(deleted_ref.key, shared).unwrap();
        glue_edges_general(
            &mut k,
            &mut b,
            Some(surviving),
            OrientType::Opposite,
            0.0,
            false,
            Some(deleted),
            None,
            &mut ed::NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &key_of,
            },
        )
        .unwrap();
        assert!(b.edge(deleted).is_err());
        assert_eq!(attributes.users(shared).unwrap(), vec![surviving_ref.key]);
        assert!(!k.context.doing_boolean());
        // one SM_NO_DESTRUCTION of the deleted edge for each installed callback
        let mut destructions: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.operation == NotifyOperation::Destruction)
            .map(|e| (e.callback, e.caller))
            .collect();
        destructions.sort_by_key(|(callback, _)| *callback);
        assert_eq!(
            destructions,
            vec![("system", deleted_ref), ("user", deleted_ref)]
        );
    }

    #[test]
    fn glue_face_pre_edit_preserves_caller_notifications_on_error() {
        use crate::geom::attribute::{Attribute, Behavior};
        use crate::geom::context::Context;
        use crate::topology::edge::notification_test_support as callbacks;
        use crate::topology::topology::Topology;

        for with_context in [true, false] {
            let mut k = Kernel::default();
            let kernel_events = callbacks::install(&mut k.context);
            let mut context = Context::default();
            let events = callbacks::install(&mut context);
            let (mut b, surviving, deleted, face) = glue_notification_fixture();
            assert!(ed::is_wire(&b, surviving).unwrap());
            assert!(!ed::is_wire(&b, deleted).unwrap());
            assert_eq!(ed::faces(&b, deleted, None).unwrap(), vec![face]);
            let key_of = |entity| {
                let mut object = crate::topology::loopuse::notify_ref(entity);
                object.key.0 += 400_000;
                object
            };
            let brep = p2::brep_ref(&b);
            let surviving_ref = key_of(EntityRef::Edge(surviving));
            let deleted_ref = key_of(EntityRef::Edge(deleted));
            let face_ref = key_of(EntityRef::Face(face));
            let mut edge_record = Topology::default();
            edge_record.set_user_index1(73);
            edge_record.set_all_flags(0x40);
            let mut face_record = Topology::default();
            face_record.set_user_index1(912);
            let mut records =
                crate::topology::topology::NotifyRecords::from(std::collections::BTreeMap::from([
                    (surviving_ref.key, edge_record),
                    (deleted_ref.key, edge_record),
                    (face_ref.key, face_record),
                ]));
            let before = records.clone();
            let mut attributes = AttributeStore::default();
            let label = attributes
                .insert(Attribute::long(81, 912, Behavior::Reference))
                .unwrap();
            attributes.add_attribute(face_ref.key, label).unwrap();
            let result = glue_edges_general(
                &mut k,
                &mut b,
                Some(surviving),
                OrientType::Opposite,
                0.0,
                false,
                Some(deleted),
                None,
                &mut ed::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: if with_context {
                        Some(&mut context)
                    } else {
                        None
                    },
                    key_of: &key_of,
                },
            );
            assert_eq!(result, Err(SolidError::sm_err(OP)));
            assert_eq!(b.face(face).unwrap().rectangular_trim, fp2::UNSURE);
            assert_eq!((b.edges().len(), b.vertices().len()), (4, 5));
            assert_eq!(records, before);
            assert_eq!(attributes.find_attribute(face_ref.key, 81), Some(label));
            callbacks::assert_installed(&context, &events);
            callbacks::assert_installed(&k.context, &kernel_events);
            assert!(kernel_events.lock().unwrap().is_empty());
            let expected = if with_context {
                [
                    (NotifyOperation::PreEdit, brep, [Some(brep), None, None]),
                    (
                        NotifyOperation::PreEdit,
                        surviving_ref,
                        [Some(surviving_ref), Some(brep), None],
                    ),
                    (
                        NotifyOperation::MergeInBrep,
                        brep,
                        [Some(surviving_ref), Some(deleted_ref), Some(brep)],
                    ),
                    (
                        NotifyOperation::PreEdit,
                        face_ref,
                        [Some(face_ref), Some(brep), None],
                    ),
                ]
                .into_iter()
                .flat_map(|(operation, caller, data)| {
                    ["user", "system"].map(|callback| callbacks::Event {
                        callback,
                        operation,
                        caller,
                        data,
                    })
                })
                .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            assert_eq!(*events.lock().unwrap(), expected);
        }
    }

    #[test]
    fn make_face_needs_a_surface() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let opts = MakeFaceOptions::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        let e = make_face(
            &mut k,
            &mut b,
            None,
            &[],
            &[],
            &[],
            None,
            &dom(),
            OrientType::Same,
            &opts,
            &face_ref,
            &mut env,
        );
        assert!(matches!(
            e,
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        let e = make_face(
            &mut k,
            &mut b,
            None,
            &[],
            &[],
            &[],
            Some(plane()),
            &dom(),
            OrientType::Same,
            &opts,
            &face_ref,
            &mut env,
        );
        // no infinite region: MakeFaceTopology's NULL region
        assert!(matches!(
            e,
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        let inf = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        b.set_infinite_region_record(Some(inf));
        let e = make_face(
            &mut k,
            &mut b,
            None,
            &[],
            &[],
            &[],
            Some(plane()),
            &dom(),
            OrientType::Same,
            &opts,
            &face_ref,
            &mut env,
        );
        // the infinite region: an edgeless face in a new shell of that region
        let made = e.unwrap();
        let face = made.face.unwrap();
        let shell = made.shell.unwrap();
        assert_eq!(made.region, None);
        assert_eq!(b.region(inf).unwrap().shells, vec![shell]);
        assert_eq!(b.face(face).unwrap().uv_domain, dom());
        assert_eq!(
            b.face(face).unwrap().surface.as_ref().unwrap().owner(),
            Some(face_ref(face))
        );
    }

    #[test]
    fn transfer_installs_original_surface_identity_in_actual_face() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let mut surface = plane();
        let key = k
            .context
            .geometry_key(surface.notify_identity_slot())
            .unwrap();
        let mut transfer = p6::FaceSurfaceTransfer::new(surface);
        let mut out = NewFace::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        make_face_with_curves_transfer(
            &mut k,
            &mut b,
            None,
            FaceCurvesInput {
                loops: vec![1],
                curves_3d: Some(vec![None]),
                orientations: vec![OrientType::Same],
                ..Default::default()
            },
            &mut transfer,
            &dom(),
            OrientType::Same,
            None,
            &face_ref,
            &mut env,
            &mut out,
        )
        .unwrap();
        let face = out.face.unwrap();
        assert_eq!(transfer.installed_face(), Some(face));
        assert!(
            matches!(transfer.into_location().unwrap(), p6::FaceSurfaceLocation::Installed(id) if id==face)
        );
        let installed = b.face(face).unwrap().surface.as_ref().unwrap();
        // Clone resets the issued identity: this proves transfer of the original,
        // rather than merely equal geometry or a copied owner field.
        assert_eq!(installed.notify_key(), Some(key));
        assert_eq!(installed.owner(), Some(face_ref(face)));
        assert_eq!(
            b.geometry_owner_keys.get(&EntityRef::Face(face)),
            Some(&face_ref(face))
        );
        assert_eq!(
            installed.owner_zone_tol_3d(),
            Some(b.face(face).unwrap().tolerance)
        );
    }

    #[test]
    fn transfer_failure_keeps_surface_owner_consistent_with_its_location() {
        // Invalid arena ids are Rust-only failure injection, never C++ inputs.
        for late in [false, true] {
            for old_owner in [None, Some(face_ref(FaceId::from_index(99)))] {
                let mut k = Kernel::default();
                let mut b = if late {
                    Brep::new_with_infinite_region().unwrap()
                } else {
                    Brep::new()
                };
                let mut surface = plane();
                surface.set_owner(old_owner);
                let key = k
                    .context
                    .geometry_key(surface.notify_identity_slot())
                    .unwrap();
                let mut transfer = p6::FaceSurfaceTransfer::new(surface);
                let mut out = MakeFaceAttempt {
                    status: Ok(()),
                    region: None,
                    shell: None,
                    face: None,
                };
                let mut records = crate::topology::topology::NotifyRecords::new();
                let mut attributes = AttributeStore::default();
                let mut env = ed::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: None,
                    key_of: &key_of,
                };
                let vertices = [VertexId::from_index(99)];
                let result = make_face_transfer_attempt_with_state(
                    &mut k,
                    &mut b,
                    None,
                    &[],
                    &[],
                    if late { &vertices } else { &[] },
                    &mut transfer,
                    &dom(),
                    OrientType::Same,
                    &MakeFaceOptions::default(),
                    &face_ref,
                    &mut ed::NotifyState::from_env(&mut env),
                    false,
                    &mut out,
                );
                assert_eq!(out.region, None);
                if late {
                    assert!(matches!(result, Err(SolidError::NotFound { .. })));
                    assert!(out.status.is_ok());
                    let face = out.face.unwrap();
                    assert!(out.shell.is_some());
                    assert_eq!(transfer.installed_face(), Some(face));
                    assert!(
                        matches!(transfer.into_location().unwrap(), p6::FaceSurfaceLocation::Installed(id) if id == face)
                    );
                    let record = b.face(face).unwrap();
                    let installed = record.surface.as_ref().unwrap();
                    assert!(matches!(
                        installed.kind(),
                        crate::surface::surface::SurfaceKind::Plane(_)
                    ));
                    assert_eq!(installed.notify_key(), Some(key));
                    assert_eq!(installed.owner(), Some(face_ref(face)));
                    assert_eq!(
                        b.geometry_owner_keys.get(&EntityRef::Face(face)),
                        Some(&face_ref(face))
                    );
                    assert_eq!(installed.owner_zone_tol_3d(), Some(record.tolerance));
                } else {
                    result.unwrap();
                    assert!(matches!(
                        out.status,
                        Err(SolidError::Upstream {
                            code: crate::error::SM_ERR_NULL_POINTER,
                            ..
                        })
                    ));
                    assert_eq!(out.face, None);
                    assert_eq!(out.shell, None);
                    assert!(b.faces().is_empty());
                    assert!(b.geometry_owner_keys.is_empty());
                    match transfer.into_location().unwrap() {
                        p6::FaceSurfaceLocation::Owned(surface) => {
                            assert_eq!(surface.notify_key(), Some(key));
                            assert_eq!(surface.owner(), old_owner);
                        }
                        p6::FaceSurfaceLocation::Installed(_) => {
                            panic!("surface installed before early failure")
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn failed_transfer_retains_original_surface_and_partial_wire_topology() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let mut surface = plane();
        let key = k
            .context
            .geometry_key(surface.notify_identity_slot())
            .unwrap();
        let mut transfer = p6::FaceSurfaceTransfer::new(surface);
        let mut out = NewFace::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        let error = make_face_with_curves_transfer(
            &mut k,
            &mut b,
            None,
            FaceCurvesInput {
                loops: vec![1],
                curves_3d: Some(vec![Some(Curve::from(
                    crate::curve::line::Line::from_points(Point3::ZERO, Point3::X, 3),
                ))]),
                orientations: vec![OrientType::Same],
                ..Default::default()
            },
            &mut transfer,
            &dom(),
            OrientType::Same,
            None,
            &face_ref,
            &mut env,
            &mut out,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SolidError::InvalidInput {
                reason: "loop does not close",
                ..
            }
        ));
        assert_eq!(out.face, None);
        assert_eq!(transfer.installed_face(), None);
        match transfer.into_location().unwrap() {
            p6::FaceSurfaceLocation::Owned(surface) => {
                assert_eq!(surface.notify_key(), Some(key));
                assert_eq!(surface.owner(), None);
            }
            p6::FaceSurfaceLocation::Installed(_) => {
                panic!("surface installed before failed loop construction")
            }
        }
        assert_eq!(b.vertices().len(), 1);
        assert_eq!(b.shells().len(), 1);
        assert_eq!(b.vertexuses().len(), 1);
        assert!(b.faces().is_empty());
        let (vertex, _) = b.vertices().iter().next().unwrap();
        assert!(records.contains_key(&key_of(EntityRef::Vertex(vertex)).key));
    }

    #[test]
    fn face_with_curves_forwards_owner_keys_in_both_context_modes() {
        use crate::geom::context::Context;
        use crate::topology::edge::notification_test_support as callbacks;
        use std::cell::Cell;

        for kernel_context in [false, true] {
            let mut k = Kernel::default();
            let kernel_events = callbacks::install(&mut k.context);
            let mut external = Context::default();
            let external_events = callbacks::install(&mut external);
            let mut b = Brep::new();
            let region = b
                .add_region_record(Region {
                    in_brep: true,
                    ..Region::default()
                })
                .unwrap();
            let shell = b.add_shell().unwrap();
            b.add_face(shell).unwrap();
            let expected_id = FaceId::from_index(1);
            let owner = ObjectRef {
                type_code: fp2::TYPE,
                key: ObjectKey(9001),
            };
            let requested = Cell::new(None);
            let this_face = |face| {
                requested.set(Some(face));
                owner
            };
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attributes = AttributeStore::default();
            let input = FaceCurvesInput {
                loops: vec![1],
                curves_3d: Some(vec![None]),
                orientations: vec![OrientType::Same],
                ..Default::default()
            };
            let key = |entity| match entity {
                EntityRef::Face(face) => {
                    assert_eq!(face, expected_id);
                    owner
                }
                _ => key_of(entity),
            };
            let result = if kernel_context {
                make_face_with_curves_with_kernel_context(
                    &mut k,
                    &mut b,
                    Some(region),
                    input,
                    plane(),
                    &dom(),
                    OrientType::Same,
                    None,
                    &this_face,
                    &mut ed::NotificationStores {
                        records: &mut records,
                        attributes: &mut attributes,
                        key_of: &key,
                    },
                )
            } else {
                make_face_with_curves(
                    &mut k,
                    &mut b,
                    Some(region),
                    input,
                    plane(),
                    &dom(),
                    OrientType::Same,
                    None,
                    &this_face,
                    &mut ed::NotifyEnv {
                        records: &mut records,
                        attributes: &mut attributes,
                        context: Some(&mut external),
                        key_of: &key,
                    },
                )
            };
            let made = result.unwrap();
            assert_eq!(made.face, Some(expected_id));
            assert_eq!(requested.get(), Some(expected_id));
            assert_eq!(
                b.face(expected_id)
                    .unwrap()
                    .surface
                    .as_ref()
                    .unwrap()
                    .owner(),
                Some(owner)
            );
            assert!(records.contains_key(&owner.key));
            let kernel_guard = kernel_events.lock().unwrap();
            let external_guard = external_events.lock().unwrap();
            let (selected, other) = if kernel_context {
                (&*kernel_guard, &*external_guard)
            } else {
                (&*external_guard, &*kernel_guard)
            };
            assert!(!selected.is_empty());
            assert!(other.is_empty());
            drop(kernel_guard);
            drop(external_guard);
            callbacks::assert_installed(&k.context, &kernel_events);
            callbacks::assert_installed(&external, &external_events);
        }
    }

    #[test]
    fn kernel_face_constructor_validates_before_and_after_shell_vertex_construction() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let region = b.add_region_record(Region::default()).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let key_of = crate::topology::loopuse::notify_ref;
        let mut stores = ed::NotificationStores {
            records: &mut records,
            attributes: &mut attributes,
            key_of: &key_of,
        };
        let mut run = |input| {
            make_face_with_curves_with_kernel_context(
                &mut k,
                &mut b,
                Some(region),
                input,
                plane(),
                &dom(),
                OrientType::Same,
                None,
                &|_| panic!("no face before shell vertex construction"),
                &mut stores,
            )
        };
        assert!(matches!(
            run(FaceCurvesInput {
                loops: vec![1],
                ..Default::default()
            }),
            Err(SolidError::InvalidInput {
                reason: "no curves",
                ..
            })
        ));
        let input = FaceCurvesInput {
            loops: vec![1],
            curves_3d: Some(vec![Some(Curve::from(
                crate::curve::line::Line::from_points(Point3::ZERO, Point3::new(1.0, 0.0, 0.0), 3),
            ))]),
            orientations: vec![OrientType::Same],
            ..Default::default()
        };
        assert!(matches!(
            run(input),
            Err(SolidError::InvalidInput {
                reason: "loop does not close",
                ..
            })
        ));
        assert_eq!(b.vertices().len(), 1);
        assert_eq!(b.shells().len(), 1);
        assert_eq!(b.vertexuses().len(), 1);
        assert!(b.edges().is_empty());
        assert!(b.faces().is_empty());
        assert_eq!(records.len(), 3);
        let (vertex, v) = b.vertices().iter().next().unwrap();
        let (shell, s) = b.shells().iter().next().unwrap();
        assert_eq!(v.point, Point3::ZERO);
        assert_eq!(s.region, Some(region));
        assert_eq!(b.region(region).unwrap().shells, vec![shell]);
        for entity in [
            EntityRef::Vertex(vertex),
            EntityRef::Shell(shell),
            EntityRef::Vertexuse(v.vertexuses[0]),
        ] {
            assert!(records.contains_key(&key_of(entity).key));
        }
    }

    #[test]
    fn face_with_curves_input_errors() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let r = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        let mut run = |b: &mut Brep, input: FaceCurvesInput, region| {
            make_face_with_curves(
                &mut k,
                b,
                region,
                input,
                plane(),
                &dom(),
                OrientType::Same,
                None,
                &face_ref,
                &mut env,
            )
        };
        // no curves
        let e = run(
            &mut b,
            FaceCurvesInput {
                loops: vec![1],
                ..Default::default()
            },
            Some(r),
        );
        assert!(matches!(e, Err(SolidError::InvalidInput { .. })));
        // no loops
        let input = FaceCurvesInput {
            curves_3d: Some(Vec::new()),
            ..Default::default()
        };
        assert!(matches!(
            run(&mut b, input, Some(r)),
            Err(SolidError::InvalidInput { .. })
        ));
        // a NULL region is the infinite region: a degenerate curve only leaves no
        // edges, so MakeFace makes an edgeless face
        b.set_infinite_region_record(Some(r));
        let input = FaceCurvesInput {
            loops: vec![1],
            curves_3d: Some(vec![None]),
            orientations: vec![OrientType::Same],
            ..Default::default()
        };
        let first = run(&mut b, input.clone(), None).unwrap();
        let second = run(&mut b, input, Some(r)).unwrap();
        assert_eq!(first.face, Some(FaceId::from_index(0)));
        assert_eq!(second.face, Some(FaceId::from_index(1)));
        // more loop entries than curves
        let input = FaceCurvesInput {
            loops: vec![3],
            curves_3d: Some(vec![None]),
            orientations: vec![OrientType::Same],
            ..Default::default()
        };
        assert!(matches!(
            run(&mut b, input, Some(r)),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn merge_self_without_curves() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        let curve_ref = |_: EdgeId, type_code: i64| ObjectRef {
            type_code,
            key: ObjectKey(0),
        };
        assert!(merge_self_intersection_curves(
            &mut k,
            &mut b,
            Some(f),
            1e-5,
            &[],
            &mut env,
            &curve_ref
        )
        .unwrap()
        .is_empty());
        assert!(merge_self_intersection_curves(
            &mut k,
            &mut b,
            None,
            1e-5,
            &[],
            &mut env,
            &curve_ref
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn read_errors_and_deferral() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let mut store = AttributeStore::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut env = TopologyEnv {
            all_attributes: &[],
            attributes: &mut store,
            key_of: &key_of,
            records: &mut records,
            next_key: 1,
        };
        // an empty file reads as an empty B-rep, whose (absent) UV trim curves
        // sm_CheckAndFixUVTrimCurves finds nothing to fix in
        let r = read_from_file(&mut k, &mut b, Vec::new(), FileType::Ascii, false, &mut env);
        assert!(r.is_ok(), "read: {r:?}");
        let mut src = Brep::new();
        let mut dest = Brep::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        // no faces to copy: past SetTolerance (topology.brep_p2) and the infinite region
        // (topology.brep_p1, #246) nothing runs
        let r = copy_faces(&mut k, &mut src, &[], &mut dest, false, &face_ref, &mut env);
        assert!(
            matches!(&r, Ok((faces, true)) if faces.is_empty()),
            "copy: {r:?}"
        );
    }
}

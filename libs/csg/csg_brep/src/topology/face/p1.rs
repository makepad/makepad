//! `SmFace`, part 1: construction and destruction, loop insertion and removal, inner
//! loop deletion, the surface swap and replacement, orientation flip, UV trim curve
//! removal, the trim boundary and UV domain built from the UV trim curves, the
//! rectangular-trim test,
//! geometry shrinking, and the seam repair of faces on periodic surfaces (`MoveSeam`,
//! `SplitAtSeam`).
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`crate::topology::Face::new`]               | `SmFace::SmFace`                             |
//! | [`destroy`]                                  | `SmFace::~SmFace`                            |
//! | [`calculate_uv_domain_from_uv_trim_curves`], [`calculate_uv_domain_from_uv_trim_curves_into`], [`UvDomainFromTrim`] | `SmFace::CalculateUVDomainFromUVTrimCurves` |
//! | [`create_oriented_trim_boundary`], [`OrientedTrimBoundary`] | `SmFace::CreateOrientedTrimBoundary` |
//! | [`find_if_rectangular_trim`], [`RectangularTrim`] | `SmFace::FindIfRectangularTrim`          |
//! | [`flip_face_orientation`]                    | `SmFace::FlipFaceOrientation`                |
//! | [`set_surface`], [`set_surface_notified`]    | `SmFace::SetSurface`                         |
//! | [`insert_loop`]                              | `SmFace::InsertLoop`                         |
//! | [`remove_loop`]                              | `SmFace::RemoveLoop`                         |
//! | [`remove_uv_trim_curves`]                    | `SmFace::RemoveUVTrimCurves`                 |
//! | [`delete_inner_loop`], [`delete_inner_loop_with_kernel_callbacks`] | `SmFace::DeleteInnerLoop` |
//! | [`swap_uv`]                                  | `SmFace::SwapUV`                             |
//! | [`move_seam`], [`move_seam_notified`], [`MoveSeamOptions`], [`MoveSeamResult`], [`SeamProblem`] | `SmFace::MoveSeam` |
//! | [`split_at_seam`], [`SeamSplit`]             | `SmFace::SplitAtSeam`                        |
//! | [`shrink_geometry`], [`shrink_geometry_with`] | `SmFace::ShrinkGeometry`                    |
//! | [`FixSeamPlanType`]                          | `SmFixSeamPlanType` (`SmTypes.h`)            |
//!
//! Deviations:
//! - **Object model.** A face is a record in the [`Brep`]'s arenas; the methods are free
//!   functions taking the B-rep and the face's id. Out-parameters are return values,
//!   with an additional UV-domain `_into` entry point for partial outputs on error.
//!   Where upstream dereferences null (a face without surface or face use, a loop
//!   without loop uses, an edge use without edge, an edge without curve, a loop use
//!   without face use) the port returns [`SolidError::InvalidInput`]. The identity a
//!   face, edge or edge use is known by (a surface's or curve's owner) comes from the
//!   caller's `key_of`, as [`crate::topology::edge::NotifyEnv`]'s.
//! - **The constructor** is [`crate::topology::Face::new`] (its `SM_NO_CONSTRUCTION`
//!   notification reaches only the object layer's user callback, which the port does
//!   not have). [`destroy`] stands for the destructor: arena records are never freed,
//!   so it notifies, unlinks the face use and drops a surface the face owns.
//! - **Side-effect free getters** whose result upstream only reads later
//!   (`GetUpwardLoopuses` of `CalculateUVDomainFromUVTrimCurves`, `GetBrep` and
//!   `SmTol::GetZoneTol3d` of `MoveSeam`, `GetBrep` of `SplitAtSeam`) are called where
//!   their result is first used.
//! - **Pointer identity.** `SetSurface` compares the new surface pointer with the
//!   current one; a surface passed by value is never the current one, so the "same
//!   surface" no-op has no counterpart. Without `delete_current` the old surface is
//!   handed back (upstream leaves it to its other owners).
//! - **Loop use lists.** `SmFaceuse::PostInsert` / `Remove` (inline `SmOwningTopology`
//!   list edits) append to / remove from [`crate::topology::Faceuse::loopuses`] and set /
//!   clear the loop use's face use. `Remove` of a loop use not on the face use's list is
//!   `SM_ERR`, as upstream's owner check.
//! - **`SwapUV` notifications.** [`swap_uv`] (no environment) sends the face's
//!   notifications under the empty environment and the surface's to nobody;
//!   [`swap_uv_with_notifications`] sends both through the caller's (the surface's
//!   under its notify identity from the env's context, reported owned by the face).
//!   The owner links of surfaces nested in an offset surface or a surface in a volume
//!   are not reported: a callback walking their owners finds none.
//! - **`CreateOrientedTrimBoundary`** returns the curves and per-loop counts in
//!   [`OrientedTrimBoundary`]. A failed `SmLine::CreateLineSegment` (only possible for
//!   coincident points, which the gap test excludes) is [`SolidError::InvalidInput`]
//!   (upstream adds an uninitialised pointer). `SM_CAST_PTR(SmBSplineCurve, ..)` of a
//!   3D edge curve of another kind is `SM_ERR_NULL_POINTER` (`NER`). The mirror of a
//!   line UV trim curve in `SwapUV` is the mirror of its NURB (as upstream's
//!   `MakeNurb` and `SmBSplineCurve` copy).
//! - **`MoveSeam` / `SplitAtSeam`** take the seam classifications and the optional face
//!   properties over any [`LoopObject`]; their output arrays are returned in
//!   [`MoveSeamResult`] / [`SeamSplit`]. Statuses upstream ignores (curve
//!   `EvaluateGeometric`, `DropPoint`, `DropVectors`, `ComputeUVPoint`, `GetKnots`,
//!   `SmPeriodicExtent1d::SetMinMax` / `AddInterval`, `CopyAndAddAnalytics`, the
//!   recursive `SplitAtSeam` / `HasSeamProblem` of `SplitAtSeam`) are ignored
//!   (`SplitAtSeam`'s `MergeIntervals` statuses are checked); [`SolidError::Unported`]
//!   always propagates. A failed drop leaves the UV
//!   point undefined (upstream's initial `rResultUV`). An empty vertex use list in the
//!   full-period branch of `MoveSeam` is [`SolidError::InvalidInput`] (upstream reads
//!   index 0 of an empty array), and the paired vertex use loop does not underflow on
//!   an empty pairing (upstream's unsigned `GetSize() - 1`). `SplitAt` / `JoinSurface`
//!   halves that are not plain B-spline surfaces (an assert upstream, then a C cast)
//!   are [`SolidError::InvalidInput`]. A failed `JoinSurface` (unchecked upstream,
//!   which then uses a null or stale joined surface: `plan/upstream-issues.md` UB-138)
//!   returns its error. The `ShellSort` of the vertex parameters is a sort by value
//!   (equal doubles are indistinguishable). `SplitAtSeam`'s V pass after a U merge
//!   recurses on this face with the classifications of each new face, as upstream
//!   (UB-137).
//! - **`bUseJGU`** (`static constexpr FALSE` in `SplitAtSeam`) branches are dead code
//!   upstream and are not ported. The `SmTemporaryChangeValue`s of `SplitAtSeam`
//!   (the B-rep's editing flag, `GetDoingBooleanRef`) set both flags from the merges
//!   to the routine's end and restore them on every return.
//! - **`MergeIntervals`** edits the face's own B-rep
//!   ([`crate::topology::face::p4::BrepSeamObjects::for_merge`], through
//!   `CurveClassification::merge_intervals_mut`), so `MoveSeam` / `SplitAtSeam` take
//!   the kernel mutably and no caller classification geometry. Its notifications go
//!   to the merge objects' own empty records, not to `move_seam_notified`'s `env`.
//!   The seam curve upstream's classification owns is rebuilt from the face's surface
//!   as `HasSeamProblem` builds it (`merge_seam_intervals`); a vertex/face gap the
//!   merge reads is the vertex use's cached one (the merge holds the kernel). It
//!   returns handles; the port maps its new faces, edges and vertices to ids through
//!   the face's objects, else the caller's `resolve`, and returns
//!   [`SolidError::InvalidInput`] for a handle that does not resolve to an entity of
//!   the expected kind (upstream's lists hold the pointers directly). Its own status
//!   is ignored, as upstream, keeping the lists it filled before a failure.
//! - **`IsCrossedByEdge` / `HasNearMisses`** run on the face's own B-rep objects
//!   ([`crate::topology::face::p4::BrepSeamObjects`]) and return a plain boolean
//!   upstream; a failure of the port other than [`SolidError::Unported`] (or, for
//!   `HasNearMisses`, [`SolidError::InvalidInput`] for a handle the objects cannot
//!   resolve) reads as `FALSE`.
//! - **Notifications** (`SmFace::Notify`, `topology.face_p3`) run with the empty
//!   notification environment a B-rep face carries (as `topology::loopuse`'s do):
//!   `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` / `SM_NO_DESTRUCTION` with the face as the
//!   first data argument. `SetSurface`'s `SM_NO_CHANGE_GEOMETRY` has two forms:
//!   [`set_surface`] runs it in that empty environment with null data (upstream: the
//!   new surface, `SM_NO_GET_BREP(this)` (null) and the old surface). Upstream the
//!   surfaces only reach `SmTopology::Notify`, which copies the old surface's
//!   attributes to the new one (`SmAObject::Notify`) and hands them to the context's
//!   callbacks; the empty environment has no attribute store or callbacks to give them
//!   to. [`set_surface_notified`] (`SmBrep::ReplaceSurface`'s call) runs it in the
//!   caller's environment with the surfaces' identities ([`Surface::notify_key`]) the
//!   caller's context issued, so a change tracker sees the same event as upstream.
//!   `CreateUVTrimCurves` is `topology::face::p3::create_uv_trim_curves`.
//! - **`HasSeamProblem`** (`topology.face_p4`) runs on the face's own B-rep objects
//!   ([`crate::topology::face::p4::has_seam_problem_in_brep_mut`]), so the classification
//!   handle type must name them. The seam curves its classifications name are the
//!   handle type's temporaries (upstream's classifications own them); `SplitAtSeam`'s
//!   merges rebuild them (above), and a handle the merge cannot resolve is
//!   [`SolidError::InvalidInput`].
//!   `SmEdgeuse::NormalizedEvaluate` and `GetOrCreateUVTrimCurve` call
//!   [`crate::topology::edgeuse::p3`] (wired in PR #117).
//!   `SmBrep::ReplaceSurface` is [`brep_p2::replace_surface`], which reaches
//!   `SmFace::Notify`.
//! - **Bounds.** `SplitAtSeam` recurses into the faces its U merge made, once per face
//!   (upstream asserts the recursion only fixes V seams); the port carries a depth and
//!   returns [`SolidError::InvalidInput`] past [`MAX_SPLIT_AT_SEAM_DEPTH`]. Every other
//!   loop is over B-rep arrays, knots or a fixed sample count.
//! - **`DeleteInnerLoop`** notifies through the caller's environment
//!   ([`delete_inner_loop`]) or the kernel context's callbacks
//!   ([`delete_inner_loop_with_kernel_callbacks`]); its `RemoveLoop` sends the face's
//!   edit notifications there too (the [`remove_loop`] entry uses the empty environment
//!   above). The destructors' `~SmTopology` list removals (vertex use from its vertex,
//!   edge use from its edge, vertex and edge from the B-rep's lists) are explicit; the
//!   records leave their arenas. A loop without both loop uses, or an edge use without
//!   edge, vertex use or vertex (upstream dereferences null) is
//!   [`SolidError::InvalidInput`].
//! - **Not upstream: [`shrink_geometry_with`]** (source issue #472) passes
//!   [`TrimOptions`] to the surface trim; [`shrink_geometry`] uses the defaults
//!   (upstream). See `nurbs::b_spline_surface_n_lib::p2`'s Deviations.

use core::cmp::Ordering;

use crate::curve::curve::p2::evaluate_geometric;
use crate::curve::curve::p2::CurveKind;
use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::attribute::ObjectKey;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::IdentitySlot;
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::periodic_extent1d::PeriodicExtent1d;
use crate::geom::tol::are_same_to_tol;
use crate::geom::vector2d::{Point2, Vec2};
use crate::heal::face_props::{FaceProps, LoopObject, Tri};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::{ClassGeometry, PointClassType};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::{OrientType, TrimOptions};
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::B_SPLINE_SURFACE_TYPE;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::brep::p2::{self as brep_p2, OldSurface};
use crate::topology::brep::p3 as brep_p3;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p2 as fp2;
use crate::topology::face::p3 as fp3;
use crate::topology::face::p4::BrepSeamObjects;
use crate::topology::loopuse as lu1;
use crate::topology::trimming_tools as tt;
use crate::topology::vertex as vx;
use crate::topology::vertexuse as vu1;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, LoopId, LoopuseId, VertexId, VertexuseId,
};

const OP: &str = "topology::face::p1";

/// Upstream `SM_ERR_ASSERT_FAILURE` (`SmMessages.h`).
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// The deepest [`split_at_seam`] recursion accepted (upstream recurses once, from a U
/// merge into the V merges of the faces it made).
pub const MAX_SPLIT_AT_SEAM_DEPTH: usize = 8;

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `SmFixSeamPlanType`: what [`move_seam`] did to a seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixSeamPlanType {
    /// `SM_FSP_UNKNOWN`: not set.
    #[default]
    Unknown = 0,
    /// `SM_FSP_NO_CHANGES`: the face has no seam problem in this direction.
    NoChanges = 1,
    /// `SM_FSP_MOVE_OUT_OF_FACE`: the seam was moved out of the face.
    MoveOutOfFace = 2,
    /// `SM_FSP_MOVE_IN_FACE`: the seam was moved into the face (to be split there).
    MoveInFace = 3,
    /// `SM_FSP_SPLIT_FACE`: the face was split at the seam.
    SplitFace = 4,
}

// ---- unported callees ---------------------------------------------------------------------

/// `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT, this, SM_NO_GET_BREP(this) (0), NULL)`:
/// the face itself is the first argument. Runs [`fp3::notify`] with the empty
/// notification environment the B-rep model allows (as the loop use code does).
fn face_notify(b: &mut Brep, f: FaceId, operation: NotifyOperation) -> Result<()> {
    lu1::face_notify_edit(b, f, operation)
}

/// `Notify(SM_NO_CHANGE_GEOMETRY, pSurface, SM_NO_GET_BREP(this), m_pSurface)`:
/// [`fp3::notify`] under the empty notification environment of
/// [`lu1::face_notify_data`]. The new and the old surface (data 1 and 3) have no object
/// identity in the B-rep model and the B-rep (data 2) none either, so all three data
/// arguments are null (see the Deviations).
fn face_notify_change_geometry(b: &mut Brep, f: FaceId) -> Result<()> {
    lu1::face_notify_data(b, f, NotifyOperation::ChangeGeometry, [None; 3])
}

/// `SmFace::CreateUVTrimCurves(FALSE, NULL, NULL, ..)`.
fn create_uv_trim_curves(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    // the gap statistics (the NULL out-parameters) are not read
    fp3::create_uv_trim_curves(k, b, f, false, None).map(|_| ())
}

/// `SmFace::GetUpwardLoopuses(rLoopuses)` (default arguments).
fn upward_loopuses(b: &Brep, f: FaceId) -> Result<Vec<LoopuseId>> {
    super::p4::upward_loopuses(b, f, false, None)
}

/// `SmFace::GetUpwardVertexuses(rVertexuses)` (default arguments).
fn upward_vertexuses(b: &Brep, f: FaceId) -> Result<Vec<VertexuseId>> {
    super::p4::upward_vertexuses(b, f, false, None)
}

/// `SmFace::GetEdges(rEdges)` (default arguments).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    super::p4::edges(b, f, None, false, None)
}

/// `SmFace::GetBrep() != NULL`.
fn has_brep(b: &Brep, f: FaceId) -> Result<bool> {
    Ok(super::p4::brep(b, f)?.is_some())
}

/// The seam problem of a face (`HasSeamProblem`'s enum outputs; `SM_SP_*` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeamProblem {
    /// `eCrossedSeam`: the seams face edges cross.
    pub crossed: SurfParamType,
    /// `eMissingSeam`: the seams no seam edge represents.
    pub missing: SurfParamType,
    /// `eNearMissSeam`: the seams face edges nearly touch.
    pub near_miss: SurfParamType,
}

/// `SmFace::HasSeamProblem(rCrvClassU, rCrvClassV, eCrossed, eMissing, eNearMiss,
/// bTgtLoopsOnly)`: rebuilds the classifications and the problem.
///
/// The classifications name the face's own B-rep objects
/// ([`crate::topology::face::p4::has_seam_problem_in_brep_mut`]); the other optional
/// arguments keep their defaults (periodicity tested, local face properties).
fn has_seam_problem<O: LoopObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    problem: &mut SeamProblem,
    tgt_loops_only: bool,
) -> Result<bool> {
    let check = crate::topology::face::p4::has_seam_problem_in_brep_mut(
        k,
        b,
        f,
        crate::topology::face::p4::HasSeamProblemOptions {
            outer_loop_only: tgt_loops_only,
            ..Default::default()
        },
    )?;
    *crv_class_u = check.class_u;
    *crv_class_v = check.class_v;
    *problem = check.problem;
    Ok(check.has_problem)
}

/// `pEU->NormalizedEvaluate(dT, TRUE, rUV)` (UV evaluation; default arguments: no
/// first derivative, `bOKToMakeUVTrimCurve = TRUE`).
fn normalized_evaluate_uv(k: &Kernel, b: &mut Brep, eu: EdgeuseId, t: f64) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, eu, t, true, false, true)?.point)
}

/// `pEU->GetOrCreateUVTrimCurve(rpUVCurve)` (default arguments: no distance or
/// deviation, `bOKToMakeUVTrimCurve = TRUE`): the returned status, and the edge use's
/// (or its mate's) UV trim curve afterwards, which a failing status can still leave
/// attached.
fn fetch_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
) -> Result<(std::result::Result<(), SolidError>, Option<Curve>)> {
    let fetch = eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
    let curve = eu1::uv_trim_curve(b, eu)?.0.cloned();
    Ok((fetch.status, curve))
}

/// `SER(pEU->GetOrCreateUVTrimCurve(rpUVCurve))`: the UV trim curve; a failing status
/// returns, and a missing curve is the null dereference that follows upstream.
fn get_or_create_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<Curve> {
    let (status, curve) = fetch_uv_trim_curve(k, b, eu)?;
    status?;
    curve.ok_or_else(|| SolidError::null_pointer(OP))
}

/// `rCrvClass.IsCrossedByEdge()`: upstream returns a plain boolean, so a failure other
/// than [`SolidError::Unported`] reads as `FALSE`.
fn is_crossed_by_edge<O: LoopObject>(
    geometry: &impl ClassGeometry<O>,
    c: &CurveClassification<O>,
) -> Result<bool> {
    Ok(ignored(c.is_crossed_by_edge(geometry))?.unwrap_or(false))
}

/// `rCrvClass.HasNearMisses()` (default `bInSurfaceGaps = FALSE`): upstream returns a
/// plain boolean, so a failure reads as `FALSE`, except an unresolvable caller handle
/// ([`SolidError::InvalidInput`]) and [`SolidError::Unported`], which propagate.
fn has_near_misses<O: LoopObject>(
    k: &Kernel,
    c: &CurveClassification<O>,
    geometry: &impl ClassGeometry<O>,
) -> Result<bool> {
    match c.has_near_misses(k, geometry, false) {
        Ok(v) => Ok(v),
        Err(e)
            if e.is_unported()
                || matches!(
                    e,
                    SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
                ) =>
        {
            Err(e)
        }
        Err(_) => Ok(false),
    }
}

/// `pCrvClass->MergeIntervals(&faces, &edges, &vertices, this)` (unit
/// `intersect.curve_class_p5`; the original face is a debug-only argument there) on
/// face `f`'s own B-rep for its `dir` seam
/// ([`crate::topology::face::p4::merge_seam_intervals`]): the faces, edges and
/// vertices the merge made, resolved to ids by the face's objects, else by `resolve`.
fn merge_intervals<O: LoopObject>(
    k: &mut Kernel,
    c: &mut CurveClassification<O>,
    dir: SurfParamType,
    b: &mut Brep,
    env: Option<&mut crate::topology::edge::NotifyEnv<'_>>,
    f: FaceId,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
) -> Result<SeamSplit> {
    // (SER: a failing merge returns its status; v1.0.1 checks `SplitAtSeam`'s MergeIntervals)
    let merged = crate::topology::face::p4::merge_seam_intervals(k, b, env, f, dir, c, false)?
        .map_err(|e| e.error)?;
    let objects = BrepSeamObjects::new(k, &*b, f)?;
    let resolve = |o: O| objects.entity(o).or_else(|| resolve(o));
    let mut out = SeamSplit::default();
    for o in merged.faces {
        match resolve(o) {
            Some(EntityRef::Face(f)) => out.faces.push(f),
            _ => return Err(null("MergeIntervals made a face the caller cannot resolve")),
        }
    }
    for o in merged.edges {
        match resolve(o) {
            Some(EntityRef::Edge(e)) => out.edges.push(e),
            _ => {
                return Err(null(
                    "MergeIntervals made an edge the caller cannot resolve",
                ))
            }
        }
    }
    for o in merged.vertices {
        match resolve(o) {
            Some(EntityRef::Vertex(v)) => out.vertices.push(v),
            _ => {
                return Err(null(
                    "MergeIntervals made a vertex the caller cannot resolve",
                ))
            }
        }
    }
    Ok(out)
}

/// `pBrep->ReplaceSurface(this, pNewSurface, FALSE, NULL, FALSE)` of `MoveSeam`:
/// delete the old surface here ([`OldSurface::Delete`]), no modified-face list, no new
/// UV trim curves. Upstream ignores the status, which is always `SM_SUCCESS` without
/// new UV trim curves; only [`SolidError::Unported`] and the port's null-dereference
/// [`SolidError::InvalidInput`] can come back, and both propagate. Its face
/// notifications run under the empty environment of [`lu1::face_notify_as`] (the
/// B-rep carries no attribute store, topology records or context callbacks), with
/// the face notified as `this`.
fn replace_surface(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    this: ObjectRef,
    new_surface: Surface,
) -> Result<()> {
    let mut records = crate::topology::topology::NotifyRecords::new();
    let mut attributes = crate::geom::attribute::AttributeStore::default();
    let key_of = |e: EntityRef| {
        if e == EntityRef::Face(f) {
            this
        } else {
            lu1::notify_ref(e)
        }
    };
    let mut env = crate::topology::edge::NotifyEnv {
        records: &mut records,
        attributes: &mut attributes,
        context: None,
        key_of: &key_of,
    };
    brep_p2::replace_surface(
        k,
        b,
        &mut env,
        f,
        new_surface,
        this,
        OldSurface::Delete,
        false,
    )?;
    Ok(())
}

// ---- helpers ------------------------------------------------------------------------------

/// The surface of `f`, which upstream dereferences.
fn surface_deref(b: &Brep, f: FaceId) -> Result<&Surface> {
    fp2::surface(b, f)?.ok_or_else(|| null("face without surface"))
}

/// Component `i` (`0` u, else v) of a UV point (`SmPoint2d::operator[]`).
const fn comp(p: Point2, i: usize) -> f64 {
    if i == 0 {
        p.x
    } else {
        p.y
    }
}

/// The edge of `eu`, which upstream dereferences.
fn edge_deref(b: &Brep, eu: EdgeuseId) -> Result<EdgeId> {
    eu1::edge(b, eu)?.ok_or_else(|| null("edge use without edge"))
}

/// The normal bounding box of `curve` over its natural interval
/// (`CalculateBoundingBox(GetNaturalInterval(), &box)`), as a UV extent.
fn uv_curve_box(k: &Kernel, curve: &Curve) -> Result<crate::geom::extent3d::Extent3d> {
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    curve
        .calculate_bounding_box(k, &curve.natural_interval(), req, true)?
        .normal
        .ok_or_else(|| null("bounding box not computed"))
}

// ---- destruction --------------------------------------------------------------------------

/// `~SmFace`: notifies the destruction, clears the face use and drops the surface when
/// the face (`this`) owns it.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::Unported`] for the
/// notification (`SmFace::Notify`, `topology.face_p3`).
pub fn destroy(b: &mut Brep, f: FaceId, this: ObjectRef) -> Result<()> {
    // Notify DESTRUCTION behavior: Notify(SM_NO_DESTRUCTION, this, NULL, NULL)
    lu1::face_notify_as(b, f, this, NotifyOperation::Destruction)?;
    let face = b.face_mut(f)?;
    face.faceuse = None;

    // If there is a surface and I own it then delete it.
    if face
        .surface
        .as_ref()
        .is_some_and(|s| s.owner() == Some(this))
    {
        face.surface = None;
    }
    Ok(())
}

// ---- UV domain from the UV trim curves ----------------------------------------------------

/// The outputs of [`calculate_uv_domain_from_uv_trim_curves`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvDomainFromTrim {
    /// `rNewFaceDomain`: the union of the UV trim curves' boxes (limited to the
    /// surface's natural domain when asked).
    pub domain: Extent2d,
    /// `*pOptMaxUVLoopGap`: the largest start-point UV loop gap of the loops (`0` when
    /// not asked for).
    pub max_uv_loop_gap: f64,
    /// `*pOpt2ndMaxUVLoopGap`: the largest second-largest gap of a loop (`0` when not
    /// asked for, or when the largest gap was not asked for).
    pub second_max_uv_loop_gap: f64,
}

/// `CalculateUVDomainFromUVTrimCurves`: makes the face's UV trim curves and returns the
/// union of their bounding boxes, intersected with the surface's natural domain when
/// `limit_domain` (the natural domain itself without edge uses). A face without surface
/// keeps its UV domain; one whose UV trim curves cannot be made gets its UV domain
/// intersected with the surface's natural domain (an `Init` extent when they are
/// disjoint). `want_max_gap` /
/// `want_2nd_max_gap` ask for the loop gap outputs (`pOptMaxUVLoopGap`,
/// `pOpt2ndMaxUVLoopGap`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// use without UV trim curve; those of the bounding boxes, the loop gaps and the domain
/// intersection (`SM_ERR_INVALID_INPUT` for disjoint domains); [`SolidError::Unported`]
/// (`CreateUVTrimCurves`, `GetUpwardLoopuses`).
pub fn calculate_uv_domain_from_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    limit_domain: bool,
    want_max_gap: bool,
    want_2nd_max_gap: bool,
) -> Result<UvDomainFromTrim> {
    let mut out = UvDomainFromTrim {
        domain: Extent2d::new(),
        max_uv_loop_gap: 0.0,
        second_max_uv_loop_gap: 0.0,
    };
    calculate_uv_domain_from_uv_trim_curves_into(
        k,
        b,
        f,
        limit_domain,
        want_max_gap,
        want_2nd_max_gap,
        &mut out,
    )?;
    Ok(out)
}

/// Calculates the trim domain, retaining initialized and partial outputs on error.
///
/// The output follows native assignment order: the face/surface intersection remains
/// available if a later bounding-box, loop-gap, or final domain intersection fails.
/// If the face id is invalid, `out` is unchanged.
///
/// # Errors
///
/// As [`calculate_uv_domain_from_uv_trim_curves`].
pub fn calculate_uv_domain_from_uv_trim_curves_into(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    limit_domain: bool,
    want_max_gap: bool,
    want_2nd_max_gap: bool,
    out: &mut UvDomainFromTrim,
) -> Result<()> {
    // Init output
    *out = UvDomainFromTrim {
        domain: fp2::uv_domain(b, f)?,
        max_uv_loop_gap: 0.0,
        second_max_uv_loop_gap: 0.0,
    };

    // Check State - no Face->Surface
    let Some(surface) = fp2::surface(b, f)? else {
        return Ok(());
    };
    let surf_domain = surface.natural_uv_domain_with_nurb(k)?;

    // limit ThisDomain to XSect(Face->UVDomain, Face->Surface->UVDomain) (a fix for a
    // common problem); its status is ignored: disjoint domains give an Init()-ed extent
    out.domain = out
        .domain
        .intersect(&surf_domain)
        .unwrap_or_else(|_| Extent2d::new());

    // place a UVTrimCurve on each Face->Edgeuse
    match create_uv_trim_curves(k, b, f) {
        Ok(()) => {}
        Err(e) if e.is_unported() => return Err(e),
        // Return the default from above.
        Err(_) => return Ok(()),
    }
    let loopuses = upward_loopuses(b, f)?;

    // Build union of all face->Edgeuse UVBoundingBoxes
    // Save Loop Edgeuse_to_Edgeuse End_to_End gaps
    let mut face_domain = Extent2d::new();
    let mut box_set = false;
    for (ii, &lu) in loopuses.iter().enumerate() {
        let edgeuses = eu1::loopuse_edgeuses(b, lu)?;
        let mut this_max = 0.0;
        let mut this_2nd_max = 0.0;

        for (jj, &eu) in edgeuses.iter().enumerate() {
            let uv_box = {
                let curve = eu1::uv_trim_curve(b, eu)?
                    .0
                    .ok_or_else(|| null("edge use without UV trim curve"))?;
                uv_curve_box(k, curve)?
            };

            // Get Max Edgeuse_to_Edgeuse 2d UVSpace gap
            if want_max_gap {
                let this_gap = eu1::start_point_uv_loop_gap(k, b, eu)?;
                if this_max < this_gap {
                    this_2nd_max = this_max;
                    this_max = this_gap;
                }
            }

            // convert 3d Extent to 2d
            let uv_crv_box = Extent2d::from_extent3d(&uv_box);

            // get union of Edgeuse UVTrimCurve BoundingBoxes
            face_domain = if ii == 0 && jj == 0 {
                uv_crv_box
            } else {
                face_domain.union(&uv_crv_box)
            };
            box_set = true;
        }

        if want_max_gap && out.max_uv_loop_gap < this_max {
            out.max_uv_loop_gap = this_max;
        }
        if want_2nd_max_gap && out.second_max_uv_loop_gap < this_2nd_max {
            out.second_max_uv_loop_gap = this_2nd_max;
        }
    }

    // If we are limiting the size of the return domain,
    if limit_domain {
        face_domain = if box_set {
            face_domain.intersect(&surf_domain)?
        } else {
            surf_domain
        };
    }

    // all done - set output and return
    out.domain = face_domain;
    Ok(())
}

// ---- oriented trim boundary ---------------------------------------------------------------

/// The outputs of [`create_oriented_trim_boundary`].
#[derive(Debug, Clone)]
pub struct OrientedTrimBoundary {
    /// `rLoops`: the number of curves of each (edge) loop.
    pub loops: Vec<usize>,
    /// `rTrimBoundary`: the curves, loop by loop (counter-clockwise for the first loop,
    /// clockwise for the others).
    pub curves: Vec<Curve>,
}

/// `CreateOrientedTrimBoundary`: copies of the face's boundary curves in loop order,
/// oriented so the face lies on their left: its UV trim curves (2D) when
/// `parameter_space`, else its trimmed 3D edge curves. With `parameter_space` and
/// `fill_singularities`, UV lines close the gaps between consecutive curves (surface
/// singularities). Only `loop_to_use` is walked when given. Missing UV trim curves are
/// made first. A copied UV trim curve is owned by its edge use (`key_of`).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for a loop of another face; `SM_ERR_NULL_POINTER` for a 3D
/// edge curve that is not a B-spline kind; [`SolidError::InvalidInput`] where upstream
/// dereferences null; those of the copies, conversions, reversals and evaluations;
/// [`SolidError::Unported`] (`CreateUVTrimCurves`, and the deferred callees of
/// `NormalizedEvaluate` / `GetOrCreateUVTrimCurve`).
#[allow(clippy::too_many_lines)] // one upstream routine
pub fn create_oriented_trim_boundary(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    parameter_space: bool,
    fill_singularities: bool,
    loop_to_use: Option<LoopId>,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<OrientedTrimBoundary> {
    if let Some(l) = loop_to_use {
        if lu1::loop_face(b, l)? != Some(f) {
            return Err(SolidError::invalid_input(OP, "loop is not on this face"));
        }
    }

    let have_trimming_curves = match loop_to_use {
        Some(l) => tt::loop_uv_curves_present(b, l)?,
        None => tt::face_uv_curves_present(b, f)?,
    };

    // Create trimCurves for all Face->Edgeuses
    if !have_trimming_curves {
        create_uv_trim_curves(k, b, f)?;
    }

    let mut out = OrientedTrimBoundary {
        loops: Vec::new(),
        curves: Vec::new(),
    };

    // get face's loop-use array
    let loopuses = if let Some(l) = loop_to_use {
        vec![lu1::loop_loopuses(b, l)?.0]
    } else {
        let fu = fp2::upward_faceuse(b, f)?.ok_or_else(|| null("face without face use"))?;
        eu1::faceuse_loopuses(b, fu, false, None)?
    };

    // WR-491, opt-in: the periods of the directions in which the surface is closed
    let periods = if k.fixes.trim_boundary_seam_wrap && parameter_space && fill_singularities {
        seam_periods(k, b, f)?
    } else {
        [None, None]
    };

    // for every loop
    for lu in loopuses {
        // no work - lone vertex
        if lu1::is_vertex_loopuse(b, lu)? {
            continue;
        }

        // get array of loop's Edgeuses
        let edgeuses = eu1::loopuse_edgeuses(b, lu)?;
        let Some(&last_eu) = edgeuses.last() else {
            continue;
        };

        let first_curve = out.curves.len();
        // With the fix, a first pass shifts the curves after a seam wrap; a loop that does
        // not close that way is made again as upstream does.
        for wrap in [periods != [None, None], false] {
            out.curves.truncate(first_curve);
            let mut shift = Vec3::new(0.0, 0.0, 0.0);

            // the last edge's end-point (status ignored)
            let mut last_pnt = Vec3::UNDEF;
            if let Some(p) = ignored(normalized_evaluate_uv(k, b, last_eu, 1.0))? {
                last_pnt = p;
            }

            let mut edges_in_loop = 0usize;

            // for every edgeuse in this loop
            for &eu in &edgeuses {
                if eu1::orientation(b, eu)? == OrientType::Same {
                    let uv_curve = get_or_create_uv_trim_curve(k, b, eu)?;
                    let ivl = uv_curve.natural_interval();

                    // get trim curve's start UV point
                    let mut pnt = uv_curve.evaluate_point(k, ivl.min())?;
                    if wrap {
                        shift = shift + seam_wrap(k, b, f, last_pnt, pnt, shift, periods)?;
                        pnt += shift;
                    }

                    // get dist from this start pt to last curve's end-point
                    let diff = pnt - last_pnt;
                    let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());
                    if diff.length_squared() > scaled_zero && parameter_space && fill_singularities
                    {
                        // If there is a gap create a line to fill it
                        edges_in_loop += 1;
                        out.curves.push(gap_line(last_pnt, pnt)?);
                    }

                    // Get UVCurve->End UVPoint
                    last_pnt = uv_curve.evaluate_point(k, ivl.max())?;
                    if wrap {
                        last_pnt += shift;
                    }
                    edges_in_loop += 1;
                    if parameter_space {
                        // (status ignored upstream; only NER(pOrig))
                        let orig = fetch_uv_trim_curve(k, b, eu)?
                            .1
                            .ok_or_else(|| SolidError::null_pointer(OP))?;
                        let mut bsc = orig.copy(k)?;
                        bsc.convert_to_2d(k)?;
                        let _ = bsc.set_owner(Some(key_of(EntityRef::Edgeuse(eu))), None);
                        if wrap {
                            shift_curve(k, &mut bsc, shift)?;
                        }
                        out.curves.push(bsc);
                    } else {
                        let e = edge_deref(b, eu)?;
                        out.curves.push(trimmed_edge_curve(k, b, e)?);
                    }
                } else {
                    let mut bsc = if parameter_space {
                        let orig = get_or_create_uv_trim_curve(k, b, eu)?;
                        let mut bsc = orig.copy(k)?;
                        bsc.convert_to_2d(k)?;
                        let _ = bsc.set_owner(orig.owner(), None);
                        bsc
                    } else {
                        let e = edge_deref(b, eu)?;
                        let owner = edge_bspline_curve(b, e)?.owner();
                        let mut bsc = trimmed_edge_curve(k, b, e)?;
                        let _ = bsc.set_owner(owner, None);
                        bsc
                    };

                    let ivl = bsc.natural_interval();
                    let ivl = bsc.reverse_parameterization(k, &ivl)?;
                    let mut pnt = bsc.evaluate_point(k, ivl.min())?;
                    if wrap {
                        shift = shift + seam_wrap(k, b, f, last_pnt, pnt, shift, periods)?;
                        pnt += shift;
                    }
                    let diff = pnt - last_pnt;
                    let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());
                    if diff.length_squared() > scaled_zero && parameter_space && fill_singularities
                    {
                        // If there is a gap create a line to fill it
                        edges_in_loop += 1;
                        out.curves.push(gap_line(last_pnt, pnt)?);
                    }
                    last_pnt = bsc.evaluate_point(k, ivl.max())?;
                    if wrap {
                        last_pnt += shift;
                    }
                    edges_in_loop += 1;
                    if wrap {
                        shift_curve(k, &mut bsc, shift)?;
                    }
                    out.curves.push(bsc);
                }
            }

            // a loop shifted by a net period is not closed: upstream's gap lines instead
            if wrap && (shift.x != 0.0 || shift.y != 0.0) {
                continue;
            }
            out.loops.push(edges_in_loop);
            break;
        }
    }

    Ok(out)
}

/// How near a whole number of periods a UV gap must be to count as a seam wrap, as a
/// fraction of the period (WR-491).
const SEAM_WRAP_RESIDUAL: f64 = 0.01;

/// The periods (natural domain lengths) of the directions in which the face's surface is
/// closed within twice the face tolerance (two of its points each that near a shared place), `None` in the others (WR-491).
fn seam_periods(k: &Kernel, b: &Brep, f: FaceId) -> Result<[Option<f64>; 2]> {
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| null("face without surface"))?;
    let tolerance = fp2::tolerance(b, f)?;
    let dom = surface.natural_uv_domain_with_nurb(k)?;
    let mut periods = [None, None];
    for (slot, (length, direction)) in periods.iter_mut().zip([
        (dom.x_length(), SurfParamType::U),
        (dom.y_length(), SurfParamType::V),
    ]) {
        if length.is_finite()
            && length > 0.0
            && surface
                .is_closed(k, &dom, direction, Some(2.0 * tolerance), false)?
                .0
        {
            *slot = Some(length);
        }
    }
    Ok(periods)
}

/// The further shift that takes a curve starting at `start` (not yet shifted) onto
/// `last_end`, the shifted end of the curve before it, given the `shift` of the curves so
/// far: where the gap is whole periods in a closed direction up to [`SEAM_WRAP_RESIDUAL`]
/// (WR-491), a seam crossing. A gap along a singular side of the surface (a sphere's
/// pole, a cone's apex) is a surface singularity instead, whatever its length: no shift
/// (#1437).
fn seam_wrap(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    last_end: Point3,
    start: Point3,
    shift: Vec3,
    periods: [Option<f64>; 2],
) -> Result<Vec3> {
    let wrap = seam_wrap_of_gap(start + shift - last_end, periods);
    if (wrap.x == 0.0 && wrap.y == 0.0) || gap_is_singular(k, b, f, last_end - shift, start)? {
        return Ok(Vec3::new(0.0, 0.0, 0.0));
    }
    Ok(wrap)
}

/// Whether the UV line from `end` to `start` maps to one point on face `f`'s surface
/// within twice the face tolerance (the closedness test of [`seam_periods`]): sampled at
/// its quarter points, as on a sphere's pole or a cone's apex. Across a seam the line runs
/// round the whole surface instead (#1437, WR-491).
fn gap_is_singular(k: &Kernel, b: &Brep, f: FaceId, end: Point3, start: Point3) -> Result<bool> {
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| null("face without surface"))?;
    let tolerance = 2.0 * fp2::tolerance(b, f)?;
    let at = |t: f64| {
        let p = end + (start - end) * t;
        surface.evaluate_point(k, Point2::new(p.x, p.y))
    };
    let anchor = at(0.0)?;
    for t in [0.25, 0.5, 0.75, 1.0] {
        if (at(t)? - anchor).length() > tolerance {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The shift that takes a curve starting at `gap` from the end of the one before it onto
/// that end, where `gap` is whole periods in a closed direction up to
/// [`SEAM_WRAP_RESIDUAL`] (WR-491).
fn seam_wrap_of_gap(gap: Vec3, periods: [Option<f64>; 2]) -> Vec3 {
    let mut shift = Vec3::new(0.0, 0.0, 0.0);
    for (d, period, s) in [
        (gap.x, periods[0], &mut shift.x),
        (gap.y, periods[1], &mut shift.y),
    ] {
        if let Some(p) = period {
            let n = (d / p).round();
            if n != 0.0 && (d - n * p).abs() <= SEAM_WRAP_RESIDUAL * p {
                *s = -n * p;
            }
        }
    }
    shift
}

/// `curve` (2D) moved by `shift` in the parameter plane (WR-491).
fn shift_curve(k: &Kernel, curve: &mut Curve, shift: Vec3) -> Result<()> {
    if shift.x == 0.0 && shift.y == 0.0 {
        return Ok(());
    }
    let x = Vec3::new(1.0, 0.0, 0.0);
    let y = Vec3::new(0.0, 1.0, 0.0);
    curve.transform(
        k,
        &Axis2Placement::new(Point3::new(shift.x, shift.y, 0.0), x, y),
        None,
    )?;
    curve.convert_to_2d(k)
}

/// `SmLine::CreateLineSegment(ctx, 2, start, end, pLine)`.
fn gap_line(start: Point3, end: Point3) -> Result<Curve> {
    Line::create_line_segment(2, start, end, None)
        .map(Curve::from)
        .map_err(|_| null("CreateLineSegment failed (upstream adds an uninitialised curve)"))
}

/// `SM_CAST_PTR(SmBSplineCurve, pEdge->GetCurve())`, `NER` on null.
fn edge_bspline_curve(b: &Brep, e: EdgeId) -> Result<&Curve> {
    match b.edge(e)?.curve.as_ref() {
        Some(c) if c.is_kind_of(BSPLINE_CURVE_TYPE) => Ok(c),
        _ => Err(SolidError::null_pointer(OP)),
    }
}

/// A copy of edge `e`'s B-spline curve trimmed to the edge interval.
fn trimmed_edge_curve(k: &Kernel, b: &Brep, e: EdgeId) -> Result<Curve> {
    let orig = edge_bspline_curve(b, e)?;
    let mut bsc = orig.copy(k)?;
    let mut edge_ivl = b.edge(e)?.interval;
    bsc.trim(k, &mut edge_ivl, TrimMode::Notify)?;
    Ok(bsc)
}

// ---- rectangular trim ---------------------------------------------------------------------

/// The outputs of [`find_if_rectangular_trim`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectangularTrim {
    /// `rUVDomain`: the rectangle of the four iso UV trim curves (clamped to the natural
    /// domain); the "inside out" [`Extent2d::new`] when not rectangular.
    pub uv_domain: Extent2d,
    /// `rbRectangularTrim`: one outer loop of four iso UV trim curves.
    pub rectangular: bool,
    /// `rbNaturalTrim`: that rectangle is the surface's natural domain.
    pub natural: bool,
}

/// `FindIfRectangularTrim`: whether the face is bounded by one loop of four iso UV
/// trim curves (after filling pole gaps), and that rectangle.
///
/// # Errors
///
/// `SM_ERR` when the trim boundary cannot be made (its error is not passed on);
/// [`SolidError::InvalidInput`] for a face without surface; those of
/// [`fp2::loops`], [`lu1::loop_edges`] and the bounding boxes;
/// [`SolidError::Unported`] from [`create_oriented_trim_boundary`].
pub fn find_if_rectangular_trim(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<RectangularTrim> {
    // init return value
    let mut out = RectangularTrim {
        uv_domain: Extent2d::new(),
        rectangular: false,
        natural: false,
    };

    // naturally trimmed faces may only have 1 loop
    let face_loops = fp2::loops(b, f, false, None)?;
    let [l] = face_loops.as_slice() else {
        return Ok(out);
    };

    // naturally trimmed face outer loops with no poles have 4 edges, with poles 3 or 2.
    let n_edges = lu1::loop_edges(b, *l)?.len();
    if !(2..=4).contains(&n_edges) {
        return Ok(out);
    }

    let boundary = match create_oriented_trim_boundary(k, b, f, true, true, None, key_of) {
        Ok(r) => r,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => return Err(SolidError::sm_err(OP)),
    };

    // No work - more than 1 loop or more than 4 edges.
    if boundary.loops.len() != 1 || boundary.curves.len() != 4 {
        return Ok(out);
    }

    // for every UVTrimCurve - building a union of each UVTrimCurve's bounding box
    let mut uv_bbox = crate::geom::extent3d::Extent3d::new();
    for (ii, crv) in boundary.curves.iter().enumerate() {
        let crv_bbox = uv_curve_box(k, crv)?;
        uv_bbox = if ii == 0 {
            crv_bbox
        } else {
            uv_bbox.union(&crv_bbox)
        };

        // When any edge is not an iso-parameter line - quit.
        let size = crv_bbox.size();
        if size.x > EFF_ZERO && size.y > EFF_ZERO {
            return Ok(out);
        }
    }

    let min = Point2::new(uv_bbox.min().x, uv_bbox.min().y);
    let max = Point2::new(uv_bbox.max().x, uv_bbox.max().y);
    let nat_domain = surface_deref(b, f)?.natural_uv_domain_with_nurb(k)?;

    // set output (SetMinMax's status ignored)
    let _ = out
        .uv_domain
        .set_min_max(nat_domain.clamp_point(min), nat_domain.clamp_point(max));
    out.rectangular = true;
    out.natural = out.uv_domain.max_boundary_dist(&nat_domain) < EFF_ZERO_PARAM;
    Ok(out)
}

// ---- orientation, surface and loop edits --------------------------------------------------

/// `FlipFaceOrientation`: makes each loop's other loop use primary and the face's other
/// face use upward (the face becomes invalid if it was valid, and vice versa).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without face use, mate, or loop uses (`NER`); those of
/// [`fp2::loops`]; [`SolidError::Unported`] (`SmFace::Notify`).
pub fn flip_face_orientation(b: &mut Brep, f: FaceId) -> Result<()> {
    face_notify(b, f, NotifyOperation::PreEdit)?;
    let top = fp2::upward_faceuse(b, f)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let bot = eu1::faceuse_mate(b, top)?.ok_or_else(|| SolidError::null_pointer(OP))?;

    // Flip ordering of loopuses within the loops
    for l in fp2::loops(b, f, false, None)? {
        let lu1_ = b
            .loop_(l)?
            .loopuse
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let lu2_ = b
            .loopuse(lu1_)?
            .mate
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        b.loop_mut(l)?.loopuse = Some(lu2_);
    }

    // Flip ordering of faceuses within the face and their orientation.
    b.face_mut(f)?.faceuse = Some(bot);
    b.faceuse_mut(top)?.orientation = OrientType::Opposite;
    b.faceuse_mut(bot)?.orientation = OrientType::Same;

    face_notify(b, f, NotifyOperation::PostEdit)?;
    Ok(())
}

/// `SetSurface`: gives the face `surface` (owned by the face, `this`) and its natural
/// domain (`GetNaturalUVDomain`, which makes a lazy analytic's missing NURB). The
/// previous surface is dropped with `delete_current`, else returned.
/// The `SM_NO_CHANGE_GEOMETRY` notification runs with the empty notification
/// environment of a B-rep face and null data (see [`set_surface_notified`] for a
/// caller's environment and the surfaces' identities).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of the `SM_NO_CHANGE_GEOMETRY`
/// notification ([`fp3::notify`]); [`SolidError::Unported`] from making the NURB.
pub fn set_surface(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    this: ObjectRef,
    surface: Surface,
    delete_current: bool,
) -> Result<Option<Surface>> {
    // inform the public
    face_notify_change_geometry(b, f)?;
    store_surface(k, b, f, this, surface, delete_current)
}

/// [`set_surface`] notified through the caller's `env`: `SM_NO_CHANGE_GEOMETRY` carries
/// `surface_refs` (the new and the old surface's identities, `None` for a surface
/// without one) as its first and third data arguments; the second
/// (`SM_NO_GET_BREP(this)`) is null, as upstream's macro.
///
/// # Errors
///
/// As [`set_surface`].
pub fn set_surface_notified(
    k: &Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    f: FaceId,
    this: ObjectRef,
    surface: Surface,
    delete_current: bool,
    surface_refs: [Option<ObjectRef>; 2],
) -> Result<Option<Surface>> {
    let [new_ref, old_ref] = surface_refs;
    // inform the public
    fp3::notify(
        b,
        f,
        env,
        None,
        NotifyOperation::ChangeGeometry,
        [new_ref, None, old_ref],
    )?;
    store_surface(k, b, f, this, surface, delete_current)
}

/// `SetSurface` after its notification.
fn store_surface(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    this: ObjectRef,
    mut surface: Surface,
    delete_current: bool,
) -> Result<Option<Surface>> {
    // (the domain is read first, so that a failing make leaves the face as it was)
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;

    // when asked - delete current surface
    let face = b.face_mut(f)?;
    let mut old = face.surface.take();
    if delete_current {
        old = None;
    }

    // update the Face domain, change the surface owner, save the surface
    face.uv_domain = uv_domain;
    surface.set_owner(Some(this));
    face.surface = Some(surface);
    b.geometry_owner_keys.insert(EntityRef::Face(f), this);
    // the surface reads the face's tolerance through GetFace()
    b.sync_face_surface_tolerance(f)?;
    Ok(old)
}

/// `SmFaceuse::PostInsert(lu)`: appends `lu` to `fu`'s loop uses.
fn faceuse_post_insert(b: &mut Brep, fu: FaceuseId, lu: LoopuseId) -> Result<()> {
    b.loopuse(lu)?;
    b.faceuse_mut(fu)?.loopuses.push(lu);
    b.loopuse_mut(lu)?.faceuse = Some(fu);
    Ok(())
}

/// `SmFaceuse::Remove(lu)`: unlinks `lu` from `fu`'s loop uses (`SM_ERR` when it is
/// not there).
fn faceuse_remove(b: &mut Brep, fu: FaceuseId, lu: LoopuseId) -> Result<()> {
    if b.loopuse(lu)?.faceuse != Some(fu) {
        return Err(SolidError::sm_err(OP));
    }
    let list = &mut b.faceuse_mut(fu)?.loopuses;
    let Some(at) = list.iter().position(|&x| x == lu) else {
        return Err(SolidError::sm_err(OP));
    };
    list.remove(at);
    b.loopuse_mut(lu)?.faceuse = None;
    Ok(())
}

/// `InsertLoop`: appends the loop's two loop uses to the face's two face uses (so an
/// outer loop can only be inserted into a face without loops).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without both loop uses or a face without
/// both face uses (upstream dereferences null); [`SolidError::Unported`]
/// (`SmFace::Notify`).
pub fn insert_loop(b: &mut Brep, f: FaceId, l: LoopId) -> Result<()> {
    face_notify(b, f, NotifyOperation::PreEdit)?;
    let (lu_1, lu_2) = lu1::loop_loopuses(b, l)?;
    let lu_2 = lu_2.ok_or_else(|| null("loop without second loop use"))?;
    let (fu_1, fu_2) = fp2::faceuses(b, f)?;
    let fu_1 = fu_1.ok_or_else(|| null("face without face use"))?;
    let fu_2 = fu_2.ok_or_else(|| null("face use without mate"))?;
    faceuse_post_insert(b, fu_1, lu_1)?;
    faceuse_post_insert(b, fu_2, lu_2)?;
    face_notify(b, f, NotifyOperation::PostEdit)?;
    Ok(())
}

/// `RemoveLoop`: unlinks the loop's two loop uses from their face uses; nothing is
/// deleted.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a loop use without face use (`NER`); `SM_ERR` from the
/// list removal; [`SolidError::InvalidInput`] for a loop without both loop uses;
/// [`SolidError::Unported`] (`SmFace::Notify`).
pub fn remove_loop(b: &mut Brep, f: FaceId, l: LoopId) -> Result<()> {
    face_notify(b, f, NotifyOperation::PreEdit)?;
    let (lu_1, lu_2) = lu1::loop_loopuses(b, l)?;
    let lu_2 = lu_2.ok_or_else(|| null("loop without second loop use"))?;
    let fu_1 = lu1::faceuse(b, lu_1)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let fu_2 = lu1::faceuse(b, lu_2)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    faceuse_remove(b, fu_1, lu_1)?;
    faceuse_remove(b, fu_2, lu_2)?;
    face_notify(b, f, NotifyOperation::PostEdit)?;
    Ok(())
}

/// `RemoveUVTrimCurves`: deletes the UV trim curves of the upward face use's edge uses
/// (which also resets their cached gaps).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without face use; those of [`eu1::faceuse_edgeuses`];
/// [`SolidError::Unported`] (`SmFace::Notify`).
pub fn remove_uv_trim_curves(b: &mut Brep, f: FaceId) -> Result<()> {
    remove_uv_trim_curves_notifying(b, f, &mut |b, op| face_notify(b, f, op))
}

/// Remove UV trims with the actual face notification records, attributes and context.
/// Geometry notifications inside lower trim ports retain their existing behavior.
///
/// # Errors
/// Those of [`remove_uv_trim_curves`] and the real face notification.
pub fn remove_uv_trim_curves_with_notifications(
    b: &mut Brep,
    f: FaceId,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<()> {
    remove_uv_trim_curves_notifying(b, f, &mut |b, op| {
        let this = (env.key_of)(EntityRef::Face(f));
        fp3::notify(b, f, env, None, op, [Some(this), None, None])
    })
}

fn remove_uv_trim_curves_notifying(
    b: &mut Brep,
    f: FaceId,
    notify: &mut impl FnMut(&mut Brep, NotifyOperation) -> Result<()>,
) -> Result<()> {
    notify(b, NotifyOperation::PreEdit)?;
    let fu = fp2::upward_faceuse(b, f)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    for eu in eu1::faceuse_edgeuses(b, fu, false, None)? {
        // status ignored upstream
        ignore_status(eu1::set_uv_trim_curve(
            b,
            eu,
            eu1::UvTrimCurveArg::Null,
            0.0,
            true,
            None,
        ))?;
    }
    Ok(())
}

/// `SwapUV`: transposes the face's UV domain, mirrors every UV trim curve about the
/// `u = v` line, swaps the surface's `u` and `v` and makes the other face use and loop
/// uses primary (reversing the face). The mirrors are made before anything changes, so a
/// failing mirror leaves the face unchanged.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without B-rep, surface or face use, a loop use
/// without loop or mate, an edge use without mate; those of the mirror and surface
/// swap and of the edit notifications.
pub fn swap_uv(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    swap_uv_notifying(k, b, f, &mut Unnotified)
}

/// [`swap_uv`] with its `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` (`this`,
/// `SM_NO_GET_BREP(this)`, `NULL`) sent through the caller's notification environment
/// (`env.key_of` names the face; `SM_NO_GET_BREP` is a null pointer upstream, so no
/// B-rep; the context's callbacks see them, as upstream's B-rep context's do), and so
/// are the surface's own (`SetCanonical`'s, `Surface::swap_uv_notifying`): `this` and
/// data 1 the edited surface under its identity from `env.context` (the face's
/// surface reported owned by the face first, the `GetOwner()` a callback reads),
/// data 2 `SM_NO_GET_OWNER` (null). Without context, a surface that has an identity
/// notifies its attributes only; one without has nothing to notify.
///
/// # Errors
///
/// Those of [`swap_uv`] and of the identity issue (an exhausted domain).
pub fn swap_uv_with_notifications(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<()> {
    let face = (env.key_of)(EntityRef::Face(f));
    // the face owns its surface (the link a callback's owner walk reads)
    if let Some(ctx) = env.context.as_deref_mut() {
        if let Some(s) = b.face_mut(f)?.surface.as_mut() {
            let key = ctx.geometry_key(s.notify_identity_slot())?;
            crate::geom::object::report_owner_link(Some(ctx), key, Some(face));
        }
    }
    swap_uv_notifying(k, b, f, &mut EnvNotified { env, face })
}

/// Where [`swap_uv_notifying`] sends the face's and the surface's edit notifications.
trait SwapUvNotify {
    /// The face's `Notify(op, this, SM_NO_GET_BREP(this), NULL)`.
    fn face(&mut self, b: &mut Brep, f: FaceId, op: NotifyOperation) -> Result<()>;
    /// An edited surface's `SmAObject::Notify` ([`Surface::swap_uv_notifying`]).
    fn surface(
        &mut self,
        slot: &mut IdentitySlot,
        type_code: i64,
        op: NotifyOperation,
    ) -> Result<()>;
}

/// [`swap_uv`]'s notifications: the face's under the empty environment
/// ([`face_notify`]), the surface's to nobody (no context or attribute store).
struct Unnotified;

impl SwapUvNotify for Unnotified {
    fn face(&mut self, b: &mut Brep, f: FaceId, op: NotifyOperation) -> Result<()> {
        face_notify(b, f, op)
    }

    fn surface(&mut self, _: &mut IdentitySlot, _: i64, _: NotifyOperation) -> Result<()> {
        Ok(())
    }
}

/// [`swap_uv_with_notifications`]' notifications, through `env`.
struct EnvNotified<'e, 'a> {
    env: &'e mut crate::topology::edge::NotifyEnv<'a>,
    face: ObjectRef,
}

impl SwapUvNotify for EnvNotified<'_, '_> {
    fn face(&mut self, b: &mut Brep, f: FaceId, op: NotifyOperation) -> Result<()> {
        // SM_NO_GET_BREP(this) is a null pointer upstream (SmTypes.h)
        fp3::notify(b, f, self.env, None, op, [Some(self.face), None, None])
    }

    fn surface(
        &mut self,
        slot: &mut IdentitySlot,
        type_code: i64,
        op: NotifyOperation,
    ) -> Result<()> {
        let env = &mut *self.env;
        let key = match env.context.as_deref_mut() {
            Some(ctx) => ctx.geometry_key(slot)?,
            None => match slot.value() {
                Some(v) => ObjectKey(v),
                None => return Ok(()),
            },
        };
        let this = ObjectRef { type_code, key };
        // SM_NO_GET_OWNER(this) is a null pointer upstream (SmTypes.h)
        crate::geom::a_object::notify(
            env.attributes,
            env.context.as_deref_mut(),
            op,
            this,
            Some(this),
            None,
            None,
        )
    }
}

/// [`swap_uv`]'s body, `notify` sending the face's and the surface's edit
/// notifications.
fn swap_uv_notifying(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    notify: &mut dyn SwapUvNotify,
) -> Result<()> {
    // GetBrep()->GetContext()
    if !has_brep(b, f)? {
        return Err(null(
            "SwapUV: face without B-rep (upstream dereferences null)",
        ));
    }
    surface_deref(b, f)?;
    let x_axis = Vec3::new(1.0, 1.0, 0.0).unitize()?;
    let mut mirror = Axis2Placement::default();
    // x:[1 1 0] y:[0 0 1] z=cross(x,y):[1 -1 0] (status ignored)
    let _ = mirror.set_canonical(Point3::new(0.0, 0.0, 0.0), x_axis, Vec3::new(0.0, 0.0, 1.0));

    // Mirror the UV trims and swap the surface (the steps that can fail) before changing
    // the face, so that a failure leaves the face unchanged.

    let upward = upward_loopuses(b, f)?;

    let mut to_set: Vec<(EdgeuseId, Curve)> = Vec::new();
    for &lu in &upward {
        // for every Edgeuse mated pair - mirror the UVTrimCurve
        for eu in eu1::loopuse_edgeuses(b, lu)? {
            if let Some(uv) = eu1::uv_trim_curve(b, eu)?.0 {
                // (upstream dereferences the mate only for an edge use with a curve)
                let eu_mate = eu1::mate(b, eu)?.ok_or_else(|| null("edge use without mate"))?;
                let mirrored = match uv.kind() {
                    CurveKind::BSpline(c) => c.create_mirror_curve(&mirror)?,
                    // (MakeNurb's status is ignored upstream: mirror the base as it is)
                    CurveKind::Line(c) => match c.base_with_nurb() {
                        Ok(base) => base.create_mirror_curve(&mirror)?,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => c.base().create_mirror_curve(&mirror)?,
                    },
                    // the conics: MakeNurb (their override) when missing, then the
                    // SmBSplineCurve mirror of that NURB (statuses ignored as upstream)
                    CurveKind::Ellipse(c) => {
                        let mut c = c.clone();
                        if c.base().gw_nurb().is_none() {
                            ignore_status(c.make_nurb(k))?;
                        }
                        c.base().create_mirror_curve(&mirror)?
                    }
                    CurveKind::Parabola(c) => {
                        let mut c = c.clone();
                        if c.base().gw_nurb().is_none() {
                            ignore_status(c.make_nurb(None))?;
                        }
                        c.base().create_mirror_curve(&mirror)?
                    }
                    CurveKind::Hyperbola(c) => {
                        let mut c = c.clone();
                        if c.base().gw_nurb().is_none() {
                            ignore_status(c.make_nurb(None))?;
                        }
                        c.base().create_mirror_curve(&mirror)?
                    }
                    // A UV trim curve is an SmBSplineCurve*: upstream's only subclasses are
                    // SmLine, SmEllipse, SmHyperbola and SmParabola (the arms above);
                    // SmHermiteCurve, SmIsoCurve, SmOffsetCurve, SmProjectedCurve,
                    // SmCompositeCurve, SmConic, SmCrvOnSurf and SmCrvInVolume derive from
                    // SmCurve and cannot be one.
                    _ => return Err(null("UV trim curve is not a B-spline curve")),
                };
                to_set.push((eu_mate, Curve::from(mirrored)));
            }
        }
    }

    // prepare to Edit
    notify.face(b, f, NotifyOperation::PreEdit)?;

    // swap theSurface UV orientation
    b.face_mut(f)?
        .surface
        .as_mut()
        .ok_or_else(|| null("face without surface"))?
        .swap_uv_notifying(k, &mut |slot, type_code, op| {
            notify.surface(slot, type_code, op)
        })?;

    // Transpose the Face->UVDomain
    let d = fp2::uv_domain(b, f)?;
    let transpose = Extent2d::from_points(
        Point2::new(d.min().y, d.min().x),
        Point2::new(d.max().y, d.max().x),
    );
    fp2::set_uv_domain(b, f, &transpose)?;

    // Swap primary and secondary loopuses
    for &lu in &upward {
        let l = lu1::loop_(b, lu)?.ok_or_else(|| null("loop use without loop"))?;
        let mate = lu1::other_loopuse(b, lu)?;
        b.loop_mut(l)?.loopuse = mate;
    }

    for (eu, c) in to_set {
        // status ignored upstream
        ignore_status(eu1::set_uv_trim_curve(
            b,
            eu,
            eu1::UvTrimCurveArg::New(Box::new(c)),
            0.0,
            true,
            None,
        ))?;
    }

    // Swap the Face->Faceuses swapping the upper and lower faceuses with one another
    let fu = fp2::upward_faceuse(b, f)?.ok_or_else(|| null("face without face use"))?;
    b.faceuse_mut(fu)?.orientation = OrientType::Opposite;
    let fu_mate = eu1::faceuse_mate(b, fu)?.ok_or_else(|| null("face use without mate"))?;
    b.faceuse_mut(fu_mate)?.orientation = OrientType::Same;
    b.face_mut(f)?.faceuse = Some(fu_mate);

    notify.face(b, f, NotifyOperation::PostEdit)?;
    Ok(())
}

// ---- shrink -------------------------------------------------------------------------------

/// `ShrinkGeometry`: shrinks the face's UV domain to its UV trim curves (snapping to the
/// surface domain within a thousandth of the new domain's diagonal), trims the surface
/// to it, removes the UV trim curves and trims each edge curve owned by an edge to that
/// edge's interval. Nothing is done for a face without surface or whose surface has no
/// owner. `key_of` names the edges and edge uses a curve's owner chain may pass.
///
/// # Errors
///
/// Those of [`calculate_uv_domain_from_uv_trim_curves`] and [`Curve::edge`];
/// [`SolidError::Unported`] (`GetEdges`, `SmFace::Notify`).
pub fn shrink_geometry(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    shrink_geometry_with(k, b, f, key_of, TrimOptions::default())
}

/// [`shrink_geometry`] with the [`TrimOptions`] of its surface trim (not upstream,
/// source issue #472). The fillet executive's shrink of the new fillet faces
/// passes [`TrimOptions::snap_roundoff_to_knots`], so that a face domain one rounding
/// error inside a knot does not leave a sliver span on a round. The default options
/// are upstream's `ShrinkGeometry`.
///
/// # Errors
///
/// Those of [`shrink_geometry`].
pub fn shrink_geometry_with(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    trim: TrimOptions,
) -> Result<()> {
    shrink_geometry_notifying(k, b, f, key_of, trim, &mut |b, op| face_notify(b, f, op))
}

/// Shrink with default native trim options and the caller's actual face notifications.
/// The lower surface/curve ports keep their existing geometry notification behavior.
///
/// # Errors
/// Those of [`shrink_geometry`] and the real face notification.
pub fn shrink_geometry_with_notifications(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<()> {
    let key_of = env.key_of;
    shrink_geometry_notifying(k, b, f, key_of, TrimOptions::default(), &mut |b, op| {
        let this = (env.key_of)(EntityRef::Face(f));
        fp3::notify(b, f, env, None, op, [Some(this), None, None])
    })
}

fn shrink_geometry_notifying(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    trim: TrimOptions,
    notify: &mut impl FnMut(&mut Brep, NotifyOperation) -> Result<()>,
) -> Result<()> {
    // no work - don't work on faces when in an intermediate state
    match fp2::surface(b, f)? {
        Some(s) if s.owner().is_some() => {}
        _ => return Ok(()),
    }

    // compute face domain from boundary curves
    let face_domain = calculate_uv_domain_from_uv_trim_curves(k, b, f, true, false, false)?.domain;
    let tol = face_domain.size().length() / 1000.0;

    // get surface's current natural UVDomain
    let surf_domain = surface_deref(b, f)?.natural_uv_domain_with_nurb(k)?;

    // If the face domain is dTol close within surface domain, choose the surface domain
    let mut min = face_domain.min();
    let mut max = face_domain.max();
    if min.x - surf_domain.min().x < tol {
        min.x = surf_domain.min().x;
    }
    if min.y - surf_domain.min().y < tol {
        min.y = surf_domain.min().y;
    }
    if surf_domain.max().x - max.x < tol {
        max.x = surf_domain.max().x;
    }
    if surf_domain.max().y - max.y < tol {
        max.y = surf_domain.max().y;
    }

    // set the (revised) face domain
    let mut new_domain = Extent2d::from_points(min, max);
    fp2::set_uv_domain(b, f, &new_domain)?;

    // Trim the surface (status ignored)
    if let Some(s) = b.face_mut(f)?.surface.as_mut() {
        ignore_status(s.trim_with_domain_with(k, &mut new_domain, trim))?;
    }

    // remove all associated UVTrimCurves (status ignored)
    ignore_status(remove_uv_trim_curves_notifying(b, f, notify))?;

    // Now Trim all Curves to the face edges
    for e in face_edges(b, f)? {
        let Some(curve) = b.edge(e)?.curve.as_ref() else {
            return Err(null("edge without curve"));
        };
        let owner_of = |o: ObjectRef| -> Option<ObjectRef> {
            b.edgeuses()
                .ids()
                .find(|&eu| key_of(EntityRef::Edgeuse(eu)) == o)
                .and_then(|eu| b.edgeuse(eu).ok()?.edge)
                .map(|e| key_of(EntityRef::Edge(e)))
        };

        // skip curves without owners
        let Some(owner) = curve.edge(owner_of)? else {
            continue;
        };
        let Some(owner_edge) = b
            .edges()
            .ids()
            .find(|&x| key_of(EntityRef::Edge(x)) == owner)
        else {
            continue;
        };
        let mut ivl = b.edge(owner_edge)?.interval;

        // trim the curve to the used interval - status ignored
        if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
            ignore_status(c.trim(k, &mut ivl, TrimMode::Notify))?;
        }
    }
    Ok(())
}

// ---- seams --------------------------------------------------------------------------------

/// Faces, edges and vertices a seam fix made (`pOptNewFaces`, `pOptNewSeamEdges`,
/// `pOptSeamVertices`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeamSplit {
    /// New faces (not including the input face).
    pub faces: Vec<FaceId>,
    /// New seam edges.
    pub edges: Vec<EdgeId>,
    /// New vertices on the seam.
    pub vertices: Vec<VertexId>,
}

/// The flags of [`move_seam`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveSeamOptions {
    /// `bOkToMoveToCenter`: the seam may move into the face (to avoid sliver faces) when
    /// it cannot move out of it.
    pub ok_to_move_to_center: bool,
    /// `bOkToSplit`: the face may be split at the seam when it cannot move out of it.
    pub ok_to_split: bool,
}

/// The outputs of [`move_seam`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveSeamResult {
    /// `eMoveSeamResult_U`.
    pub u: FixSeamPlanType,
    /// `eMoveSeamResult_V`.
    pub v: FixSeamPlanType,
    /// What the split made (with `ok_to_split`).
    pub split: SeamSplit,
}

/// Whether a seam direction value names `dir` (`== dir || == SM_SP_BOTH`).
fn names(p: SurfParamType, dir: SurfParamType) -> bool {
    p == dir || p == SurfParamType::Both
}

/// Whether a classification has a seam problem in the face (`size < 1` or a single
/// interval whose middle is not on the face means no problem).
fn class_is_quiet<O: LoopObject>(c: &CurveClassification<O>) -> bool {
    match c.intervals() {
        [] => true,
        [one] => one.mid.point_class() != PointClassType::Face,
        _ => false,
    }
}

/// The UV drop of curve point `param` of `curve` on `surface`
/// (`EvaluateGeometric` then `DropPoint` and `DropVectors`, statuses ignored).
fn sample_uv(
    k: &Kernel,
    curve: &Curve,
    surface: &Surface,
    uv_domain: &Extent2d,
    param: f64,
    pv: &mut [Vec3; 2],
) -> Result<Point2> {
    if let Some(v) = ignored(evaluate_geometric(k, curve, param, 1, true))? {
        if let [p, d, ..] = v.as_slice() {
            *pv = [*p, *d];
        }
    }
    let mut uv = Vec2::UNDEF;
    if let Some(r) =
        ignored(surface.drop_point::<NoObject>(k, pv[0], uv_domain, &DropPointOptions::default()))?
    {
        uv = r.result_uv;
    }
    // DropVectors of the tangent (its output is never read)
    ignore_status(drop_vectors_at(
        k,
        surface,
        uv,
        Side::Left,
        Side::Left,
        &pv[1..],
    ))?;
    Ok(uv)
}

/// The UV point of vertex use `vu`: its cached vertex/face gap's face UV, else
/// `ComputeUVPoint(.., FALSE)` (status ignored, keeping `prev`).
fn vertexuse_uv(k: &Kernel, b: &mut Brep, vu: VertexuseId, prev: Point2) -> Result<Point2> {
    if let Some(Some(gap)) = ignored(vu1::vertex_face_gap(k, b, vu, false))? {
        return Ok(gap.face_uv);
    }
    Ok(ignored(vu1::compute_uv_point(k, b, vu, false))?.unwrap_or(prev))
}

/// `MoveSeam`: on a periodic B-spline surface whose seam the face's edges cross (or
/// nearly touch), moves the seam (rebuilding the surface split at a new parameter and
/// joined the other way round) out of the face, or into it when the face spans the
/// whole period (with `ok_to_move_to_center` or `ok_to_split`); with `ok_to_split` a
/// face that still has seam problems is then split at the seam ([`split_at_seam`]).
/// `crv_class_u` / `crv_class_v` and `problem` come from `HasSeamProblem` (and may be
/// rebuilt); `props` replaces the classifications' verdicts when given. `this` is the
/// face's identity (the owner of a new surface), `resolve` maps a classification
/// object to a vertex (for the split).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for a surface that is not a B-spline kind; `SM_ERR` when the
/// surface cannot be split; [`SolidError::InvalidInput`] as in the module docs; those
/// of the surface join, the knot query and the classification state;
/// [`SolidError::Unported`] (`GetUpwardLoopuses`, `IsCrossedByEdge`, `HasNearMisses`,
/// `GetUpwardVertexuses`, `GetBrep`, `SmFace::Notify` (also through `ReplaceSurface`),
/// `HasSeamProblem`, the split).
#[allow(clippy::too_many_arguments)] // one upstream routine
pub fn move_seam<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    this: ObjectRef,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    problem: SeamProblem,
    opts: MoveSeamOptions,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
) -> Result<MoveSeamResult> {
    move_seam_notified(
        k,
        b,
        None,
        f,
        this,
        crv_class_u,
        crv_class_v,
        problem,
        opts,
        props,
        resolve,
    )
}

/// [`move_seam`] with an optional notification environment `env` for its surface
/// replacement (`ReplaceSurface` / `SetSurface`), so callers such as
/// `SmHealData::Fix_MoveSeam` receive the face's `SM_NO_POST_EDIT` in their topology
/// change tracker while `this` stays the surface's owner identity.
///
/// # Errors
///
/// As [`move_seam`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream routine
pub fn move_seam_notified<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    mut env: Option<&mut crate::topology::edge::NotifyEnv<'_>>,
    f: FaceId,
    this: ObjectRef,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    problem: SeamProblem,
    opts: MoveSeamOptions,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
) -> Result<MoveSeamResult> {
    use SurfParamType as Sp;
    // (the seam problem is passed by value; HasSeamProblem updates this copy)
    let mut problem = problem;
    let mut out = MoveSeamResult {
        u: FixSeamPlanType::NoChanges,
        v: FixSeamPlanType::NoChanges,
        split: SeamSplit::default(),
    };

    // no work - no seam problems
    let quiet = match props {
        Some(p) => {
            matches!(p.bad_crossed_seam_dir, Sp::Neither | Sp::Unknown)
                && matches!(p.bad_missing_seam_dir, Sp::Neither | Sp::Unknown)
        }
        None => class_is_quiet(crv_class_u) && class_is_quiet(crv_class_v),
    };
    if quiet {
        return Ok(out);
    }

    // locals
    let lsmp_cnt: usize = 6;
    // (upstream reads the surface's domain before its null check)
    let surface0 = surface_deref(b, f)?;
    let uv_domain = surface0.natural_uv_domain_with_nurb(k)?;
    let ivl_u = uv_domain.u_interval();
    let ivl_v = uv_domain.v_interval();
    let mut periodic_u = PeriodicExtent1d::full(ivl_u.length());
    let _ = periodic_u.set_min_max(0.0, 0.0, None, true);
    let mut periodic_v = PeriodicExtent1d::full(ivl_v.length());
    let _ = periodic_v.set_min_max(0.0, 0.0, None, true);

    // check State - SmBSplineSurfaces only
    if !surface0.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
        return Err(SolidError::invalid_input(
            OP,
            "MoveSeam: only SmBSplineSurface kinds are supported",
        ));
    }

    let loopuses = upward_loopuses(b, f)?;
    if loopuses.is_empty() {
        return Ok(out);
    }

    let surface_is_analytic = surface0.is_analytic()?;
    let zone_tol = crate::surface::surface::p1::zone_tol_3d(k, surface0)?;
    let mut surface = surface0.clone();
    *surface.owner_mut() = surface0.owner();
    // the copy reaches the same face, so it carries the face tolerance cached on the
    // original (a B-spline kind: no owned bases)
    surface.set_owner_zone_tol_3d(surface0.owner_zone_tol_3d());

    // (the classifications name the face's own B-rep objects)
    let (crosses_u, crosses_v, near_miss_u, near_miss_v) = if let Some(p) = props {
        (
            names(p.bad_crossed_seam_dir, Sp::U),
            names(p.bad_crossed_seam_dir, Sp::V),
            names(p.bad_near_miss_seam_dir, Sp::U),
            names(p.bad_near_miss_seam_dir, Sp::V),
        )
    } else {
        let objects = BrepSeamObjects::new(k, &*b, f)?;
        (
            is_crossed_by_edge(&objects, crv_class_u)?,
            is_crossed_by_edge(&objects, crv_class_v)?,
            has_near_misses(k, crv_class_u, &objects)?,
            has_near_misses(k, crv_class_v, &objects)?,
        )
    };
    let fix_seam_both = (crosses_u || near_miss_u) && (crosses_v || near_miss_v);

    for ii in 0..2usize {
        let (ivl, surf_param, crosses_seam, near_miss_seam) = if ii == 0 {
            (ivl_u, Sp::U, crosses_u, near_miss_u)
        } else {
            (ivl_v, Sp::V, crosses_v, near_miss_v)
        };
        let ivl_length = ivl.length();
        let mut ok_to_snap = true;

        if !(crosses_seam || near_miss_seam) {
            continue;
        }

        // skip the walk for V when both are fixed (the U walk fills both)
        if !fix_seam_both || ii == 0 {
            // sPV[0] (first sample) and sPV[1] (the walk)
            let mut pv0 = [Vec3::UNDEF; 2];
            let mut pv1 = [Vec3::UNDEF; 2];
            let first_lu = *loopuses.first().ok_or_else(|| null("no loop uses"))?;
            let first_eus = eu1::loopuse_edgeuses(b, first_lu)?;
            let eu0 = *first_eus
                .first()
                .ok_or_else(|| null("loop use without edge uses"))?;
            let e0 = edge_deref(b, eu0)?;
            let norm0 = if eu1::orientation(b, eu0)? == OrientType::Same {
                0.0
            } else {
                1.0
            };
            let mut uv_prev = {
                let edge = b.edge(e0)?;
                let curve = edge
                    .curve
                    .as_ref()
                    .ok_or_else(|| null("edge without curve"))?;
                let param = edge.interval.evaluate(norm0);
                sample_uv(k, curve, &surface, &uv_domain, param, &mut pv0)?
            };

            // init the periodic interval at the first sample
            {
                let periodic = if ii == 0 {
                    &mut periodic_u
                } else {
                    &mut periodic_v
                };
                let s = comp(uv_prev, ii) - ivl.min();
                let _ = periodic.set_min_max(s, s, None, true);
            }

            'loops: for (j0, &lu) in loopuses.iter().enumerate() {
                if periodic_full(&periodic_u, &periodic_v, ii) {
                    break;
                }
                let eus = eu1::loopuse_edgeuses(b, lu)?;
                for (j1, &eu) in eus.iter().enumerate() {
                    if periodic_full(&periodic_u, &periodic_v, ii) {
                        break 'loops;
                    }
                    let orient = eu1::orientation(b, eu)?;
                    let e = edge_deref(b, eu)?;
                    let start = usize::from(j0 + j1 == 0);
                    for kk in start..=lsmp_cnt {
                        let norm = if orient == OrientType::Same {
                            if kk == lsmp_cnt {
                                1.0
                            } else {
                                kk as f64 / lsmp_cnt as f64
                            }
                        } else if kk == 0 {
                            1.0
                        } else {
                            (lsmp_cnt - kk) as f64 / lsmp_cnt as f64
                        };
                        let uv1 = {
                            let edge = b.edge(e)?;
                            let curve = edge
                                .curve
                                .as_ref()
                                .ok_or_else(|| null("edge without curve"))?;
                            let param = edge.interval.evaluate(norm);
                            sample_uv(k, curve, &surface, &uv_domain, param, &mut pv1)?
                        };

                        // add the sampled span the short way round
                        let (lo, hi) = order(comp(uv_prev, ii), comp(uv1, ii));
                        let periodic = if ii == 0 {
                            &mut periodic_u
                        } else {
                            &mut periodic_v
                        };
                        let no_seam = hi - lo;
                        let with_seam = periodic.period() - no_seam;
                        let forward = no_seam < with_seam;
                        let _ = periodic.add_interval(lo - ivl.min(), hi - ivl.min(), forward);

                        if fix_seam_both && ii == 0 {
                            let (lo, hi) = order(uv_prev.y, uv1.y);
                            let no_seam = hi - lo;
                            // (upstream reads the U period and U minimum here)
                            let with_seam = periodic_u.period() - no_seam;
                            let forward = no_seam < with_seam;
                            let _ =
                                periodic_v.add_interval(lo - ivl.min(), hi - ivl.min(), forward);
                        }

                        uv_prev = uv1;
                    }
                }
            }
        }

        let periodic = if ii == 0 { periodic_u } else { periodic_v };
        if periodic.is_full_period() && !near_miss_seam {
            continue;
        }

        let mut new_param;
        if periodic.is_full_period() {
            // face uses all of the periodic surface: move to the vertex spacing
            let vus = upward_vertexuses(b, f)?;
            let mut params: Vec<f64> = Vec::new();
            let mut unique: Vec<f64> = Vec::new();
            let mut param_ivl = Extent1d::new();
            let mut pt_uv = uv_domain.mid();
            for &vu in &vus {
                pt_uv = vertexuse_uv(k, b, vu, pt_uv)?;
                let p = comp(pt_uv, ii);
                params.push(p);
                param_ivl.add_value(p);
                let dup = unique.iter().any(|&q| {
                    are_same_to_tol(p, q, EFF_ZERO_PARAM)
                        || are_same_to_tol(p, q + ivl_length, EFF_ZERO_PARAM)
                        || are_same_to_tol(p, q - ivl_length, EFF_ZERO_PARAM)
                });
                if !dup {
                    unique.push(p);
                }
            }
            let by_value = |a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(Ordering::Equal);
            params.sort_by(by_value);
            unique.sort_by(by_value);
            let _ = (params, param_ivl);

            let cnt = unique.len();
            let at = |i: usize| -> Result<f64> {
                unique
                    .get(i)
                    .copied()
                    .ok_or_else(|| null("MoveSeam: no vertex parameters"))
            };

            // the largest gap between parameters
            let mut max_span = (at(0)? + ivl_length) - at(cnt.wrapping_sub(1))?;
            let mut max_indx = cnt - 1;
            for jj in 0..cnt.saturating_sub(1) {
                let this_span = at(jj + 1)? - at(jj)?;
                if max_span < this_span {
                    max_span = this_span;
                    max_indx = jj;
                }
            }

            // the most spaced out parameter
            let span_of = |this: usize, next: usize| -> Result<f64> {
                Ok((at(next)? - at(this)?) + if next <= this { ivl_length } else { 0.0 })
            };
            let last_span0 = (at(0)? - at(cnt - 1)?) + ivl_length;
            let mut next_span = span_of(0, 1 % cnt)?;
            let mut max_spacing = smos_min(last_span0, next_span);
            let mut max_space_indx = 0usize;
            for jj in 1..cnt {
                let last_span = next_span;
                next_span = span_of(jj, (jj + 1) % cnt)?;
                let this_spacing = smos_min(last_span, next_span);
                if max_spacing < this_spacing {
                    max_spacing = this_spacing;
                    max_space_indx = jj;
                }
            }

            let mut use_max_point_spacing = false;
            if max_spacing > ivl_length / 10.0 {
                use_max_point_spacing = true;
                let target = at(max_space_indx)?;
                let mut paired: Vec<VertexuseId> = Vec::new();
                for &vu in &vus {
                    pt_uv = vertexuse_uv(k, b, vu, pt_uv)?;
                    if are_same_to_tol(target, comp(pt_uv, ii), EFF_ZERO_PARAM) {
                        paired.push(vu);
                    }
                }
                'pairs: for (jj, &vu_a) in paired.iter().enumerate() {
                    if jj + 1 >= paired.len() {
                        break;
                    }
                    let v1 = vu1::vertex(b, vu_a)?;
                    let v1 = v1.ok_or_else(|| null("vertex use without vertex"))?;
                    for &vu2 in paired.get(jj + 1..).unwrap_or_default() {
                        let v2 = vu1::vertex(b, vu2)?;
                        if !vx::common_edges(b, v1, v2, Some(f))?.is_empty() {
                            use_max_point_spacing = false;
                            break 'pairs;
                        }
                    }
                }
            }

            if use_max_point_spacing {
                new_param = at(max_space_indx)?;
                ok_to_snap = false;
            } else {
                let min_param = at(max_indx)?;
                let max_param = at((max_indx + 1) % cnt)?;
                new_param = if max_indx != cnt {
                    (max_param + min_param) / 2.0
                } else if (min_param - ivl.min()) > (ivl.max() - max_param) {
                    min_param - ((max_param - min_param + ivl_length) / 2.0)
                } else {
                    max_param + ((max_param - min_param + ivl_length) / 2.0)
                };
            }
        } else {
            // face does not use all of the periodic surface
            let mut unused = periodic;
            unused.complement();
            new_param = unused.mid() + ivl.min();
        }

        // snap to a knot that keeps away from the face
        if ok_to_snap {
            let far = periodic.period() / 10.0;
            let mut best: Option<usize> = None;
            let mut dist = BIG_DOUBLE;
            let mut this_ivl = periodic;
            this_ivl.set_full_period(false);
            let knots = ignored(surface.knots_with_nurb(k, surf_param, None))?
                .map(|(k, _)| k)
                .unwrap_or_default();
            let k0 = knots.first().copied().unwrap_or(0.0);
            for (jj, &knot) in knots.iter().enumerate() {
                let this_knot = knot - k0;
                let this_dist = (this_knot - new_param).abs();
                if this_dist < dist {
                    let lo = periodic.map_value(this_knot - ivl.min() - far);
                    let hi = periodic.map_value(this_knot - ivl.min() + far);
                    let _ = this_ivl.set_min_max(lo, hi, None, false);
                    if periodic.are_disjoint(&this_ivl) {
                        best = Some(jj);
                        dist = this_dist;
                    }
                }
            }
            if let Some(&knot) = best.and_then(|i| knots.get(i)) {
                new_param = knot;
            }
        }

        if ivl.classify_point(new_param, EFF_ZERO) == ExtentPointType::Inside
            && (!periodic.is_full_period() || opts.ok_to_move_to_center || opts.ok_to_split)
        {
            let (left, right) = surface.split_at(k, new_param, surf_param).map_err(|e| {
                if e.is_unported() {
                    e
                } else {
                    SolidError::sm_err(OP)
                }
            })?;
            let (SurfaceKind::BSpline(left), SurfaceKind::BSpline(right)) =
                (left.into_kind(), right.into_kind())
            else {
                return Err(null("MoveSeam: split halves are not SmBSplineSurfaces"));
            };
            let (mut left, mut right) = (left, right);
            let joined = right.join_surface(&mut left, surf_param, zone_tol)?;
            let mut joined = Surface::from(joined);
            if surface_is_analytic {
                if let SurfaceKind::BSpline(j) = joined.kind() {
                    if let Some(a) = ignored(j.copy_and_add_analytics(k))? {
                        if a.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
                            joined = a;
                        }
                    }
                }
            }

            if ii == 0 {
                out.u = if periodic.is_full_period() {
                    FixSeamPlanType::MoveInFace
                } else {
                    FixSeamPlanType::MoveOutOfFace
                };
            } else {
                out.v = if periodic.is_full_period() {
                    FixSeamPlanType::MoveInFace
                } else {
                    FixSeamPlanType::MoveOutOfFace
                };
            }

            // replace the face surface
            surface = joined.clone();
            if let Some(env) = env.as_deref_mut() {
                if has_brep(b, f)? {
                    brep_p2::replace_surface(
                        k,
                        b,
                        env,
                        f,
                        joined,
                        this,
                        OldSurface::Delete,
                        false,
                    )?;
                } else {
                    set_surface_notified(k, b, env, f, this, joined, true, [None, None])?;
                }
            } else if has_brep(b, f)? {
                replace_surface(k, b, f, this, joined)?;
            } else {
                set_surface(k, b, f, this, joined, true)?;
            }
        }
    }

    // split the face at its seams when they are still a problem
    if opts.ok_to_split
        && (periodic_u.is_full_period() || periodic_v.is_full_period())
        && ignored(has_seam_problem(
            k,
            b,
            f,
            crv_class_u,
            crv_class_v,
            &mut problem,
            false,
        ))?
        .unwrap_or(false)
    {
        // status ignored upstream
        if let Some(s) = ignored(split_at_seam_depth(
            k,
            b,
            env,
            f,
            crv_class_u,
            crv_class_v,
            problem,
            props,
            resolve,
            0,
        ))? {
            out.split = s;
        }
    }
    Ok(out)
}

/// `smos_Min(a, b)`: `a < b ? a : b` (unlike `f64::min`, a NaN `a` gives `b` and a
/// NaN `b` gives `b`).
fn smos_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// Whether the periodic extent of direction `ii` covers the whole period.
fn periodic_full(u: &PeriodicExtent1d, v: &PeriodicExtent1d, ii: usize) -> bool {
    if ii == 0 {
        u.is_full_period()
    } else {
        v.is_full_period()
    }
}

/// `(min, max)` of two values, the first when they are equal.
fn order(a: f64, b: f64) -> (f64, f64) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

/// `SplitAtSeam`: inserts seam edges where the face's periodic surface has a crossed
/// or missing seam, splitting the face (first along U, then each resulting face along
/// V). `crv_class_u` / `crv_class_v` and `problem` come from `HasSeamProblem` (they are
/// rebuilt over all loops when they were made for target loops only); `props`
/// replaces the surface periodicity and missing-seam verdict when given; `resolve` maps
/// a classification object to the face, edge or vertex it names. A whole closed surface
/// that had only vertex loops ([`fp3::is_closed_surface_with_vertex_loops`]) needs no
/// pole vertices or loop orientation beforehand: afterwards its only edge loop is the
/// outer loop (heading the face uses' lists) and its vertex loops are inner.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface or a recursion deeper than
/// [`MAX_SPLIT_AT_SEAM_DEPTH`]; those of [`Surface::is_periodic`],
/// [`fp2::remove_suspect_split_at_seam_uv_trim_curves`] and `MergeIntervals`;
/// `SM_ERR_ASSERT_FAILURE` for a vertex-only closed surface left with two edge loops,
/// `SM_ERR_NULL_POINTER` for one left with none; [`SolidError::Unported`]
/// (`HasSeamProblem`; the deferrals of `MergeIntervals`' callees).
#[allow(clippy::too_many_arguments)] // one upstream routine
pub fn split_at_seam<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    problem: SeamProblem,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
) -> Result<SeamSplit> {
    split_at_seam_notified(
        k,
        b,
        None,
        f,
        crv_class_u,
        crv_class_v,
        problem,
        props,
        resolve,
    )
}

/// [`split_at_seam`] notifying its `MergeIntervals` edits through `env` when given
/// (upstream's `SmObject::Notify` reaches the context's system callback, e.g. a
/// healer's `SmTrackTopologyChanges`).
///
/// # Errors
///
/// As [`split_at_seam`].
#[allow(clippy::too_many_arguments)] // one upstream routine
pub fn split_at_seam_notified<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    env: Option<&mut crate::topology::edge::NotifyEnv<'_>>,
    f: FaceId,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    problem: SeamProblem,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
) -> Result<SeamSplit> {
    split_at_seam_depth(
        k,
        b,
        env,
        f,
        crv_class_u,
        crv_class_v,
        problem,
        props,
        resolve,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn split_at_seam_depth<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    env: Option<&mut crate::topology::edge::NotifyEnv<'_>>,
    f: FaceId,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    mut problem: SeamProblem,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
    depth: usize,
) -> Result<SeamSplit> {
    use SurfParamType as Sp;
    if depth > MAX_SPLIT_AT_SEAM_DEPTH {
        return Err(null("SplitAtSeam: recursion too deep"));
    }
    let out = SeamSplit::default();

    // no work - FaceProps says no missing seam
    if let Some(p) = props {
        if matches!(p.bad_missing_seam_dir, Sp::Neither | Sp::Unknown) {
            return Ok(out);
        }
    }

    let surface = surface_deref(b, f)?;
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let (periodic_u, periodic_v) = match props {
        // an UNSURE flag is not FALSE
        Some(p) => (p.closed_u != Tri::False, p.closed_v != Tri::False),
        // (IsPeriodic returns a flag upstream; its SER inside makes a failure TRUE)
        None => (
            ignored(surface.is_periodic(k, &uv_domain, Sp::U))?.unwrap_or(true),
            ignored(surface.is_periodic(k, &uv_domain, Sp::V))?.unwrap_or(true),
        ),
    };
    if !periodic_u && !periodic_v {
        return Ok(out);
    }

    // classify against all loops when the classifications were made for some only
    if crv_class_u.tgt_loops_only() || crv_class_v.tgt_loops_only() {
        crv_class_u.set_tgt_loops_only(false);
        crv_class_v.set_tgt_loops_only(false);
        // (result ignored upstream; HasSeamProblem returns a flag, not a status)
        ignore_status(has_seam_problem(
            k,
            b,
            f,
            crv_class_u,
            crv_class_v,
            &mut problem,
            false,
        ))?;
    }

    let any = |p: SurfParamType| matches!(p, Sp::Both | Sp::U | Sp::V);
    if !(any(problem.crossed) || any(problem.missing)) {
        return Ok(out);
    }

    let vertex_only_closed_surface = fp3::is_closed_surface_with_vertex_loops(k, b, f, props)?;

    let merge_u = names(problem.crossed, Sp::U) || names(problem.missing, Sp::U);
    let merge_v = names(problem.crossed, Sp::V) || names(problem.missing, Sp::V);

    // allow Brep modifications (upstream's SmTemporaryChangeValue of
    // pBrep->m_bEditingEnabled and the context's DoingBoolean flag, restored on
    // every return)
    let editing = b.editing_enabled();
    let doing_boolean = k.context.doing_boolean();
    b.set_editing_enabled(true);
    k.context.set_doing_boolean(true);
    let r = split_at_seam_merges(
        k,
        b,
        env,
        f,
        crv_class_u,
        crv_class_v,
        props,
        resolve,
        depth,
        (merge_u, merge_v),
        vertex_only_closed_surface,
    );
    b.set_editing_enabled(editing);
    k.context.set_doing_boolean(doing_boolean);
    r
}

/// `GetLast()`'s index of a classification of `size` intervals (the callers check
/// `GetSize() > 0` first); an empty one is `SM_ERR_ASSERT_FAILURE`.
fn last_index(size: usize) -> Result<usize> {
    size.checked_sub(1).ok_or(SolidError::Upstream {
        operation: OP,
        code: SM_ERR_ASSERT_FAILURE,
        entity: None,
    })
}

/// The vertex a seam classification's end names (upstream's `GetVertexObject`): the B-rep
/// vertex its handle names, as `MergeIntervals` resolves the classification's handles
/// ([`merge_intervals`]: the B-rep's own objects first, then the caller's `resolve`). The
/// ends `HasSeamProblem` classifies to vertices hold the B-rep's own handles, which the
/// caller's `resolve` need not know.
fn seam_vertex<O: LoopObject>(o: O, resolve: &dyn Fn(O) -> Option<EntityRef>) -> Option<VertexId> {
    match o.as_topology().or_else(|| resolve(o)) {
        Some(EntityRef::Vertex(v)) => Some(v),
        _ => None,
    }
}

/// The merges of [`split_at_seam_depth`] (`merge` = `(bMergeU, bMergeV)`), with the
/// B-rep's editing and the context's Boolean flag set; then, for a closed surface that
/// had only vertex loops, the inserted seams' loop becomes the outer loop.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn split_at_seam_merges<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    mut env: Option<&mut crate::topology::edge::NotifyEnv<'_>>,
    f: FaceId,
    crv_class_u: &mut CurveClassification<O>,
    crv_class_v: &mut CurveClassification<O>,
    props: Option<&FaceProps<O>>,
    resolve: &dyn Fn(O) -> Option<EntityRef>,
    depth: usize,
    (merge_u, merge_v): (bool, bool),
    vertex_only_closed_surface: bool,
) -> Result<SeamSplit> {
    use SurfParamType as Sp;
    let mut out = SeamSplit::default();
    let mut this_new = SeamSplit::default();
    if merge_u {
        fp2::remove_suspect_split_at_seam_uv_trim_curves(b, crv_class_u, |o| {
            seam_vertex(o, resolve)
        })?;
        this_new = merge_intervals(k, crv_class_u, Sp::U, b, env.as_deref_mut(), f, resolve)?;
        if !merge_v {
            out.faces.extend_from_slice(&this_new.faces);
            out.edges.extend_from_slice(&this_new.edges);
            out.vertices.extend_from_slice(&this_new.vertices);
        }
    }

    if merge_u && merge_v && !this_new.faces.is_empty() {
        let mut targets = this_new.faces.clone();
        if !targets.contains(&f) {
            targets.push(f);
        }
        for tgt in targets {
            let mut cu = CurveClassification::new(k);
            let mut cv = CurveClassification::new(k);
            let mut this_problem = SeamProblem {
                crossed: Sp::Unknown,
                missing: Sp::Unknown,
                near_miss: Sp::Unknown,
            };
            let has = ignored(has_seam_problem(
                k,
                b,
                tgt,
                &mut cu,
                &mut cv,
                &mut this_problem,
                false,
            ))?
            .unwrap_or(false);
            if has {
                let s = ignored(split_at_seam_depth(
                    k,
                    b,
                    env.as_deref_mut(),
                    f,
                    &mut cu,
                    &mut cv,
                    this_problem,
                    props,
                    resolve,
                    depth + 1,
                ))?
                .unwrap_or_default();
                out.faces.extend_from_slice(&s.faces);
                if tgt != f && !out.faces.contains(&tgt) {
                    out.faces.push(tgt);
                }
                out.edges.extend_from_slice(&s.edges);
                out.vertices.extend_from_slice(&s.vertices);
            }
        }
    } else if merge_v {
        // A nonseparating U seam leaves the face intact. Keep the known interior
        // classification of V and attach its ends to the shared seam vertex. Either seam
        // may be divided at existing vertices; carry the corner between the complete
        // walks without changing intermediate classifications. Reclassifying this
        // intermediate two-boundary face loses the V interior.
        if merge_u && !this_new.edges.is_empty() && crv_class_u.size() > 0 && crv_class_v.size() > 0
        {
            let first_u = &crv_class_u.at(0)?.start;
            let last_u = &crv_class_u.at(last_index(crv_class_u.size())?)?.end;
            if first_u.point_class() == PointClassType::Vertex
                && last_u.point_class() == PointClassType::Vertex
                && first_u.object() == last_u.object()
            {
                let corner = first_u.vertex_object();
                let this = O::from_face(f);
                let last_v = last_index(crv_class_v.size())?;
                for (interval, at_start) in [(0, true), (last_v, false)] {
                    let ivl = crv_class_v.at_mut(interval)?;
                    let end = if at_start {
                        &mut ivl.start
                    } else {
                        &mut ivl.end
                    };
                    if end.point_class() == PointClassType::Face && end.object() == this {
                        end.set_class_object(PointClassType::Vertex, corner);
                    }
                }
            }
        }
        if merge_u {
            out.edges.extend_from_slice(&this_new.edges);
            out.vertices.extend_from_slice(&this_new.vertices);
        }
        fp2::remove_suspect_split_at_seam_uv_trim_curves(b, crv_class_v, |o| {
            seam_vertex(o, resolve)
        })?;
        this_new = merge_intervals(k, crv_class_v, Sp::V, b, env, f, resolve)?;
        out.faces.extend_from_slice(&this_new.faces);
        out.edges.extend_from_slice(&this_new.edges);
        out.vertices.extend_from_slice(&this_new.vertices);
    }

    if vertex_only_closed_surface {
        // Seam insertion makes the only edge loop the natural outer boundary. Any
        // original vertices off the seams remain inner vertex loops.
        let loops = fp2::loops(b, f, false, None)?;
        let mut outer_loop: Option<LoopId> = None;
        for &l in &loops {
            let (lu, _) = lu1::loop_loopuses(b, l)?;
            if !lu1::is_vertex_loopuse(b, lu)? {
                // AERN(pOuterLoop == NULL, SM_ERR_ASSERT_FAILURE)
                if outer_loop.is_some() {
                    return Err(SolidError::Upstream {
                        operation: OP,
                        code: SM_ERR_ASSERT_FAILURE,
                        entity: None,
                    });
                }
                outer_loop = Some(l);
            }
        }
        let Some(outer_loop) = outer_loop else {
            return Err(SolidError::null_pointer(OP));
        };
        face_notify(b, f, NotifyOperation::PreEdit)?;
        for &l in &loops {
            let (lu, mate) = lu1::loop_loopuses(b, l)?;
            let mate = mate.ok_or_else(|| null("SplitAtSeam: loop use without mate"))?;
            for side in [lu, mate] {
                let orientation = if l == outer_loop {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                };
                lu1::set_orientation(b, side, orientation)?;
                if l == outer_loop {
                    // pUses[lSide]->GetFaceuse()->SetList(pUses[lSide]): the loop use
                    // heads its face use's circular list
                    let fu = lu1::faceuse(b, side)?
                        .ok_or_else(|| null("SplitAtSeam: loop use without face use"))?;
                    let list = &mut b.faceuse_mut(fu)?.loopuses;
                    let at = list
                        .iter()
                        .position(|&x| x == side)
                        .ok_or_else(|| null("SplitAtSeam: loop use not on its face use"))?;
                    list.rotate_left(at);
                }
            }
        }
        remove_uv_trim_curves(b, f)?;
        face_notify(b, f, NotifyOperation::PostEdit)?;
    }
    Ok(out)
}

// ---- DeleteInnerLoop --------------------------------------------------------------------

/// `DeleteInnerLoop`: removes the inner loop `l` from face `f` and deletes the loop, its
/// two loop uses, their edge uses and vertex uses, the loop's edges, and those of its
/// vertices no vertex use is left on. Notified through the caller's `env`:
/// `SM_NO_PRE_EDIT` of the face and of its B-rep, then `SM_NO_DESTRUCTION` of every
/// deleted object, each followed by `~SmAObject`'s `ClearAttributes` (SMLib's default
/// handlers: a deleted object's dependent attributes go with their last user).
///
/// # Errors
///
/// `SM_ERR` when `l` is the face's outer loop or not one of its loops, or one of its
/// edges is not lamina (another face, or another loop of this face, uses it); nothing
/// has changed then. [`SolidError::InvalidInput`] where upstream dereferences null (a
/// loop without both loop uses, an edge use without edge, vertex use or vertex) and
/// for a caller key in the kernel's reserved range; those of the notifications
/// ([`SolidError::Unported`]).
pub fn delete_inner_loop(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    f: FaceId,
    l: LoopId,
) -> Result<()> {
    brep_p3::with_caller_identities_for(k, b, env, None, 0, |k, b, env| {
        delete_inner_loop_in(k, b, env, f, l)
    })
}

/// [`delete_inner_loop`] for a caller without notification state of its own: through the
/// kernel context's callbacks, with the B-rep's own attributes
/// ([`Brep::add_attribute`]) lent for the edit
/// ([`crate::topology::brep::p3::with_kernel_callbacks`]).
///
/// # Errors
///
/// As [`delete_inner_loop`].
pub fn delete_inner_loop_with_kernel_callbacks(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    l: LoopId,
) -> Result<()> {
    brep_p3::with_kernel_callbacks(k, b, |k, b, env| delete_inner_loop(k, b, env, f, l))
}

/// [`delete_inner_loop`] once the identities are checked and seeded.
fn delete_inner_loop_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    f: FaceId,
    l: LoopId,
) -> Result<()> {
    // Check that this loop is indeed in this Face, and that it's not an outer loop.
    let face_loops = fp2::loops(b, f, false, None)?;

    // Outer loop? (for a face without loops upstream reads past its empty array; the
    // port compares with nothing and the membership test below fails)
    if face_loops.first() == Some(&l) {
        return Err(SolidError::sm_err(OP));
    }

    // One of our loops?
    if !face_loops.contains(&l) {
        return Err(SolidError::sm_err(OP));
    }

    // Check that there are no other Faces connected to this Loop.
    let loop_edges = lu1::loop_edges(b, l)?
        .into_iter()
        .map(|e| e.ok_or_else(|| null("DeleteInnerLoop: edge use without edge")))
        .collect::<Result<Vec<EdgeId>>>()?;
    for &e in &loop_edges {
        if !crate::topology::edge::is_lamina(b, e)? {
            return Err(SolidError::sm_err(OP));
        }
    }

    // Notify the face that we are about to start
    let data = face_edit_data(b, env, f)?;
    ignore_status(fp3::notify(b, f, env, None, NotifyOperation::PreEdit, data))?;

    // Must notify brep (owner) as well to delete the cache
    if has_brep(b, f)? {
        let brep = brep_p2::brep_ref(b);
        ignore_status(brep_p2::notify(
            b,
            env,
            None,
            NotifyOperation::PreEdit,
            [Some(brep_p2::NotifyArg::Object(brep)), None, None],
        ))?;
    }

    // Note, Loop's Loopuse's Edgeuses are required in order to retrieve the Loop's
    // Vertices, so get them first.
    let verts = lu1::loop_vertices(b, l)?
        .into_iter()
        .map(|v| v.ok_or_else(|| null("DeleteInnerLoop: vertex use without vertex")))
        .collect::<Result<Vec<VertexId>>>()?;

    // Now remove the ...uses.
    let (lu_1, lu_2) = lu1::loop_loopuses(b, l)?;
    let lu_2 = lu_2.ok_or_else(|| null("DeleteInnerLoop: loop without second loop use"))?;
    let mut eus_to_delete = Vec::new();
    let mut vus_to_delete = Vec::new();
    // For both Loopuses:
    for lu in [lu_1, lu_2] {
        eus_to_delete.extend(lu1::edgeuses(b, lu, None, None)?);
        vus_to_delete.extend(lu1::vertexuses(b, lu, None)?);
    }

    for vu in vus_to_delete {
        let vu = vu.ok_or_else(|| null("DeleteInnerLoop: edge use without vertex use"))?;
        destroy_vertexuse(k, b, env, vu)?; // Also deletes: nothing.
    }

    for eu in eus_to_delete {
        destroy_edgeuse(k, b, env, eu)?; // Also deletes: its uv trim curve.
    }

    // Now Vertices and Edges.
    for v in verts {
        if b.vertex(v)?.vertexuses.is_empty() {
            destroy_vertex(k, b, env, v)?; // Also deletes: nothing.
        }
    }
    for e in loop_edges {
        crate::topology::edge::destroy(k, b, e, env)?; // Also deletes: its curve.
    }

    // Finally, remove the Loop.
    // Removes the Loop and its Loopuses from the Face but deletes nothing.
    remove_loop_notified(b, env, f, l)?;

    for lu in [lu_1, lu_2] {
        destroy_loop_record(k, b, env, EntityRef::Loopuse(lu))?;
    }
    destroy_loop_record(k, b, env, EntityRef::Loop(l))
}

/// The data of the face's edit notifications: the face and `SM_NO_GET_BREP(this)`.
fn face_edit_data(
    b: &Brep,
    env: &crate::topology::edge::NotifyEnv<'_>,
    f: FaceId,
) -> Result<[Option<ObjectRef>; 3]> {
    let brep = if has_brep(b, f)? {
        Some(brep_p2::brep_ref(b))
    } else {
        None
    };
    Ok([Some((env.key_of)(EntityRef::Face(f))), brep, None])
}

/// [`remove_loop`] with the face's notifications through the caller's `env`.
fn remove_loop_notified(
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    f: FaceId,
    l: LoopId,
) -> Result<()> {
    // inform the public
    let data = face_edit_data(b, env, f)?;
    ignore_status(fp3::notify(b, f, env, None, NotifyOperation::PreEdit, data))?;
    let (lu_1, lu_2) = lu1::loop_loopuses(b, l)?;
    let lu_2 = lu_2.ok_or_else(|| null("loop without second loop use"))?;
    let fu_1 = lu1::faceuse(b, lu_1)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let fu_2 = lu1::faceuse(b, lu_2)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    faceuse_remove(b, fu_1, lu_1)?;
    faceuse_remove(b, fu_2, lu_2)?;
    // (the port-only face loop list)
    b.face_mut(f)?.loops.retain(|&x| x != l);
    let data = face_edit_data(b, env, f)?;
    ignore_status(fp3::notify(
        b,
        f,
        env,
        None,
        NotifyOperation::PostEdit,
        data,
    ))
}

/// `delete pVU`: `~SmVertexuse` notifies `SM_NO_DESTRUCTION` (status ignored), then
/// `~SmTopology` takes it off its vertex's list and `~SmAObject` clears its attributes.
fn destroy_vertexuse(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    vu: VertexuseId,
) -> Result<()> {
    let this = (env.key_of)(EntityRef::Vertexuse(vu));
    crate::topology::edge::with_object_context(k, env, |n| {
        ignore_status(vu1::notify(
            b,
            vu,
            &mut *n.records,
            &mut *n.attributes,
            n.context.as_deref_mut(),
            NotifyOperation::Destruction,
            this,
            [Some(this), None, None],
        ))
    })?;
    if let Some(v) = b.vertexuse(vu)?.vertex {
        b.vertex_mut(v)?.vertexuses.retain(|&x| x != vu);
    }
    b.remove_vertexuse_record(vu)?;
    env.attributes.clear_attributes(this.key)
}

/// `delete pEU`: [`crate::topology::edgeuse::p2::destroy`] (`~SmEdgeuse`'s notification,
/// its UV trim curve, `~SmAObject`); `~SmTopology` takes it off its edge's list.
fn destroy_edgeuse(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    eu: EdgeuseId,
) -> Result<()> {
    let e = eu1::edge(b, eu)?;
    crate::topology::edgeuse::p2::destroy(k, b, eu, env)?;
    if let Some(e) = e {
        b.edge_mut(e)?.edgeuses.retain(|&x| x != eu);
    }
    Ok(())
}

/// `delete pVtx`: `~SmVertex` notifies `SM_NO_DESTRUCTION` (status ignored), then
/// `~SmTopology` takes it off the B-rep's vertex list and `~SmAObject` clears its
/// attributes.
fn destroy_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    v: VertexId,
) -> Result<()> {
    let this = (env.key_of)(EntityRef::Vertex(v));
    let key_of = env.key_of;
    crate::topology::edge::with_object_context(k, env, |n| {
        ignore_status(vx::notify(
            b,
            v,
            &mut *n.records,
            &mut *n.attributes,
            n.context.as_deref_mut(),
            NotifyOperation::Destruction,
            this,
            [Some(this), None, None],
            &|vu| key_of(EntityRef::Vertexuse(vu)),
        ))
    })?;
    b.remove_vertex_record(v)?;
    env.attributes.clear_attributes(this.key)
}

/// `delete pLU` / `delete pLoopToDelete` once off their face uses: `~SmLoopuse` /
/// `~SmLoop` notify `SM_NO_DESTRUCTION` (`SmTopology::Notify`, status ignored) and
/// `~SmAObject` clears the attributes.
fn destroy_loop_record(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
    entity: EntityRef,
) -> Result<()> {
    let this = (env.key_of)(entity);
    crate::topology::edge::with_object_context(k, env, |n| {
        ignore_status(crate::topology::topology::notify(
            &mut *n.records,
            &mut *n.attributes,
            n.context.as_deref_mut(),
            NotifyOperation::Destruction,
            this,
            Some(this),
            None,
            None,
        ))
    })?;
    match entity {
        EntityRef::Loopuse(lu) => drop(b.remove_loopuse_record(lu)?),
        EntityRef::Loop(l) => drop(b.remove_loop_record(l)?),
        _ => return Err(null("DeleteInnerLoop: not a loop or loop use")),
    }
    env.attributes.clear_attributes(this.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::ObjectKey;
    use crate::intersect::point_class::{ClassObject, ClassObjectKind};
    use crate::surface::plane::Plane;
    use crate::topology::{Faceuse, FaceuseId};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(LoopId);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Loop
        }
    }

    impl LoopObject for H {
        fn from_loop(l: LoopId) -> Self {
            Self(l)
        }
    }

    fn me(f: FaceId) -> ObjectRef {
        ObjectRef {
            type_code: fp2::TYPE,
            key: ObjectKey(1 + u64::from(f.index())),
        }
    }

    fn key_of(e: EntityRef) -> ObjectRef {
        match e {
            EntityRef::Face(f) => me(f),
            _ => ObjectRef {
                type_code: 0,
                key: ObjectKey(0),
            },
        }
    }

    fn bare_face() -> (Brep, FaceId) {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        (b, f)
    }

    fn plane_face() -> (Brep, FaceId) {
        let (mut b, f) = bare_face();
        let mut p = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        p.set_owner(Some(me(f)));
        b.face_mut(f).unwrap().surface = Some(p);
        (b, f)
    }

    #[test]
    fn plan_type_codes() {
        assert_eq!(FixSeamPlanType::Unknown as u8, 0);
        assert_eq!(FixSeamPlanType::NoChanges as u8, 1);
        assert_eq!(FixSeamPlanType::SplitFace as u8, 4);
        assert_eq!(FixSeamPlanType::default(), FixSeamPlanType::Unknown);
    }

    #[test]
    fn uv_domain_without_surface_keeps_the_face_domain() {
        let (mut b, f) = bare_face();
        let d = Extent2d::from_bounds(0.0, 1.0, 2.0, 3.0);
        fp2::set_uv_domain(&mut b, f, &d).unwrap();
        let k = Kernel::default();
        let r = calculate_uv_domain_from_uv_trim_curves(&k, &mut b, f, true, true, true).unwrap();
        assert_eq!(r.domain, d);
        assert_eq!(r.max_uv_loop_gap, 0.0);
        assert_eq!(r.second_max_uv_loop_gap, 0.0);
        // with a surface the UV trim curves are made first (topology.face_p3)
        let (mut b, f) = plane_face();
        calculate_uv_domain_from_uv_trim_curves(&k, &mut b, f, true, false, false).unwrap();
    }

    #[test]
    fn rectangular_trim_needs_one_loop() {
        let (mut b, f) = plane_face();
        let k = Kernel::default();
        let r = find_if_rectangular_trim(&k, &mut b, f, &key_of).unwrap();
        assert!(!r.rectangular && !r.natural);
        assert_eq!(r.uv_domain, Extent2d::new());
    }

    fn is_null_pointer(e: &SolidError) -> bool {
        matches!(
            e,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        )
    }

    /// `plane_face` with the pair of face uses (no loops) the edits walk.
    fn plane_face_with_faceuses() -> (Brep, FaceId, FaceuseId, FaceuseId) {
        let (mut b, f) = plane_face();
        let top = b
            .add_faceuse_record(Faceuse {
                face: Some(f),
                mate: Some(FaceuseId::from_index(1)),
                orientation: OrientType::Same,
                ..Faceuse::default()
            })
            .unwrap();
        let bot = b
            .add_faceuse_record(Faceuse {
                face: Some(f),
                mate: Some(top),
                orientation: OrientType::Opposite,
                ..Faceuse::default()
            })
            .unwrap();
        fp2::set_faceuse(&mut b, f, Some(top)).unwrap();
        (b, f, top, bot)
    }

    /// With `SmFace::Notify` ported (`topology.face_p3`) the edits no longer stop at the
    /// notification: the pre-edit notification runs (it resets the cached rectangular
    /// trim), then each edit does what the C++ does until it reaches the next unported
    /// callee (`GetUpwardEdgeuses` of the post-edit notification, `GetBrep`) or the
    /// `NER` on a face without face use.
    #[test]
    fn edits_run_notify_then_the_edit() {
        let k = Kernel::default();

        // no face use: the pre-edit notification ran, then `NER(pFU)`
        let (mut b, f) = plane_face();
        b.face_mut(f).unwrap().rectangular_trim = 1;
        assert!(is_null_pointer(
            &flip_face_orientation(&mut b, f).unwrap_err()
        ));
        assert_eq!(b.face(f).unwrap().rectangular_trim, fp2::UNSURE);
        b.face_mut(f).unwrap().rectangular_trim = 1;
        assert!(is_null_pointer(
            &remove_uv_trim_curves(&mut b, f).unwrap_err()
        ));
        assert_eq!(b.face(f).unwrap().rectangular_trim, fp2::UNSURE);

        // with face uses and no loops: RemoveUVTrimCurves has nothing to delete and
        // (as upstream) no post-edit notification
        let (mut b, f, _, _) = plane_face_with_faceuses();
        remove_uv_trim_curves(&mut b, f).unwrap();

        // FlipFaceOrientation swaps the face uses, then its post-edit notification runs
        // through `GetUpwardEdgeuses`
        let (mut b, f, top, bot) = plane_face_with_faceuses();
        flip_face_orientation(&mut b, f).unwrap();
        assert_eq!(fp2::upward_faceuse(&b, f).unwrap(), Some(bot));
        assert_eq!(b.faceuse(top).unwrap().orientation, OrientType::Opposite);
        assert_eq!(b.faceuse(bot).unwrap().orientation, OrientType::Same);

        // SetSurface: the change-geometry notification (null data) runs through
        // `GetUpwardEdgeuses`, then the surface and its natural domain are stored and the
        // old surface is dropped (`bDeleteCurrFace`) or handed back
        let (mut b, f, _, _) = plane_face_with_faceuses();
        let s = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::X));
        let domain = s.natural_uv_domain_with_nurb(&k).unwrap();
        assert!(set_surface(&k, &mut b, f, me(f), s, true)
            .unwrap()
            .is_none());
        let face = b.face(f).unwrap();
        assert_eq!(face.uv_domain, domain);
        assert_eq!(face.surface.as_ref().unwrap().owner(), Some(me(f)));
        let s = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Y));
        assert!(set_surface(&k, &mut b, f, me(f), s, false)
            .unwrap()
            .is_some());

        // a lazy analytic (no NURB yet) gives the face the domain of the NURB it makes,
        // as `GetNaturalUVDomain` does upstream, not its NURB-less base's empty one
        let lazy = Surface::from(crate::surface::cone::Cone::new_bounded(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::X,
            Vec3::Y,
            2.0,
            1.0,
            0.0,
            270.0,
            3.0,
            false,
            false,
            false,
        ));
        assert!(lazy.natural_uv_domain_if_built().is_degenerate(0.0));
        let domain = lazy.natural_uv_domain_with_nurb(&k).unwrap();
        assert!(!domain.is_degenerate(0.0));
        set_surface(&k, &mut b, f, me(f), lazy, true).unwrap();
        assert_eq!(b.face(f).unwrap().uv_domain, domain);

        // SwapUV starts at `GetBrep()->GetContext()`: null without a B-rep
        let (mut b, f) = plane_face();
        let e = swap_uv(&k, &mut b, f).unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));

        // the destructor notifies DESTRUCTION (no hierarchy to inform), clears the
        // face use and drops the surface the face owns
        let (mut b, f, _, _) = plane_face_with_faceuses();
        destroy(&mut b, f, me(f)).unwrap();
        let face = b.face(f).unwrap();
        assert!(face.faceuse.is_none() && face.surface.is_none());
    }

    #[test]
    fn contextual_uv_removal_preserves_actual_notify_payload_and_null_prefix() {
        use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior};
        use crate::geom::context::Context;
        use crate::topology::brep::p3::{notify_serial, topology_ref};
        use crate::topology::edge::notification_test_support as callbacks;
        use crate::topology::topology::Topology;
        for mode in 0..3 {
            let mut k = Kernel::default();
            let (mut b, f) = if mode == 0 {
                plane_face()
            } else {
                let (b, f, _, _) = plane_face_with_faceuses();
                (b, f)
            };
            let serial = notify_serial(&mut k.context, &mut b).unwrap();
            let actual_key = |entity| topology_ref(serial, entity);
            let this = actual_key(EntityRef::Face(f));
            let events = callbacks::install(&mut k.context);
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut metadata = Topology::new();
            metadata.set_user_index1(42);
            records.insert(this.key, metadata);
            let mut attributes = AttributeStore::new();
            let marker = attributes
                .insert(Attribute::new(
                    ids::ALL_FILLET_EDGES,
                    Behavior::StandaloneReference,
                ))
                .unwrap();
            attributes.add_attribute(this.key, marker).unwrap();
            b.face_mut(f).unwrap().rectangular_trim = 1;
            let result = Context::with_notification_context(&mut k, |k, context| {
                let mut env = crate::topology::edge::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: Some(context),
                    key_of: &actual_key,
                };
                if mode == 2 {
                    shrink_geometry_with_notifications(k, &mut b, f, &mut env)
                } else {
                    remove_uv_trim_curves_with_notifications(&mut b, f, &mut env)
                }
            })
            .unwrap();
            if mode == 0 {
                assert!(is_null_pointer(&result.unwrap_err()));
            } else {
                result.unwrap();
            }
            assert_eq!(b.face(f).unwrap().rectangular_trim, fp2::UNSURE);
            assert_eq!(attributes.users(marker).unwrap(), vec![this.key]);
            assert_eq!(records.get(&this.key).unwrap().user_index1(), 42);
            let observed = events.lock().unwrap();
            assert_eq!(observed.len(), 2);
            for (event, callback) in observed.iter().zip(["user", "system"]) {
                assert_eq!(event.callback, callback);
                assert_eq!(event.operation, NotifyOperation::PreEdit);
                assert_eq!(event.caller, this);
                assert_eq!(event.data, [Some(this), None, None]);
            }
            drop(observed);
            callbacks::assert_installed(&k.context, &events);
        }
    }

    #[test]
    fn shrink_skips_faces_without_owned_surface() {
        let (mut b, f) = bare_face();
        let k = Kernel::default();
        shrink_geometry(&k, &mut b, f, &key_of).unwrap();
        b.face_mut(f).unwrap().surface =
            Some(Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)));
        shrink_geometry(&k, &mut b, f, &key_of).unwrap();
        let (mut b, f) = plane_face();
        shrink_geometry(&k, &mut b, f, &key_of).unwrap();
    }

    #[test]
    fn seams_quiet_and_non_periodic() {
        let (mut b, f) = plane_face();
        let mut k = Kernel::default();
        let mut cu = CurveClassification::<H>::new(&k);
        let mut cv = CurveClassification::<H>::new(&k);
        let mut p = SeamProblem {
            crossed: SurfParamType::Neither,
            missing: SurfParamType::Neither,
            near_miss: SurfParamType::Neither,
        };
        let r = move_seam(
            &mut k,
            &mut b,
            f,
            me(f),
            &mut cu,
            &mut cv,
            p,
            MoveSeamOptions::default(),
            None,
            &|_| None,
        )
        .unwrap();
        assert_eq!(r.u, FixSeamPlanType::NoChanges);
        assert_eq!(r.v, FixSeamPlanType::NoChanges);
        // a plane is periodic in neither direction
        p.crossed = SurfParamType::Both;
        let s = split_at_seam(&mut k, &mut b, f, &mut cu, &mut cv, p, None, &|_| None).unwrap();
        assert_eq!(s, SeamSplit::default());
        // face properties saying no missing seam end the split early
        let mut props = FaceProps::<H>::new();
        props.bad_missing_seam_dir = SurfParamType::Neither;
        let s = split_at_seam(
            &mut k,
            &mut b,
            f,
            &mut cu,
            &mut cv,
            p,
            Some(&props),
            &|_| None,
        )
        .unwrap();
        assert_eq!(s, SeamSplit::default());
        // a problem reported by the face properties walks the face's loop uses: a plane
        // face has nothing to move
        props.bad_crossed_seam_dir = SurfParamType::U;
        let e = move_seam(
            &mut k,
            &mut b,
            f,
            me(f),
            &mut cu,
            &mut cv,
            p,
            MoveSeamOptions::default(),
            Some(&props),
            &|_| None,
        );
        let r = e.unwrap();
        assert_eq!(r.u, FixSeamPlanType::NoChanges);
        assert_eq!(r.v, FixSeamPlanType::NoChanges);
        assert_eq!(r.split, SeamSplit::default());
    }

    /// A handle that names the B-rep's topology too, as the `BrepSeamObjects` handles
    /// `HasSeamProblem` classifies seam ends with.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum T {
        Loop(LoopId),
        Topo(EntityRef),
    }

    impl ClassObject for T {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Loop(_) => ClassObjectKind::Loop,
                Self::Topo(e) => e.class_kind(),
            }
        }
    }

    impl LoopObject for T {
        fn from_loop(l: LoopId) -> Self {
            Self::Loop(l)
        }

        fn as_loop(&self) -> Option<LoopId> {
            match self {
                Self::Loop(l) => Some(*l),
                Self::Topo(e) => e.as_loop(),
            }
        }

        fn as_vertex(&self) -> Option<VertexId> {
            match self {
                Self::Topo(EntityRef::Vertex(v)) => Some(*v),
                _ => None,
            }
        }

        fn from_topology(t: EntityRef) -> Option<Self> {
            Some(Self::Topo(t))
        }

        fn as_topology(&self) -> Option<EntityRef> {
            match self {
                Self::Topo(t) => Some(*t),
                Self::Loop(l) => Some(EntityRef::Loop(*l)),
            }
        }
    }

    /// A seam whose classification starts at an existing vertex (a vertex loop on the seam,
    /// a corner): the vertex is named by the B-rep's own handle and the caller's `resolve`
    /// knows nothing, as for every caller but the healer. Clearing the suspect UV trims
    /// before the merge must still find the vertex (upstream: `GetVertexObject`) instead of
    /// failing `InvalidInput` ("unresolved vertex").
    #[test]
    fn split_at_seam_resolves_seam_vertices_by_their_own_handles() {
        let (mut b, f) = plane_face();
        let v = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let mut k = Kernel::default();
        let mut cu = CurveClassification::<T>::new(&k);
        cu.set_size(&k, 1).unwrap();
        cu.at_mut(0)
            .unwrap()
            .start
            .set_class_object(PointClassType::Vertex, Some(T::Topo(EntityRef::Vertex(v))));
        let mut cv = CurveClassification::<T>::new(&k);
        let r = split_at_seam_merges(
            &mut k,
            &mut b,
            None,
            f,
            &mut cu,
            &mut cv,
            None,
            &|_| None,
            0,
            (true, false),
            false,
        );
        // (the plane has no seam curve, so the merge after it may still fail; not on the
        // seam's vertex)
        let unresolved = matches!(
            &r,
            Err(SolidError::InvalidInput { reason, .. }) if reason.contains("unresolved vertex")
        );
        assert!(!unresolved, "{r:?}");
    }

    #[test]
    fn split_at_seam_depth_is_bounded() {
        let (mut b, f) = plane_face();
        let mut k = Kernel::default();
        let mut cu = CurveClassification::<H>::new(&k);
        let mut cv = CurveClassification::<H>::new(&k);
        let p = SeamProblem {
            crossed: SurfParamType::Both,
            missing: SurfParamType::Both,
            near_miss: SurfParamType::Neither,
        };
        let e = split_at_seam_depth(
            &mut k,
            &mut b,
            None,
            f,
            &mut cu,
            &mut cv,
            p,
            None,
            &|_| None,
            MAX_SPLIT_AT_SEAM_DEPTH + 1,
        );
        assert!(matches!(e, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn oriented_boundary_of_a_face_without_loops() {
        let (mut b, f) = plane_face();
        let k = Kernel::default();
        // no face use: upstream dereferences null
        assert!(create_oriented_trim_boundary(&k, &mut b, f, true, true, None, &key_of).is_err());
    }
}

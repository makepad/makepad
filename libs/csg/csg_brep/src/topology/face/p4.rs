//! `SmFace`, part 4: the upward-use getters (B-rep, edges, vertices, loop uses, vertex
//! uses, edge uses of the primary face use), the connection test, the line/face
//! intersection, the vertex sector a curve leaves into, and the seam check of faces on
//! periodic surfaces.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`brep`]                                     | `SmFace::GetBrep`                            |
//! | [`edges`]                                    | `SmFace::GetEdges`                           |
//! | [`vertices`]                                 | `SmFace::GetVertices`                        |
//! | [`upward_loopuses`]                          | `SmFace::GetUpwardLoopuses`                  |
//! | [`upward_vertexuses`]                        | `SmFace::GetUpwardVertexuses`                |
//! | [`upward_edgeuses`]                          | `SmFace::GetUpwardEdgeuses`                  |
//! | [`is_connected_to`]                          | `SmFace::IsConnectedTo`                      |
//! | [`global_line_intersect`], [`global_line_intersect_held`] | `SmFace::GlobalLineIntersect`  |
//! | [`find_vertex_sector`], [`VertexSector`], [`FindVertexSectorArgs`] | `SmFace::FindVertexSector` |
//! | [`has_seam_problem`], [`SeamCheck`], [`HasSeamProblemOptions`] | `SmFace::HasSeamProblem`  |
//! | [`BrepSeamObjects`], `merge_seam_intervals` (crate) | the `SmObject`s of `HasSeamProblem` / `SplitAtSeam`'s `MergeIntervals` |
//! | [`ClassifyProblem`]                          | `SmClassifyProblem` (`SmCurveClass.h`)       |
//! | [`assert_test_face37`]                       | `sm_AssertTestFace37` (file static)          |
//! | `loop_sample_uv_point` (private)             | `sm_GetLoopSampleUVPoint` (file static)      |
//!
//! Deviations:
//! - **Object model.** As [`super::p1`]: free functions over the [`Brep`]'s arenas,
//!   out-parameters are return values. Where upstream dereferences null (a loop use
//!   without face use in `IsConnectedTo`, a face without face uses or a face use
//!   without shell for a shell or region target, a face without surface, an edge use
//!   without edge or CW neighbour) the port returns [`SolidError::InvalidInput`].
//! - **Null entries.** Upstream's getters append whatever the loop uses hold, null
//!   pointers included. A null edge, vertex, vertex use (of an edge use) or face use
//!   face is [`SolidError::InvalidInput`] here, so the outputs hold ids only.
//! - **Attribute filters** (`pOptAttributeId`) are a predicate over the entity, as in
//!   [`crate::topology::loopuse`].
//! - **`GetBrep`** returns the arena B-rep when the face's upward face use reaches a
//!   region on the B-rep's region list, as [`crate::topology::shell::shell_brep`].
//! - **`GlobalLineIntersect`.** The surface's solutions are those of
//!   [`Surface::global_line_intersect`] with the caller's `surface` handle as object
//!   (upstream's surface pointer); [`global_line_intersect_held`] keeps the surface's
//!   cache in the caller's [`HeldSolveCache`] across calls, as upstream keeps it on the
//!   surface. [`global_line_intersect`] builds the surface's cache with the face
//!   ([`crate::query::face::p2::face_surface_cache`], `GetFace()` of the trimmed
//!   cache) when the solve fetches it, so the solve runs on the face's trimmed tree;
//!   the trimmed cache names the surface `EntityRef::Face(f)` (upstream's
//!   `m_cpSurface`, which every node's `mBA_pSurface` becomes after the build; the
//!   leaf solves compute their polar boxes on it). The solve gets the cache with
//!   every handle naming `surface` ([`SurfaceCache::map_handles`]: the line solve
//!   reads only node classes, boxes and the cache surface), its `sm_IsPointValid`
//!   runs the trimmed cache's `PointTest` (a face classification,
//!   [`TrimSrfCache::point_test_in`]), and `SmFace::PointClassify` of the single values gets the trimmed cache
//!   itself (upstream finds it on the surface). The held variant and
//!   `sm_FaceClassifyUVLine` classify without it. `SmFace::PointClassify` gets the face's tolerance
//!   options as `query.face_p2`'s callers pass them; its status is ignored as
//!   upstream ignores it ([`SolidError::Unported`], `InvalidInput` and `NotFound`
//!   propagate). A range whose `sm_FaceClassifyUVLine` returns no interval reads
//!   `sCC[0]` of an empty array upstream (`plan/upstream-issues.md` UB-180); the port
//!   returns `InvalidInput`.
//! - **Zone tolerances.** `GlobalLineIntersect` and `FindVertexSector` take an
//!   `SmZoneTol3d` by value; the `f64` here is converted as that constructor does
//!   ([`tol::zone_tol3d`]: `fabs`, [`tol::USE_DEFAULT`] -> the system zone tolerance).
//! - **`FindVertexSector`'s interval** starts as the curve's natural interval and takes
//!   the orientation's ends through `SetMinMax`, whose failure on a reversed pair is
//!   ignored as upstream (the natural interval stays).
//! - **`FindVertexSector` of a 2D curve** lifts it onto an evaluation snapshot of the
//!   face's concrete surface as a [`CrvOnSurf`] (upstream's `SetAll` takes any
//!   `SmSurface`). The shim passes the edge's 3D curve, so the UV lift is covered by
//!   a Rust regression rather than the native corpus.
//! - **`FindVertexSector`** returns [`VertexSector`]. The out-of-scope `sSurfDomain` of
//!   the 3D branch is reproduced: it is the natural UV domain only on the pass that first
//!   fetches the poles, and an empty extent on later passes, as upstream (a block-local
//!   `SmExtent2d` declared in the loop; WR-177). The seam curves of the closure checks are built
//!   once and dropped at the end (`SmObjDelete`). Statuses upstream ignores
//!   (`NormalizedEvaluate` of the CW neighbour, `DropPoint`, the curve `Evaluate`) are
//!   ignored, keeping [`SolidError::Unported`]; a failed `SER` returns its error.
//! - **`HasSeamProblem`** returns [`SeamCheck`]. Its stage-2 face properties branch
//!   returns `SM_SUCCESS` (`1000`) as its `SmBoolean`, so `has_problem` is `true` there
//!   whatever the properties found, as upstream (WR-176). A seam curve
//!   `CreateSeamCurve` does not make is [`SolidError::InvalidInput`] before any use,
//!   where upstream classifies the NULL curve (UB-253, #519); its periodicity flags are
//!   `bool`, so the port always builds the seam, as upstream does for `TRUE`.
//! - **Face tolerance.** `SmTol::GetZoneTol3d` of a face-owned surface is the face's
//!   `GetTolerance`, which the surface layer cannot reach (`topology.face_p2` stubs
//!   there). `FindVertexSector` passes the face's tolerance explicitly to `IsOnSeam`
//!   and to the pole search of `GetSingularities` / `IsSingularity`; the surface calls
//!   without such an argument (`IsClosed`, `IsPeriodic`, the line intersection) still
//!   return [`SolidError::Unported`] for a face-owned surface.
//! - **Seam curves.** Upstream's seam classifications own their seam curves. Here
//!   the U seam and its UV curve are the caller's temporaries 0 and 1, the V seam's 2
//!   and 3, and [`SeamCheck`] returns the curves (`seam_u`, `seam_v`). The copies
//!   `SetProps_Stage2` keeps in the face properties hold the handles only: a later
//!   caller that resolves their curves needs the curves of the `SeamCheck`.
//!   `SplitAtSeam`'s merges (`merge_seam_intervals`) rebuild them from the face's
//!   surface as this function builds them.
//! - **[`BrepSeamObjects`] as merge objects.** Over `&mut Brep`
//!   ([`BrepSeamObjects::for_merge`]) they are the [`MergeIntervalBrep`] of
//!   `SplitAtSeam`'s `MergeIntervals`: its edits notify into the objects' own empty
//!   records under [`crate::topology::loopuse`]'s identities (a split edge's new curve
//!   in a kind of its own), and a vertex/face gap is the vertex use's cached one,
//!   since the merge holds the kernel mutably.
//! - **`HasSeamProblem` and `SetProps_Stage2`** call each other upstream. Without stage 2
//!   face properties the port runs [`FaceProps::set_props_stage2_with`] with this
//!   function (and the caller's classification objects) as its seam call, passing the
//!   properties' closed flags (an `UNSURE` flag reads as `TRUE`) and the properties
//!   themselves, as `SetProps_Stage2` does. The seam curves are temporaries of the
//!   caller's handle type ([`ClassObject::temporary`]).
//!   Mutable healer callers carry explicit query state through both directions and
//!   retain the vertex/face and edge/face gap-cache updates, including error paths.
//! - **`sm_FaceClassifyUVLine` and `SmFace::SectorClassifyCurveOn`** call
//!   `query::face::p3`; the UV line's classification handles are `topology::face::p2`'s.
//! - **`ClassifyProblem`** is the co-port of `query.face_p1` (re-exported).
//! - **`GetLoopCrossings`** (`intersect.curve_class_p4`) returns its touch lists, which
//!   `HasSeamProblem` stores in the face properties' U or V lists; its status is ignored
//!   as upstream (an ignored error leaves both lists empty, as its own initialisation
//!   does).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{self, SmallTopoType};
use crate::geom::vector2d::Point2;
use crate::heal::face_props::{FaceProps, HealerOpType, LoopObject, PropStage, Tri};
use crate::intersect::curve_class::p1::{CrvDirOnFaceType, CurveClassification, CurveInterval};
use crate::intersect::curve_class::p5::{ClassGeometryMut, MergedTopology, PartialMergeError};
use crate::intersect::curve_class::p6::{MergeContext, MergeIntervalBrep};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p2::{
    curve_on_classify_mut, face_cache_tree, face_surface_cache, point_classify, BrepCacheFace,
    CurveOnClassifyBrep, CurveOnClassifyCache, CurveOnClassifyObjects, FaceBrep, FaceSurfaceCache,
    PointClassifyOptions, SnapshotObjects,
};
use crate::surface::surface::p5::{SS_NONE, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
use crate::surface::surface::p8::{FaceSolveCache, HeldSolveCache};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::SurfaceCache;
use crate::surface::trim_srf_cache::TrimSrfCache;
use crate::surface::volume::Volume;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3::{self as eu3, BinormalOptions};
use crate::topology::face::p1::SeamProblem;
use crate::topology::face::p2::{
    review_start_and_stop_on_seam_crv_classifies, UvLineHandle, UvLineObjects,
};
use crate::topology::loopuse::{self, AttributeFilter};
use crate::topology::vertex;
use crate::topology::vertexuse;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, LoopId, LoopuseId, VertexId, VertexuseId};

const OP: &str = "topology::face::p4";

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// Whether an error is one upstream's ignored status cannot hide: an unported callee, or
/// the port's report of a null dereference or dangling id.
const fn must_propagate(e: &SolidError) -> bool {
    matches!(
        e,
        SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }
    )
}

// ---- upward-use getters ----------------------------------------------------------------

/// `GetBrep`: the B-rep of the face's upward face use's shell, `None` while the face has
/// no face use (during construction or destruction) or its shell no B-rep.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a face
/// use without shell (an assert upstream, then `NULL`: reported instead).
pub fn brep(b: &Brep, f: FaceId) -> Result<Option<&Brep>> {
    let Some(fu) = b.face(f)?.faceuse else {
        // Can be null during destruction or during construction before being connected
        return Ok(None);
    };
    let Some(sh) = b.faceuse(fu)?.shell else {
        return Ok(None);
    };
    crate::topology::shell::shell_brep(b, sh)
}

/// The loop uses of the upward face use selected as [`eu1::faceuse_loopuses`] selects
/// them; none without face use.
fn selected_loopuses(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<LoopuseId>> {
    match b.face(f)?.faceuse {
        Some(fu) => eu1::faceuse_loopuses(b, fu, tgt_loops_only, tgt_loops),
        None => Ok(Vec::new()),
    }
}

/// `GetEdges`: the edges of each selected loop use ([`loopuse::edges`]: distinct within
/// the loop, in CCW order) appended loop by loop, so an edge two loops share is listed
/// twice; with `attribute` only those holding the attribute. None without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`] and [`loopuse::edges`];
/// [`SolidError::InvalidInput`] for an edge use without edge.
pub fn edges(
    b: &Brep,
    f: FaceId,
    attribute: Option<AttributeFilter<'_>>,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<EdgeId>> {
    // init output
    let mut out = Vec::new();

    // for every UpwardFaceuse->Loopuse
    for lu in selected_loopuses(b, f, tgt_loops_only, tgt_loops)? {
        // accumulate edges
        for e in loopuse::edges(b, lu, attribute)? {
            out.push(e.ok_or_else(|| null("GetEdges: edge use without edge"))?);
        }
    }
    Ok(out)
}

/// `GetVertices`: the vertices of each selected loop use ([`loopuse::vertices`]: the
/// vertex of a vertex loop use, else the distinct start vertices of its edge uses in
/// CCW order) appended loop by loop; with `attribute` only those holding the attribute.
/// None without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`] and [`loopuse::vertices`];
/// [`SolidError::InvalidInput`] for a vertex use without vertex.
pub fn vertices(
    b: &Brep,
    f: FaceId,
    attribute: Option<AttributeFilter<'_>>,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<VertexId>> {
    // init output
    let mut out = Vec::new();

    // for every UpwardLoopuse
    for lu in selected_loopuses(b, f, tgt_loops_only, tgt_loops)? {
        // accumulate vertices
        for v in loopuse::vertices(b, lu, attribute)? {
            out.push(v.ok_or_else(|| null("GetVertices: vertex use without vertex"))?);
        }
    }
    Ok(out)
}

/// `GetUpwardLoopuses`: the loop uses of the upward face use selected as
/// [`eu1::faceuse_loopuses`] selects them (all, the outer one, or the uses of
/// `tgt_loops` on this face use); none without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`].
pub fn upward_loopuses(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<LoopuseId>> {
    selected_loopuses(b, f, tgt_loops_only, tgt_loops)
}

/// `GetUpwardVertexuses`: the vertex uses of each selected loop use
/// ([`loopuse::vertexuses`]) appended loop by loop; none without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`] and [`loopuse::vertexuses`];
/// [`SolidError::InvalidInput`] for an edge use without vertex use.
pub fn upward_vertexuses(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<VertexuseId>> {
    // init output
    let mut out = Vec::new();

    // for every upwardFaceuse->Loopuse
    for lu in selected_loopuses(b, f, tgt_loops_only, tgt_loops)? {
        // accumulate vertices
        for vu in loopuse::vertexuses(b, lu, None)? {
            out.push(vu.ok_or_else(|| null("GetUpwardVertexuses: edge use without vertex use"))?);
        }
    }
    Ok(out)
}

/// `GetUpwardEdgeuses`: the edge uses of each selected loop use
/// ([`loopuse::edgeuses`], CCW order) appended loop by loop; none without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`] and [`loopuse::edgeuses`].
pub fn upward_edgeuses(
    b: &Brep,
    f: FaceId,
    tgt_loops_only: bool,
    tgt_loops: Option<&[LoopId]>,
) -> Result<Vec<EdgeuseId>> {
    // init output
    let mut out = Vec::new();

    // for every upwardFaceuse->Loopuse
    for lu in selected_loopuses(b, f, tgt_loops_only, tgt_loops)? {
        // accumulate edgeuses
        out.extend(loopuse::edgeuses(b, lu, None, None)?);
    }
    Ok(out)
}

// ---- connection ------------------------------------------------------------------------

/// The face of a face use.
fn faceuse_face(b: &Brep, fu: crate::topology::FaceuseId) -> Result<Option<FaceId>> {
    Ok(b.faceuse(fu)?.face)
}

/// `pFaceuse->GetShell()` of a face use upstream dereferences (`None` is the null face
/// use).
fn faceuse_shell(
    b: &Brep,
    fu: Option<crate::topology::FaceuseId>,
) -> Result<Option<crate::topology::ShellId>> {
    let fu = fu.ok_or_else(|| null("IsConnectedTo: face without face use"))?;
    Ok(b.faceuse(fu)?.shell)
}

/// `pFaceuse->GetShell()->GetRegion()`: both dereferenced upstream.
fn faceuse_region(
    b: &Brep,
    fu: Option<crate::topology::FaceuseId>,
) -> Result<Option<crate::topology::RegionId>> {
    let sh = faceuse_shell(b, fu)?.ok_or_else(|| null("IsConnectedTo: face use without shell"))?;
    Ok(b.shell(sh)?.region)
}

/// `IsConnectedTo`: whether the face connects to `target`: itself, one of its face uses,
/// a loop or loop use of it, a vertex or edge on it, a vertex use or edge use of one of
/// its face uses, the shell or region of either face use; `true` for `None` and any
/// other kind.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`vertex::faces`] and
/// [`edge::faces`]; [`SolidError::InvalidInput`] where upstream dereferences null: a
/// loop use without face use; for a shell target a missing face use it reaches (the
/// second only when the first's shell is not the target); for a region target also a
/// face use without shell.
pub fn is_connected_to(b: &Brep, f: FaceId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };

    // switch on cpConnectTgt type
    match target {
        EntityRef::Vertexuse(vu) => match vertexuse::faceuse(b, vu)? {
            Some(fu) => Ok(faceuse_face(b, fu)? == Some(f)),
            None => Ok(false),
        },
        EntityRef::Vertex(v) => Ok(vertex::faces(b, v)?.contains(&f)),
        EntityRef::Edgeuse(eu) => match eu1::faceuse(b, eu)? {
            Some(fu) => Ok(faceuse_face(b, fu)? == Some(f)),
            None => Ok(false),
        },
        EntityRef::Edge(e) => Ok(edge::faces(b, e, None)?.contains(&f)),
        EntityRef::Face(x) => Ok(x == f),
        EntityRef::Faceuse(fu) => Ok(faceuse_face(b, fu)? == Some(f)),
        // `pLoop->GetFace()`: through the loop use and its face use (the port's
        // `Loop::face` cache only where those links are missing)
        EntityRef::Loop(l) => {
            let cached = b.loop_(l)?.face;
            Ok(crate::topology::integrity::derived_loop_face(b, l).unwrap_or(cached) == f)
        }
        EntityRef::Loopuse(lu) => {
            let fu = b
                .loopuse(lu)?
                .faceuse
                .ok_or_else(|| null("IsConnectedTo: loop use without face use"))?;
            Ok(faceuse_face(b, fu)? == Some(f))
        }
        EntityRef::Shell(s) => {
            let (fu1, fu2) = super::p2::faceuses(b, f)?;
            Ok(faceuse_shell(b, fu1)? == Some(s) || faceuse_shell(b, fu2)? == Some(s))
        }
        EntityRef::Region(r) => {
            let (fu1, fu2) = super::p2::faceuses(b, f)?;
            Ok(faceuse_region(b, fu1)? == Some(r) || faceuse_region(b, fu2)? == Some(r))
        }
        // arrive here when cpConnectTgt is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

// ---- line intersection -----------------------------------------------------------------

/// The intervals of [`face_classify_uv_line`] (over `topology::face::p2`'s UV line
/// handles).
type UvLineIntervals = Vec<CurveInterval<UvLineHandle>>;

/// `sm_FaceClassifyUVLine(face, start, end, tol, classification)`: the face
/// classification of the UV line from `start` to `end`, parameterised over `[0, 1]`
/// (its intervals; the status is the error).
fn face_classify_uv_line(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    start: Point2,
    end: Point2,
    tol: f64,
) -> Result<UvLineIntervals> {
    let mut objects = UvLineObjects::new(b, f);
    let mut cc = CurveClassification::new(k);
    crate::query::face::p3::face_classify_uv_line_mut(
        k,
        f,
        &mut objects,
        start,
        end,
        tol,
        &mut cc,
    )?;
    Ok(cc.intervals().to_vec())
}

/// The class of the UV point `uv` of face `f` (`SmFace::PointClassify(uv, tol, true,
/// true, pc)` into a classification of tolerance `tol`, status ignored).
fn classify_uv_point(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    uv: Point2,
    tol: f64,
    trim_cache: Option<&mut TrimSrfCache<EntityRef>>,
) -> Result<PointClassType> {
    let mut pc = PointClassification::new(tol);
    let opts = PointClassifyOptions {
        src_zone_tol_3d: tol,
        do_boundary_intersections: true,
        use_uv_classification: true,
        trim_cache,
        ..PointClassifyOptions::default()
    };
    match point_classify(k, b, f, uv, &mut pc, opts) {
        Err(e) if must_propagate(&e) => Err(e),
        _ => Ok(pc.point_class()),
    }
}

/// `GlobalLineIntersect`: the intersections of the line `line_point + t * line_vector`
/// (bounded by `line_interval` when given) with the face: the solutions of the face's
/// surface over `uv_domain` (all of them, `bFireRay` off), keeping the single values whose
/// UV point classifies to the face and replacing each range by its subranges whose
/// middle classifies to the face (parameters interpolated linearly); with `fire_ray`
/// only the first. Each solution holds the line parameter then the surface `(u, v)`.
///
/// # Errors
///
/// Those of [`Surface::global_line_intersect`] (returned as upstream returns the
/// surface's status) and of the face classifications;
/// [`SolidError::InvalidInput`] for a face without surface (upstream dereferences null)
/// and for a range whose UV line classification has no interval.
///
/// [`Surface::global_line_intersect`]: crate::surface::surface::Surface::global_line_intersect
#[allow(clippy::too_many_arguments)]
pub fn global_line_intersect<O: SolutionObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: O,
    uv_domain: &Extent2d,
    line_point: Point3,
    line_vector: Vec3,
    line_interval: Option<&Extent1d>,
    fire_ray: bool,
    src_zone_tol_3d: f64,
) -> Result<SolutionArray<O>> {
    global_line_intersect_with(
        k,
        b,
        f,
        surface,
        None,
        uv_domain,
        line_point,
        line_vector,
        line_interval,
        fire_ray,
        src_zone_tol_3d,
    )
}

/// [`global_line_intersect`] with the face surface's cache kept by the caller in
/// `held` across calls, as upstream keeps it on the surface (see
/// [`Surface::global_line_intersect_held`]). `held` must only be used with this face,
/// whose surface the caller does not change between the calls, and the same `surface`
/// handle.
///
/// # Errors
///
/// As [`global_line_intersect`].
///
/// [`Surface::global_line_intersect_held`]: crate::surface::surface::Surface::global_line_intersect_held
#[allow(clippy::too_many_arguments)]
pub fn global_line_intersect_held<O: SolutionObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: O,
    held: &mut HeldSolveCache<O>,
    uv_domain: &Extent2d,
    line_point: Point3,
    line_vector: Vec3,
    line_interval: Option<&Extent1d>,
    fire_ray: bool,
    src_zone_tol_3d: f64,
) -> Result<SolutionArray<O>> {
    global_line_intersect_with(
        k,
        b,
        f,
        surface,
        Some(held),
        uv_domain,
        line_point,
        line_vector,
        line_interval,
        fire_ray,
        src_zone_tol_3d,
    )
}

/// The face's trimmed surface cache as its surface's line solve fetches it
/// (`smsurf_GetSurfaceCache(pSurface)`), named by `surface` in the solve.
struct FaceLineCache<'b, O> {
    b: &'b mut Brep,
    f: FaceId,
    surface: O,
    /// The face surface's cache, once fetched; given back to the surface after the
    /// solve.
    trim: Option<FaceSurfaceCache>,
}

impl<O: SolutionObject> FaceSolveCache<O> for FaceLineCache<'_, O> {
    fn build(&mut self, k: &Kernel) -> Result<SurfaceCache<O>> {
        // the face's trimmed cache (kept on the surface), its tree built (GetTree); the
        // surface needs a name in the cache's object space: after the build every
        // node's `mBA_pSurface` is the cache surface (the polar boxes of the leaf solves
        // are computed on it)
        let mut lease = face_surface_cache(k, self.b, self.f)?;
        let built = face_cache_tree(k, self.b, self.f, &mut lease.cache);
        let surface = self.surface;
        let solve_cache = built.and_then(|()| {
            let mut c = lease.cache.base().map_handles(|_| Ok(surface))?;
            c.set_surface_handle(Some(surface));
            Ok(c)
        });
        self.trim = Some(lease);
        solve_cache
    }

    fn point_test(&mut self, k: &Kernel, uv: Point2) -> Result<bool> {
        let trim = &self
            .trim
            .as_ref()
            .ok_or_else(|| null("GlobalLineIntersect: point test without cache"))?
            .cache;
        // PointTest(uv, ok, NULL, NULL): the face's zone tolerance
        let (ok, _) = trim.point_test_in(
            k,
            &mut BrepCacheFace {
                b: &mut *self.b,
                f: self.f,
            },
            uv,
            None,
        )?;
        Ok(ok)
    }
}

/// The body of [`global_line_intersect`] and [`global_line_intersect_held`].
#[allow(clippy::too_many_arguments)]
fn global_line_intersect_with<O: SolutionObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    surface: O,
    held: Option<&mut HeldSolveCache<O>>,
    uv_domain: &Extent2d,
    line_point: Point3,
    line_vector: Vec3,
    line_interval: Option<&Extent1d>,
    fire_ray: bool,
    src_zone_tol_3d: f64,
) -> Result<SolutionArray<O>> {
    // the double becomes the `SmZoneTol3d` parameter: `fabs`, and `SM_USE_DEFAULT` -> the
    // system zone tolerance
    let src_zone_tol_3d = tol::zone_tol3d(k, src_zone_tol_3d);

    // First find the solutions with the underlying surface
    // Note: pass False as the bFireRay argument, otherwise it will return
    // only the first hit in the surface, and that might not be in this Face.
    let face_surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| null("GlobalLineIntersect: face without surface"))?
        .clone_for_evaluation()?;
    // A surface with a face solves on the face's trimmed cache, kept on the surface:
    // a cache held by the caller stands only for a surface without face.
    let held = match held {
        Some(held) if face_surface.face()?.is_none() => Some(held),
        _ => None,
    };
    let mut solutions = if let Some(held) = held {
        face_surface.global_line_intersect_held(
            k,
            surface,
            held,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            false,
            src_zone_tol_3d,
        )?
    } else {
        let mut face_cache = FaceLineCache {
            b: &mut *b,
            f,
            surface,
            trim: None,
        };
        let solutions = face_surface.global_line_intersect_built(
            k,
            surface,
            &mut face_cache,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            false,
            src_zone_tol_3d,
        );
        // the cache goes back to the surface (PointClassify below finds it there)
        if let Some(lease) = face_cache.trim.take() {
            lease.release();
        }
        solutions?
    };

    // If there are no solutions then return whatever the surface method returns.
    if solutions.is_empty() {
        return Ok(solutions);
    }

    // - Don't actually remove elements within the loop,
    //   just keep track of which ones will be removed.
    // - When adding elements, add just after the current one: should be in order.
    let mut removal_indices: Vec<usize> = Vec::new();

    // Now remove solutions that are not active on face
    let mut ii = 0usize;
    while ii < solutions.len() {
        let sol = solutions
            .get(ii)
            .copied()
            .ok_or_else(|| null("GlobalLineIntersect: solution index"))?;

        if sol.solution_type == SolutionType::SingleValue {
            // Get uv on surface
            let uv = Point2::new(sol.start.parameters[1], sol.start.parameters[2]);
            if classify_uv_point(k, b, f, uv, src_zone_tol_3d, None)? == PointClassType::Unknown {
                removal_indices.push(ii);
                ii += 1;
                continue;
            }
        } else if sol.solution_type == SolutionType::RangeOfValues {
            // Classify the Surface Range against the Face.
            // Remove the Surface Range solution and replace with
            // every curve interval that classifies to the Face.
            let crv_prm0 = sol.start.parameters[0];
            let crv_prm1 = sol.end.parameters[0];
            let uv_start = Point2::new(sol.start.parameters[1], sol.start.parameters[2]);
            let uv_end = Point2::new(sol.end.parameters[1], sol.end.parameters[2]);
            let cc = match face_classify_uv_line(k, b, f, uv_start, uv_end, src_zone_tol_3d) {
                Ok(cc) => Some(cc),
                Err(e) if must_propagate(&e) => return Err(e),
                Err(_) => None,
            };

            if let Some(cc) = cc {
                // If only a single solution, then the entire segment classified
                // the same, so just check the midpoint of that segment.
                let num_segs = cc.len();
                if num_segs <= 1 {
                    let first = cc
                        .first()
                        .ok_or_else(|| null("GlobalLineIntersect: empty UV line classification"))?;
                    if first.mid.point_class() != PointClassType::Face {
                        removal_indices.push(ii);
                    }
                    ii += 1;
                    continue;
                }

                // We will remove this one, and possibly add subsets of it.
                removal_indices.push(ii);

                // Insert just after the current segment.
                let mut insert_idx = ii + 1;
                for ivl in &cc {
                    if ivl.mid.point_class() == PointClassType::Face {
                        // Copy the whole surface range solution,
                        // and adjust start and end parameters.
                        let mut s = sol;
                        let t0 = ivl.interval.min();
                        let t1 = ivl.interval.max();

                        s.start.parameters[0] = crv_prm0 + t0 * (crv_prm1 - crv_prm0);
                        s.end.parameters[0] = crv_prm0 + t1 * (crv_prm1 - crv_prm0);

                        // Since the line is linear, we can just interpolate linearly.
                        let uv = uv_start + t0 * (uv_end - uv_start);
                        s.start.parameters[1] = uv.x;
                        s.start.parameters[2] = uv.y;

                        let uv = uv_start + t1 * (uv_end - uv_start);
                        s.end.parameters[1] = uv.x;
                        s.end.parameters[2] = uv.y;

                        solutions.insert_at(insert_idx, &s, 1);
                        insert_idx += 1;

                        // But: don't check this subset (that we just added) again
                        // on the next iteration.
                        ii += 1;
                    }
                } // end for each CurveClassification within this Range solution.
            } // end if sm_FaceClassifyUVLine() succeeded.
        } // end if Range solution
        ii += 1;
    } // end for each solution

    // Now remove the indicated elements, the higher ones first.
    for &jj in removal_indices.iter().rev() {
        solutions.remove_at(jj, 1);
    }

    // Now, if indicated, return only the first valid solution if more than one.
    if fire_ray && solutions.len() > 1 {
        solutions.set_size(1);
    }
    Ok(solutions)
}

// ---- vertex sector ---------------------------------------------------------------------

/// `SmClassifyProblem` (`SmCurveClass.h`), co-ported by `query.face_p1`.
pub use crate::query::face::p1::ClassifyProblem;

/// The inputs of [`find_vertex_sector`] (`FindVertexSector`'s arguments).
#[derive(Debug, Clone, Copy)]
pub struct FindVertexSectorArgs<'a> {
    /// `cpVertex`: the face vertex the curve crosses.
    pub vertex: VertexId,
    /// `cr3DCurve`: the curve being classified (a 2D curve is lifted onto the face's
    /// surface).
    pub curve: &'a Curve,
    /// `cpUVCurve`: the optional associated UV curve.
    pub uv_curve: Option<&'a Curve>,
    /// `dCurveParameterAtVertex`: the curve parameter at the vertex crossing.
    pub param_at_vertex: f64,
    /// `dCurveParameterIvlEnd`: the limit of the curve parameter examined.
    pub param_ivl_end: f64,
    /// `eCurveOrientation`: [`OrientType::Same`] classifies the curve forwards from the
    /// vertex, [`OrientType::Opposite`] backwards.
    pub orientation: OrientType,
    /// `sZoneTol3d`: the zone tolerance of the curve's source object.
    pub zone_tol_3d: f64,
    /// `bValidateSector`: test every sector (`false`: take a single sector as is).
    pub validate_sector: bool,
    /// `bUVSpaceOkay` (upstream default `TRUE`): UV space classification may be tried.
    pub uv_space_okay: bool,
}

/// What [`find_vertex_sector`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VertexSector {
    /// `rpVUOfSector`: the vertex use (on the upward face use) whose sector holds the
    /// curve, `None` when the curve is not inside the face.
    pub vertexuse: Option<VertexuseId>,
    /// `rbPossibleProblem`: the answer cannot be trusted for important decisions.
    pub possible_problem: bool,
    /// `reClassifyProblem`: why, when [`VertexSector::possible_problem`].
    pub problem: ClassifyProblem,
}

/// `SmFace::SectorClassifyCurveOn(eu1, p1, tol1, SAME, eu2, p2, tol2, OPPOSITE, curve,
/// ivl, orient, tol, singular, uv, ...)`: `(inside, problem, cause)`.
#[allow(clippy::too_many_arguments)]
fn sector_classify_curve_on(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    eu1: EdgeuseId,
    eu1_param: f64,
    eu1_tol: f64,
    eu2: EdgeuseId,
    eu2_param: f64,
    eu2_tol: f64,
    curve: &Curve,
    interval: &Extent1d,
    orientation: OrientType,
    zone_tol_3d: f64,
    on_singularity: bool,
    singular_uv: Point2,
    uv_space_okay: bool,
) -> Result<(bool, bool, ClassifyProblem)> {
    use crate::query::face::p3::{self as qf3, SectorClassifyInput, SectorEdgeuse};
    let r = qf3::sector_classify_curve_on(
        k,
        b,
        f,
        &SectorClassifyInput {
            eu1: SectorEdgeuse {
                edgeuse: eu1,
                param: eu1_param,
                zone_tol_3d: eu1_tol,
                orientation: OrientType::Same,
            },
            eu2: SectorEdgeuse {
                edgeuse: eu2,
                param: eu2_param,
                zone_tol_3d: eu2_tol,
                orientation: OrientType::Opposite,
            },
            curve,
            curve_interval: *interval,
            curve_orientation: orientation,
            curve_zone_tol_3d: zone_tol_3d,
            on_singularity,
            singularity_uv: singular_uv,
            uv_space_okay,
        },
    )?;
    Ok((r.inside, r.possible_problem, r.problem))
}

/// `smos_Min(a, b)`: `a < b ? a : b`.
fn min_of(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `smos_Max(a, b)`: `a > b ? a : b`.
fn max_of(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `SmTol::GetZoneTol3d(GetSurface())`: the face's tolerance when the surface has a
/// face (a face's surface is owned by that face), else the system zone tolerance.
fn surface_zone_tol_3d(k: &Kernel, b: &Brep, f: FaceId, surface: &Surface) -> Result<f64> {
    if surface.face()?.is_some() {
        super::p2::tolerance(b, f)
    } else {
        Ok(tol::zone_tol_3d(k, SmallTopoType::NormalSize))
    }
}

/// `GetSingularities(&poles)` of the face's surface (status ignored, keeping
/// [`SolidError::Unported`]): the singular sides and the four pole points (undefined
/// where not singular). A face-owned surface takes its tolerance from the face.
fn surface_singularities(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    surface: &Surface,
) -> Result<(u64, [Point3; 4])> {
    let r = if surface.face()?.is_some() {
        surface.get_singularities_of_face(k, super::p2::tolerance(b, f)?, true, false)
    } else {
        surface.get_singularities(k, true, false)
    };
    match r {
        Ok(s) => Ok((
            u64::from(s.singular),
            s.pole_points.unwrap_or([Vec3::UNDEF; 4]),
        )),
        Err(e) if e.is_unported() => Err(e),
        // a failing evaluation inside it leaves its initial outputs (SM_SS_NONE)
        Err(_) => Ok((SS_NONE, [Vec3::UNDEF; 4])),
    }
}

/// `GetSurface()->IsSingularity(uv, rDir)` (defaults `SM_EFF_ZERO`, `FALSE`), with the
/// singular sides of a face-owned surface computed from the face's tolerance.
fn surface_is_singularity(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    surface: &Surface,
    uv: Point2,
) -> Result<(bool, SurfParamType)> {
    let s = if surface.face()?.is_some() {
        let (sides, poles) = surface_singularities(k, b, f, surface)?;
        surface.is_singularity_with_poles(k, uv, sides, Some(&poles), EFF_ZERO, false)?
    } else {
        surface.is_singularity(k, uv, EFF_ZERO, false)?
    };
    Ok((s.singular, s.direction))
}

/// `NormalizedEvaluate(t, TRUE, rPnt, NULL, bUVSpaceOkay)`'s UV point.
fn eu_uv_at(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    uv_space_okay: bool,
) -> Result<Point2> {
    let p = eu3::normalized_evaluate(k, b, eu, t, true, false, uv_space_okay)?.point;
    Ok(Point2::new(p.x, p.y))
}

/// The closure state `FindVertexSector` computes once: `IsClosed` of the face's UV
/// domain in each direction and the seam curves `CreateSeamCurve` builds.
struct Closure {
    closed_u: bool,
    closed_v: bool,
    seam_u: Option<Curve>,
    seam_v: Option<Curve>,
}

fn closure_of(k: &Kernel, surface: &Surface, uv_domain: &Extent2d) -> Result<Closure> {
    let closed_u = surface
        .is_closed(k, uv_domain, SurfParamType::U, None, false)?
        .0;
    let closed_v = surface
        .is_closed(k, uv_domain, SurfParamType::V, None, false)?
        .0;
    let seam_u = surface
        .create_seam_curve(k, SurfParamType::U, closed_u)?
        .map(Curve::from);
    let seam_v = surface
        .create_seam_curve(k, SurfParamType::V, closed_v)?
        .map(Curve::from);
    Ok(Closure {
        closed_u,
        closed_v,
        seam_u,
        seam_v,
    })
}

/// `pSeam->DropPoint(natural interval, point, NULL, tol, NULL, bSuccess, t, rdDist,
/// SM_SO_MINIMIZE)`'s distance (status ignored: `dist` stays as it was), upstream
/// dereferencing a null seam curve.
fn seam_distance(
    k: &Kernel,
    seam: Option<&Curve>,
    point: Point3,
    tol: f64,
    dist: f64,
) -> Result<f64> {
    let seam = seam.ok_or_else(|| null("FindVertexSector: closed surface without seam curve"))?;
    match seam.drop_point(
        k,
        &seam.natural_interval(),
        point,
        None,
        tol,
        None,
        SolverOperation::Minimize,
    ) {
        Ok(d) => Ok(d.distance),
        Err(e) if must_propagate(&e) => Err(e),
        Err(_) => Ok(dist),
    }
}

/// The face's surface, which upstream dereferences.
fn face_surface(b: &Brep, f: FaceId) -> Result<Surface> {
    b.face(f)?
        .surface
        .clone()
        .ok_or_else(|| null("FindVertexSector: face without surface"))
}

/// `FindVertexSector`: for a curve on the face that crosses face vertex
/// `args.vertex`, the sector of the upward face use's vertex uses at the vertex the
/// curve segment (from `args.param_at_vertex` towards `args.param_ivl_end` in
/// `args.orientation`) runs into; `None` when it is not inside the face. A single
/// sector is taken without work unless `args.validate_sector` (or it is a strut or a
/// loop vertex use); otherwise each sector is tested, skipping those on the other side
/// of a seam, and the curve is classified against its two edge uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a 2D
/// curve on a face without surface, a
/// vertex use without edge use, an edge use without edge or CW edge use, a closed
/// surface without seam curve, and a curve tangent upstream reads uninitialised; the
/// errors of the evaluations upstream checks (`SER`); [`SolidError::Unported`]
/// (module docs).
#[allow(clippy::too_many_lines)]
pub fn find_vertex_sector(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    args: FindVertexSectorArgs<'_>,
) -> Result<VertexSector> {
    let FindVertexSectorArgs {
        vertex: v,
        curve: curve_3d,
        uv_curve,
        param_at_vertex,
        param_ivl_end,
        orientation,
        zone_tol_3d,
        validate_sector,
        uv_space_okay,
    } = args;
    // the double becomes the `SmZoneTol3d` parameter: `fabs`, and `SM_USE_DEFAULT` -> the
    // system zone tolerance
    let zone_tol_3d = tol::zone_tol3d(k, zone_tol_3d);

    // work in 3d: when cr3DCurve's domain_dim == 2, use it as an SmCrvOnSurf object
    let lifted;
    let curve: &Curve = if curve_3d.dim() == 2 {
        let surface = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("FindVertexSector: face without surface"))?
            .clone_for_evaluation()?;
        lifted = Curve::from(CrvOnSurf::new(
            k,
            curve_3d.clone_for_evaluation()?,
            surface,
            None,
            0,
        )?);
        &lifted
    } else {
        curve_3d
    };

    // initialize output
    let mut out = VertexSector::default();

    // get curve interval to be classified, determined by CurveOrientation (SetMinMax's
    // status is ignored: a reversed pair leaves the natural interval)
    let natural = curve_3d.natural_interval();
    let mut ivl = natural;
    let _ = if orientation == OrientType::Same {
        ivl.set_min_max(param_at_vertex, min_of(param_ivl_end, natural.max()))
    } else {
        ivl.set_min_max(max_of(param_ivl_end, natural.min()), param_at_vertex)
    };

    // get upward faceuse and face domain
    let upward_fu = b.face(f)?.faceuse;
    let uv_domain = b.face(f)?.uv_domain;

    // let sTestVU = all Vertexuses that share the upward faceuse
    let mut test_vu: Vec<VertexuseId> = Vec::new();
    for vu in vertex::vertexuses(b, v)? {
        if vertexuse::faceuse(b, vu)? == upward_fu {
            test_vu.push(vu);
        }
    }

    // low work - no sectors on upward face
    let [first, ..] = test_vu[..] else {
        return Ok(out);
    };

    // low work: one sector on upward face and (no need to validate, or a strut, or a
    // loop vertexuse) - use single sector as the classification
    if test_vu.len() == 1
        && (!validate_sector
            || vertexuse::is_strut_vertexuse(b, first)?
            || vertexuse::is_loop_vertexuse(b, first)?)
    {
        out.vertexuse = Some(first);
        return Ok(out);
    }

    // get 3DCurve SurfaceUVPoint when given cpUVCurve
    let mut vertex_uv = Point2::new(0.0, 0.0);
    if let Some(uvc) = uv_curve {
        let p = uvc.evaluate_point(k, param_at_vertex)?;
        vertex_uv = Point2::new(p.x, p.y);
    }

    // get TestDist = 1/4 min_dimension(Face->UVdomain)
    let domain_size = uv_domain.size();
    let test_dist_uv = 0.25 * min_of(domain_size.x, domain_size.y);

    // locals for no UVTrimCurve inputs
    let mut checked_poles = false;
    let mut closure: Option<Closure> = None;
    let mut singularities = SS_NONE;
    let mut vertex_point = Point3::new(0.0, 0.0, 0.0);
    let mut pole_points = [Vec3::UNDEF; 4];

    // for every Vertex->Vertexuse connected to this Face->UpwardFaceuse
    for &vu in &test_vu {
        // locals
        let vertex_zone_tol_3d = vertex::tolerance(b, v)?;

        // low work: Vertex is a loopVertexuse - all curves map to this sector
        if vertexuse::is_loop_vertexuse(b, vu)? {
            out.vertexuse = Some(vu);
            return Ok(out);
        }

        // else Vertexuse is an EdgeVertexuse - get Edgeuse and its locals for this vertex
        let eu1 = vertexuse::edgeuse(b, vu)?
            .ok_or_else(|| null("FindVertexSector: vertex use without edge use"))?;
        let e1 = b
            .edgeuse(eu1)?
            .edge
            .ok_or_else(|| null("FindVertexSector: edge use without edge"))?;
        let eu1_same = b.edgeuse(eu1)?.orientation == OrientType::Same;
        let e1_interval = b.edge(e1)?.interval;
        let eu1_param = if eu1_same {
            e1_interval.min()
        } else {
            e1_interval.max()
        };
        let has_uv_trim_curve = eu1::uv_trim_curve(b, eu1)?.0.is_some();
        let mut on_singularity = false;
        let on_seam;
        let mut missing_seam_u = false;
        let mut missing_seam_v = false;
        let mut sing_dir = SurfParamType::Unknown;
        let mut dist_uv = 0.0;
        let mut eu1_uv = Point2::new(0.0, 0.0);

        // get UVDist to 3DCurve->UVPoint
        if has_uv_trim_curve {
            // get pEU1EndPoint->SurfaceUVPoint at edgeuse start
            eu1_uv = eu_uv_at(k, b, eu1, 0.0, uv_space_okay)?;

            // get UVDist to 3DCurve->UVPoint
            dist_uv = if uv_curve.is_some() {
                vertex_uv.distance(eu1_uv)
            } else {
                0.0
            };

            // If uv dist is large, check for closed Edge in 3d, running from seam to seam.
            if dist_uv > test_dist_uv {
                let surface = face_surface(b, f)?;
                let tol = surface_zone_tol_3d(k, b, f, &surface)?;
                if surface.is_on_seam(k, eu1_uv, Some(tol))?.on_seam {
                    // Evaluate at other end of pEU1.
                    let eu1_uv2 = eu_uv_at(k, b, eu1, 1.0, uv_space_okay)?;
                    let dist_uv2 = if uv_curve.is_some() {
                        vertex_uv.distance(eu1_uv2)
                    } else {
                        0.0
                    };
                    if dist_uv2 < test_dist_uv {
                        eu1_uv = eu1_uv2;
                        dist_uv = dist_uv2;
                    }
                }
            }

            // remember when Surface is singular at pEU1EndPoint->SurfaceUVPOINT
            let surface = face_surface(b, f)?;
            (on_singularity, sing_dir) = surface_is_singularity(k, b, f, &surface, eu1_uv)?;
            if on_singularity {
                out.possible_problem = true;
                out.problem = ClassifyProblem::VertexOnSingularity;
            }

            // look for broken Brep condition - sector includes a missing Seam
            let m = vertexuse::is_sector_missing_seam(k, b, vu, uv_space_okay)?;
            on_seam = m.missing;
            missing_seam_u = m.u;
            missing_seam_v = m.v;
        } else {
            // just find bIsOnSingularity and bIsOnSeam with 3d checks
            let mut min_pole_dist_sq = BIG_DOUBLE;
            // `sSurfDomain` is declared in this block upstream: only the pass that fetches
            // the poles sets it.
            let mut surf_domain = Extent2d::new();
            let surface = face_surface(b, f)?;

            // just once - get pole and vertex positions
            if !checked_poles {
                checked_poles = true;
                (singularities, pole_points) = surface_singularities(k, b, f, &surface)?;
                vertex_point = vertex::point(b, v)?;
                surf_domain = surface.natural_uv_domain_with_nurb(k)?;
            }

            // just once - get closed status
            if closure.is_none() {
                closure = Some(closure_of(k, &surface, &uv_domain)?);
            }

            // When the surface has poles - get 3d dist to poles
            let pole_sides = [
                (SS_UMIN, 0usize, 0.0, 0.5),
                (SS_VMIN, 1, 0.5, 0.0),
                (SS_UMAX, 2, 1.0, 0.5),
                (SS_VMAX, 3, 0.5, 1.0),
            ];
            for (side, i, nu, nv) in pole_sides {
                if singularities & side != 0 {
                    let pole = pole_points.get(i).copied().unwrap_or(Vec3::UNDEF);
                    let dist_sq = pole.distance_squared(vertex_point);
                    if min_pole_dist_sq > dist_sq {
                        min_pole_dist_sq = dist_sq;
                    }
                    eu1_uv = surf_domain.evaluate(nu, nv);
                }
            }

            // when 3dPoint is at pole
            if min_pole_dist_sq < zone_tol_3d * zone_tol_3d {
                on_singularity = true;
                out.possible_problem = true;
                out.problem = ClassifyProblem::VertexOnSingularity;
            }

            // when surface has seam - check point/Seam distance
            let c = closure
                .as_ref()
                .ok_or_else(|| null("FindVertexSector: closure state"))?;
            let xsect_tol_3d = tol::xsect_tol_3d_from_zones(k, zone_tol_3d, vertex_zone_tol_3d);
            let mut dist_u = 0.0;
            let mut dist_v = 0.0;
            if c.closed_u {
                dist_u = seam_distance(k, c.seam_u.as_ref(), vertex_point, xsect_tol_3d, dist_u)?;
            }
            if c.closed_v {
                dist_v = seam_distance(k, c.seam_v.as_ref(), vertex_point, xsect_tol_3d, dist_v)?;
            }
            on_seam =
                (c.closed_u && dist_u < xsect_tol_3d) || (c.closed_v && dist_v < xsect_tol_3d);
        } // end find bIsOnSingularity with 3d checks

        // get this loop's Edgeuse->neighbor and its param for this vertex
        let eu_cw = eu1::cw_edgeuse(b, eu1)?
            .ok_or_else(|| null("FindVertexSector: edge use without CW edge use"))?;
        let e_cw = b
            .edgeuse(eu_cw)?
            .edge
            .ok_or_else(|| null("FindVertexSector: CW edge use without edge"))?;
        let e_cw_interval = b.edge(e_cw)?.interval;
        let (eu_cw_param, eu_cw_t) = if b.edgeuse(eu_cw)?.orientation == OrientType::Same {
            (e_cw_interval.max(), 1.0)
        } else {
            (e_cw_interval.min(), 0.0)
        };
        // the neighbour's UV point (status ignored; only the debug drawing reads it)
        match eu3::normalized_evaluate(k, b, eu_cw, eu_cw_t, true, false, uv_space_okay) {
            Err(e) if must_propagate(&e) => return Err(e),
            _ => {}
        }

        // low work: vertex is a strut VertexUse - all curves map to this sector
        if eu1::radial(b, eu1)? == Some(eu_cw) {
            out.vertexuse = Some(vu);
            return Ok(out);
        }

        // when the 3DCurve and the vertexuse->Edgeuse both have UVTrimCurves
        if has_uv_trim_curve && uv_curve.is_some() {
            // skip vertices with different UVValues (they will only be face boundaries)
            // except for UV Values considered the same on Singular surfaces
            if uv_domain.is_point_on_boundary(eu1_uv, EFF_ZERO_SQRT).0
                && dist_uv > test_dist_uv
                && (!on_singularity
                    || (sing_dir == SurfParamType::V
                        && (vertex_uv.x - eu1_uv.x).abs() > 100.0 * EFF_ZERO_SQRT)
                    || (sing_dir == SurfParamType::U
                        && (vertex_uv.y - eu1_uv.y).abs() > 100.0 * EFF_ZERO_SQRT))
                && (!on_seam || (!missing_seam_u && !missing_seam_v))
            {
                continue;
            }
        } else if test_vu.len() > 1 {
            // skip vertex uses whose edgeuse normalXbinormal planes do not contain the
            // 3dCurve tangent
            if closure.is_none() {
                let surface = face_surface(b, f)?;
                closure = Some(closure_of(k, &surface, &uv_domain)?);
            }
            let c = closure
                .as_ref()
                .ok_or_else(|| null("FindVertexSector: closure state"))?;

            // when the face has seams - it's possible to look at the wrong vertexuse
            if c.closed_u || c.closed_v {
                // get associated edgeuse/face plane orientation in the tangent and binormal vectors
                let bn = eu3::evaluate_binormal(
                    k,
                    b,
                    eu1,
                    eu1_param,
                    !uv_space_okay,
                    &BinormalOptions::default(),
                )?;

                // get the curve tangent at the vertex parameter (status ignored upstream,
                // which then reads the tangent uninitialised)
                let pv = match curve.evaluate(k, param_at_vertex, 1, true, true) {
                    Ok(pv) => pv,
                    Err(e) if must_propagate(&e) => return Err(e),
                    Err(_) => {
                        return Err(null("FindVertexSector: curve tangent left uninitialised"))
                    }
                };
                let tangent = pv
                    .get(1)
                    .copied()
                    .ok_or_else(|| null("FindVertexSector: curve tangent left uninitialised"))?;

                // check the angle between the edgeuse/face plane and the curve tangent
                let normal = bn.tangent.cross(bn.binormal);

                // skip vertexuses associated with edgeuse/face planes that do not contain the 3dCurve tangent
                if !tangent.is_perpendicular_to(normal, 2.0) {
                    continue;
                }
            }
        }

        // find out if the curve is in or out of this face sector
        let (inside_face, my_problem, classify_problem) = sector_classify_curve_on(
            k,
            b,
            f,
            eu1,
            eu1_param,
            vertex_zone_tol_3d,
            eu_cw,
            eu_cw_param,
            vertex_zone_tol_3d,
            curve,
            &ivl,
            orientation,
            zone_tol_3d,
            on_singularity,
            eu1_uv,
            uv_space_okay,
        )?;

        // Pass on SectorClassifyCurveOn problem classification status
        if my_problem {
            out.possible_problem = true;
            out.problem = classify_problem;
        }

        if inside_face {
            // Yes we found the correct sector
            out.vertexuse = Some(vu);
            return Ok(out);
        }
    } // end iter every Vertexuse

    // If make it to here no valid sector found.
    Ok(out)
}

// ---- seam check ------------------------------------------------------------------------

/// Indices of the U seam curve's and its UV curve's temporary handles
/// ([`ClassObject::temporary`]).
const SEAM_U_TEMPS: (usize, usize) = (0, 1);
/// Indices of the V seam curve's and its UV curve's temporary handles.
const SEAM_V_TEMPS: (usize, usize) = (2, 3);

/// The caller's classification objects with the seam curve `HasSeamProblem` builds
/// (and its UV curve) as temporary curves.
struct WithSeam<'a, R, O> {
    inner: R,
    seam: Option<(O, &'a Curve)>,
    seam_uv: Option<(O, &'a Curve)>,
}

impl<R: core::ops::Deref, O: ClassObject> ClassGeometry<O> for WithSeam<'_, R, O>
where
    R::Target: ClassGeometry<O>,
{
    fn curve(&self, o: O) -> Option<&Curve> {
        for (h, c) in [self.seam, self.seam_uv].into_iter().flatten() {
            if h == o {
                return Some(c);
            }
        }
        self.inner.curve(o)
    }
    fn surface(&self, o: O) -> Option<&Surface> {
        self.inner.surface(o)
    }
    fn volume(&self, o: O) -> Option<&Volume> {
        self.inner.volume(o)
    }
    fn point(&self, o: O) -> Option<Point3> {
        self.inner.point(o)
    }
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        self.inner.classification_vertex_point(vertex)
    }
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        self.inner.topology_zone_tol_3d(object)
    }
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        self.inner.vertex_face_gap(vertex, surface, face)
    }
    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        self.inner.topology_entity(object)
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle(entity)
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<O> {
        self.inner.topology_handle_in(brep, entity)
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        self.inner.owner_of(owner)
    }
}

impl<R: core::ops::Deref, O: ClassObject> CurveOnClassifyObjects<O> for WithSeam<'_, R, O>
where
    R::Target: CurveOnClassifyObjects<O>,
{
    fn face(&self, f: FaceId) -> O {
        self.inner.face(f)
    }
    fn vertex(&self, v: VertexId) -> O {
        self.inner.vertex(v)
    }
    fn edge(&self, e: EdgeId) -> O {
        self.inner.edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> O {
        self.inner.vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> O {
        self.inner.edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> O {
        self.inner.edge_curve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> O {
        self.inner.trim_curve(eu)
    }
    fn face_surface(&self, f: FaceId) -> Option<O> {
        self.inner.face_surface(f)
    }
    fn loop_id(&self, tgt_loop: O) -> Option<LoopId> {
        self.inner.loop_id(tgt_loop)
    }
}

impl<R: core::ops::DerefMut, O: ClassObject> ClassGeometryMut<O> for WithSeam<'_, R, O>
where
    R::Target: ClassGeometryMut<O>,
{
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve> {
        if [self.seam, self.seam_uv]
            .into_iter()
            .flatten()
            .any(|(h, _)| h == o)
        {
            // the seam curves are the classification's own (not edited)
            return None;
        }
        self.inner.curve_mut(o)
    }
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        self.inner.topology_entity_mut(o)
    }
}

impl<R: core::ops::Deref, O> FaceBrep for WithSeam<'_, R, O>
where
    R::Target: FaceBrep,
{
    fn brep(&self) -> &Brep {
        self.inner.brep()
    }
}

impl<R: core::ops::DerefMut, O: ClassObject> CurveOnClassifyBrep<O> for WithSeam<'_, R, O>
where
    R::Target: CurveOnClassifyBrep<O>,
{
    fn brep_mut(&mut self) -> &mut Brep {
        self.inner.brep_mut()
    }
    fn classify_surface(&self, f: FaceId) -> Result<Option<&Surface>> {
        self.inner.classify_surface(f)
    }
    fn keeps_made_trim(&self, eu: EdgeuseId) -> Result<bool> {
        self.inner.keeps_made_trim(eu)
    }
}

impl<R: core::ops::DerefMut, O: ClassObject> MergeIntervalBrep<O> for WithSeam<'_, R, O>
where
    R::Target: MergeIntervalBrep<O>,
{
    fn merge_context(&mut self, object: O) -> Option<MergeContext<'_, O>> {
        self.inner.merge_context(object)
    }
    fn merge_context_with_curve_identity(
        &mut self,
        object: O,
    ) -> Option<crate::intersect::curve_class::p6::MergeContextWithIdentity<'_, O>> {
        self.inner.merge_context_with_curve_identity(object)
    }
}

/// `pCrvClass->MergeIntervals(&faces, &edges, &vertices, this)` of
/// `SmFace::SplitAtSeam` on face `f`'s own B-rep ([`BrepSeamObjects::for_merge`]), for
/// the `dir` seam classification [`has_seam_problem`] made: the merge's status with its
/// partial output ([`CurveClassification::merge_intervals_mut`]). `vertices_only` is
/// upstream's `bMergeVerticesOnly` (`FALSE` in `SplitAtSeam`, `TRUE` in
/// `SmHealData::Fix_SplitEdgesAtSeam`); the merge's edits are notified through `notify`
/// when given ([`BrepSeamObjects::for_merge_notified`]).
///
/// Upstream's classification owns its seam curve; the port's names it with a
/// temporary handle ([`ClassObject::temporary`]), so the seam curve and its UV curve
/// are rebuilt here from the face's surface exactly as [`has_seam_problem`] builds
/// them (`CreateSeamCurve(dir, TRUE)`). A surface that makes no seam curve leaves the
/// temporaries unresolved (an invalid-input error of the merge).
///
/// # Errors
///
/// Those of [`BrepSeamObjects::for_merge`] (the merge's own failures come back as the
/// inner result).
pub(crate) fn merge_seam_intervals<O: LoopObject>(
    k: &mut Kernel,
    b: &mut Brep,
    notify: Option<&mut edge::NotifyEnv<'_>>,
    f: FaceId,
    dir: SurfParamType,
    c: &mut CurveClassification<O>,
    vertices_only: bool,
) -> Result<core::result::Result<MergedTopology<O>, PartialMergeError<O>>> {
    // (a failure to rebuild the seam leaves it unresolved, as above)
    let seam = match b.face(f)?.surface.as_ref() {
        Some(s) => s.create_seam_curve(k, dir, true).ok().flatten(),
        None => None,
    };
    let seam = seam.map(|s| (s.uv_curve().clone(), Curve::from(s)));
    let (ci, ui) = if dir == SurfParamType::V {
        SEAM_V_TEMPS
    } else {
        SEAM_U_TEMPS
    };
    let mut inner = match notify {
        Some(n) => BrepSeamObjects::for_merge_notified(
            b,
            f,
            edge::NotifyEnv {
                records: &mut *n.records,
                attributes: &mut *n.attributes,
                context: n.context.as_deref_mut(),
                key_of: n.key_of,
            },
        )?,
        None => BrepSeamObjects::for_merge(b, f)?,
    };
    let mut objects = WithSeam {
        inner: &mut inner,
        seam: seam
            .as_ref()
            .and_then(|(_, c)| O::temporary(ci, ClassObjectKind::Curve).map(|h| (h, c))),
        seam_uv: seam
            .as_ref()
            .and_then(|(uv, _)| O::temporary(ui, ClassObjectKind::Curve).map(|h| (h, uv))),
    };
    Ok(c.merge_intervals_mut_with_partial(k, &mut objects, vertices_only))
}

/// The inputs of [`has_seam_problem`] after the face (`HasSeamProblem`'s optional
/// arguments).
pub struct HasSeamProblemOptions<'a, O: LoopObject> {
    /// `bOuterLoopOnly`: only classify against the outer loop (upstream default
    /// `FALSE`).
    pub outer_loop_only: bool,
    /// `pOptPeriodicU`: known U periodicity, `None` to test.
    pub periodic_u: Option<bool>,
    /// `pOptPeriodicV`: known V periodicity, `None` to test.
    pub periodic_v: Option<bool>,
    /// `pOptFaceProps`: pre-cached face properties (stage 2), `None` for a local set.
    pub face_props: Option<&'a mut FaceProps<O>>,
}

impl<O: LoopObject> Default for HasSeamProblemOptions<'_, O> {
    fn default() -> Self {
        Self {
            outer_loop_only: false,
            periodic_u: None,
            periodic_v: None,
            face_props: None,
        }
    }
}

/// What [`has_seam_problem`] found.
#[derive(Debug, Clone)]
pub struct SeamCheck<O> {
    /// The return value: a seam is crossed or missing.
    pub has_problem: bool,
    /// `rCrvClassU`: the U seam (constant `u`) against the face.
    pub class_u: CurveClassification<O>,
    /// `rCrvClassV`: the V seam against the face.
    pub class_v: CurveClassification<O>,
    /// `reCrossedSeam`, `reMissingSeam`, `reNearMissSeam`.
    pub problem: SeamProblem,
    /// The curves `class_u`'s temporaries 0 and 1 name (upstream's classification owns
    /// its seam curve); `None` when the U seam was not classified here.
    pub seam_u: Option<SeamCurves>,
    /// The curves `class_v`'s temporaries 2 and 3 name.
    pub seam_v: Option<SeamCurves>,
}

/// A seam curve [`has_seam_problem`] classified, and its UV curve.
#[derive(Debug, Clone)]
pub struct SeamCurves {
    /// The seam curve on the surface (`m_cpCurve`).
    pub curve: Curve,
    /// Its UV curve (`m_cpUVCurve1`).
    pub uv_curve: Curve,
}

/// `SmCurveClassification::GetLoopCrossings(loops, props, &counts,
/// &props->m_sRawTouchList<dir>, &props->m_sDoneTouchList<dir>)` (status ignored; the
/// counts are unused).
fn get_loop_crossings<O: LoopObject, G: ClassGeometry<O>>(
    k: &Kernel,
    geometry: &G,
    class: &CurveClassification<O>,
    loops: &[LoopId],
    props: &mut FaceProps<O>,
    dir: SurfParamType,
) -> Result<()> {
    let handles: Vec<O> = loops.iter().map(|&l| O::from_loop(l)).collect();
    let (raw, done) = match class.get_loop_crossings(k, geometry, &handles, Some(props)) {
        Ok(c) => (c.raw_touches, c.done_touches),
        Err(e) if must_propagate(&e) => return Err(e),
        Err(_) => (Vec::new(), Vec::new()),
    };
    if dir == SurfParamType::U {
        props.raw_touch_list_u = raw;
        props.done_touch_list_u = done;
    } else {
        props.raw_touch_list_v = raw;
        props.done_touch_list_v = done;
    }
    Ok(())
}

/// `SM_SP_BOTH` / `U` / `V` / `NEITHER` of two flags.
const fn sp_of(u: bool, v: bool) -> SurfParamType {
    match (u, v) {
        (true, true) => SurfParamType::Both,
        (true, false) => SurfParamType::U,
        (false, true) => SurfParamType::V,
        (false, false) => SurfParamType::Neither,
    }
}

/// Whether a classification is one interval with every point class unknown.
fn no_xsect<O: ClassObject>(c: &CurveClassification<O>) -> Result<bool> {
    if c.size() != 1 {
        return Ok(false);
    }
    let i = c.at(0)?;
    Ok(i.start.point_class() == PointClassType::Unknown
        && i.mid.point_class() == PointClassType::Unknown
        && i.end.point_class() == PointClassType::Unknown)
}

/// Marks the single interval of `c` as inside face `face` (`SetClassObject(SM_PC_FACE,
/// this)` at its start, middle and end).
fn mark_in_face<O: ClassObject>(c: &mut CurveClassification<O>, face: O) -> Result<()> {
    let i = c.at_mut(0)?;
    i.start.set_class_object(PointClassType::Face, Some(face));
    i.mid.set_class_object(PointClassType::Face, Some(face));
    i.end.set_class_object(PointClassType::Face, Some(face));
    Ok(())
}

/// The seam classification of one direction: the seam curve classified against the
/// face, the touch lists filled, false seam edge end classifications cleared;
/// `(classification, crossed, missing, near_miss)`.
#[allow(clippy::too_many_arguments)]
fn classify_seam<O: LoopObject, M: CurveOnClassifyBrep<O>>(
    k: &Kernel,
    f: FaceId,
    objects: &mut M,
    dir: SurfParamType,
    periodic: bool,
    surface_zone_tol_3d: f64,
    outer_loop_only: bool,
    loops: &[LoopId],
    props: &mut FaceProps<O>,
) -> Result<(CurveClassification<O>, bool, bool, bool, SeamCurves)> {
    // Build SeamConst curve
    let seam = objects
        .classify_surface(f)?
        .ok_or_else(|| null("HasSeamProblem: face without surface"))?
        .create_seam_curve(k, dir, periodic)?
        .ok_or_else(|| null("HasSeamProblem: periodic surface without seam curve"))?;
    let seam_uv = seam.uv_curve().clone();
    let interval = seam.natural_interval();
    let seam = Curve::from(seam);
    let temp = |i: usize| -> Result<O> {
        O::temporary(i, ClassObjectKind::Curve).ok_or_else(|| {
            null("HasSeamProblem: classification handles cannot name the seam curve")
        })
    };
    let (ci, ui) = if dir == SurfParamType::V {
        SEAM_V_TEMPS
    } else {
        SEAM_U_TEMPS
    };
    let (seam_h, seam_uv_h) = (temp(ci)?, temp(ui)?);
    let mut objects = WithSeam {
        inner: objects,
        seam: Some((seam_h, &seam)),
        seam_uv: Some((seam_uv_h, &seam_uv)),
    };

    // build Seam classification object (owning the curve, edge not modified)
    let mut class =
        CurveClassification::with_curve_interval(k, seam_h, interval, surface_zone_tol_3d);
    class.set_uv_curve1(Some(seam_uv_h));
    class.set_surface1(objects.face_surface(f));
    class.set_is_curve_owned(true);

    // inform the public - DoingSeams - user requested OuterSeamOnly value
    class.set_tgt_loops_only(outer_loop_only);
    class.set_ok_to_set_tolerance(false);

    // classify the SeamCurve3d against this face (status ignored)
    match curve_on_classify_mut(k, f, &mut objects, false, &mut class, true, false, false) {
        Err(e) if must_propagate(&e) => return Err(e),
        _ => {}
    }

    // How many times is the Seam crossed by loop
    get_loop_crossings(k, &objects, &class, loops, props, dir)?;

    // Find and Fix False SeamEdge EndPoint Classifications (status ignored)
    match review_start_and_stop_on_seam_crv_classifies(k, &objects, &mut class) {
        Err(e) if must_propagate(&e) => return Err(e),
        _ => {}
    }

    // analyze the classification for problems (Crossing or Missing)
    let crossed = class.is_crossed_by_edge(&objects)?; // only count seam crossings by an Edge
    let missing = class.is_crossing_face(Some(objects.face(f))); // segments in Face but not on an Edge
    let near_miss = class.has_near_misses(k, &objects, false)?;
    let curves = SeamCurves {
        curve: seam,
        uv_curve: seam_uv,
    };
    Ok((class, crossed, missing, near_miss, curves))
}

/// `HasSeamProblem`: whether the seams of the face's periodic surface are badly
/// represented: crossed by face edges without a vertex there, or not represented by
/// seam edges (allowing for edge/face gaps beyond tolerance). Each periodic direction's
/// seam curve is classified against the face (`class_u`, `class_v`). A whole closed
/// surface with only vertex loops has the unclassified parts of its seams in the face
/// (missing). On a doubly periodic surface a seam that does not meet any loop is missing
/// when the other seam starts in or enters the face (the torus case; crossed only when
/// that other seam touches an edge loop), and both are missing when neither meets a loop
/// and every closed edge loop runs clockwise in 3D. Without stage 2 face properties
/// (`opts.face_props` below `PropStage::Stage2A`), `SetProps_Stage2` runs instead
/// (which calls back here) and its results are returned with `has_problem` `true` (the
/// `SM_SUCCESS`, `1000`, upstream returns as `SmBoolean`).
///
/// `objects` names the face's topology and geometry in the classifications, as for
/// [`crate::query::face::p2::curve_on_classify`]; the seam curves are its temporaries 0 and 1
/// ([`ClassObject::temporary`]).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for
/// handles that cannot name temporaries, a periodic surface without seam curve and for
/// stage 2 properties without seam classifications (upstream dereferences them); those
/// of the periodicity tests, [`FaceProps::set_props_stage2`], the classification
/// predicates; [`SolidError::Unported`] (module docs).
#[allow(clippy::too_many_lines)]
pub fn has_seam_problem<O: LoopObject, M: CurveOnClassifyObjects<O>>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    objects: &M,
    opts: HasSeamProblemOptions<'_, O>,
) -> Result<SeamCheck<O>> {
    has_seam_problem_cached(k, b, f, objects, opts, &mut CurveOnClassifyCache::default())
}

/// Seam checking on the private copy of `b` that `cache` holds (the temporary
/// classification-copy deviation of `query::face::p2`). Sharing it across U and V
/// preserves lazy gap writes and sibling invalidations in native query order.
fn has_seam_problem_cached<O: LoopObject, M: CurveOnClassifyObjects<O>>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    objects: &M,
    opts: HasSeamProblemOptions<'_, O>,
    cache: &mut CurveOnClassifyCache,
) -> Result<SeamCheck<O>> {
    has_seam_problem_mut(
        k,
        f,
        &mut SnapshotObjects::new(objects, b, cache.snapshot(b)),
        opts,
    )
}

/// [`has_seam_problem`] on the face's own B-rep (`objects`' [`FaceBrep::brep`]): what
/// the seam classifications and the `SetProps_Stage2` they may run leave on the B-rep
/// (UV trim curves, the trimmed cache's tolerance growth, gap caches) stays there, made
/// in upstream's order, and the V seam reads the B-rep as the U seam left it.
///
/// # Errors
///
/// As [`has_seam_problem`].
#[allow(clippy::too_many_lines)]
pub fn has_seam_problem_mut<O: LoopObject, M: CurveOnClassifyBrep<O>>(
    k: &Kernel,
    f: FaceId,
    objects: &mut M,
    opts: HasSeamProblemOptions<'_, O>,
) -> Result<SeamCheck<O>> {
    let HasSeamProblemOptions {
        outer_loop_only,
        periodic_u: opt_periodic_u,
        periodic_v: opt_periodic_v,
        face_props,
    } = opts;

    // Init outputs
    let mut out = SeamCheck {
        has_problem: false,
        class_u: CurveClassification::new(k),
        class_v: CurveClassification::new(k),
        problem: SeamProblem {
            crossed: SurfParamType::Unknown,
            missing: SurfParamType::Unknown,
            near_miss: SurfParamType::Unknown,
        },
        seam_u: None,
        seam_v: None,
    };

    // Use the face's actual surface, as upstream does. A geometry copy drops its
    // owner and would make surface_zone_tol_3d use the system tolerance instead.
    // no work - no surface - return Okay
    let Some(surface) = objects.classify_surface(f)? else {
        return Ok(out);
    };

    // classify periodic surface and prepare for upcoming classifications
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let closed_c1 = |closed: Tri, val: ContinuityType| {
        closed != Tri::False && val as i32 >= ContinuityType::C1 as i32
    };
    let periodic_u = match (opt_periodic_u, face_props.as_deref()) {
        (Some(p), _) => p,
        (None, Some(fp)) => closed_c1(fp.closed_u, fp.closed_val_u),
        (None, None) => surface.is_periodic(k, &uv_domain, SurfParamType::U)?,
    };
    let periodic_v = match (opt_periodic_v, face_props.as_deref()) {
        (Some(p), _) => p,
        (None, Some(fp)) => closed_c1(fp.closed_v, fp.closed_val_v),
        (None, None) => surface.is_periodic(k, &uv_domain, SurfParamType::V)?,
    };

    // low work - Face->Surface is not periodic - return Okay
    if !periodic_u && !periodic_v {
        out.problem = SeamProblem {
            crossed: SurfParamType::Neither,
            missing: SurfParamType::Neither,
            near_miss: SurfParamType::Neither,
        };
        return Ok(out);
    }

    // locals
    let surface_zone_tol_3d = surface_zone_tol_3d(k, objects.brep(), f, surface)?;
    let mut crossed_u = false;
    let mut crossed_v = false;
    let mut missing_u = false;
    let mut missing_v = false;
    let mut near_miss_u = false;
    let mut near_miss_v = false;
    let loops = super::p2::loops(objects.brep(), f, false, None)?;

    // make sure there is a FaceProps that's at least running SmFaceProps::SetProps_Stage2
    let mut local_props;
    let props: &mut FaceProps<O> = if let Some(p) = face_props {
        p
    } else {
        local_props = FaceProps::new();
        &mut local_props
    };
    if props.prop_stage < PropStage::Stage2A {
        // run FaceProps->SetProps_Stage2: This is a recursive call after which
        // HasSeamProblem has already run
        let index = if props.face_index == crate::heal::face_props::UNDEF_ULONG {
            0
        } else {
            props.face_index
        };
        props.set_props_stage2_with(
            k,
            objects,
            f,
            index,
            HealerOpType::All,
            &mut |k, objects, f, cu, cv, p| {
                // HasSeamProblem(*m_pCrvClassU, *m_pCrvClassV, .., FALSE, &m_bClosedU,
                // &m_bClosedV, this): an UNSURE closed flag reads as TRUE
                let (closed_u, closed_v) = (p.closed_u != Tri::False, p.closed_v != Tri::False);
                let check = has_seam_problem_mut(
                    k,
                    f,
                    objects,
                    HasSeamProblemOptions {
                        outer_loop_only: false,
                        periodic_u: Some(closed_u),
                        periodic_v: Some(closed_v),
                        face_props: Some(p),
                    },
                )?;
                *cu = check.class_u;
                *cv = check.class_v;
                Ok(crate::heal::face_props::SeamProblem {
                    crossed: check.problem.crossed,
                    missing: check.problem.missing,
                    near_miss: check.problem.near_miss,
                })
            },
        )?;

        // set output and return
        out.class_u = props
            .crv_class_u
            .clone()
            .ok_or_else(|| null("HasSeamProblem: face properties without U seam classification"))?;
        out.class_v = props
            .crv_class_v
            .clone()
            .ok_or_else(|| null("HasSeamProblem: face properties without V seam classification"))?;
        out.problem = SeamProblem {
            crossed: props.bad_crossed_seam_dir,
            missing: props.bad_missing_seam_dir,
            near_miss: props.bad_near_miss_seam_dir,
        };

        // all done: upstream returns SM_SUCCESS (1000) as the SmBoolean result
        out.has_problem = true;
        return Ok(out);
    }

    // arrive here when HasSeamProblems() is called from SetProps_Stage2

    // periodic in U
    if periodic_u {
        let (class, crossed, missing, near_miss, curves) = classify_seam(
            k,
            f,
            objects,
            SurfParamType::U,
            periodic_u,
            surface_zone_tol_3d,
            outer_loop_only,
            &loops,
            props,
        )?;
        out.class_u = class;
        out.seam_u = Some(curves);
        (crossed_u, missing_u, near_miss_u) = (crossed, missing, near_miss);
    }

    // periodic in V
    if periodic_v {
        let (class, crossed, missing, near_miss, curves) = classify_seam(
            k,
            f,
            objects,
            SurfParamType::V,
            periodic_v,
            surface_zone_tol_3d,
            outer_loop_only,
            &loops,
            props,
        )?;
        out.class_v = class;
        out.seam_v = Some(curves);
        (crossed_v, missing_v, near_miss_v) = (crossed, missing, near_miss);
    }

    // With only isolated vertex loops, the whole closed surface is retained. Fill the
    // unknown portions of each seam, keeping existing vertex hits so insertion connects
    // to the original graph, including shared vertices.
    if super::p3::is_closed_surface_with_vertex_loops(k, objects.brep(), f, Some(&*props))? {
        let this = objects.face(f);
        let seams = [
            (&mut out.class_u, periodic_u),
            (&mut out.class_v, periodic_v),
        ];
        for (class, periodic) in seams {
            if !periodic {
                continue;
            }
            for interval in 0..class.size() {
                let ivl = class.at_mut(interval)?;
                for point in [&mut ivl.start, &mut ivl.mid, &mut ivl.end] {
                    if point.point_class() == PointClassType::Unknown {
                        point.set_class_object(PointClassType::Face, Some(this));
                    }
                }
            }
        }
        missing_u = periodic_u && out.class_u.is_crossing_face(Some(this));
        missing_v = periodic_v && out.class_v.is_crossing_face(Some(this));
    }

    // remember when SeamCrvU / SeamCrvV has no FaceLoop Bndry XSects
    let no_xsect_seam_u = no_xsect(&out.class_u)?;
    let no_xsect_seam_v = no_xsect(&out.class_v)?;

    // If Surface is doubly periodic, handle special torus case: an entire
    // NonIntersecting Seam may be Missing
    if periodic_u && periodic_v {
        let this = objects.face(f);

        // Both seam curves start at (Umin,Vmin). If a nonintersecting seam has this point
        // inside the face, its entire curve is inside. Coincident IN/ON/IN touches can
        // disappear from the processed crossing list.
        let starts_in_face = |class: &CurveClassification<O>| -> Result<bool> {
            Ok(class.size() > 0 && {
                let start = &class.at(0)?.start;
                start.point_class() == PointClassType::Face && start.object() == Some(this)
            })
        };
        let v_start_in_face = starts_in_face(&out.class_v)?;
        let u_start_in_face = starts_in_face(&out.class_u)?;

        // Point-only touches divide a seam but do not cross a face boundary. Keep them out
        // of the crossing flags used to request MoveSeam. Use the raw lists so coincident
        // edge touches remain visible after processing.
        let mut touches_boundary = [false, false];
        for (touches_seam, touches) in touches_boundary
            .iter_mut()
            .zip([&props.raw_touch_list_u, &props.raw_touch_list_v])
        {
            for touch in touches {
                let Some(o) = touch.connected_to else {
                    continue;
                };
                // (SmLoop*) cast
                let l = o
                    .as_loop()
                    .ok_or_else(|| null("HasSeamProblem: touch not connected to a loop"))?;
                let lu = objects
                    .brep()
                    .loop_(l)?
                    .loopuse
                    .ok_or_else(|| null("HasSeamProblem: loop without loop use"))?;
                if !loopuse::is_vertex_loopuse(objects.brep(), lu)? {
                    *touches_seam = true;
                    break;
                }
            }
        }

        // use the shared seam point or crossing list to detect a missing SeamU
        if no_xsect_seam_u
            && (v_start_in_face
                || props
                    .done_touch_list_v
                    .first()
                    .is_some_and(|t| t.loop_dir == CrvDirOnFaceType::In))
        {
            missing_u = true;
            mark_in_face(&mut out.class_u, this)?;

            // A boundary crossing can require relocating either seam. Isolated vertex
            // loops only require insertion through their existing vertices.
            if touches_boundary[1] {
                crossed_u = true;
                crossed_v = true;
            }
        }

        // use the shared seam point or crossing list to detect a missing SeamV
        if no_xsect_seam_v
            && (u_start_in_face
                || props
                    .done_touch_list_u
                    .first()
                    .is_some_and(|t| t.loop_dir == CrvDirOnFaceType::In))
        {
            missing_v = true;
            mark_in_face(&mut out.class_v, this)?;

            // A boundary crossing can require relocating either seam. Isolated vertex
            // loops only require insertion through their existing vertices.
            if touches_boundary[0] {
                crossed_u = true;
                crossed_v = true;
            }
        }
    }

    // If neither seam intersects a boundary on a doubly periodic surface, all closed
    // clockwise area loops describe holes in the retained face. Use the defining curves,
    // not stored loop labels or optional UV trims. Stage 3 loop containment is not
    // available while these Stage 2 properties are built.
    if periodic_u && periodic_v && no_xsect_seam_u && no_xsect_seam_v && !loops.is_empty() {
        let mut all_clockwise = true;
        for &l in &loops {
            // Isolated vertices do not enclose a retained patch. They have no measured
            // loop orientation and cannot disprove missing seams.
            let lu = objects
                .brep()
                .loop_(l)?
                .loopuse
                .ok_or_else(|| null("HasSeamProblem: loop without loop use"))?;
            if loopuse::is_vertex_loopuse(objects.brep(), lu)? {
                continue;
            }
            if !loopuse::is_closed_ptrs(objects.brep(), lu)? {
                all_clockwise = false;
                break;
            }
            // (a failing status is not clockwise)
            let orient = match super::p3::compute_loop_orientation_3d(k, objects.brep(), f, l, None)
            {
                Ok(o) => o,
                Err(e) if must_propagate(&e) => return Err(e),
                Err(_) => OrientType::Unknown,
            };
            if orient != OrientType::Opposite {
                all_clockwise = false;
                break;
            }
        }
        if all_clockwise {
            missing_u = true;
            missing_v = true;
            let this = objects.face(f);
            mark_in_face(&mut out.class_u, this)?;
            mark_in_face(&mut out.class_v, this)?;
        }
    }

    // Other nonintersecting cases remain unresolved. In particular, stored loop
    // order/orientation cannot replace geometric evidence for singly periodic surfaces
    // or ambiguous, degenerate, and mixed-orientation loops.

    out.problem = SeamProblem {
        crossed: sp_of(crossed_u, crossed_v),
        missing: sp_of(missing_u, missing_v),
        near_miss: sp_of(near_miss_u, near_miss_v),
    };
    out.has_problem = crossed_u || crossed_v || missing_u || missing_v;
    Ok(out)
}

// ---- B-rep classification handles ------------------------------------------------------

/// Operation name of [`BrepSeamObjects`].
const OP_SEAM_OBJECTS: &str = "topology::face::p4::BrepSeamObjects";

/// The classification objects of face `f`'s own B-rep for a [`has_seam_problem`] caller
/// that holds [`LoopObject`] handles only (`SmFaceProps::SetProps_Stage2`, the seam
/// checks of `SmFace::MoveSeam` / `SmFace::SplitAtSeam`): upstream's `SmObject` pointers
/// name the B-rep's own entities and geometry. The face, its surface, the B-rep
/// topology, edge curves and UV trim curves are the handles [`LoopObject::from_face`],
/// [`LoopObject::from_face_surface`], [`LoopObject::from_topology`],
/// [`LoopObject::from_edge_curve`] and [`LoopObject::from_trim_curve`] name (and their
/// inverses resolve); the seam curves are the handle type's temporaries
/// ([`ClassObject::temporary`]). Tolerances, points and vertex/face gaps are those
/// stored in `b` (a gap is computed on a snapshot, as `GetVertexFaceGap` fills a lazy
/// cache).
///
/// Over `&mut Brep` the objects are also the [`MergeIntervalBrep`] of
/// `SmFace::SplitAtSeam`'s `MergeIntervals` ([`BrepSeamObjects::for_merge`]): the
/// merge edits the face's own B-rep, with its notifications kept in the objects' own
/// (otherwise empty) records and attribute store.
#[derive(Debug)]
pub struct BrepSeamObjects<'b, O, B = &'b Brep> {
    /// The kernel of the vertex/face gap computation; `None` for the merge objects,
    /// whose merge holds the kernel mutably (a gap is then the vertex use's cached one).
    k: Option<&'b Kernel>,
    b: B,
    f: FaceId,
    face: O,
    surface: Option<O>,
    /// The notification records of the merge's edits (see [`MergeIntervalBrep`]).
    records: crate::topology::topology::NotifyRecords,
    /// The attribute store of the merge's edits.
    attributes: AttributeStore,
    /// The caller's notification environment of the merge's edits (its records,
    /// attributes, context callbacks and identities), when the caller tracks them
    /// ([`BrepSeamObjects::for_merge_notified`]); `None` keeps them in
    /// [`Self::records`] / [`Self::attributes`] without context callbacks.
    notify: Option<edge::NotifyEnv<'b>>,
}

impl<'b, O: LoopObject, B: FaceBrep> BrepSeamObjects<'b, O, B> {
    /// The objects of face `f` of `b`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling face; [`SolidError::InvalidInput`] when
    /// `O` cannot name the face, its surface, the B-rep topology, edge curves, trim
    /// curves or the seam temporaries (a limitation of the handle type, not an upstream
    /// status).
    pub fn new(k: &'b Kernel, b: B, f: FaceId) -> Result<Self> {
        Self::build(Some(k), b, f)
    }

    /// [`Self::new`] with an optional gap kernel.
    fn build(k: Option<&'b Kernel>, b: B, f: FaceId) -> Result<Self> {
        let no = || {
            SolidError::invalid_input(
                OP_SEAM_OBJECTS,
                "classification handles cannot name the face's topology, curves or seam curves",
            )
        };
        // a handle type names every entity of a kind or none (LoopObject), so the first
        // ids answer for all
        let e0 = EdgeId::from_index(0);
        let eu0 = EdgeuseId::from_index(0);
        let names_all = O::from_topology(EntityRef::Vertex(VertexId::from_index(0))).is_some()
            && O::from_topology(EntityRef::Edge(e0)).is_some()
            && O::from_topology(EntityRef::Vertexuse(VertexuseId::from_index(0))).is_some()
            && O::from_topology(EntityRef::Edgeuse(eu0)).is_some()
            && O::from_edge_curve(e0).is_some()
            && O::from_trim_curve(eu0).is_some()
            && [
                SEAM_U_TEMPS.0,
                SEAM_U_TEMPS.1,
                SEAM_V_TEMPS.0,
                SEAM_V_TEMPS.1,
            ]
            .iter()
            .all(|&i| O::temporary(i, ClassObjectKind::Curve).is_some());
        if !names_all {
            return Err(no());
        }
        let surface = if b.brep().face(f)?.surface.is_some() {
            Some(O::from_face_surface(f).ok_or_else(no)?)
        } else {
            None
        };
        Ok(Self {
            k,
            b,
            f,
            face: O::from_face(f).ok_or_else(no)?,
            surface,
            records: crate::topology::topology::NotifyRecords::new(),
            attributes: AttributeStore::default(),
            notify: None,
        })
    }

    /// The B-rep entity `o` names (the face handle included).
    pub(crate) fn entity(&self, o: O) -> Option<EntityRef> {
        if o == self.face {
            return Some(EntityRef::Face(self.f));
        }
        o.as_topology()
    }

    /// The handle of B-rep entity `t` ([`Self::new`] checked that `O` names topology, so
    /// the face handle fallback is never taken).
    fn topology(&self, t: EntityRef) -> O {
        match t {
            EntityRef::Face(f) if f == self.f => self.face,
            _ => O::from_topology(t).unwrap_or(self.face),
        }
    }
}

impl<O: LoopObject, B: FaceBrep> ClassGeometry<O> for BrepSeamObjects<'_, O, B> {
    fn curve(&self, o: O) -> Option<&Curve> {
        if let Some(e) = o.as_edge_curve() {
            return self.b.brep().edge(e).ok()?.curve.as_ref();
        }
        if let Some(eu) = o.as_trim_curve() {
            return eu1::uv_trim_curve(self.b.brep(), eu).ok()?.0;
        }
        None
    }

    fn surface(&self, o: O) -> Option<&Surface> {
        if self.surface == Some(o) {
            return self.b.brep().face(self.f).ok()?.surface.as_ref();
        }
        None
    }

    fn volume(&self, _o: O) -> Option<&Volume> {
        None
    }

    fn point(&self, o: O) -> Option<Point3> {
        self.classification_vertex_point(o)
    }

    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        match self.entity(vertex)? {
            EntityRef::Vertex(v) => self.b.brep().vertex(v).ok().map(|v| v.point),
            _ => None,
        }
    }

    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        match self.entity(object)? {
            EntityRef::Vertex(v) => self.b.brep().vertex(v).ok().map(|v| v.tolerance),
            EntityRef::Edge(e) => self.b.brep().edge(e).ok().map(|e| e.tolerance),
            EntityRef::Face(f) => self.b.brep().face(f).ok().map(|f| f.tolerance),
            _ => None,
        }
    }

    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        let EntityRef::Vertex(v) = self.entity(vertex)? else {
            return None;
        };
        let f = match face {
            Some(h) => match self.entity(h)? {
                EntityRef::Face(f) => f,
                _ => return None,
            },
            // the face whose surface `surface` is (`SmSurface::GetFace`)
            None => self
                .b
                .brep()
                .faces()
                .iter()
                .find(|(_, r)| r.surface.as_ref().is_some_and(|s| std::ptr::eq(s, surface)))
                .map(|(f, _)| f)?,
        };
        let vu = vertex::vertexuse_of_face(self.b.brep(), v, Some(f)).ok()??;
        let Some(k) = self.k else {
            // the merge objects: the vertex use's gap when there is none or it is
            // cached; an uncached gap is unresolved (the merge holds the kernel)
            if !vertexuse::has_vertex_face_gap(self.b.brep(), vu).ok()? {
                return Some(0.0);
            }
            let gap = self.b.brep().vertexuse(vu).ok()?.vertex_face_gap;
            return gap.is_init().then_some(gap.gap_3d);
        };
        let mut snapshot = self.b.brep().clone_sharing_surface_caches();
        let gap = vertexuse::vertex_face_gap(k, &mut snapshot, vu, false).ok()?;
        Some(gap.map_or(0.0, |g| g.gap_3d))
    }

    fn topology_entity(&self, object: O) -> Option<(&Brep, EntityRef)> {
        Some((self.b.brep(), self.entity(object)?))
    }

    fn topology_handle(&self, entity: EntityRef) -> Option<O> {
        Some(self.topology(entity))
    }
}

impl<O: LoopObject, B: FaceBrep> CurveOnClassifyObjects<O> for BrepSeamObjects<'_, O, B> {
    fn face(&self, f: FaceId) -> O {
        self.topology(EntityRef::Face(f))
    }

    fn vertex(&self, v: VertexId) -> O {
        self.topology(EntityRef::Vertex(v))
    }

    fn edge(&self, e: EdgeId) -> O {
        self.topology(EntityRef::Edge(e))
    }

    fn vertexuse(&self, vu: VertexuseId) -> O {
        self.topology(EntityRef::Vertexuse(vu))
    }

    fn edgeuse(&self, eu: EdgeuseId) -> O {
        self.topology(EntityRef::Edgeuse(eu))
    }

    fn edge_curve(&self, e: EdgeId) -> O {
        O::from_edge_curve(e).unwrap_or(self.face)
    }

    fn trim_curve(&self, eu: EdgeuseId) -> O {
        O::from_trim_curve(eu).unwrap_or(self.face)
    }

    fn face_surface(&self, f: FaceId) -> Option<O> {
        if f == self.f {
            self.surface
        } else {
            None
        }
    }

    fn loop_id(&self, tgt_loop: O) -> Option<LoopId> {
        tgt_loop.as_loop()
    }
}

/// `pFace->HasSeamProblem(..)` for a caller holding [`LoopObject`] handles only:
/// [`has_seam_problem`] with the face's own B-rep objects ([`BrepSeamObjects`]).
///
/// # Errors
///
/// Those of [`BrepSeamObjects::new`] and [`has_seam_problem`].
pub fn has_seam_problem_in_brep<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    opts: HasSeamProblemOptions<'_, O>,
) -> Result<SeamCheck<O>> {
    has_seam_problem_in_brep_cached(k, b, f, opts, &mut CurveOnClassifyCache::default())
}

impl<O: LoopObject> ClassGeometryMut<O> for BrepSeamObjects<'_, O, &mut Brep> {
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve> {
        match o.as_edge_curve() {
            Some(e) => self.b.edge_mut(e).ok()?.curve.as_mut(),
            None => None,
        }
    }
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        let e = self.entity(o)?;
        Some((&mut *self.b, e))
    }
}

impl<'b, O: LoopObject> BrepSeamObjects<'b, O, &'b mut Brep> {
    /// The objects of face `f` of `b` for `SmFace::SplitAtSeam`'s `MergeIntervals`
    /// ([`CurveClassification::merge_intervals_mut`]), which holds the kernel
    /// mutably: a vertex/face gap is the vertex use's cached one.
    ///
    /// # Errors
    ///
    /// As [`BrepSeamObjects::new`].
    pub fn for_merge(b: &'b mut Brep, f: FaceId) -> Result<Self> {
        Self::build(None, b, f)
    }

    /// [`Self::for_merge`] notifying the merge's edits through `notify` (upstream's
    /// `SmObject::Notify` reaches the context's system callback, e.g. a healer's
    /// `SmTrackTopologyChanges`), under `notify`'s identities.
    ///
    /// # Errors
    ///
    /// As [`BrepSeamObjects::new`].
    pub fn for_merge_notified(
        b: &'b mut Brep,
        f: FaceId,
        notify: edge::NotifyEnv<'b>,
    ) -> Result<Self> {
        let mut objects = Self::build(None, b, f)?;
        objects.notify = Some(notify);
        Ok(objects)
    }
}

/// The face's own B-rep for every topology handle: `MergeIntervals` edits the
/// entities upstream's `SmObject` pointers name. Its notifications go to the objects'
/// own records under `loopuse::notify_ref` identities (the B-rep carries no
/// attribute store or context callbacks), with a split edge's new curve named the
/// same way.
impl<O: LoopObject> MergeIntervalBrep<O> for BrepSeamObjects<'_, O, &mut Brep> {
    fn merge_context(&mut self, o: O) -> Option<MergeContext<'_, O>> {
        let target = self.entity(o)?;
        if !matches!(
            target,
            EntityRef::Vertex(_) | EntityRef::Edge(_) | EntityRef::Face(_) | EntityRef::Region(_)
        ) {
            return None;
        }
        let (f, face) = (self.f, self.face);
        Some(MergeContext {
            brep: &mut *self.b,
            target,
            handle: Box::new(move |e| match e {
                EntityRef::Face(x) if x == f => face,
                _ => O::from_topology(e).unwrap_or(face),
            }),
            notifications: match self.notify.as_mut() {
                Some(n) => edge::NotifyEnv {
                    records: &mut *n.records,
                    attributes: &mut *n.attributes,
                    context: n.context.as_deref_mut(),
                    key_of: n.key_of,
                },
                None => edge::NotifyEnv {
                    records: &mut self.records,
                    attributes: &mut self.attributes,
                    context: None,
                    key_of: &loopuse::notify_ref,
                },
            },
            curve_ref: &merge_curve_ref,
        })
    }
}

/// The identity of edge `e`'s curve of type `type_code` made by a seam merge: the
/// edge's [`loopuse::notify_ref`] key in a kind of its own.
fn merge_curve_ref(e: EdgeId, type_code: i64) -> ObjectRef {
    ObjectRef {
        type_code,
        key: ObjectKey(1 + (11_u64 << 32) + u64::from(e.index())),
    }
}

impl<O, B: FaceBrep> FaceBrep for BrepSeamObjects<'_, O, B> {
    fn brep(&self) -> &Brep {
        self.b.brep()
    }
}

impl<O: LoopObject> CurveOnClassifyBrep<O> for BrepSeamObjects<'_, O, &mut Brep> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.b
    }
}

/// [`has_seam_problem_in_brep`] on the face's B-rep in place
/// ([`has_seam_problem_mut`] over [`BrepSeamObjects`]): what the seam check leaves on
/// the B-rep stays there, as upstream's `const` `HasSeamProblem` leaves it.
///
/// # Errors
///
/// Those of [`BrepSeamObjects::new`] and [`has_seam_problem`].
pub fn has_seam_problem_in_brep_mut<O: LoopObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    opts: HasSeamProblemOptions<'_, O>,
) -> Result<SeamCheck<O>> {
    let mut objects = BrepSeamObjects::new(k, b, f)?;
    has_seam_problem_mut(k, f, &mut objects, opts)
}

/// The B-rep seam objects with caller-owned query state, for the mutable healer.
pub(crate) fn has_seam_problem_in_brep_cached<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    opts: HasSeamProblemOptions<'_, O>,
    cache: &mut CurveOnClassifyCache,
) -> Result<SeamCheck<O>> {
    let objects = BrepSeamObjects::new(k, b, f)?;
    has_seam_problem_cached(k, b, f, &objects, opts, cache)
}

// ---- AssertValid helpers (sm_GetLoopSampleUVPoint, sm_AssertTestFace37) -------------------

/// `sm_GetLoopSampleUVPoint(pLoop, rUVPoint)` (file static of `SmFace.cpp`): the UV
/// point at the middle (`NormalizedEvaluate(0.5, TRUE, ..)`) of the first edge use of
/// the loop that evaluates; `None` for a vertex loop and a loop whose edge uses all
/// fail (a missing UV trim curve is made, as upstream).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of the loop's edge-use walk;
/// [`SolidError::Unported`] from the evaluation (its other failures move on to the next
/// edge use, as upstream).
fn loop_sample_uv_point(k: &Kernel, b: &mut Brep, l: LoopId) -> Result<Option<Point2>> {
    let edgeuses = match b.loop_(l)?.loopuse {
        Some(lu) => eu1::loopuse_edgeuses(b, lu)?,
        None => Vec::new(),
    };
    for eu in edgeuses {
        match eu3::normalized_evaluate(k, b, eu, 0.5, true, false, true) {
            Ok(e) => return Ok(Some(Point2::new(e.point.x, e.point.y))),
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => (),
        }
    }

    // arrive here for VertexLoops and Loops with no usable UVTrimCurve
    Ok(None)
}

/// `sm_AssertTestFace37(pFace)` (`SmFace::AssertValid` test 37): whether every inner
/// loop of the face lies inside its outer loop (the first loop) and outside every other
/// inner loop, judged at one sample UV point per inner loop ([`loop_sample_uv_point`])
/// with `SmLoop::ContainsUVPoint` to the face tolerance mapped to UV. An inner loop
/// outside or on the boundary of the outer loop, or inside another inner loop, fails;
/// a face with fewer than two loops, an inner loop without a sample point and an
/// unknown containment pass.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a face
/// without surface that has a sampled inner loop (upstream dereferences null); those
/// of [`crate::query::loop::contains_uv_point`] and `MapTo2d`.
pub fn assert_test_face37(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    use crate::geom::point_set::PointContainment;
    use crate::query::r#loop::contains_uv_point;

    // locals
    let loops = super::p2::loops(b, f, false, None)?;
    let num_loops = loops.len();

    // no work - a Face with fewer than 2 Loops has no nesting to check
    if num_loops < 2 {
        return Ok(true);
    }

    // the OuterLoop is the first Loop in the Faceuse Loopuse list
    let Some(&outer_loop) = loops.first() else {
        return Ok(true);
    };

    let zone_tol_3d = super::p2::tolerance(b, f)?;

    // for every InnerLoop
    for (ii, &inner_loop) in loops.iter().enumerate().skip(1) {
        let Some(sample_uv) = loop_sample_uv_point(k, b, inner_loop)? else {
            continue; // inconclusive - nothing to classify
        };

        let uv_tol = {
            let surface = b.face(f)?.surface.as_ref().ok_or_else(|| {
                SolidError::invalid_input(OP, "AssertTestFace37: face without surface")
            })?;
            crate::surface::surface::p1::map_to_2d(k, zone_tol_3d, sample_uv, surface)?
        };

        // every InnerLoop must lie inside the OuterLoop
        let containment = contains_uv_point(k, b, outer_loop, None, sample_uv, uv_tol)?.containment;
        if containment == PointContainment::Outside || containment == PointContainment::OnBoundary {
            return Ok(false);
        }

        // no InnerLoop may lie inside another InnerLoop
        for (jj, &other) in loops.iter().enumerate().skip(1) {
            if jj == ii {
                continue;
            }
            if contains_uv_point(k, b, other, None, sample_uv, uv_tol)?.containment
                == PointContainment::Inside
            {
                return Ok(false);
            }
        }
    }

    // arrive here when all InnerLoops are properly nested
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vec3};
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;
    use crate::surface::plane::Plane;
    use crate::topology::{Faceuse, Loopuse, LoopuseStart, Vertexuse, VertexuseOwner};

    #[test]
    fn find_vertex_sector_lifts_uv_curve_on_plane_surface() {
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let face = b.add_face(shell).unwrap();
        b.face_mut(face).unwrap().surface =
            Some(Surface::from(Plane::from_normal(Point3::ZERO, Vec3::Z)));

        let faceuse = b
            .add_faceuse_record(Faceuse {
                face: Some(face),
                ..Faceuse::default()
            })
            .unwrap();
        b.face_mut(face).unwrap().faceuse = Some(faceuse);
        let loopuse = b
            .add_loopuse_record(Loopuse {
                faceuse: Some(faceuse),
                ..Loopuse::default()
            })
            .unwrap();
        b.faceuse_mut(faceuse).unwrap().loopuses.push(loopuse);
        let vertex = b.add_vertex(Point3::ZERO).unwrap();
        let vertexuse = b
            .add_vertexuse_record(Vertexuse {
                vertex: Some(vertex),
                owner: VertexuseOwner::Loopuse(Some(loopuse)),
                ..Vertexuse::default()
            })
            .unwrap();
        b.loopuse_mut(loopuse).unwrap().start = LoopuseStart::Vertexuse(Some(vertexuse));
        b.vertex_mut(vertex).unwrap().vertexuses.push(vertexuse);

        let mut uv_data = allocate_nurb_curve(2, 2, 5).unwrap();
        for (point, (u, v)) in
            uv_data
                .cpts_mut()
                .iter_mut()
                .zip([(0.1, 0.2), (0.5, 0.9), (0.9, 0.3)])
        {
            point.x = u;
            point.y = v;
            point.z = f64::MAX;
            point.w = -f64::MAX;
        }
        uv_data
            .knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        let uv_curve = Curve::from(BSplineCurve::from_nurb(uv_data, 2, false));
        let result = find_vertex_sector(
            &Kernel::default(),
            &mut b,
            face,
            FindVertexSectorArgs {
                vertex,
                curve: &uv_curve,
                uv_curve: None,
                param_at_vertex: 0.0,
                param_ivl_end: 1.0,
                orientation: OrientType::Same,
                zone_tol_3d: 0.0,
                validate_sector: false,
                uv_space_okay: true,
            },
        )
        .unwrap();

        assert_eq!(result.vertexuse, Some(vertexuse));
    }
}

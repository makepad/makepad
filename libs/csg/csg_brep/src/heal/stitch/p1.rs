//! Stitching coincident topology, part 1: the default keep/delete selection, gluing
//! paired vertices and the coincident edges between them, face-pair stitching,
//! consistent face orientation, and the `SM_API` stitch entry points.
//!
//! Upstream: `source/SMLib/src/SmStitch.cpp`, `source/SMLib/inc/SmStitch.h`,
//! `source/SM_API/src/SmApiBrep.cpp`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`StitchCallback`], [`DefaultStitchCallback`] | `SmStitchCallback` (constructor, destructor)  |
//! | [`StitchCallback::select_topology_to_keep`], [`select_topology_to_keep`] | `SmStitchCallback::SelectTopologyToKeep` |
//! | [`KeepChoice`]                               | `SelectTopologyToKeep`'s out-params            |
//! | [`Stitch`], [`Stitch::new`]                  | `SmStitch`, `SmStitch::SmStitch`               |
//! | `Drop` (implicit)                            | `SmStitch::~SmStitch`                          |
//! | [`Stitch::set_stitch_tol_3d`], [`Stitch::stitch_tol_3d`] | `SmStitch::SetStitchTol3d`, `m_dStitchTol3d` |
//! | [`check_interior_edge`]                      | `sm_CheckInteriorEdge` (file-static)           |
//! | [`test_and_split_edge`], [`SplitReason`], [`EdgeSplitTest`] | `sm_TestAndSplitEdge`, `sReasonType` (file-static) |
//! | [`Stitch::do_simple_face_stitch`], [`SimpleFaceStitch`] | `SmStitch::DoSimpleFaceStitch`      |
//! | [`Stitch::unify_normals`]                    | `SmStitch::UnifyNormals`                       |
//! | [`Stitch::stitch_vertex_pairs`], [`VertexPairs`] | `SmStitch::StitchVertexPairs` (two-list overload) |
//! | [`Stitch::stitch_edges_of_vertices`], [`EdgesOfVertices`] | `SmStitch::StitchEdgesOfVertices` |
//! | [`api_stitch`]                               | `SmApiStitch(SmBrep*)`                         |
//! | [`api_stitch_face`]                          | `SmApiStitch(SmBrep*, SmFace*)`                |
//! | [`api_stitch_into_solid`]                    | `SmApiStitchIntoSolid`                         |
//! | [`api_stitch_into_shell`]                    | `SmApiStitchIntoShell`                         |
//! | [`api_unify_normals`]                        | `SmApiUnifyNormals`                            |
//! | [`api_simple_face_stitch`]                   | `SmApiSimpleFaceStitch`                        |
//! | [`api_stitch_advanced`], [`AdvancedStitch`], [`AdvancedStitchOptions`] | `SmApiStitchAdvanced` |
//!
//! Deviations:
//! - Out-params become return values. The coincident objects of
//!   `SelectTopologyToKeep` are [`EntityRef`]s (upstream `SmTopology*`, never null
//!   here); a null `SmBrep*`/`SmFace*` of the API entry points is `None`.
//! - `SmStitch` holds `&mut dyn StitchCallback` (upstream a callback reference whose
//!   `SelectTopologyToKeep` is virtual); the default selection calls back through the
//!   callback for its edge pairs, as the virtual call does.
//! - Marks come from [`with_mark_lock`] (`SmNewMarkAndLock`) and live in a
//!   [`Marks`] record local to the call (upstream the objects' mark members; nothing
//!   reads them after the call).
//! - `SelectTopologyToKeep` reads `SurfaceMaxDistanceBetween`'s gap even when the call
//!   fails (upstream status ignored): the previous gap, as upstream's reused local.
//!   Upstream reads an uninitialised local when the very first call fails; the port
//!   returns [`SolidError::Upstream`] `SM_ERR` there.
//! - `test_and_split_edge` records the live edge's zone tolerance on its curve for
//!   the drop's cache, which cannot dereference the owning B-rep. The independent
//!   stitch/snap tolerance is unchanged; no curve copy or extra arena lookup is
//!   needed.
//! - `StitchVertexPairs` indexes its sorted index arrays with the unsorted counts, which
//!   reads past them upstream when a list holds null entries; the port returns
//!   [`SolidError::InvalidInput`] there. Its rescan of a vertex after gluing a closer
//!   one is bounded by `4 * (n1 + n2) + 16` passes (each glue empties a slot, so
//!   upstream needs at most `n1 + 2 * n2`).
//! - Legacy `SmVertex::SetPoint` and `SmBrep::MakeVertexSplitEdge` notifications run on
//!   record sets local to the call (no attributes are attached by the stitch). The
//!   split's refusal (`SM_ERR` with the nearby endpoint) is reported as `SM_ERR`, the
//!   only distinction its caller makes. Contextual stitching instead borrows the
//!   caller's actual notification state; an unregistered split curve is refused
//!   at the real identity callback, retaining preceding mutations.
//! - Uninitialised reads upstream: `DoSimpleFaceStitch` ignores `CurveMaxDistanceBetween`'s
//!   status (`SE`) and reads the unset `dDistToCurve` after a failure (the port reads 0).
//! - `UnifyNormals` ignores `NormalizedEvaluate`'s status and goes on with `sPnt` / `sDir`
//!   as they were: `SmVector3d`'s default `SM_UNDEF_DOUBLE` (defined; the 3D evaluation
//!   writes them only after its last status), [`Point3::UNDEF`] / [`Vec3::UNDEF`] here.
//! - `SmBrep::GlueVertices`, `RemoveFaces` (with the B-rep's own editing flag) and
//!   `StitchAndOrient` call `topology.brep_p5`, which defers at its own barriers;
//!   `GlueEdgesGeneral` calls `topology.brep_p7` on a default record per arena object
//!   (`brep_p2::ArenaNotifyState`) and the kernel's notification context (it defers at
//!   `SmBrep::DeleteEdge`, `topology.brep_p4`, for a wire edge, else at part 7's own
//!   first unported callee).
//! - The calls into `SqueezeEdge` (`topology.brep_p3`) and `CalculateBoundingBox`
//!   (`query.brep`) return [`SolidError::Unported`]. `SmApiStitch(pBrep, pFace)` builds
//!   its `SmMerge` with `boolean.merge_p2`'s [`MergeState::with_breps`], whose computed
//!   tolerance fails on the null other B-rep where upstream crashes (UB-167), so its
//!   `MergeAddedFace` is never reached. The API entry points call `DoStitching`,
//!   `StitchIntoSolid` and `StitchIntoShell` in [`super::p2`]. `SmBrep::Notify(SM_NO_PRE_EDIT)`
//!   is `topology::brep::p2::notify` on an empty notify state (no B-rep cache, no
//!   attribute state on the heal path).

use crate::boolean::merge::p1::MergeState;
use crate::boolean::merge::p2::MergeOperand;
use crate::boolean::topology_intersector::p1::TopologyIntersector;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, EntityRef, SM_ERR};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::context::{with_mark_lock, Context, ALL_MARKS};
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol::{self, TopologyToUpdate};
use crate::geom::tree::{NodeGeomType, Tree};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::topology::brep::p1 as bp1;
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3::{self as eu3, BinormalOptions};
use crate::topology::face::p1 as fp1;
use crate::topology::face::p2 as fp2;
#[cfg(test)]
use crate::topology::topology::Topology;
use crate::topology::topology_traverser::{Entity, Marks};
use crate::topology::{
    edge, vertex, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, RegionId, VertexId,
};
use crate::{Result, SolidError};

const OP: &str = "heal::stitch::p1";
/// The operation of the `SM_API` entry points' `SM_ERR_INVALID_INPUT` (null arguments).
const API_OP: &str = "heal::stitch::p1::api";

/// `smos_Max` of two doubles (`a > b ? a : b`).
fn smos_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min` of two doubles (`a < b ? a : b`).
fn smos_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// Upstream `NER(ptr)` on a null pointer: `SM_ERR_NULL_POINTER`.
fn ner(what: &'static str) -> SolidError {
    let _ = what;
    SolidError::null_pointer(OP)
}

/// A null pointer upstream dereferences (undefined behaviour there).
fn deref(what: &'static str) -> SolidError {
    SolidError::invalid_input(OP, what)
}

fn sm_err() -> SolidError {
    SolidError::Upstream {
        code: SM_ERR,
        operation: OP,
        entity: None,
    }
}

/// The edge's curve; upstream dereferences a null curve pointer.
fn curve_of(b: &Brep, e: EdgeId) -> Result<&Curve> {
    edge::curve(b, e)?.ok_or_else(|| deref("edge without curve"))
}

/// The edge's (non-null) vertex `GetVertex` / `GetOtherVertex` result.
fn vertex_of(v: Option<VertexId>) -> Result<VertexId> {
    v.ok_or_else(|| deref("edge without vertex"))
}

// ---- the stitch callback -------------------------------------------------------------

/// Which of two coincident objects to keep (the out-params of
/// `SelectTopologyToKeep`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeepChoice {
    /// Max distance found between the two objects (`rdMaxDistance`).
    pub max_distance: f64,
    /// `TRUE` = keep the first object, `FALSE` = keep the second (`rbKeepElement1`).
    pub keep_first: bool,
}

/// Decides which of two coincident objects being stitched is kept
/// (`SmStitchCallback`). Implementors may override the selection; the default is
/// [`select_topology_to_keep`].
pub trait StitchCallback {
    /// Which of the coincident `first` and `second` (vertices or edges) to keep, given
    /// the max distance `dist_between` between them (`SelectTopologyToKeep`).
    ///
    /// # Errors
    ///
    /// Those of [`select_topology_to_keep`].
    fn select_topology_to_keep(
        &mut self,
        k: &Kernel,
        b: &Brep,
        first: EntityRef,
        second: EntityRef,
        dist_between: f64,
    ) -> Result<KeepChoice> {
        select_topology_to_keep(self, k, b, first, second, dist_between)
    }
}

/// The default stitching callback (`SmStitchCallback` itself).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultStitchCallback;

impl DefaultStitchCallback {
    /// `SmStitchCallback()`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl StitchCallback for DefaultStitchCallback {}

/// `SmStitchCallback::SelectTopologyToKeep`: two vertices keep the one with the most
/// edges (counting, when they are more than ten B-rep tolerances apart, the lamina
/// edges each would keep of the coincident lamina edge pairs, through `callback`); two
/// edges keep the one closer to the other's faces when their gap exceeds a third of
/// their summed tolerances, else the one with more faces, else the curve with fewer
/// knots. Any other object keeps `first`.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` when `first` is a vertex (an edge) and `second` is not;
/// those of the curve distance measures (`SER`) and of `callback`; a null B-rep, curve
/// or surface pointer upstream dereferences.
pub fn select_topology_to_keep<C: StitchCallback + ?Sized>(
    callback: &mut C,
    k: &Kernel,
    b: &Brep,
    first: EntityRef,
    second: EntityRef,
    dist_between: f64,
) -> Result<KeepChoice> {
    // init output
    let mut out = KeepChoice {
        max_distance: dist_between,
        keep_first: true,
    };

    // Keep vertex which has the most selected edges.
    if let EntityRef::Vertex(vertex1) = first {
        // 2nd element is expected to be a vertex
        let EntityRef::Vertex(vertex2) = second else {
            return Err(ner("second element is not a vertex"));
        };

        // get edges connected to each vertex
        let edges1 = vertex::edges(b, vertex1, None)?;
        let mut count1: u64 = 0;
        let edges2 = vertex::edges(b, vertex2, None)?;
        let mut count2: u64 = 0;

        // get Brep
        let brep = vertex::brep(b, vertex1)?.ok_or_else(|| deref("vertex without B-rep"))?;

        // when distance between vertices is much larger than Brep Tolerance
        if dist_between > brep.tolerance() * 10.0 {
            // for every v1 edge
            for &e1 in &edges1 {
                if edge::is_lamina(b, e1)? {
                    // for every v2->edge
                    for &e2 in &edges2 {
                        // skip edges between v1 and v2
                        if e2 == e1 {
                            continue;
                        }
                        // skip edges which don't connect to common vertices
                        if edge::other_vertex(b, e2, Some(vertex2))?
                            != edge::other_vertex(b, e1, Some(vertex1))?
                        {
                            continue;
                        }

                        // get curve/curve max distance
                        let ivl2 = edge::interval(b, e2)?;
                        let tol1 = edge::tolerance(b, e1)?;
                        let dist_to_curve = curve_of(b, e1)?
                            .curve_max_distance_between(
                                k,
                                &edge::interval(b, e1)?,
                                curve_of(b, e2)?,
                                ivl2.min(),
                                ivl2.max(),
                                10,
                                Some(tol1),
                            )?
                            .distance;

                        // skip coincident curves
                        if dist_to_curve < tol1 / 1000.0 {
                            continue;
                        }
                        // skip curves more than tol apart; they don't get stitched
                        if dist_to_curve > tol1 {
                            continue;
                        }

                        // count up the vertex with the most edges to keep
                        let c = callback.select_topology_to_keep(
                            k,
                            b,
                            EntityRef::Edge(e1),
                            EntityRef::Edge(e2),
                            dist_to_curve,
                        )?;
                        if c.keep_first {
                            count1 += 1;
                        }
                    }
                } else if !edge::is_wire(b, e1)? {
                    // count all nonWire-nonLamina edges
                    count1 += 1;
                }
            }

            // for every v2->edge - count all nonWire-nonLamina edges and preferred Lamina
            // Edges
            for &e2 in &edges2 {
                if edge::is_lamina(b, e2)? {
                    for &e1 in &edges1 {
                        // skip edges between v1 and v2
                        if e2 == e1 {
                            continue;
                        }
                        // skip edges which don't connect to common vertices
                        if edge::other_vertex(b, e2, Some(vertex2))?
                            != edge::other_vertex(b, e1, Some(vertex1))?
                        {
                            continue;
                        }

                        // get curve/curve max distance
                        let ivl2 = edge::interval(b, e2)?;
                        let tol1 = edge::tolerance(b, e1)?;
                        let dist_to_curve = curve_of(b, e1)?
                            .curve_max_distance_between(
                                k,
                                &edge::interval(b, e1)?,
                                curve_of(b, e2)?,
                                ivl2.min(),
                                ivl2.max(),
                                10,
                                Some(tol1),
                            )?
                            .distance;

                        // skip coincident curves
                        if dist_to_curve < tol1 / 1000.0 {
                            continue;
                        }
                        // skip curves more than tol apart; they don't get stitched
                        if dist_to_curve > tol1 {
                            continue;
                        }

                        // count up the vertex with the most edges to keep (upstream passes
                        // twice the vertex distance here, not the curve distance)
                        let c = callback.select_topology_to_keep(
                            k,
                            b,
                            EntityRef::Edge(e1),
                            EntityRef::Edge(e2),
                            dist_between * 2.0,
                        )?;
                        if !c.keep_first {
                            count2 += 1;
                        }
                    }
                } else if !edge::is_wire(b, e2)? {
                    // count all nonWire-nonLamina edges
                    count2 += 1;
                }
            }

            // keep the vertex with the highest keep-edge count
            if count2 > count1 {
                out.keep_first = false;
            }
        } else if edges2.len() > edges1.len() {
            // vertices are within 10*BrepTol: keep the vertex with the most edges
            out.keep_first = false;
        }

        // all done
        return Ok(out);
    }

    // Keep the edge with the most faces or the least number of knots.
    if let EntityRef::Edge(edge1) = first {
        // 2nd element is expected to be an edge
        let EntityRef::Edge(edge2) = second else {
            return Err(ner("second element is not an edge"));
        };

        let faces1 = edge::faces(b, edge1, None)?;
        let faces2 = edge::faces(b, edge2, None)?;

        // When edge tolerances are smaller than the specified max gap size check
        // distances between edges and otherEdge->faces to see if they can be stitched
        if dist_between > (edge::tolerance(b, edge1)? + edge::tolerance(b, edge2)?) / 3.0 {
            let mut e1_to_e2_faces = 0.0;
            let mut e2_to_e1_faces = 0.0;
            // upstream's `dMaxDistToSurface`, one local for both loops (None: not set yet)
            let mut max_to_surface: Option<f64> = None;

            // Find the distance from pEdge1->Curve to each face in sEFaces2.
            for &face2 in &faces2 {
                // (upstream reads the surface pointer first but dereferences it only in the
                // distance call, after breaking on a null curve)
                let Some(curve1) = edge::curve(b, edge1)? else {
                    break;
                };
                let surface2 =
                    fp2::surface(b, face2)?.ok_or_else(|| deref("face without surface"))?;
                // 0 samples: do a precise measurement; no max distance (status ignored)
                if let Some(g) = ignored(curve1.surface_max_distance_between(
                    k,
                    &edge::interval(b, edge1)?,
                    surface2,
                    &fp2::uv_domain(b, face2)?,
                    0,
                    None,
                ))? {
                    max_to_surface = Some(g.max_distance);
                }
                let d = max_to_surface.ok_or_else(sm_err)?;

                // remember the largest edge->Curve otherEdge->Face->Surface drop distance
                if d > e1_to_e2_faces {
                    e1_to_e2_faces = d;
                }
            }

            // Find the distance from pEdge2->Curve to each face in sEFaces1.
            for &face1 in &faces1 {
                // (upstream reads the surface pointer first but dereferences it only in the
                // distance call, after breaking on a null curve)
                let Some(curve2) = edge::curve(b, edge2)? else {
                    break;
                };
                let surface1 =
                    fp2::surface(b, face1)?.ok_or_else(|| deref("face without surface"))?;
                if let Some(g) = ignored(curve2.surface_max_distance_between(
                    k,
                    &edge::interval(b, edge2)?,
                    surface1,
                    &fp2::uv_domain(b, face1)?,
                    0,
                    Some(e1_to_e2_faces),
                ))? {
                    max_to_surface = Some(g.max_distance);
                }
                let d = max_to_surface.ok_or_else(sm_err)?;

                // remember the largest edge->Curve otherEdge->Face->Surface drop distance
                if d > e2_to_e1_faces {
                    e2_to_e1_faces = d;
                }
            }

            // keep the edge with the smallest maxDropDistance as long as edge dropped to
            // every neighbor face
            if e2_to_e1_faces < e1_to_e2_faces && e2_to_e1_faces != BIG_DOUBLE {
                return Ok(KeepChoice {
                    max_distance: e2_to_e1_faces,
                    keep_first: false,
                });
            } else if e1_to_e2_faces != BIG_DOUBLE {
                return Ok(KeepChoice {
                    max_distance: e1_to_e2_faces,
                    keep_first: true,
                });
            }
        }

        // Otherwise just do the standard simple selection of the best edge defined as
        // the edge with the most faces or the curve with the fewest knots
        if faces2.len() > faces1.len() {
            out.keep_first = false;
            return Ok(out);
        }
        if faces1.len() > faces2.len() {
            return Ok(out);
        }

        // knot counts (statuses ignored: a failing call leaves its list empty)
        let knots1 = ignored(curve_of(b, edge1)?.knots(k, None))?.map_or(0, |(k1, _)| k1.len());
        let knots2 = ignored(curve_of(b, edge2)?.knots(k, None))?.map_or(0, |(k2, _)| k2.len());

        // when knot counts vary - keep the smallest count
        if knots2 != knots1 {
            out.keep_first = knots1 < knots2;
        }
        return Ok(out);
    }

    // all done
    Ok(out)
}

// ---- file-static helpers -------------------------------------------------------------

/// `sm_CheckInteriorEdge`: whether `t` is in the interior of the edge to `tol`: inside
/// its interval by more than `SM_EFF_ZERO`, and either inside by twenty parametric
/// tolerances (when the curve speed exceeds 0.1) or farther than `tol` in 3d from both
/// vertices.
///
/// # Errors
///
/// Those of the curve evaluation (upstream then reports not interior); a null curve or
/// vertex pointer upstream dereferences.
pub fn check_interior_edge(k: &Kernel, b: &Brep, e: EdgeId, t: f64, tol: f64) -> Result<bool> {
    // Check common simple case first. Note, negative tol excludes the ends.
    let domain = edge::interval(b, e)?;
    if !domain.contains_value(t, -EFF_ZERO) {
        return Ok(false);
    }

    let pts = curve_of(b, e)?.evaluate(k, t, 1, false, true)?;
    let crv_pt = pts.first().copied().ok_or_else(sm_err)?;
    let param_len = pts.get(1).copied().ok_or_else(sm_err)?.length();
    if param_len > 0.1 {
        let mut param_tol = tol / param_len;

        // Because the param length can vary, use this test only as a rough guide: if
        // it's anywhere close, go on to the 3d test. So use a bigger tolerance.
        param_tol *= 20.0;
        if domain.contains_value(t, -param_tol) {
            return Ok(true);
        }
    }

    // Work in 3d. This is also more reliable in case of vertex/edge tolerance.
    let v = vertex_of(edge::vertex(b, e)?)?;
    if crv_pt.distance(vertex::point(b, v)?) <= tol {
        return Ok(false);
    }
    let v = vertex_of(edge::other_vertex(b, e, Some(v))?)?;
    if crv_pt.distance(vertex::point(b, v)?) <= tol {
        return Ok(false);
    }
    Ok(true)
}

/// Why [`test_and_split_edge`] did not split (`sReasonType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitReason {
    /// `NO_REASON`: the edge was split.
    NoReason = 0,
    /// `FAR_FROM_GEOM`: the point is not within tolerance of the edge.
    FarFromGeom = 1,
    /// `CLOSE_TO_BOUNDARY`: the point is near an end of the edge.
    CloseToBoundary = 2,
}

/// The result of [`test_and_split_edge`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeSplitTest {
    /// Why the edge was not split ([`SplitReason::NoReason`] when it was).
    pub reason: SplitReason,
    /// Distance from the point to the edge (always set).
    pub dist_to_edge: f64,
    /// The vertex the split created.
    pub new_vertex: Option<VertexId>,
    /// The edge the split created (the half that is not `e`).
    pub new_edge: Option<EdgeId>,
}

/// `sm_TestAndSplitEdge`: drops `point` onto the edge (global solve, snap distance
/// `tol`) and, when it lands within `max(tol, edge tolerance)` in the edge's interior
/// ([`check_interior_edge`]), splits the edge there.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for an edge without curve or a split without new vertex;
/// those of the drop (`SER`).
pub fn test_and_split_edge(
    k: &mut Kernel,
    b: &mut Brep,
    e: EdgeId,
    point: Point3,
    tol: f64,
) -> Result<EdgeSplitTest> {
    test_and_split_edge_with(k, b, e, point, tol, make_vertex_split_edge)
}

fn test_and_split_edge_with(
    k: &mut Kernel,
    b: &mut Brep,
    e: EdgeId,
    point: Point3,
    tol: f64,
    mut split: impl FnMut(
        &mut Kernel,
        &mut Brep,
        EdgeId,
        f64,
    ) -> Result<(Option<EdgeId>, Option<EdgeId>, Option<VertexId>)>,
) -> Result<EdgeSplitTest> {
    // Init outputs
    let mut out = EdgeSplitTest {
        reason: SplitReason::NoReason,
        dist_to_edge: BIG_DOUBLE,
        new_vertex: None,
        new_edge: None,
    };

    let record = b.edge_mut(e)?;
    let zone_tol_3d = record.tolerance;
    let curve = record
        .curve
        .as_mut()
        .ok_or_else(|| ner("edge without curve"))?;
    // The drop's cache uses half the owning edge's zone tolerance, not the
    // independent stitch/snap tolerance. Retain the actual curve and its memo.
    curve.set_owner_zone_tol_3d(Some(zone_tol_3d));
    let domain = record.interval;
    // This can't be too big. [B448]
    let dropped = curve.drop_point(
        k,
        &domain,
        point,
        None,
        tol,
        None,
        SolverOperation::Minimize,
    )?;
    out.dist_to_edge = dropped.distance;

    // Check far from curve.
    let this_tol = smos_max(tol, zone_tol_3d);
    if !dropped.success || dropped.distance > this_tol {
        out.reason = SplitReason::FarFromGeom;
        return Ok(out);
    }

    // Check on the interior (status ignored: a failure reports not interior).
    let interior =
        ignored(check_interior_edge(k, b, e, dropped.parameter, this_tol))?.unwrap_or(false);
    if !interior {
        out.reason = SplitReason::CloseToBoundary;
        return Ok(out);
    }

    // Must have a point which can split the Edge.
    let (e1, e2, new_v) = match split(k, b, e, dropped.parameter) {
        Ok(r) => r,
        Err(err) if err.is_unported() => return Err(err),
        Err(_) => {
            // most probable reason; don't really care too much.
            out.reason = SplitReason::CloseToBoundary;
            return Ok(out);
        }
    };
    out.new_vertex = Some(new_v.ok_or_else(|| ner("split without vertex"))?);
    out.new_edge = if e2 == Some(e) { e1 } else { e2 };
    Ok(out)
}

/// Actual notification services borrowed by a contextual stitch operation.
/// Implementations retain the caller's topology records, attributes and identities.
#[allow(clippy::too_many_arguments)]
pub(crate) trait StitchEditContext {
    fn set_point(&mut self, k: &mut Kernel, b: &mut Brep, v: VertexId, point: Point3)
        -> Result<()>;
    fn glue_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: VertexId,
        delete: VertexId,
        wire: Option<&[EdgeId]>,
    ) -> Result<()>;
    fn remove_faces(&mut self, k: &mut Kernel, b: &mut Brep, faces: &[FaceId]) -> Result<()>;
    fn glue_edges(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: EdgeId,
        orientation: OrientType,
        dist: f64,
        nest: bool,
        delete: EdgeId,
        inside: Option<&[FaceuseId]>,
    ) -> Result<()>;
    fn squeeze_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: Option<EdgeId>,
        keep: VertexId,
    ) -> Result<()>;
    fn split_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: EdgeId,
        t: f64,
    ) -> Result<(Option<EdgeId>, Option<EdgeId>, Option<VertexId>)>;
    fn brep_pre_edit(&mut self, k: &mut Kernel, b: &mut Brep) -> Result<()>;
    fn delete_face(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        f: FaceId,
        delete_ev: bool,
        nest: bool,
        out: Option<&mut Vec<EntityRef>>,
        region: Option<RegionId>,
    ) -> Result<()>;
    fn face_notify(
        &mut self,
        b: &mut Brep,
        f: FaceId,
        operation: NotifyOperation,
        data1: Option<EntityRef>,
        data3: Option<EntityRef>,
    ) -> Result<()>;
}

/// A stitch edit service borrowing the real notification environment.
/// New split-curve identities must be registered by the owner or refused explicitly.
pub(crate) struct BorrowedStitchEdits<'a, 'e> {
    pub env: &'a mut edge::NotifyEnv<'e>,
    pub curve_identity: crate::topology::brep::p9::CurveIdentity<'a>,
}

impl StitchEditContext for BorrowedStitchEdits<'_, '_> {
    fn set_point(
        &mut self,
        _k: &mut Kernel,
        b: &mut Brep,
        v: VertexId,
        point: Point3,
    ) -> Result<()> {
        let this = (self.env.key_of)(EntityRef::Vertex(v));
        let key_of = self.env.key_of;
        vertex::set_point(
            b,
            v,
            point,
            self.env.records,
            self.env.attributes,
            self.env.context.as_deref_mut(),
            this,
            &|vu| key_of(EntityRef::Vertexuse(vu)),
        )
    }
    fn glue_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: VertexId,
        delete: VertexId,
        wire: Option<&[EdgeId]>,
    ) -> Result<()> {
        crate::topology::brep::p5::glue_with_notifications(
            k,
            b,
            Some(keep),
            Some(delete),
            wire,
            Some(self.env),
        )
    }
    fn remove_faces(&mut self, k: &mut Kernel, b: &mut Brep, faces: &[FaceId]) -> Result<()> {
        crate::topology::brep::p5::remove_faces_notified(k, b, b.editing_enabled(), faces, self.env)
    }
    fn glue_edges(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: EdgeId,
        orientation: OrientType,
        dist: f64,
        nest: bool,
        delete: EdgeId,
        inside: Option<&[FaceuseId]>,
    ) -> Result<()> {
        crate::topology::brep::p7::glue_edges_general(
            k,
            b,
            Some(keep),
            orientation,
            dist,
            nest,
            Some(delete),
            inside,
            self.env,
        )
    }
    fn squeeze_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: Option<EdgeId>,
        keep: VertexId,
    ) -> Result<()> {
        let e = e.ok_or_else(|| SolidError::null_pointer("heal::stitch::p1::squeeze_edge_to"))?;
        crate::topology::brep::p3::squeeze_edge(k, b, self.env, e, keep)
    }
    fn split_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: EdgeId,
        t: f64,
    ) -> Result<(Option<EdgeId>, Option<EdgeId>, Option<VertexId>)> {
        let out = crate::topology::brep::p9::make_vertex_split_edge_with_curve_identity(
            k,
            b,
            e,
            t,
            self.env,
            self.curve_identity,
        )?;
        if !out.split {
            return Err(SolidError::sm_err(OP));
        }
        let [e1, e2] = out.edges.ok_or_else(|| ner("split without edges"))?;
        Ok((Some(e1), Some(e2), out.vertex))
    }
    fn brep_pre_edit(&mut self, _k: &mut Kernel, b: &mut Brep) -> Result<()> {
        use crate::topology::brep::p2;
        let this = Some(p2::NotifyArg::Object(p2::brep_ref(b)));
        p2::notify(
            b,
            self.env,
            None,
            NotifyOperation::PreEdit,
            [this, None, None],
        )
    }
    fn delete_face(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        f: FaceId,
        delete_ev: bool,
        nest: bool,
        out: Option<&mut Vec<EntityRef>>,
        region: Option<RegionId>,
    ) -> Result<()> {
        let mut out = out;
        if let Some(out) = out.as_deref_mut() {
            out.clear();
        }
        let deleted = crate::topology::brep::p3::delete_face(
            k,
            b,
            self.env,
            &mut Marks::default(),
            f,
            crate::topology::brep::p3::DeleteFaceOptions {
                delete_edges_vertices: delete_ev,
                do_region_nesting: nest,
                region_to_keep: region,
            },
        )?;
        if let Some(out) = out {
            *out = deleted;
        }
        Ok(())
    }
    fn face_notify(
        &mut self,
        b: &mut Brep,
        f: FaceId,
        operation: NotifyOperation,
        data1: Option<EntityRef>,
        data3: Option<EntityRef>,
    ) -> Result<()> {
        let key_of = self.env.key_of;
        crate::topology::face::p3::notify(
            b,
            f,
            self.env,
            None,
            operation,
            [data1.map(key_of), None, data3.map(key_of)],
        )
    }
}

// ---- SmStitch --------------------------------------------------------------------------

/// Stitches topological elements together (`SmStitch`).
#[allow(clippy::struct_excessive_bools)] // upstream's public option flags
pub struct Stitch<'c> {
    /// Picks the kept and the deleted object of a glued pair (`m_rStitchCallback`).
    callback: &'c mut dyn StitchCallback,
    pub(crate) edits: Option<&'c mut dyn StitchEditContext>,
    /// Geometry-owner identities for the final region-nesting query.
    pub(crate) query_identity: Option<crate::query::brep::BrepIdentity<'c>>,
    /// Max 3d distance between coincident edges and vertices (`m_dStitchTol3d`).
    tol_3d: f64,
    /// Find and set the infinite region after gluing (`m_bDoRegionNesting`).
    pub do_region_nesting: bool,
    /// Validate pointers after stitching in validating builds (`m_bValidateResult`).
    pub validate_result: bool,
    /// Don't signal an error and quit when two edges fail to glue
    /// (`m_bIgnoreProblems`, default `FALSE`).
    pub ignore_problems: bool,
    /// Only do a quick 5 point distance check for coincident edges
    /// (`m_bFastEdgeCompare`, default `FALSE`).
    pub fast_edge_compare: bool,
    /// Remove edges shorter than the stitch tolerance (`m_bSqueezeSmallEdges`,
    /// default `FALSE`).
    pub squeeze_small_edges: bool,
    /// Split edges that come within tolerance of a vertex
    /// (`m_bSplitEdgesWithVertices`, default `TRUE`).
    pub split_edges_with_vertices: bool,
    /// Only glue pairs of lamina edges and their vertices (`m_bMakingManifoldSolid`,
    /// default `TRUE`).
    pub making_manifold_solid: bool,
    /// Remove sliver faces smaller than tolerance first (`m_bRemoveLaminarSlivers`,
    /// default `FALSE`).
    pub remove_laminar_slivers: bool,
}

impl core::fmt::Debug for Stitch<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Stitch")
            .field("stitch_tol_3d", &self.tol_3d)
            .field("do_region_nesting", &self.do_region_nesting)
            .field("validate_result", &self.validate_result)
            .field("ignore_problems", &self.ignore_problems)
            .field("fast_edge_compare", &self.fast_edge_compare)
            .field("squeeze_small_edges", &self.squeeze_small_edges)
            .field("split_edges_with_vertices", &self.split_edges_with_vertices)
            .field("making_manifold_solid", &self.making_manifold_solid)
            .field("remove_laminar_slivers", &self.remove_laminar_slivers)
            .finish_non_exhaustive()
    }
}

/// The result of [`Stitch::do_simple_face_stitch`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimpleFaceStitch {
    /// Largest gap between paired vertices (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Largest gap between paired edges (`rdMaxEdgeGap`).
    pub max_edge_gap: f64,
    /// Number of edges stitched (`rlStitchedEdges`).
    pub stitched_edges: u64,
}

/// The result of [`Stitch::stitch_vertex_pairs`].
#[derive(Debug, Clone, PartialEq)]
pub struct VertexPairs {
    /// The surviving vertex of each glued pair (`rProcessedVerts`).
    pub processed: Vec<VertexId>,
    /// The deleted vertex of each glued pair (`rDeletedVerts`; stale ids).
    pub deleted: Vec<VertexId>,
    /// Largest gap between paired vertices (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Smallest gap between unpaired vertices (`rdMinUnstitchedVertGap`).
    pub min_unstitched_vert_gap: f64,
}

/// The result of [`Stitch::stitch_edges_of_vertices`].
#[derive(Debug, Clone, PartialEq)]
pub struct EdgesOfVertices {
    /// The surviving edge of each glued pair (`rGluedEdges`).
    pub glued: Vec<EdgeId>,
    /// The deleted edge of each glued pair (`rDeletedEdges`; stale ids).
    pub deleted: Vec<EdgeId>,
    /// Largest gap between glued edges (`rdMaxEdgeGap`).
    pub max_edge_gap: f64,
    /// Smallest gap between unglued edge pairs (`rdMinUnstitchedEdgeGap`).
    pub min_unstitched_edge_gap: f64,
}

impl<'c> Stitch<'c> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_set_point(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        v: VertexId,
        point: Point3,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.set_point(k, b, v, point)
        } else {
            set_point(k, b, v, point)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_glue_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: VertexId,
        delete: VertexId,
        wire: Option<&[EdgeId]>,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.glue_vertices(k, b, keep, delete, wire)
        } else {
            glue_vertices(k, b, keep, delete, wire)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_remove_faces(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        faces: &[FaceId],
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.remove_faces(k, b, faces)
        } else {
            remove_faces(k, b, faces)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_glue_edges(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        keep: EdgeId,
        orientation: OrientType,
        dist: f64,
        nest: bool,
        delete: EdgeId,
        inside: Option<&[FaceuseId]>,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.glue_edges(k, b, keep, orientation, dist, nest, delete, inside)
        } else {
            glue_edges_general(k, b, keep, orientation, dist, nest, delete, inside)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_squeeze_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: Option<EdgeId>,
        keep: VertexId,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.squeeze_edge(k, b, e, keep)
        } else {
            squeeze_edge_to(k, b, e, keep)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_split_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: EdgeId,
        t: f64,
    ) -> Result<(Option<EdgeId>, Option<EdgeId>, Option<VertexId>)> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.split_edge(k, b, e, t)
        } else {
            make_vertex_split_edge(k, b, e, t)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_brep_pre_edit(&mut self, k: &mut Kernel, b: &mut Brep) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.brep_pre_edit(k, b)
        } else {
            let _ = k;
            brep_notify_pre_edit(b)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_delete_face(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        f: FaceId,
        delete_ev: bool,
        nest: bool,
        out: Option<&mut Vec<EntityRef>>,
        region: Option<RegionId>,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.delete_face(k, b, f, delete_ev, nest, out, region)
        } else {
            super::p2::delete_face(k, b, f, delete_ev, nest, out, region)
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn edit_face_notify(
        &mut self,
        b: &mut Brep,
        f: FaceId,
        operation: NotifyOperation,
        data1: Option<EntityRef>,
        data3: Option<EntityRef>,
    ) -> Result<()> {
        if let Some(edits) = self.edits.as_deref_mut() {
            edits.face_notify(b, f, operation, data1, data3)
        } else {
            super::p2::face_notify(b, f, operation, data1, data3)
        }
    }
    pub(crate) fn edit_test_and_split_edge(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        e: EdgeId,
        point: Point3,
        tol: f64,
    ) -> Result<EdgeSplitTest> {
        test_and_split_edge_with(k, b, e, point, tol, |k, b, e, t| {
            self.edit_split_edge(k, b, e, t)
        })
    }

    /// `SmStitch(rStitchCallback, dStitchTol3d, bDoRegionNesting, bValidateResult)`
    /// (upstream defaults `TRUE`, `TRUE`); the other options take their defaults.
    pub fn new(
        callback: &'c mut dyn StitchCallback,
        stitch_tol_3d: f64,
        do_region_nesting: bool,
        validate_result: bool,
    ) -> Self {
        Self {
            callback,
            query_identity: None,
            edits: None,
            tol_3d: stitch_tol_3d,
            do_region_nesting,
            validate_result,
            ignore_problems: false,
            fast_edge_compare: false,
            squeeze_small_edges: false,
            split_edges_with_vertices: true,
            making_manifold_solid: true,
            remove_laminar_slivers: false,
        }
    }

    /// `SetStitchTol3d`.
    pub fn set_stitch_tol_3d(&mut self, stitch_tol_3d: f64) {
        self.tol_3d = stitch_tol_3d;
    }

    /// The selection callback (`m_rStitchCallback`), for the `heal.stitch_p2` members.
    pub(crate) fn callback(&mut self) -> &mut dyn StitchCallback {
        &mut *self.callback
    }

    /// The stitch tolerance (`m_dStitchTol3d`).
    #[must_use]
    pub const fn stitch_tol_3d(&self) -> f64 {
        self.tol_3d
    }

    /// `DoSimpleFaceStitch`: pairs every face to keep with its face to delete
    /// (vertices within the stitch tolerance, edges between matching vertex pairs within
    /// `max(stitch tol, summed edge tolerances)`), removes the faces to delete and glues
    /// (or squeezes) the vertex pairs. Increments an unlocked mark value.
    ///
    /// Upstream notes the routine is never called; its edge-pair lists are never
    /// filled, so no edge is glued and `stitched_edges` stays 0.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for empty or differently sized face lists, faces of another B-rep,
    /// faces whose vertices or edges do not pair up, and a selection or gap above
    /// tolerance; those of the curve evaluations, the callback and the gluing (`SER`).
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn do_simple_face_stitch(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        faces_to_keep: &[FaceId],
        faces_to_delete: &[FaceId],
    ) -> Result<SimpleFaceStitch> {
        let mut out = SimpleFaceStitch {
            max_vertex_gap: 0.0,
            max_edge_gap: 0.0,
            stitched_edges: 0,
        };

        let n_pairs = faces_to_keep.len();
        if n_pairs == 0 || n_pairs != faces_to_delete.len() {
            return Err(sm_err());
        }

        // The given Faces must be part of the given Brep.
        let (Some(&keep0), Some(&delete0)) = (faces_to_keep.first(), faces_to_delete.first())
        else {
            return Err(sm_err());
        };
        if !face_in_brep(b, keep0)? {
            return Err(sm_err());
        }
        if !face_in_brep(b, delete0)? {
            return Err(sm_err());
        }

        let mut k_vertices: Vec<VertexId> = Vec::new();
        let mut d_vertices: Vec<VertexId> = Vec::new();
        // never filled upstream
        let k_edges: Vec<EdgeId> = Vec::new();
        let d_edges: Vec<EdgeId> = Vec::new();
        for (&face_to_keep, &face_to_delete) in faces_to_keep.iter().zip(faces_to_delete) {
            // Need to find the pairs of vertices that are to be glued together
            let k_verts = face_vertices(b, face_to_keep)?;
            let d_verts = face_vertices(b, face_to_delete)?;

            // must have the same number of vertices for each face
            let n_verts = k_verts.len();
            if n_verts != d_verts.len() {
                return Err(sm_err()); // faces not compatible
            }

            let vtol = self.tol_3d; // may want a Vtol and an Etol
            let mut n_found = 0usize;
            for &k_vertex in &k_verts {
                let k_pt = vertex::point(b, k_vertex)?;
                for &d_vertex in &d_verts {
                    let vdist = k_pt.distance(vertex::point(b, d_vertex)?);
                    if vdist < vtol {
                        // found the neighbor of this vertex
                        if out.max_vertex_gap < vdist {
                            out.max_vertex_gap = vdist;
                        }
                        k_vertices.push(k_vertex);
                        d_vertices.push(d_vertex);
                        n_found += 1;
                        break; // don't need to test further
                    }
                }
            }

            // must have nVert pairs
            if n_found != n_verts {
                return Err(sm_err()); // vertices not all in tolerance
            }

            // start of Edge tests
            let k_eds = face_edges(b, face_to_keep)?;
            let d_eds = face_edges(b, face_to_delete)?;

            // must have the same number of edges for each face and the same number of
            // edges as verts
            let n_edges = k_eds.len();
            if n_edges != d_eds.len() || n_edges != n_verts {
                return Err(sm_err());
            }

            n_found = 0;
            for &k_edge in &k_eds {
                let k_v0 = edge::vertex(b, k_edge)?;
                let k_v1 = edge::other_vertex(b, k_edge, k_v0)?;

                // determine the corresponding DVerts
                let pair_of = |v: Option<VertexId>| -> Result<Option<VertexId>> {
                    let i = k_vertices
                        .iter()
                        .position(|&x| Some(x) == v)
                        .ok_or_else(sm_err)?;
                    Ok(d_vertices.get(i).copied())
                };
                let dk_v0 = pair_of(k_v0)?;
                let dk_v1 = pair_of(k_v1)?;

                for &d_edge in &d_eds {
                    let d_v0 = edge::vertex(b, d_edge)?;
                    let d_v1 = edge::other_vertex(b, d_edge, d_v0)?;
                    // need the edge with verts DKV0 and DKV1
                    if !((dk_v0 == d_v0 && dk_v1 == d_v1) || (dk_v0 == d_v1 && dk_v1 == d_v0)) {
                        continue;
                    }
                    // found an edge with matching vertices
                    n_found += 1;
                    // compare this pair of edges
                    let k_ivl = edge::interval(b, k_edge)?;
                    let d_ivl = edge::interval(b, d_edge)?;
                    let (k_start, k_end) = (k_ivl.min(), k_ivl.max());
                    let (mut d_start, mut d_end) = (d_ivl.min(), d_ivl.max());
                    let k_curve = curve_of(b, k_edge)?;
                    let d_curve = curve_of(b, d_edge)?;
                    let k_pt0 = k_curve.evaluate_point(k, k_start)?;
                    let k_pt1 = k_curve.evaluate_point(k, k_end)?;
                    let d_pt0 = d_curve.evaluate_point(k, d_start)?;
                    let d_pt1 = d_curve.evaluate_point(k, d_end)?;
                    if k_pt0.distance(d_pt0) < vtol && k_pt1.distance(d_pt1) < vtol {
                        // do nothing, continue on to check dist.
                    } else if k_pt0.distance(d_pt1) < vtol && k_pt1.distance(d_pt0) < vtol {
                        core::mem::swap(&mut d_start, &mut d_end);
                    } else {
                        return Err(sm_err());
                    }

                    let tol = smos_max(
                        self.tol_3d,
                        edge::tolerance(b, k_edge)? + edge::tolerance(b, d_edge)?,
                    );
                    // status ignored (`SE`): a failure leaves the gap unset, read as 0
                    let dist_to_curve = ignored(k_curve.curve_max_distance_between(
                        k,
                        &k_ivl,
                        d_curve,
                        d_start,
                        d_end,
                        10,
                        Some(tol),
                    ))?
                    .map_or(0.0, |m| m.distance);
                    if dist_to_curve > tol {
                        return Err(sm_err()); // edges not in tol
                    }
                    if out.max_edge_gap < dist_to_curve {
                        out.max_edge_gap = dist_to_curve;
                    }
                    break; // don't need to test any further
                }
            }
            // must have nEdge pairs
            if n_found != n_edges {
                return Err(sm_err());
            }
        }

        // UB-177: upstream's RemoveFaces frees every vertex only the delete faces use, and
        // the glue loop then reads those vertices; a repeated delete face is freed twice; a
        // vertex in more than one entry of the two vertex lists (built with plain Add across
        // the face pairs) is freed by the glue of its first entry and read by the next. Those
        // inputs are undefined upstream. A repeated vertex whose first entry frees nothing
        // (squeeze skipped, a short manifold common edge, the making-manifold-solid skip, or
        // the loop ending first with SM_ERR) is refused too: a deliberate over-refusal of a
        // rare input, as the C++ shim does.
        for (i, d) in faces_to_delete.iter().enumerate() {
            if faces_to_delete.iter().take(i).any(|x| x == d) {
                return Err(deref(
                    "DoSimpleFaceStitch: a face repeated in the delete list (UB-177)",
                ));
            }
        }
        let all_vertices: Vec<VertexId> = k_vertices.iter().chain(&d_vertices).copied().collect();
        for (i, v) in all_vertices.iter().enumerate() {
            if all_vertices.iter().take(i).any(|x| x == v) {
                return Err(deref(
                    "DoSimpleFaceStitch: a vertex in more than one vertex pair entry (UB-177)",
                ));
            }
        }
        for &dv in &d_vertices {
            if vertex::faces(b, dv)?
                .iter()
                .all(|f| faces_to_delete.contains(f))
            {
                return Err(deref(
                    "DoSimpleFaceStitch: RemoveFaces frees a vertex the glue reads (UB-177)",
                ));
            }
        }

        // the vertex and edge pairs have been found and they are in tolerance so delete
        // FacesToDelete (increments unlocked mark value; status ignored)
        ignore_status(self.edit_remove_faces(k, b, faces_to_delete))?;

        // now follow the procedure that is used in DoStitching for stitching vertex
        // and edge pairs
        let do_region_nesting = self.do_region_nesting;
        let squeeze_small_edges = self.squeeze_small_edges;
        let making_manifold_solid = self.making_manifold_solid;
        let fast_edge_compare = self.fast_edge_compare;
        let stitch_tol_3d = self.tol_3d;
        with_mark_lock(k, ALL_MARKS, |k, mark| -> Result<SimpleFaceStitch> {
            let mut marks = Marks::default();
            for (&kv, &dv) in k_vertices.iter().zip(&d_vertices) {
                marks.mark(k, Entity::Vertex(kv), mark);
                marks.mark(k, Entity::Vertex(dv), mark);
            }
            for (&ke, &de) in k_edges.iter().zip(&d_edges) {
                marks.mark(k, Entity::Edge(ke), mark);
                marks.mark(k, Entity::Edge(de), mark);
            }

            let editing = b.editing_enabled();
            b.set_editing_enabled(true);
            let r = (|| -> Result<SimpleFaceStitch> {
                // status ignored
                ignore_status(self.edit_brep_pre_edit(k, b))?;

                // glue the coincident vertices
                let wire_edges = bp1::wire_edges(b, None)?;
                for (&kv, &dv) in k_vertices.iter().zip(&d_vertices) {
                    let (v0, v1) = (kv, dv);
                    if making_manifold_solid
                        && !(vertex::is_lamina_vertex(b, v0)? && vertex::is_lamina_vertex(b, v1)?)
                    {
                        continue;
                    }
                    let dist_to_v = vertex::point(b, v0)?.distance(vertex::point(b, v1)?);
                    let c = self.callback.select_topology_to_keep(
                        k,
                        b,
                        EntityRef::Vertex(v0),
                        EntityRef::Vertex(v1),
                        dist_to_v,
                    )?;
                    let tol = smos_max(
                        stitch_tol_3d,
                        vertex::tolerance(b, v0)? + vertex::tolerance(b, v1)?,
                    );
                    if c.max_distance > tol {
                        // Don't stitch it if distance gets too big.
                        return Err(sm_err());
                    }
                    let mut do_glue = true;
                    let mut do_squeeze = false;
                    let mut squeeze_edge: Option<EdgeId> = None;
                    for e in vertex::common_edges(b, v0, Some(v1), None)? {
                        let edge_length = curve_of(b, e)?
                            .approximate_length(k, &edge::interval(b, e)?, 10, false)?
                            .0;
                        if edge_length < 2.0 * dist_to_v {
                            if do_squeeze {
                                // Too many edges between vertices
                                do_squeeze = false;
                                do_glue = false;
                                break;
                            }
                            if edge::is_lamina(b, e)? || edge::is_wire(b, e)? {
                                do_squeeze = true;
                                do_glue = false;
                                squeeze_edge = Some(e);
                            } else {
                                do_squeeze = false;
                                do_glue = false;
                            }
                        }
                    }

                    if do_squeeze && !squeeze_small_edges {
                        continue;
                    }
                    let (keep, other) = if c.keep_first { (v0, v1) } else { (v1, v0) };
                    if do_squeeze {
                        self.edit_squeeze_edge(k, b, squeeze_edge, keep)?;
                    } else if do_glue {
                        self.edit_glue_vertices(k, b, keep, other, Some(&wire_edges))?;
                    }
                }

                // All the vertices are stitched so now stitch the edge pairs
                let mut modified_faces: Vec<FaceId> = Vec::new();
                for (&e0, &e1) in k_edges.iter().zip(&d_edges) {
                    let tol = smos_max(
                        stitch_tol_3d,
                        edge::tolerance(b, e0)? + edge::tolerance(b, e1)?,
                    );
                    let e0_ivl = edge::interval(b, e0)?;
                    let e1_ivl = edge::interval(b, e1)?;
                    let e0_curve = curve_of(b, e0)?;
                    let e1_curve = curve_of(b, e1)?;
                    let mid_pnt = e0_curve.evaluate_point(k, e0_ivl.evaluate(0.5))?;
                    let dropped = e1_curve.drop_point(
                        k,
                        &e1_ivl,
                        mid_pnt,
                        None,
                        tol,
                        None,
                        SolverOperation::Minimize,
                    )?;
                    if !dropped.success {
                        return Err(sm_err());
                    }
                    let pv = e0_curve.evaluate(k, e0_ivl.evaluate(0.5), 1, true, true)?;
                    let pv_test = e1_curve.evaluate(k, dropped.parameter, 1, true, true)?;
                    let mut orientation = OrientType::Same;
                    let (mut t_start, mut t_end) = (e1_ivl.min(), e1_ivl.max());
                    if tangent(&pv)?.dot(tangent(&pv_test)?) < 0.0 {
                        orientation = OrientType::Opposite;
                        core::mem::swap(&mut t_start, &mut t_end);
                    }
                    let mut dist_to_curve = e0_curve
                        .curve_max_distance_between(
                            k,
                            &e0_ivl,
                            e1_curve,
                            t_start,
                            t_end,
                            5,
                            Some(tol),
                        )?
                        .distance;
                    if dist_to_curve > tol {
                        return Err(sm_err());
                    }
                    if !fast_edge_compare {
                        // Passed an initial quick test with 5 points now do a precise
                        // measurement
                        dist_to_curve = e0_curve
                            .curve_max_distance_between(
                                k,
                                &e0_ivl,
                                e1_curve,
                                t_start,
                                t_end,
                                0,
                                Some(tol),
                            )?
                            .distance;
                        if dist_to_curve > tol {
                            return Err(sm_err());
                        }
                    }
                    // made it here so glue edges
                    if dist_to_curve > out.max_edge_gap {
                        out.max_edge_gap = dist_to_curve;
                    }

                    let c = self.callback.select_topology_to_keep(
                        k,
                        b,
                        EntityRef::Edge(e0),
                        EntityRef::Edge(e1),
                        dist_to_curve,
                    )?;
                    if c.max_distance > tol {
                        return Err(sm_err()); // can't stitch, error
                    }
                    let (keep, other) = if c.keep_first { (e0, e1) } else { (e1, e0) };
                    for f in edge::faces(b, other, None)? {
                        if !modified_faces.contains(&f) {
                            modified_faces.push(f);
                        }
                    }
                    self.edit_glue_edges(
                        k,
                        b,
                        keep,
                        orientation,
                        dist_to_curve,
                        do_region_nesting,
                        other,
                        None,
                    )?;
                    out.stitched_edges += 1;
                }
                Ok(out)
            })();
            b.set_editing_enabled(editing);
            r
        })?
    }

    /// `UnifyNormals`: orients the faces reachable from `start_face` consistently with
    /// it, across manifold edges by their face uses and across lamina edges found
    /// within the stitch tolerance of `num_samples` points of each edge use (opposite
    /// tangents within 5 degrees, binormals more than 90 degrees apart). Flipped faces
    /// are swapped ([`fp1::swap_uv`]) and returned in flip order. Increments an
    /// unlocked mark value.
    ///
    /// # Errors
    ///
    /// Those of the B-rep box, the edge tree, the curve evaluations and drops, the
    /// binormal evaluations and the swaps (`SER`); a null pointer upstream dereferences.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn unify_normals(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        start_face: FaceId,
        num_samples: u64,
    ) -> Result<Vec<FaceId>> {
        let stitch_tol_3d = self.tol_3d;
        with_mark_lock(k, ALL_MARKS, |k, mark| -> Result<Vec<FaceId>> {
            let mut flipped: Vec<FaceId> = Vec::new();
            let mut marks = Marks::default();

            // CalculateBoundingBox(sBrepBBox) with the header defaults; status ignored
            // upstream (a failure leaves the default-initialised box)
            let brep_bbox = ignored(qbrep::calculate_bounding_box(
                k,
                &mut *b,
                BoundingBoxOptions::default(),
            ))?
            .unwrap_or_default();

            let edges = bp1::edges(b, None)?;
            // lNumPerNode = max(500, edges/10) only sizes the memory blocks
            let mut edge_tree: Tree<EdgeId> = Tree::new_spatial(&brep_bbox);
            for &e in &edges {
                let ebox = curve_of(b, e)?
                    .calculate_bounding_box(
                        k,
                        &edge::interval(b, e)?,
                        BoundingBoxRequest {
                            normal: true,
                            ..BoundingBoxRequest::default()
                        },
                        true,
                    )?
                    .normal
                    .ok_or_else(sm_err)?;
                edge_tree.add_to_spatial_tree(&ebox, Some(e), NodeGeomType::default())?;
            }

            let mut f_stack: Vec<FaceId> = vec![start_face];
            marks.mark(k, Entity::Face(start_face), mark);

            while let Some(f) = f_stack.pop() {
                let fu = fp2::upward_faceuse(b, f)?.ok_or_else(|| deref("face without faceuse"))?;
                for eu in eu1::faceuse_edgeuses(b, fu, false, None)? {
                    let eu_edge =
                        eu1::edge(b, eu)?.ok_or_else(|| deref("edge use without edge"))?;
                    if edge::is_manifold(b, eu_edge)? {
                        // we are already stitched just orient it the easy way.
                        let radial = eu1::radial(b, eu)?.ok_or_else(|| deref("no radial"))?;
                        let fuse = eu1::faceuse(b, radial)?.ok_or_else(|| deref("no faceuse"))?;
                        let rad_f = eu1::faceuse_face(b, fuse)?
                            .ok_or_else(|| deref("faceuse without face"))?;
                        if marks.is_marked(k, Entity::Face(rad_f), mark) {
                            continue;
                        }
                        if eu1::faceuse_orientation(b, fuse)? == OrientType::Opposite {
                            flipped.push(rad_f);
                            fp1::swap_uv(k, b, rad_f)?;
                        }
                        // Only oriented faces get onto stack and get marked
                        marks.mark(k, Entity::Face(rad_f), mark);
                        f_stack.push(rad_f);
                        continue;
                    }
                    for j in 0..num_samples {
                        #[allow(clippy::cast_precision_loss)] // upstream ULONG to double
                        let param = (j as f64 + 1.0) / (num_samples as f64 + 2.0);
                        let orig_ivl = edge::interval(b, eu_edge)?;
                        // NormalizedEvaluate(dParam, FALSE, sPnt, &sDir): 3d, with the
                        // first derivative, default bOKToMakeUVTrimCurve. The status is
                        // ignored upstream; a failure leaves sPnt / sDir at SmVector3d's
                        // default SM_UNDEF_DOUBLE, here UNDEF.
                        let (pnt, dir) =
                            ignored(normalized_evaluate(k, b, eu, param, false, true, true))?
                                .map_or((Point3::UNDEF, Vec3::UNDEF), |(p, d)| {
                                    (p, d.unwrap_or(Vec3::UNDEF))
                                });
                        let mut vbox = Extent3d::from_point(pnt);
                        vbox.expand_absolute(stitch_tol_3d);
                        let objects = edge_tree.objects_in_box(&vbox)?;
                        for object in objects {
                            let other = object.ok_or_else(|| deref("tree object"))?;
                            if !edge::is_lamina(b, other)? {
                                continue;
                            }
                            let mut prim_eu = edge::primary_edgeuse(b, other)?
                                .ok_or_else(|| deref("edge without edge use"))?;
                            // If already processed this face continue
                            let prim_face = eu1::face(b, prim_eu)?
                                .ok_or_else(|| deref("edge use without face"))?;
                            if marks.is_marked(k, Entity::Face(prim_face), mark) {
                                continue;
                            }
                            let ivl = edge::interval(b, other)?;
                            let dropped = curve_of(b, other)?.drop_point(
                                k,
                                &ivl,
                                pnt,
                                None,
                                stitch_tol_3d,
                                None,
                                SolverOperation::Minimize,
                            )?;
                            if !dropped.success {
                                continue;
                            }
                            let norm_param = ivl
                                .inversion(dropped.parameter)
                                .parameter
                                .unwrap_or(UNDEF_PARAM);
                            if norm_param < 0.02 || norm_param > 0.98 {
                                continue;
                            }
                            let pv = curve_of(b, other)?.evaluate(
                                k,
                                dropped.parameter,
                                1,
                                true,
                                true,
                            )?;
                            // status ignored: a failure leaves the angle 0
                            let angle_rad =
                                ignored(tangent(&pv)?.angle_between(dir))?.unwrap_or(0.0);
                            let angle_deg = angle_rad * 180.0 / PI;
                            // Make sure we are going in either same or opposite direction
                            if angle_deg > 5.0 && angle_deg < 175.0 {
                                continue;
                            }
                            if tangent(&pv)?.dot(dir) > 0.0 {
                                prim_eu = eu1::mate(b, prim_eu)?.ok_or_else(|| deref("no mate"))?;
                            }
                            // Here we should have opposite oriented edgeuses; pPrimEU's
                            // faceuse should be on side of positive normal. Only stitch
                            // cases where angle between binormals is > 90 degrees.
                            // EvaluateBinormal(t, FALSE, sPt, sBinVec): optional outputs NULL
                            let (_, bin_vec) =
                                evaluate_binormal(k, b, prim_eu, dropped.parameter, false)?;
                            let (_, bin_orig) =
                                evaluate_binormal(k, b, eu, orig_ivl.evaluate(param), false)?;
                            if bin_vec.dot(bin_orig) > 0.0 {
                                continue;
                            }
                            let prim_fu =
                                eu1::faceuse(b, prim_eu)?.ok_or_else(|| deref("no faceuse"))?;
                            let f_other = eu1::faceuse_face(b, prim_fu)?
                                .ok_or_else(|| deref("faceuse without face"))?;
                            if eu1::faceuse_orientation(b, prim_fu)? == OrientType::Opposite {
                                flipped.push(f_other);
                                fp1::swap_uv(k, b, f_other)?;
                            }
                            marks.mark(k, Entity::Face(f_other), mark);
                            f_stack.push(f_other);
                        }
                    }
                }
            }
            Ok(flipped)
        })?
    }

    /// `StitchVertexPairs` (two-list overload): glues each vertex of `list1` (kept) to
    /// the closest vertex of `list2` (deleted) within the stitch tolerance, unless
    /// another `list1` vertex is closer to it, moving the kept vertex to the midpoint
    /// when the gap exceeds half the deleted vertex's tolerance. Glued entries of both
    /// lists become `None`; `None` entries are skipped.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `list1` holds no vertex with a B-rep; a failing glue unless
    /// [`Stitch::ignore_problems`]; those of the vertex queries;
    /// [`SolidError::InvalidInput`] where upstream reads past its sorted index arrays
    /// (null entries) or the rescan bound is exceeded.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn stitch_vertex_pairs(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        list1: &mut [Option<VertexId>],
        list2: &mut [Option<VertexId>],
    ) -> Result<VertexPairs> {
        // Init outputs.
        let mut out = VertexPairs {
            processed: Vec::new(),
            deleted: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        };
        let tol = self.tol_3d;

        let n1 = list1.len();
        if n1 < 1 {
            return Ok(out);
        }
        let mut n2 = list2.len();
        if n2 < 1 {
            return Ok(out);
        }

        // Get Brep pointer from the first non-null entry.
        let mut has_brep = false;
        if let Some(&v) = list1.iter().flatten().next() {
            has_brep = vertex::brep(b, v)?.is_some();
        }
        if !has_brep {
            return Err(sm_err()); // we'll flag this as an error.
        }

        // As in DoStitching(), sort vertices in one dimension. Sort on (1,2,3), unitized
        // so that parameter differences correspond to 3d distances.
        let s_vec = Vec3::new(1.0, 2.0, 3.0).unitize()?;

        // Precompute lamina flag and sort value; VertexList1 first, keeping track of
        // min and max param values for "boxing out" vertices in VertexList2.
        let mut param1: Vec<f64> = Vec::new();
        let mut min_param = BIG_DOUBLE;
        let mut max_param = -BIG_DOUBLE;
        for v in list1.iter() {
            let Some(v) = *v else {
                param1.push(0.0);
                continue;
            };
            // (upstream also records IsLaminaVertex, which nothing reads)
            vertex::is_lamina_vertex(b, v)?;
            // parameter of projecting the vertex to the line through the origin
            let t = s_vec.dot(vertex::point(b, v)?);
            param1.push(t);
            if t < min_param {
                min_param = t;
            }
            if t > max_param {
                max_param = t;
            }
        }

        // Expand the range a bit.
        min_param -= 2.0 * tol;
        max_param += 2.0 * tol;

        // Also keep track of which vertices from List2 we're using.
        let mut using2: Vec<Option<VertexId>> = Vec::new();
        let mut using_indices2: Vec<usize> = Vec::new();
        let mut param2: Vec<f64> = Vec::new();
        for (ii, v) in list2.iter().enumerate() {
            let Some(v) = *v else {
                using2.push(None);
                using_indices2.push(0);
                param2.push(0.0);
                continue;
            };
            let t = s_vec.dot(vertex::point(b, v)?);
            // skip vertices that can't possibly pair up
            if t < min_param || t > max_param {
                continue;
            }
            // add potentially pairing vertex to check list
            using2.push(Some(v));
            using_indices2.push(ii);
            param2.push(t);
            vertex::is_lamina_vertex(b, v)?;
        }

        let n_using = using2.len();
        if n_using != n2 {
            // rVertexList2 = sUsingVerts2;  No: the caller is still using the other ones.
            n2 = n_using;
        }
        // no work - no possible pairing candidates
        if n_using < 1 {
            return Ok(out);
        }

        // Now sort the lists by projection param (upstream checks list 2's null entries
        // by the unfiltered list's index).
        let sorted1 = sort_by_param(n1, |ii| list1.get(ii).copied().flatten().is_none(), &param1)?;
        let sorted2 = sort_by_param(n2, |ii| list2.get(ii).copied().flatten().is_none(), &param2)?;

        let past = || SolidError::invalid_input(OP, "upstream reads past its sorted index array");
        let at1 = |s: &[usize], i: usize| s.get(i).copied().ok_or_else(past);

        // March down the lists in order: for each entry in List1, step along list2
        // collecting vertices within tol until the list2 parameter is past the list1
        // parameter; pick the closest, also checking for a closer List1 vertex; glue.
        let mut coin2: Vec<usize> = Vec::new();
        let mut lii_to_glue = 0usize; // In case we glue something other than [ii].
        let mut start_jj = 0usize; // So that we don't have to start at the beginning each time
        let bound = n1
            .checked_add(n2)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(16))
            .ok_or_else(past)?;
        let mut passes = 0usize;

        let mut ii = 0usize;
        while ii < n1 {
            passes += 1;
            if passes > bound {
                return Err(SolidError::invalid_input(
                    OP,
                    "vertex rescan bound exceeded",
                ));
            }
            // If we glued any vertex other than [ii] last time, then we might not be
            // finished with that one.
            if ii > 0 && ii != lii_to_glue + 1 {
                ii -= 1;
            }
            lii_to_glue = ii;

            // target keep vertex
            let s_ii = at1(&sorted1, ii)?;
            let Some(mut vtx1) = list1.get(s_ii).copied().flatten() else {
                // no work - vertex already processed
                ii += 1;
                continue;
            };
            let param_1 = param1.get(s_ii).copied().ok_or_else(past)?;
            let this_pt = vertex::point(b, vtx1)?;

            // process the next set of delete vertices looking for coincidence
            coin2.clear();
            let from_jj = start_jj;
            for jj in from_jj..n2 {
                let s_jj = at1(&sorted2, jj)?;
                let vtx2 = using2.get(s_jj).copied().ok_or_else(past)?;
                let param_2 = param2.get(s_jj).copied().ok_or_else(past)?;

                // skip already processed vertices
                let Some(vtx2) = vtx2 else {
                    continue;
                };
                // march past vertices who already missed their chance to be coincident
                if param_2 < param_1 - 2.0 * tol {
                    start_jj = jj + 1; // done with this and all before it.
                    continue;
                }
                // stop searching when delete vertex passes the target keep vertex
                if param_2 > param_1 + 2.0 * tol {
                    break;
                }

                // KeepToDelete vertex gap
                let dist = this_pt.distance(vertex::point(b, vtx2)?);
                if dist < tol {
                    // collect all delete vertices within tolerance of Keep vertex
                    coin2.push(jj);
                } else {
                    // look for minimum gap of unglued vertices
                    out.min_unstitched_vert_gap = smos_min(out.min_unstitched_vert_gap, dist);
                }
            }

            // If we found any vertices coincident to pVtx1, find the closest one and glue.
            let Some(&first_jj) = coin2.first() else {
                ii += 1;
                continue;
            };
            let mut ljj_to_glue = first_jj;
            let vtx2_of = |jj: usize| -> Result<VertexId> {
                using2
                    .get(at1(&sorted2, jj)?)
                    .copied()
                    .flatten()
                    .ok_or_else(past)
            };
            let mut min_dist = this_pt.distance(vertex::point(b, vtx2_of(ljj_to_glue)?)?);
            // when there are multiple candidates
            for &this_jj in coin2.iter().skip(1) {
                let this_dist = this_pt.distance(vertex::point(b, vtx2_of(this_jj)?)?);
                if this_dist < min_dist {
                    // remember the closest DeleteVertex
                    ljj_to_glue = this_jj;
                    min_dist = this_dist;
                } else {
                    // remember smallest gap between unstitched vertices
                    out.min_unstitched_vert_gap = smos_min(out.min_unstitched_vert_gap, this_dist);
                }
            }

            // It's possible that there are nearby vertices in List 1 that are closer to
            // this List 2 vertex: search List 1 in both directions from liiToGlue.
            let vtx2_to_glue = vtx2_of(ljj_to_glue)?;
            let point2 = vertex::point(b, vtx2_to_glue)?;
            let param_2 = param2
                .get(at1(&sorted2, ljj_to_glue)?)
                .copied()
                .ok_or_else(past)?;

            // Search downward first.
            if lii_to_glue > 0 {
                let mut iii = lii_to_glue - 1;
                loop {
                    let s_iii = at1(&sorted1, iii)?;
                    let param3 = param1.get(s_iii).copied().ok_or_else(past)?;
                    if param3 < param_2 - 2.0 * tol {
                        break;
                    }
                    if let Some(this_vtx1) = list1.get(s_iii).copied().flatten() {
                        let this_dist = point2.distance(vertex::point(b, this_vtx1)?);
                        if this_dist < min_dist {
                            lii_to_glue = iii;
                            min_dist = this_dist;
                        }
                    }
                    if iii == 0 {
                        break; // (Can't go negative.)
                    }
                    iii -= 1;
                }
            }

            // Now search upward.
            for iii in lii_to_glue + 1..n1 {
                let s_iii = at1(&sorted1, iii)?;
                let param4 = param1.get(s_iii).copied().ok_or_else(past)?;
                if param4 > param_2 + 2.0 * tol {
                    break;
                }
                let Some(this_vtx1) = list1.get(s_iii).copied().flatten() else {
                    continue;
                };
                let this_dist = point2.distance(vertex::point(b, this_vtx1)?);
                if this_dist < min_dist {
                    lii_to_glue = iii;
                    min_dist = this_dist;
                }
            }

            // remember when we found a closer keep vertex
            if lii_to_glue != ii {
                vtx1 = list1
                    .get(at1(&sorted1, lii_to_glue)?)
                    .copied()
                    .flatten()
                    .ok_or_else(past)?;
            }

            // Now glue pVtx1 and pVtx2ToGlue; pVtx2ToGlue will be deleted. First set the
            // surviving vertex to the midpoint, if they are far enough apart.
            let mut check_gaps = false;
            let tol2 = vertex::tolerance(b, vtx2_to_glue)?;
            if min_dist > (tol2 + tol2) / 4.0 {
                let mid = (vertex::point(b, vtx1)? + point2) * 0.5;
                self.edit_set_point(k, b, vtx1, mid)?;
                check_gaps = true;
            }

            let old_tol = vertex::tolerance(b, vtx2_to_glue)?;
            let s_lii = at1(&sorted1, lii_to_glue)?;
            let s_ljj = at1(&sorted2, ljj_to_glue)?;
            match self.edit_glue_vertices(k, b, vtx1, vtx2_to_glue, None) {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(e) => {
                    // Prevent infinite loop: if the glue failed, don't mess with these two
                    // vertices anymore.
                    set_none(list1, s_lii)?;
                    set_none(&mut using2, s_ljj)?;
                    if self.ignore_problems {
                        ii += 1;
                        continue;
                    }
                    return Err(e);
                }
            }

            // status ignored
            ignore_status(update_object_tolerance(
                k,
                b,
                EntityRef::Vertex(vtx1),
                min_dist,
                old_tol,
                check_gaps,
            ))?;
            out.max_vertex_gap = smos_max(out.max_vertex_gap, min_dist);

            // Remove from our vertex list, and add to Processed list.
            out.processed.push(vtx1);
            out.deleted.push(vtx2_to_glue);
            set_none(list1, s_lii)?;
            set_none(&mut using2, s_ljj)?;
            ii += 1;
        }

        // Since we used a copy of rVertxList2, Null out the vertices we processed.
        for (v, &idx) in using2.iter().zip(&using_indices2) {
            if v.is_none() {
                set_none(list2, idx)?;
            }
        }
        Ok(out)
    }

    /// `StitchEdgesOfVertices`: for each vertex, glues each pair of its edges that join
    /// the same two vertices and lie within `max(stitch tol, summed edge tolerances)`
    /// (midpoint drop, then a 5 point and, unless [`Stitch::fast_edge_compare`], a
    /// precise curve distance; lines skip both), keeping the first. Making a manifold
    /// solid skips edges with four or more edge uses and finishes an edge after one
    /// glue.
    ///
    /// # Errors
    ///
    /// Those of the curve evaluations and distances (`SER`); a failing glue, or an edge
    /// without B-rep, unless [`Stitch::ignore_problems`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn stitch_edges_of_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        verts: &[Option<VertexId>],
    ) -> Result<EdgesOfVertices> {
        // Init outputs.
        let mut out = EdgesOfVertices {
            glued: Vec::new(),
            deleted: Vec::new(),
            max_edge_gap: 0.0,
            min_unstitched_edge_gap: BIG_DOUBLE,
        };

        for &v in verts {
            let Some(v) = v else {
                continue; // Can this happen? Don't really care.
            };

            let mut v_edges: Vec<Option<EdgeId>> =
                vertex::edges(b, v, None)?.into_iter().map(Some).collect();
            let n_v_edges = v_edges.len();

            // Outer loop on all of this vertex's Edges:
            let mut jj = 0usize;
            while jj + 1 < n_v_edges {
                let Some(e) = v_edges.get(jj).copied().flatten() else {
                    jj += 1;
                    continue;
                };

                // If we are making a manifold solid only stitch lamina edges. Note,
                // don't check IsLamina, just avoid making spine edges. [100216]
                if self.making_manifold_solid && edge::edgeuses(b, e, None, None)?.len() >= 4 {
                    jj += 1;
                    continue;
                }

                // Inner loop on all of this vertex's other Edges:
                for kk in jj + 1..n_v_edges {
                    let Some(test_e) = v_edges.get(kk).copied().flatten() else {
                        continue;
                    };
                    if self.making_manifold_solid
                        && edge::edgeuses(b, test_e, None, None)?.len() >= 4
                    {
                        continue;
                    }

                    // skip edges that don't share common vertices on both ends
                    if edge::other_vertex(b, e, Some(v))? != edge::other_vertex(b, test_e, Some(v))?
                    {
                        continue;
                    }

                    // Found a pair of edges with the same two vertices. Check for
                    // coincidence; first check midpoints.
                    let e_ivl = edge::interval(b, e)?;
                    let te_ivl = edge::interval(b, test_e)?;
                    let e_curve = curve_of(b, e)?;
                    let te_curve = curve_of(b, test_e)?;
                    let mid_pnt = e_curve.evaluate_point(k, e_ivl.evaluate(0.5))?;

                    // select new tolerance
                    let this_tol = smos_max(
                        self.tol_3d,
                        edge::tolerance(b, test_e)? + edge::tolerance(b, e)?,
                    );

                    // To find rdMinUnstitchedEdgeGap, collect every solution.
                    let guess = te_ivl.mid();
                    let dropped = te_curve.drop_point(
                        k,
                        &te_ivl,
                        mid_pnt,
                        None,
                        BIG_DOUBLE,
                        Some(guess),
                        SolverOperation::Minimize,
                    )?;
                    if !dropped.success {
                        continue;
                    }
                    if dropped.distance > this_tol {
                        out.min_unstitched_edge_gap =
                            smos_min(dropped.distance, out.min_unstitched_edge_gap);
                        continue;
                    }

                    // Ok, the midpoints are close. Figure out the relative orientation.
                    let pv = e_curve.evaluate(k, e_ivl.evaluate(0.5), 1, true, true)?;
                    let pv_test = te_curve.evaluate(k, dropped.parameter, 1, true, true)?;
                    let mut orientation = OrientType::Same;
                    let (mut t_start, mut t_end) = (te_ivl.min(), te_ivl.max());
                    if tangent(&pv)?.dot(tangent(&pv_test)?) < 0.0 {
                        orientation = OrientType::Opposite;
                        core::mem::swap(&mut t_start, &mut t_end);
                    }

                    // Skip edge pairs not within tolerance of one another.
                    let mut dist_to_curve = 0.0;

                    // If both edges are lines then we don't have to do the harder test.
                    let edges_are_lines = (e_curve.degree()? == 1
                        && e_curve.number_natural_knots(k)? == 4)
                        && (te_curve.degree()? == 1 && te_curve.number_natural_knots(k)? == 4);
                    if !edges_are_lines {
                        // do a quick 5 point distance check
                        dist_to_curve = e_curve
                            .curve_max_distance_between(
                                k,
                                &e_ivl,
                                te_curve,
                                t_start,
                                t_end,
                                5,
                                Some(this_tol),
                            )?
                            .distance;
                        if dist_to_curve > this_tol {
                            out.min_unstitched_edge_gap =
                                smos_min(dist_to_curve, out.min_unstitched_edge_gap);
                            continue;
                        }

                        // do a precise distance check
                        if !self.fast_edge_compare {
                            dist_to_curve = e_curve
                                .curve_max_distance_between(
                                    k,
                                    &e_ivl,
                                    te_curve,
                                    t_start,
                                    t_end,
                                    0,
                                    Some(this_tol),
                                )?
                                .distance;
                            if dist_to_curve > this_tol {
                                out.min_unstitched_edge_gap =
                                    smos_min(dist_to_curve, out.min_unstitched_edge_gap);
                                continue;
                            }
                        }
                    }

                    // If make it here then we need to glue edges; save the maxEdgeGap
                    if dist_to_curve > out.max_edge_gap {
                        out.max_edge_gap = dist_to_curve;
                    }

                    // We just keep pE, delete pTestE.
                    if edge::brep(b, e)?.is_none() {
                        if self.ignore_problems {
                            continue;
                        }
                        return Err(sm_err());
                    }

                    // glue pE/pTestE edge pair - keep pE
                    let old_tol = edge::tolerance(b, test_e)?;
                    match self.edit_glue_edges(
                        k,
                        b,
                        e,
                        orientation,
                        dist_to_curve,
                        self.do_region_nesting,
                        test_e,
                        None,
                    ) {
                        Ok(()) => {
                            out.glued.push(e);
                            out.deleted.push(test_e);
                            ignore_status(update_object_tolerance(
                                k,
                                b,
                                EntityRef::Edge(e),
                                dist_to_curve,
                                old_tol,
                                false,
                            ))?;
                            set_none(&mut v_edges, kk)?; // per notes above
                            if !self.making_manifold_solid {
                                // spine edges are allowed: just go on to the next [kk]
                                continue;
                            }
                            // spine edges not allowed, so we're done with both of these
                            // edges. go on to the next outer-loop [jj]
                            set_none(&mut v_edges, jj)?;
                            break;
                        }
                        Err(err) if err.is_unported() => return Err(err),
                        Err(_) if !self.ignore_problems => return Err(sm_err()),
                        Err(_) => {}
                    }
                }
                jj += 1;
            }
        }
        Ok(out)
    }
}

/// `SmExtent1d::Inversion`'s `SM_UNDEF_DOUBLE` for a degenerate interval.
const UNDEF_PARAM: f64 = crate::math::UNDEF_DOUBLE;

/// The tangent of an `Evaluate(t, 1, ..)` result.
fn tangent(pv: &[Vec3]) -> Result<Vec3> {
    pv.get(1).copied().ok_or_else(sm_err)
}

fn set_none<T>(list: &mut [Option<T>], i: usize) -> Result<()> {
    *list
        .get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(OP, "index outside the list"))? = None;
    Ok(())
}

/// The insertion sort of `StitchVertexPairs`: indices `0..n` (skipping those `is_null`
/// reports) ordered by `param`, each inserted at the end, at the front or at its binary
/// search slot.
pub(crate) fn sort_by_param(
    n: usize,
    is_null: impl Fn(usize) -> bool,
    param: &[f64],
) -> Result<Vec<usize>> {
    let past = || SolidError::invalid_input(OP, "upstream reads past its parameter array");
    let p = |i: usize| param.get(i).copied().ok_or_else(past);
    let mut sorted: Vec<usize> = Vec::new();
    for ii in 0..n {
        if is_null(ii) {
            continue;
        }
        let t = p(ii)?;
        // init sSortedIndex with 1st entry
        let (Some(&first), Some(&last)) = (sorted.first(), sorted.last()) else {
            sorted.push(ii);
            continue;
        };
        let mut lmin = 0usize;
        let mut lmax = sorted.len() - 1;
        if t >= p(last)? {
            // add entry to end to list when its Param is largest seen
            sorted.push(ii);
        } else if t <= p(first)? {
            // add entry to begin of list when its param is smallest seen
            sorted.insert(0, ii);
        } else {
            // Binary search (unsigned arithmetic as upstream).
            while lmin != lmax.wrapping_sub(1) && lmin != lmax {
                let mid = (lmin + lmax) / 2;
                if t > p(sorted.get(mid).copied().ok_or_else(past)?)? {
                    lmin = mid;
                } else {
                    lmax = mid;
                }
            }
            if lmax > sorted.len() {
                return Err(past());
            }
            sorted.insert(lmax, ii);
        }
    }
    Ok(sorted)
}

/// `SmVertex::SetPoint`: the change notification (on records local to the call), then
/// the new position.
pub(crate) fn set_point(k: &mut Kernel, b: &mut Brep, v: VertexId, point: Point3) -> Result<()> {
    let mut records = crate::topology::topology::NotifyRecords::new();
    let mut attributes = AttributeStore::default();
    let this = arena_identity(CacheObject::Vertex(v));
    let vertexuse_ref = |vu: crate::topology::VertexuseId| ObjectRef {
        type_code: vertexuse::TYPE,
        key: ObjectKey(u64::from(vu.index())),
    };
    vertex::set_point(
        b,
        v,
        point,
        &mut records,
        &mut attributes,
        Some(&mut k.context),
        this,
        &vertexuse_ref,
    )
}

// ---- unported callees ----------------------------------------------------------------

/// `SmTol::UpdateObjectTolerance(SmTopology*, dThisGap, dThisTol, bCheckGaps)`:
/// [`tol::update_object_tolerance`] (co-ported by `boolean.topology_intersector_p1`)
/// on a vertex, edge or face; any other object is passed as `None` (no work).
pub(crate) fn update_object_tolerance(
    k: &Kernel,
    b: &mut Brep,
    object: EntityRef,
    gap: f64,
    this_tol: f64,
    check_gaps: bool,
) -> Result<()> {
    let topo = match object {
        EntityRef::Vertex(v) => Some(TopologyToUpdate::Vertex(v)),
        EntityRef::Edge(e) => Some(TopologyToUpdate::Edge(e)),
        EntityRef::Face(f) => Some(TopologyToUpdate::Face(f)),
        _ => None,
    };
    tol::update_object_tolerance(k, b, topo, gap, this_tol, check_gaps)
}

/// `SmBrep::GlueVertices(keep, delete, pOptWireEdges)` (unit `topology.brep_p5`):
/// [`crate::topology::brep::p5::glue_vertices`]. Upstream only reads the wire list.
pub(crate) fn glue_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    keep: VertexId,
    delete: VertexId,
    wire_edges: Option<&[EdgeId]>,
) -> Result<()> {
    crate::topology::brep::p5::glue_vertices(k, b, Some(keep), Some(delete), wire_edges)
}

/// `SmBrep::RemoveFaces` (unit `topology.brep_p5`):
/// [`crate::topology::brep::p5::remove_faces`], with the B-rep's own editing flag
/// (`m_bEditingEnabled`).
pub(crate) fn remove_faces(k: &mut Kernel, b: &mut Brep, faces: &[FaceId]) -> Result<()> {
    let editing = b.editing_enabled();
    crate::topology::brep::p5::remove_faces(k, b, editing, faces)
}

/// `SmBrep::StitchAndOrient(dStitchTol3d, bAllowSpineEdges, pOptFacesToSwap,
/// pbJustUnify)` (unit `topology.brep_p5`; header defaults -0.1, FALSE, NULL, NULL).
fn stitch_and_orient(
    k: &mut Kernel,
    b: &mut Brep,
    stitch_tol_3d: f64,
    allow_spine_edges: bool,
    faces_to_swap: Option<&[FaceId]>,
    just_unify: Option<bool>,
) -> Result<()> {
    crate::topology::brep::p5::stitch_and_orient(
        k,
        b,
        &crate::topology::brep::p5::StitchAndOrientOptions {
            stitch_tolerance: stitch_tol_3d,
            allow_spine_edges,
            faces_to_swap: faces_to_swap.map(<[FaceId]>::to_vec),
            just_unify,
        },
    )
}

/// `SmBrep::GlueEdgesGeneral(keep, orientation, dist, bDoRegionNesting, delete,
/// pInsideFaceuses)` (unit `topology.brep_p7`; header default `pInsideFaceuses = NULL`):
/// [`crate::topology::brep::p7::glue_edges_general`]. Upstream notifies the B-rep's
/// own `SmTopology` objects through its context; the heal path keeps no records of
/// its own, so the call runs on a default record per arena object
/// ([`brep_p2::ArenaNotifyState`](crate::topology::brep::p2::ArenaNotifyState), so
/// that `SmTopology::Notify`'s propagation finds both edges) and on the kernel's
/// notification context, so its installed callbacks see the events.
pub(crate) fn glue_edges_general(
    k: &mut Kernel,
    b: &mut Brep,
    keep: EdgeId,
    orientation: OrientType,
    dist: f64,
    do_region_nesting: bool,
    delete: EdgeId,
    inside_faceuses: Option<&[FaceuseId]>,
) -> Result<()> {
    // (not upstream) with `b`'s own attributes, given back afterwards
    let mut state = crate::topology::brep::p2::ArenaNotifyState::with_attributes_of(b);
    let r = Context::with_notification_context(k, |k, context| {
        let mut env = state.env(Some(context));
        crate::topology::brep::p7::glue_edges_general(
            k,
            b,
            Some(keep),
            orientation,
            dist,
            do_region_nesting,
            Some(delete),
            inside_faceuses,
            &mut env,
        )
    });
    let restored = state.restore_attributes(b);
    r??;
    restored
}

/// `SmBrep::SqueezeEdge(edge, keep)` (unit `topology.brep_p3`,
/// [`crate::topology::brep::p3::squeeze_edge`]), notifying through the kernel
/// context's callbacks under the kernel's identities
/// ([`crate::topology::brep::p3::with_kernel_callbacks`]).
///
/// # Errors
///
/// Those of the squeeze; `SM_ERR_NULL_POINTER` for a null edge (upstream dereferences
/// it).
pub(crate) fn squeeze_edge_to(
    k: &mut Kernel,
    b: &mut Brep,
    e: Option<EdgeId>,
    keep: VertexId,
) -> Result<()> {
    let e = e.ok_or_else(|| SolidError::null_pointer("heal::stitch::p1::squeeze_edge_to"))?;
    crate::topology::brep::p3::with_kernel_callbacks(k, b, |k, b, env| {
        crate::topology::brep::p3::squeeze_edge(k, b, env, e, keep)
    })
}

/// `SmBrep::MakeVertexSplitEdge(edge, t, rpE1, rpE2, rpNewV)` (unit `topology.brep_p9`),
/// with its notifications on an arena-only state local to the call
/// ([`crate::topology::brep::p2::ArenaNotifyState`]). A refused split (`SM_ERR` with
/// the nearby endpoint) is an `SM_ERR` error: the caller only tests for `SM_SUCCESS`.
///
/// (not upstream) The state holds `b`'s own attributes for the split and gives them
/// back afterwards, so both pieces of a split edge keep its attributes
/// (`SM_NO_SPLIT_IN_BREP`), as upstream's live on the edge.
pub(crate) fn make_vertex_split_edge(
    k: &mut Kernel,
    b: &mut Brep,
    e: EdgeId,
    t: f64,
) -> Result<(Option<EdgeId>, Option<EdgeId>, Option<VertexId>)> {
    // a default record per object (upstream every object has its `SmTopology` part)
    let mut state = crate::topology::brep::p2::ArenaNotifyState::with_attributes_of(b);
    // The kernel's context is the split's own `k`; no separate object context.
    let out = crate::topology::brep::p9::make_vertex_split_edge(
        k,
        b,
        e,
        t,
        &mut state.env(None),
        &|edge, kind| local_identity(kind, edge.index()),
    );
    let restored = state.restore_attributes(b);
    let out = out?;
    restored?;
    if !out.split {
        return Err(SolidError::sm_err(OP));
    }
    let [e1, e2] = out.edges.ok_or_else(|| ner("split without edges"))?;
    Ok((Some(e1), Some(e2), out.vertex))
}

/// A call-local identity: the type code above the index, outside the arena identities'
/// key space (bare indices).
fn local_identity(type_code: i64, index: u32) -> ObjectRef {
    ObjectRef {
        type_code,
        key: ObjectKey((type_code.unsigned_abs() << 32) | u64::from(index)),
    }
}

/// `SmBrep::Notify(SM_NO_PRE_EDIT, this, NULL, NULL)` ([`crate::topology::brep::p2::notify`]).
/// No Rust caller holds the B-rep's `SM_OC_BREP` cache, and the stitcher, like the rest
/// of the heal path, carries no attribute state (`plan/findings.md`, "heal: no attribute
/// state on the heal path"), so the notification runs on an empty state; `PRE_EDIT` reads
/// no topology record.
pub(crate) fn brep_notify_pre_edit(b: &mut Brep) -> Result<()> {
    use crate::topology::brep::p2;
    let mut state = p2::RegionNotifyState::new(b);
    let this = Some(p2::NotifyArg::Object(p2::brep_ref(b)));
    p2::notify(
        b,
        &mut state.env(),
        None,
        NotifyOperation::PreEdit,
        [this, None, None],
    )
}

/// Whether `SmFace::GetBrep()` is `b` (unit `topology.face_p4`): a face of this arena
/// whose shell's region is in the B-rep.
fn face_in_brep(b: &Brep, f: FaceId) -> Result<bool> {
    Ok(crate::topology::face::p4::brep(b, f)?.is_some())
}

/// `SmFace::GetVertices(rVertices)` (unit `topology.face_p4`; default arguments).
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    crate::topology::face::p4::vertices(b, f, None, false, None)
}

/// `SmFace::GetEdges(rEdges)` (unit `topology.face_p4`; default arguments).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

/// `SmEdgeuse::NormalizedEvaluate(t, bParameterSpaceEval, rPoint, pOptFirstDeriv,
/// bOKToMakeUVTrimCurve)` (`topology::edgeuse::p3::normalized_evaluate`; header defaults
/// `pOptFirstDeriv = NULL`, `bOKToMakeUVTrimCurve = TRUE`): the point and, when
/// `want_first_deriv`, the first derivative.
fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    parameter_space_eval: bool,
    want_first_deriv: bool,
    ok_to_make_uv_trim_curve: bool,
) -> Result<(Point3, Option<Vec3>)> {
    let eval = eu3::normalized_evaluate(
        k,
        b,
        eu,
        t,
        parameter_space_eval,
        want_first_deriv,
        ok_to_make_uv_trim_curve,
    )?;
    Ok((eval.point, eval.first_deriv))
}

/// `SmEdgeuse::EvaluateBinormal(t, bUse3DOnly, rBinormalPoint, rBinormal,
/// pOptEdgeuseTangent, pOptFaceuseNormal, pOptUVParameter, pOptUVDir)`
/// (`topology::edgeuse::p3::evaluate_binormal`; the four optional outputs default to
/// NULL): the point and the binormal (the stitch asks for neither optional output).
fn evaluate_binormal(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    use_3d_only: bool,
) -> Result<(Point3, Vec3)> {
    let eval = eu3::evaluate_binormal(k, b, eu, t, use_3d_only, &BinormalOptions::default())?;
    Ok((eval.point, eval.binormal))
}

// ---- SM_API entry points ---------------------------------------------------------------

/// `SmApiStitch(SmBrep*)`: `StitchAndOrient` (status ignored), then success.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep.
pub fn api_stitch(k: &mut Kernel, b: Option<&mut Brep>) -> Result<()> {
    let b = b.ok_or_else(|| SolidError::invalid_input(API_OP, "SmApiStitch: null B-rep"))?;
    // StitchAndOrient() with its header defaults
    ignore_status(stitch_and_orient(k, b, -0.1, false, None, None))?;
    // Check manifold?
    Ok(())
}

/// `SmApiStitch(SmBrep*, SmFace*)`: merges the face into the B-rep
/// (`SmMerge::MergeAddedFace` with the API context), through a merge with a null other
/// B-rep and the default tolerance, which upstream dereferences (UB-167).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep or face, and from the merge
/// constructor for the null other B-rep (after the primary's
/// `SmBrep::GetMinEdgeBBoxSize`).
pub fn api_stitch_face(k: &mut Kernel, b: Option<&mut Brep>, face: Option<FaceId>) -> Result<()> {
    let (Some(b), Some(face)) = (b, face) else {
        return Err(SolidError::invalid_input(
            API_OP,
            "SmApiStitch: null B-rep or face",
        ));
    };
    // SmMerge sMerge(*pContext, pBrep, NULL): the default tolerance 0 is computed from
    // both B-reps, and upstream dereferences the null other B-rep there (UB-167). The
    // constructor returns that error (after the primary's GetMinEdgeBBoxSize), so the
    // MergeAddedFace that follows upstream is never reached.
    let merge = MergeState::<TopologyIntersector<MergeOperand>>::with_breps(
        k,
        (&*b, MergeOperand::Primary),
        None,
        0.0,
        0.0,
        None,
    )?;
    let _ = (k, merge, face);
    Err(SolidError::invalid_input(
        API_OP,
        "SmApiStitch: SmMerge with a null other B-rep (UB-167)",
    ))
}

/// The outputs of the goal-directed and advanced stitch entry points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvancedStitch {
    /// Whether the result is a solid (`rbProducesASolid`; [`api_stitch_into_solid`]
    /// only).
    pub produces_a_solid: bool,
    /// Number of edges stitched (`rlStitchedEdges`).
    pub stitched_edges: u64,
    /// Number of lamina edges left ([`api_stitch_advanced`] only).
    pub lamina_edges: u64,
    /// Largest vertex gap considered (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Largest edge gap considered (`rdMaxEdgeGap`).
    pub max_edge_gap: f64,
}

/// `SmApiStitchIntoSolid`: `SmStitch::StitchIntoSolid`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep; those of the stitch.
pub fn api_stitch_into_solid(k: &mut Kernel, b: Option<&mut Brep>) -> Result<AdvancedStitch> {
    let b =
        b.ok_or_else(|| SolidError::invalid_input(API_OP, "SmApiStitchIntoSolid: null B-rep"))?;
    super::p2::stitch_into_solid(k, b)
}

/// `SmApiStitchIntoShell`: `SmStitch::StitchIntoShell`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep; those of the stitch.
pub fn api_stitch_into_shell(
    k: &mut Kernel,
    b: Option<&mut Brep>,
    shell_is_well_formed: bool,
    max_stitching_ratio: f64,
) -> Result<AdvancedStitch> {
    let b =
        b.ok_or_else(|| SolidError::invalid_input(API_OP, "SmApiStitchIntoShell: null B-rep"))?;
    super::p2::stitch_into_shell(k, b, shell_is_well_formed, max_stitching_ratio)
}

/// `SmApiUnifyNormals`: [`Stitch::unify_normals`] with the default callback and the
/// B-rep tolerance as stitch tolerance.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep or start face; those of
/// [`Stitch::unify_normals`].
pub fn api_unify_normals(
    k: &mut Kernel,
    b: Option<&mut Brep>,
    start_face: Option<FaceId>,
    num_samples: u64,
) -> Result<Vec<FaceId>> {
    let (Some(b), Some(start_face)) = (b, start_face) else {
        return Err(SolidError::invalid_input(
            API_OP,
            "SmApiUnifyNormals: null B-rep or face",
        ));
    };
    let tol = b.tolerance();
    let mut callback = DefaultStitchCallback::new();
    let mut stitch = Stitch::new(&mut callback, tol, true, true);
    stitch.unify_normals(k, b, start_face, num_samples)
}

/// `SmApiSimpleFaceStitch`: [`Stitch::do_simple_face_stitch`] with the default
/// callback and the B-rep tolerance as stitch tolerance.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep; those of
/// [`Stitch::do_simple_face_stitch`].
pub fn api_simple_face_stitch(
    k: &mut Kernel,
    b: Option<&mut Brep>,
    faces_to_keep: &[FaceId],
    faces_to_delete: &[FaceId],
) -> Result<SimpleFaceStitch> {
    let b =
        b.ok_or_else(|| SolidError::invalid_input(API_OP, "SmApiSimpleFaceStitch: null B-rep"))?;
    let tol = b.tolerance();
    let mut callback = DefaultStitchCallback::new();
    let mut stitch = Stitch::new(&mut callback, tol, true, true);
    stitch.do_simple_face_stitch(k, b, faces_to_keep, faces_to_delete)
}

/// The options of [`api_stitch_advanced`] (the `SmStitch` flags it exposes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // upstream's flag arguments
pub struct AdvancedStitchOptions {
    /// `bSqueezeSmallEdges`.
    pub squeeze_small_edges: bool,
    /// `bSplitEdgesWithVertices`.
    pub split_edges_with_vertices: bool,
    /// `bMakingManifoldSolid`.
    pub making_manifold_solid: bool,
    /// `bRemoveLaminarSlivers`.
    pub remove_laminar_slivers: bool,
}

/// `SmApiStitchAdvanced`: `SmStitch::DoStitching` over every vertex and edge with the
/// given tolerance and options.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null B-rep; those of the stitch.
pub fn api_stitch_advanced(
    k: &mut Kernel,
    b: Option<&mut Brep>,
    stitch_tol_3d: f64,
    options: AdvancedStitchOptions,
) -> Result<AdvancedStitch> {
    let b =
        b.ok_or_else(|| SolidError::invalid_input(API_OP, "SmApiStitchAdvanced: null B-rep"))?;
    let mut callback = DefaultStitchCallback::new();
    let mut stitch = Stitch::new(&mut callback, stitch_tol_3d, true, true);
    stitch.squeeze_small_edges = options.squeeze_small_edges;
    stitch.split_edges_with_vertices = options.split_edges_with_vertices;
    stitch.making_manifold_solid = options.making_manifold_solid;
    stitch.remove_laminar_slivers = options.remove_laminar_slivers;
    let r = stitch.do_stitching(k, b, None, None, super::p2::MinGapRequest::default())?;
    Ok(AdvancedStitch {
        produces_a_solid: false,
        stitched_edges: r.stitched_edges,
        lamina_edges: r.lamina_edges,
        max_vertex_gap: r.max_vertex_gap,
        max_edge_gap: r.max_edge_gap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::object::NotifyOperation;
    use crate::topology::EdgeuseOwner;

    #[test]
    fn actual_stitch_split_keeps_curve_identity_and_refusal_prefix() {
        use crate::curve::curve::Curve;
        use crate::geom::attribute::{ids, Attribute, Behavior};
        use crate::topology::brep::{p3, p9};
        use crate::topology::edge::notification_test_support as callbacks;
        use std::cell::RefCell;
        for refuse_copy in [false, true] {
            let mut k = Kernel::default();
            let mut b = crate::primitive::api_primitives::create_box(
                &mut k,
                Point3::ZERO,
                10.0,
                20.0,
                30.0,
            )
            .unwrap();
            b.set_editing_enabled(true);
            let edge = b.edge_list()[0];
            let serial = b.notify_serial().unwrap();
            let key_of = |entity| p3::topology_ref(serial, entity);
            let interval = b.edge(edge).unwrap().interval;
            let parameter = interval.evaluate(0.5);
            let original = b.edge_mut(edge).unwrap().curve.as_mut().unwrap();
            let original_key = k
                .context
                .geometry_key(original.notify_identity_slot())
                .unwrap();
            let before = (b.edge_list().len(), b.vertex_list().len());
            let mut records = crate::topology::topology::NotifyRecords::new();
            p3::seed_records(&mut records, &b, &key_of);
            let mut attributes = AttributeStore::default();
            let marker = attributes
                .insert(Attribute::long(ids::USER_1, 73, Behavior::Copy))
                .unwrap();
            attributes.add_attribute(original_key, marker).unwrap();
            let events = callbacks::install(&mut k.context);
            let issued = RefCell::new(Vec::new());
            let issuer = |id: EdgeId, curve: &mut Curve, env: &mut edge::NotifyEnv<'_>| {
                assert!(env.context.as_deref().unwrap().doing_boolean());
                if refuse_copy && id != edge {
                    assert_eq!(curve.notify_key(), None);
                    return Err(SolidError::invalid_input(
                        "test stitch issuer",
                        "refused actual copy",
                    ));
                }
                let identity = p9::issue_curve_identity(id, curve, env)?;
                issued.borrow_mut().push((id, identity));
                Ok(identity)
            };
            k.context.set_doing_boolean(true);
            let result = Context::with_notification_context(&mut k, |k, context| {
                let mut env = edge::NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: Some(context),
                    key_of: &key_of,
                };
                let mut edits = BorrowedStitchEdits {
                    env: &mut env,
                    curve_identity: p9::CurveIdentity::Actual(&issuer),
                };
                edits.split_edge(k, &mut b, edge, parameter)
            })
            .unwrap();
            assert!(k.context.doing_boolean());
            callbacks::assert_installed(&k.context, &events);
            assert_eq!(
                (b.edge_list().len(), b.vertex_list().len()),
                (before.0 + 1, before.1 + 1)
            );
            assert_eq!(issued.borrow()[0].1.key, original_key);
            let new_edge = *b.edge_list().last().unwrap();
            if refuse_copy {
                assert!(result.is_err());
                assert_eq!(issued.borrow().len(), 1);
                assert!(b.edge(new_edge).unwrap().curve.is_none());
                assert!(!events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.operation == NotifyOperation::Split));
            } else {
                let (_, Some(created), Some(_)) = result.unwrap() else {
                    panic!("missing real split outputs")
                };
                assert_eq!(created, new_edge);
                let key = issued.borrow()[1].1.key;
                assert_ne!(key, original_key);
                assert_ne!(key, key_of(EntityRef::Edge(created)).key);
                let curve = b.edge(created).unwrap().curve.as_ref().unwrap();
                assert_eq!(curve.notify_key(), Some(key));
                assert_eq!(curve.owner(), Some(key_of(EntityRef::Edge(created))));
                let copied = attributes.find_attribute(key, ids::USER_1).unwrap();
                assert_ne!(copied, marker);
                assert_eq!(attributes.get(copied).unwrap().long_value(), Some(73));
                assert!(events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event.operation == NotifyOperation::Split));
            }
        }
    }

    #[test]
    fn sort_by_param_matches_insertion_order() {
        let p = [3.0, 1.0, 2.0, 1.0, 5.0];
        let s = sort_by_param(5, |_| false, &p).unwrap();
        assert_eq!(s, vec![3, 1, 2, 0, 4]);
    }

    #[test]
    fn stitch_defaults() {
        let mut cb = DefaultStitchCallback::new();
        let s = Stitch::new(&mut cb, 0.01, true, true);
        assert!(s.split_edges_with_vertices && s.making_manifold_solid);
        assert!(!s.ignore_problems && !s.fast_edge_compare && !s.squeeze_small_edges);
        assert!(!s.remove_laminar_slivers);
        assert!((s.stitch_tol_3d() - 0.01).abs() < 1e-15);
    }

    #[test]
    fn contextual_notifications_use_actual_attribute_users_and_callback() {
        use crate::geom::attribute::{ids, Attribute, Behavior};
        use crate::topology::brep::p3::{notify_serial, topology_ref};
        let (mut k, events) = recording_kernel();
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let face = b.add_face(shell).unwrap();
        let copied_face = b.add_face(shell).unwrap();
        let vertex = b.add_vertex(Point3::ZERO).unwrap();
        let serial = notify_serial(&mut k.context, &mut b).unwrap();
        let key_of = |entity| topology_ref(serial, entity);
        let face_key = key_of(EntityRef::Face(face)).key;
        let vertex_key = key_of(EntityRef::Vertex(vertex)).key;
        let copied_face_key = key_of(EntityRef::Face(copied_face)).key;
        let mut attributes = AttributeStore::new();
        let marker = attributes
            .insert(Attribute::new(
                ids::ALL_FILLET_EDGES,
                Behavior::StandaloneReference,
            ))
            .unwrap();
        attributes.add_attribute(face_key, marker).unwrap();
        attributes.add_attribute(vertex_key, marker).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        records.insert(face_key, Topology::new());
        records.insert(copied_face_key, Topology::new());
        records.insert(vertex_key, Topology::new());
        let reject_curve = |_, _| {
            Err(SolidError::Unported {
                upstream: "unregistered split curve",
                unit: "topology.brep_p9",
            })
        };
        Context::with_notification_context(&mut k, |k, context| {
            let mut env = edge::NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: Some(context),
                key_of: &key_of,
            };
            let mut edits = BorrowedStitchEdits {
                env: &mut env,
                curve_identity: crate::topology::brep::p9::CurveIdentity::Checked(&reject_curve),
            };
            let mut callback = DefaultStitchCallback::new();
            let mut stitch = Stitch::new(&mut callback, 0.01, true, true);
            stitch.edits = Some(&mut edits);
            stitch.edit_set_point(k, &mut b, vertex, Point3::X).unwrap();
            stitch
                .edit_face_notify(
                    &mut b,
                    face,
                    NotifyOperation::Copy,
                    Some(EntityRef::Face(copied_face)),
                    None,
                )
                .unwrap();
        })
        .unwrap();
        assert_eq!(b.vertex(vertex).unwrap().point, Point3::X);
        assert_eq!(
            attributes.users(marker).unwrap(),
            vec![face_key, vertex_key, copied_face_key]
        );
        assert_eq!(attributes.attributes(copied_face_key), vec![marker]);
        assert!(events
            .lock()
            .unwrap()
            .contains(&NotifyOperation::ChangeGeometry));
        assert!(events.lock().unwrap().contains(&NotifyOperation::Copy));
    }

    /// A notify callback that records the operations it sees.
    #[derive(Debug, Default)]
    struct Recorder(std::sync::Arc<std::sync::Mutex<Vec<NotifyOperation>>>);

    impl crate::geom::context::NotifyCallback for Recorder {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            _caller: ObjectRef,
            _data1: Option<ObjectRef>,
            _data2: Option<ObjectRef>,
            _data3: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.lock().unwrap().push(operation);
            Ok(())
        }
    }

    /// A kernel with a recording user notify callback, and the record it writes.
    fn recording_kernel() -> (
        Kernel,
        std::sync::Arc<std::sync::Mutex<Vec<NotifyOperation>>>,
    ) {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut k = Kernel::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Recorder(events.clone()))));
        (k, events)
    }

    /// Two vertices, a shell and `n` edges between the vertices, each with two edge
    /// uses owned by `owner` (a shell: wire edges; a loop use: face-bounded edges).
    fn edges_between(b: &mut Brep, owner: EdgeuseOwner, n: usize) -> Vec<EdgeId> {
        use crate::geom::extent1d::Extent1d;
        use crate::topology::{Edge, Edgeuse, Vertex, Vertexuse, VertexuseOwner};
        let mut vertex = |point| {
            b.add_vertex_record(Vertex {
                point,
                ..Vertex::default()
            })
            .unwrap()
        };
        let (a, c) = (vertex(Vec3::ZERO), vertex(Vec3::X));
        (0..n)
            .map(|_| {
                let e = b
                    .add_edge_record(Edge {
                        vertices: [a, c],
                        edgeuses: Vec::new(),
                        interval: Extent1d::from_raw(0.0, 1.0),
                        tolerance: 0.0,
                        curve: None,
                    })
                    .unwrap();
                for (v, orientation) in [(a, OrientType::Same), (c, OrientType::Opposite)] {
                    let eu = b
                        .add_edgeuse_record(Edgeuse {
                            edge: Some(e),
                            owner,
                            orientation,
                            ..Edgeuse::default()
                        })
                        .unwrap();
                    let vu = b
                        .add_vertexuse_record(Vertexuse {
                            vertex: Some(v),
                            owner: VertexuseOwner::Edgeuse(Some(eu)),
                            ..Vertexuse::default()
                        })
                        .unwrap();
                    b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
                    b.vertex_mut(v).unwrap().vertexuses.push(vu);
                    b.edge_mut(e).unwrap().edgeuses.push(eu);
                }
                e
            })
            .collect()
    }

    #[test]
    fn glue_edges_general_forwards_callbacks_and_defers_at_delete_edge() {
        use crate::topology::Shell;
        // Two wire edges between the same vertices: the wrapper runs the part 7 routine
        // (its notifies are live since #149) on the kernel's context, so the installed
        // callback sees the B-rep PreEdit that upstream emits (SmBrep.cpp:9840-9848)
        // first, then the wire edge's `SmBrep::DeleteEdge` (topology.brep_p4) runs,
        // and the callback is back on the kernel afterwards. The same edge twice is the
        // upstream no-op.
        let (mut k, events) = recording_kernel();
        let mut b = Brep::new();
        let shell = b.add_shell_record(Shell::default()).unwrap();
        let edges = edges_between(&mut b, EdgeuseOwner::Shell(Some(shell)), 2);
        let (e0, e1) = (edges[0], edges[1]);
        glue_edges_general(&mut k, &mut b, e0, OrientType::Same, 0.0, false, e1, None).unwrap();
        let seen = events.lock().unwrap().len();
        assert_eq!(
            events.lock().unwrap().first(),
            Some(&NotifyOperation::PreEdit)
        );
        assert!(k.context.user_notify_callback().is_some());
        glue_edges_general(&mut k, &mut b, e0, OrientType::Same, 0.0, false, e0, None).unwrap();
        assert_eq!(events.lock().unwrap().len(), seen);
    }

    #[test]
    fn glue_edges_general_notifies_merge_in_brep_for_face_bounded_edges() {
        use crate::topology::Loopuse;
        // Face-bounded (non-wire) edges reach part 7's SM_NO_MERGE_IN_BREP notify
        // (SmBrep.cpp:9854), whose SmTopology propagation needs a record for both
        // edges: with the seeded arena records it runs and the callback sees it (with
        // an empty record map it would fail as "not a topology object", swallowed).
        let (mut k, events) = recording_kernel();
        let mut b = Brep::new();
        let lu = b.add_loopuse_record(Loopuse::default()).unwrap();
        let edges = edges_between(&mut b, EdgeuseOwner::Loopuse(Some(lu)), 2);
        let result = glue_edges_general(
            &mut k,
            &mut b,
            edges[0],
            OrientType::Same,
            0.0,
            false,
            edges[1],
            None,
        );
        let seen = events.lock().unwrap().clone();
        assert!(
            seen.contains(&NotifyOperation::MergeInBrep),
            "{seen:?} {result:?}"
        );
        // Past the notify this minimal fixture is degenerate (its loop use has no face
        // use): part 7 stops at its face lookup, where upstream would dereference null
        // (SmEdgeuse.cpp:336), not at a notification failure.
        assert!(
            matches!(&result, Err(SolidError::InvalidInput { reason, .. }) if reason.contains("loop use without face use")),
            "{result:?}"
        );
        assert!(k.context.user_notify_callback().is_some());
    }
}

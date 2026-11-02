//! Stitching coincident topology, part 2: the general stitch driver (`DoStitching`),
//! the goal-directed solid and shell stitches, the one-list vertex pairing and the
//! splitting of edges at nearby vertices.
//!
//! Upstream: `source/SMLib/src/SmStitch.cpp`, `source/SMLib/inc/SmStitch.h`.
//!
//! | Rust                                          | Upstream                                      |
//! |-----------------------------------------------|-----------------------------------------------|
//! | [`Stitch::do_stitching`], [`Stitching`], [`MinGapRequest`] | `SmStitch::DoStitching`          |
//! | [`stitch_into_solid`]                         | `SmStitch::StitchIntoSolid` (static)          |
//! | [`solid_stitch_start`]                        | `SmStitch::StitchIntoSolid` (its first lines) |
//! | [`stitch_into_shell`]                         | `SmStitch::StitchIntoShell` (static)          |
//! | [`Stitch::stitch_vertex_list`], [`VertexList`] | `SmStitch::StitchVertexPairs` (one-list overload) |
//! | [`Stitch::split_edges_with_vertices`], [`SplitEdges`] | `SmStitch::SplitEdgesWithVertices`    |
//!
//! Deviations:
//! - Out-params become return values. `DoStitching`'s optional `pdMinUnstitched*Gap`
//!   pointers are the [`MinGapRequest`] flags (a requested edge gap also widens the
//!   drop tolerance, as upstream); the gaps come back as `Some` when requested.
//!   Vertex and edge lists hold `Option` ids (upstream null entries). The one-list
//!   `StitchVertexPairs` is [`Stitch::stitch_vertex_list`] (Rust has no overloads; the
//!   two-list one is [`Stitch::stitch_vertex_pairs`]); its "added to, not reset"
//!   deleted-vertex and deleted-edge lists are returned fresh.
//! - `SmTemporaryChangeValue` on the B-rep's editing flag and the context's
//!   doing-Boolean flag is a save/restore around the body that also runs on errors.
//! - Bounds on input-driven loops (upstream has none): `DoStitching`'s per-vertex edge
//!   gluing rescans a vertex at most `edges + 2` times (each successful glue removes an
//!   edge); `StitchIntoSolid` / `StitchIntoShell` double the tolerance at most
//!   [`MAX_TOL_DOUBLINGS`] times (upstream never terminates from a zero tolerance,
//!   HANG-33/HANG-24 in `plan/upstream-issues.md`);
//!   `SplitEdgesWithVertices` splits at most as many edges per vertex as the edge list
//!   held when that vertex started (a split puts the vertex at both new edge ends).
//!   Past a bound the port returns [`SolidError::InvalidInput`].
//! - `DoStitching` collects the modified faces (`sModifiedFaces`) as upstream, though
//!   only commented-out code reads them; its `dMinEdgeLength` is dropped for the same
//!   reason (the lengths are still computed).
//! - `SmBrep::Notify(SM_NO_PRE_EDIT)` is `super::p1::brep_notify_pre_edit`
//!   (`topology::brep::p2::notify`), which still defers inside `topology.brep_p2`.
//!   This file's own stub returns [`SolidError::Unported`]: `CalculateBoundingBox`
//!   (`query.brep`). `FindAndSetInfiniteRegion` is `topology.brep_p5`'s.
//!   `DeleteFace` and `SqueezeEdge` are `topology.brep_p3`'s, notifying through the
//!   kernel context's callbacks; `GlueVertices`, `GlueEdgesGeneral` and
//!   `MakeVertexSplitEdge` (through [`super::p1::test_and_split_edge`]) defer only where their own
//!   units still do.

use super::p1::{sort_by_param, AdvancedStitch, DefaultStitchCallback, SplitReason, Stitch};
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, EntityRef, SM_ERR};
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::NotifyOperation;
use crate::geom::tol::{update_object_tolerance, zone_tol3d, TopologyToUpdate};
use crate::geom::tree::{NodeGeomType, Tree};
use crate::kernel::Kernel;
use crate::math::{Vec3, BIG_DOUBLE};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::topology::brep::p1::{self as bp1, SetRegionVoidType};
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, RegionId, ShellId, VertexId};
use crate::{Result, SolidError};

const OP: &str = "heal::stitch::p2";

/// How often `StitchIntoSolid` / `StitchIntoShell` may double the stitch tolerance
/// (doubling the smallest positive double reaches the largest in about 2100 steps).
pub const MAX_TOL_DOUBLINGS: u32 = 2200;

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

fn at<T: Copy>(list: &[T], i: usize) -> Result<T> {
    list.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP, "index outside the list"))
}

/// `SmCurve::CalculateBoundingBox(ivl, &box)` of the edge's curve over its interval.
fn edge_box(k: &Kernel, b: &Brep, e: EdgeId) -> Result<Extent3d> {
    curve_of(b, e)?
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
        .ok_or_else(sm_err)
}

/// `SmCurve::ApproximateLength(edge interval, samples)` of the edge's curve.
fn edge_length(k: &Kernel, b: &Brep, e: EdgeId, samples: usize) -> Result<f64> {
    Ok(curve_of(b, e)?
        .approximate_length(k, &edge::interval(b, e)?, samples, false)?
        .0)
}

fn add_unique<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// Which of `DoStitching`'s optional min-gap outputs the caller asks for
/// (`pdMinUnstitchedVertGap` / `pdMinUnstitchedEdgeGap` non-null).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MinGapRequest {
    /// Report the smallest gap between vertices that were not glued.
    pub vertex: bool,
    /// Report the smallest gap between edges that were not glued (also makes the
    /// midpoint drop collect every solution).
    pub edge: bool,
}

/// The result of [`Stitch::do_stitching`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stitching {
    /// Number of edge glue attempts (`rlStitchedEdges`).
    pub stitched_edges: u64,
    /// Number of lamina edges left (`rlLaminaEdges`).
    pub lamina_edges: u64,
    /// Max gap between coincident vertices considered for gluing (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Max gap between coincident edges considered for gluing (`rdMaxEdgeGap`).
    pub max_edge_gap: f64,
    /// Min gap between vertices that were not glued, when requested.
    pub min_unstitched_vert_gap: Option<f64>,
    /// Min gap between edges that were not glued, when requested.
    pub min_unstitched_edge_gap: Option<f64>,
}

/// The result of [`Stitch::stitch_vertex_list`].
#[derive(Debug, Clone, PartialEq)]
pub struct VertexList {
    /// The surviving vertex of each visited set (`rSurvivingVerts`).
    pub surviving: Vec<VertexId>,
    /// The deleted vertex of each glued pair (`rDeletedVerts`; stale ids).
    pub deleted: Vec<VertexId>,
    /// The squeezed-out edges (`rDeletedEdges`; stale ids).
    pub deleted_edges: Vec<EdgeId>,
    /// Largest gap between paired vertices (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Smallest gap between unpaired vertices (`rdMinUnstitchedVertGap`).
    pub min_unstitched_vert_gap: f64,
}

/// The result of [`Stitch::split_edges_with_vertices`].
#[derive(Debug, Clone, PartialEq)]
pub struct SplitEdges {
    /// The vertices that split an edge, once per split (`rProcessedVerts`).
    pub processed: Vec<VertexId>,
    /// The edges that were split (`rSplitEdges`).
    pub split: Vec<EdgeId>,
    /// The edges the splits created (`rNewEdges`).
    pub new_edges: Vec<EdgeId>,
    /// Largest vertex-to-edge gap of a split (`rdMaxVertexGap`).
    pub max_vertex_gap: f64,
    /// Smallest gap of a vertex too far from an edge (`rdMinUnstitchedVertGap`).
    pub min_unstitched_vert_gap: f64,
}

impl Stitch<'_> {
    /// `DoStitching`: glues coincident vertex and edge pairs of the B-rep. Optionally
    /// first removes lamina sliver faces and squeezes out open edges shorter than the
    /// stitch tolerance; then, up to ten times while edges were split, splits edges at
    /// nearby vertices (when asked), glues (or squeezes) coincident vertices found
    /// along the unit `(1,1,1)` direction, and glues coincident edge pairs sharing both
    /// vertices. Candidate lists restrict which vertices and edges may be glued. Then
    /// counts the lamina edges and, with region nesting, sets the infinite region.
    ///
    /// # Errors
    ///
    /// Those of the callees upstream checks (`SER`); `SM_ERR` for a failing edge glue
    /// unless [`Stitch::ignore_problems`]; [`SolidError::InvalidInput`] past the
    /// per-vertex rescan bound.
    pub fn do_stitching(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        candidate_vertices: Option<&[VertexId]>,
        candidate_edges: Option<&[EdgeId]>,
        min_gaps: MinGapRequest,
    ) -> Result<Stitching> {
        // init output
        let mut out = Stitching {
            stitched_edges: 0,
            lamina_edges: 0,
            max_vertex_gap: 0.0,
            max_edge_gap: 0.0,
            min_unstitched_vert_gap: min_gaps.vertex.then_some(BIG_DOUBLE),
            min_unstitched_edge_gap: min_gaps.edge.then_some(BIG_DOUBLE),
        };

        // set Brep to allow modifications (SmTemporaryChangeValue x2)
        let old_editing = b.editing_enabled();
        b.set_editing_enabled(true);
        let old_doing_boolean = k.context.doing_boolean();
        k.context.set_doing_boolean(true);
        let r = self.do_stitching_body(k, b, candidate_vertices, candidate_edges, &mut out);
        k.context.set_doing_boolean(old_doing_boolean);
        b.set_editing_enabled(old_editing);
        r.map(|()| out)
    }

    #[allow(clippy::too_many_lines)] // one upstream routine
    fn do_stitching_body(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        candidate_vertices: Option<&[VertexId]>,
        candidate_edges: Option<&[EdgeId]>,
        out: &mut Stitching,
    ) -> Result<()> {
        let tol_3d = self.stitch_tol_3d();
        // (status not checked)
        ignore_status(self.edit_brep_pre_edit(k, b))?;

        // when asked find and remove sliver laminar faces
        if self.remove_laminar_slivers {
            let zone_tol = zone_tol3d(k, tol_3d / 2.0);
            for f in bp1::faces(b, None)? {
                // when face is a lamina sliver
                if face_is_lamina(b, f)? && face_is_degenerate(k, b, f, true, Some(zone_tol))? {
                    // delete Face and its boundary edges and vertices from the Brep
                    ignore_status(self.edit_delete_face(k, b, f, true, true, None, None))?;
                }
            }
        }

        // (the candidate marking upstream is commented out)
        let mut cand_edges: Vec<EdgeId> =
            candidate_edges.map(<[EdgeId]>::to_vec).unwrap_or_default();
        let mut cand_vertices: Vec<VertexId> = candidate_vertices
            .map(<[VertexId]>::to_vec)
            .unwrap_or_default();
        let mut modified_faces: Vec<FaceId> = Vec::new();
        let mut has_wire_vertex = false;

        // when asked - squeeze out edges shorter than m_dStitchTol3d
        if self.squeeze_small_edges {
            for e in bp1::edges(b, None)? {
                // only check lamina edges when making a ManifoldSolid
                if self.making_manifold_solid && !edge::is_lamina(b, e)? {
                    continue;
                }

                // get edge->curve approximate length
                let length = edge_length(k, b, e, 5)?;

                // when edge is shorter than stitching tolerance and open (else upstream
                // only tightens dMinEdgeLength, which nothing reads)
                if length < tol_3d && !edge::is_closed(b, e)? {
                    // add every face connected to this edge to the sModifiedFaces list
                    for f in edge::faces(b, e, None)? {
                        add_unique(&mut modified_faces, f);
                    }

                    // get edge's vertices
                    let surv_v = edge::vertex(b, e)?;
                    let del_v = edge::other_vertex(b, e, surv_v)?;

                    // skip vertices if neither is on the optCandidateVertex list
                    if candidate_vertices.is_some()
                        && !surv_v.is_some_and(|v| cand_vertices.contains(&v))
                        && !del_v.is_some_and(|v| cand_vertices.contains(&v))
                    {
                        continue;
                    }

                    let surv_v = surv_v.ok_or_else(|| deref("edge without vertex"))?;
                    // remove and delete edge from topology graph by squeezing
                    // [vertex-edge-vertex] into [vertex]
                    self.edit_squeeze_edge(k, b, Some(e), surv_v)?;

                    if candidate_vertices.is_some() {
                        add_unique(&mut cand_vertices, surv_v); // This becomes a candidate.
                    }
                }
            }
        }

        // for 10 iterations or until all work is done
        let mut count = 0u32;
        let mut more_work_needed = true;
        while more_work_needed && count < 10 {
            count += 1;
            more_work_needed = false;

            // Check for Vertices coincident with Edges if asked.
            if self.split_edges_with_vertices {
                // get modified Brep Bounding box (status not checked)
                let brep_bbox = ignored(qbrep::calculate_bounding_box(
                    k,
                    &mut *b,
                    BoundingBoxOptions::default(),
                ))?
                .unwrap_or_default();

                // build modified Brep Edge spatial tree (lNumPerNode only sizes the
                // memory blocks)
                let edges = bp1::edges(b, None)?;
                let mut edge_tree: Tree<EdgeId> = Tree::new_spatial(&brep_bbox);
                for &e in &edges {
                    let ebox = edge_box(k, b, e)?;
                    edge_tree.add_to_spatial_tree(&ebox, Some(e), NodeGeomType::default())?;
                }

                let vertices = bp1::vertices(b, None)?;

                // Check only if either vertex or edge is on candidate list (upstream
                // never resets these two flags).
                let mut candidate_vtx = true;
                let mut candidate_edge = true;

                for &v in &vertices {
                    // If we are making a manifold solid only stitch lamina and wire
                    // topology. [B311]
                    if self.making_manifold_solid
                        && !(vertex::is_lamina_vertex(b, v)? || vertex::is_wire_vertex(b, v)?)
                    {
                        continue;
                    }

                    if candidate_vertices.is_some() && !cand_vertices.contains(&v) {
                        candidate_vtx = false;
                    }

                    // for every edge near this vertex
                    let mut vbox = Extent3d::from_point(vertex::point(b, v)?);
                    vbox.expand_absolute(tol_3d);
                    for object in edge_tree.objects_in_box(&vbox)? {
                        let e = object.ok_or_else(|| deref("tree object"))?;

                        // If we are making a manifold solid only stitch lamina and wire
                        // topology. [B311]
                        if self.making_manifold_solid
                            && !(edge::is_lamina(b, e)? || edge::is_wire(b, e)?)
                        {
                            continue;
                        }

                        if candidate_edges.is_some() && !cand_edges.contains(&e) {
                            candidate_edge = false;
                        }

                        if !candidate_vtx && !candidate_edge {
                            continue;
                        }

                        // split edges when vertex is within tolerance
                        let this_tol =
                            smos_max(tol_3d, vertex::tolerance(b, v)? + edge::tolerance(b, e)?);
                        let split =
                            self.edit_test_and_split_edge(k, b, e, vertex::point(b, v)?, this_tol)?;

                        // when split created a new vertex
                        if let Some(new_v) = split.new_vertex {
                            // set new vertex tolerance (SM_USE_OLDTOL) and add it to the
                            // candidates
                            let new_tol =
                                smos_max(vertex::tolerance(b, v)?, vertex::tolerance(b, new_v)?);
                            vertex::set_tolerance(b, new_v, zone_tol3d(k, new_tol), true, true)?;
                            more_work_needed = true;

                            cand_vertices.push(new_v);
                            if !candidate_vtx {
                                cand_vertices.push(v);
                            }
                        }

                        // when split created a new edge
                        if let Some(new_e) = split.new_edge {
                            // add new edge->curve to edge spatial tree
                            let ebox = edge_box(k, b, new_e)?;
                            edge_tree.add_to_spatial_tree(
                                &ebox,
                                Some(new_e),
                                NodeGeomType::default(),
                            )?;

                            cand_edges.push(new_e);
                            if !candidate_edge {
                                cand_edges.push(e);
                            }
                        }
                    }
                }
            }

            // Now glue coincident vertices, including those created in the Vertex/Edge
            // intersections, sorted along the unit (1,1,1) vector so that the parameter
            // corresponds to 3d distance.
            let s_vec = Vec3::new(1.0, 1.0, 1.0).unitize()?;

            let wire_edges = bp1::wire_edges(b, None)?;

            // for every vertex - label lamina vertices and store common line
            // projection parameter for later sorting
            let mut vertices: Vec<Option<VertexId>> =
                bp1::vertices(b, None)?.into_iter().map(Some).collect();
            let mut is_lamina_vertex: Vec<bool> = Vec::new();
            let mut sort_param: Vec<f64> = Vec::new();
            for &v in vertices.iter().flatten() {
                // lamina vertex := a vertex with one or more lamina edges
                is_lamina_vertex.push(vertex::is_lamina_vertex(b, v)?);
                // wire vertex := a vertex with only wire edges
                if vertex::is_wire_vertex(b, v)? {
                    has_wire_vertex = true;
                }
                sort_param.push(s_vec.dot(vertex::point(b, v)?));
            }

            // sorted index of the vertices by projection parameter; sort only lamina
            // vertices when making manifold solids, unless there's a wire vertex
            let skip_non_lamina = !has_wire_vertex && self.making_manifold_solid;
            let sorted = sort_by_param(
                vertices.len(),
                |ii| skip_non_lamina && !is_lamina_vertex.get(ii).copied().unwrap_or(false),
                &sort_param,
            )?;

            // for every vertex - build list of coincident vertices, then stitch (glue
            // or squeeze) them
            let mut coin_vertices: Vec<VertexId> = Vec::new();
            for ii in 0..sorted.len() {
                let s_ii = at(&sorted, ii)?;
                // skip vertices already processed
                let Some(mut v) = at(&vertices, s_ii)? else {
                    continue;
                };
                // mark this vertex
                set_none(&mut vertices, s_ii)?;

                let v_param = at(&sort_param, s_ii)?;

                // load sCoinVertices with all vertices coincident with pV
                coin_vertices.clear();
                for jj in ii + 1..sorted.len() {
                    let s_jj = at(&sorted, jj)?;
                    let tv_param = at(&sort_param, s_jj)?;
                    // skip vertices already tested
                    let Some(test_v) = at(&vertices, s_jj)? else {
                        continue;
                    };

                    let test_tol = smos_max(
                        tol_3d,
                        vertex::tolerance(b, test_v)? + vertex::tolerance(b, v)?,
                    );

                    // done looking when pTestV is too far along the vector
                    if tv_param - v_param > test_tol {
                        break;
                    }

                    let dist = vertex::point(b, v)?.distance(vertex::point(b, test_v)?);

                    // when vertices are within tolerance
                    if dist < test_tol {
                        coin_vertices.push(test_v);
                        set_none(&mut vertices, s_jj)?;
                        if dist > out.max_vertex_gap {
                            out.max_vertex_gap = dist;
                        }
                    } else if let Some(min_gap) = out.min_unstitched_vert_gap.as_mut() {
                        if dist < *min_gap {
                            *min_gap = dist;
                        }
                    }
                }

                // when stitching is restricted to a candidate list and pV is not on it,
                // keep only the coincident vertices on the list
                if candidate_vertices.is_some()
                    && !coin_vertices.is_empty()
                    && !cand_vertices.contains(&v)
                {
                    coin_vertices.retain(|cv| cand_vertices.contains(cv));
                }

                // Now glue (or squeeze) every vertex that is coincident with pV.
                for jj in 0..coin_vertices.len() {
                    let cv = at(&coin_vertices, jj)?;

                    // when m_bMakingManifoldSolid - only glue pairs of Lamina vertices or
                    // wire vertices; else glue all pairs of coincident vertices
                    if self.making_manifold_solid
                        && !((vertex::is_lamina_vertex(b, v)? && vertex::is_lamina_vertex(b, cv)?)
                            || (vertex::is_wire_vertex(b, v)? || vertex::is_wire_vertex(b, cv)?))
                    {
                        continue;
                    }

                    let dist_to_v = vertex::point(b, v)?.distance(vertex::point(b, cv)?);

                    // select the vertex to keep
                    let choice = self.callback().select_topology_to_keep(
                        k,
                        b,
                        EntityRef::Vertex(v),
                        EntityRef::Vertex(cv),
                        dist_to_v,
                    )?;

                    // skip vertices farther apart than allowed stitching tolerance +
                    // vertex tolerances
                    let this_tol =
                        smos_max(tol_3d, vertex::tolerance(b, v)? + vertex::tolerance(b, cv)?);
                    if choice.max_distance > this_tol {
                        if let Some(min_gap) = out.min_unstitched_vert_gap.as_mut() {
                            if dist_to_v < *min_gap {
                                *min_gap = dist_to_v;
                            }
                        }
                        continue; // Don't stitch it if distance gets too big.
                    }

                    let (do_glue, do_squeeze, squeeze_edge) =
                        glue_or_squeeze(k, b, v, cv, dist_to_v)?;

                    // skip squeezing vertices when m_bSqueezeSmallEdges is FALSE
                    if do_squeeze && !self.squeeze_small_edges {
                        continue;
                    }

                    // squeeze or glue vertices together (make pV the retained vertex)
                    if choice.keep_first {
                        if do_squeeze {
                            self.edit_squeeze_edge(k, b, squeeze_edge, v)?;
                        } else if do_glue {
                            self.edit_glue_vertices(k, b, v, cv, Some(&wire_edges))?;
                        }
                    } else {
                        if do_squeeze {
                            self.edit_squeeze_edge(k, b, squeeze_edge, cv)?;
                        } else if do_glue {
                            self.edit_glue_vertices(k, b, cv, v, Some(&wire_edges))?;
                        }
                        v = cv;
                    }

                    // The surviving vertex (which is pV) should be a candidate.
                    if candidate_vertices.is_some() {
                        add_unique(&mut cand_vertices, v);
                    }
                }
            }

            // next: glue edges; because vertices have been glued, all edge pairs to be
            // glued are connected to common vertices at both their ends.
            for v in bp1::vertices(b, None)? {
                self.glue_edges_at_vertex(
                    k,
                    b,
                    v,
                    candidate_edges.is_some(),
                    &mut cand_edges,
                    &mut modified_faces,
                    out,
                )?;
            }
        }

        // count the number of lamina edges in modified Brep
        for e in bp1::edges(b, None)? {
            if edge::is_lamina(b, e)? {
                out.lamina_edges += 1;
            }
        }

        // if edges were stitched - notify all modified Brep faces of impending change
        if out.stitched_edges > 0 {
            ignore_status(self.edit_brep_pre_edit(k, b))?;
            for f in bp1::faces(b, None)? {
                ignore_status(self.edit_face_notify(
                    b,
                    f,
                    NotifyOperation::PreEdit,
                    Some(EntityRef::Face(f)),
                    None,
                ))?;
            }
        }

        // when asked - find and set the infinite region by classifying a ray from a
        // point outside the Brep
        if self.do_region_nesting && bp1::regions(b, None)?.len() > 1 {
            let old = b.editing_enabled();
            b.set_editing_enabled(false);
            let r = find_and_set_infinite_region_with_identity(
                k,
                b,
                self.query_identity,
                None,
                SetRegionVoidType::InfiniteRegionSwap,
                None,
            );
            b.set_editing_enabled(old);
            r?;
        }

        // (removing the modified faces' UV trim curves is commented out upstream)
        let _ = modified_faces;
        Ok(())
    }

    /// `DoStitching`'s edge gluing at one vertex: while a glue succeeded, rescans the
    /// vertex's edges for pairs sharing the other vertex too, within
    /// `max(stitch tol, summed edge tolerances)`.
    #[allow(clippy::too_many_lines)] // one upstream loop
    fn glue_edges_at_vertex(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        v: VertexId,
        restricted: bool,
        cand_edges: &mut Vec<EdgeId>,
        modified_faces: &mut Vec<FaceId>,
        out: &mut Stitching,
    ) -> Result<()> {
        let tol_3d = self.stitch_tol_3d();
        let bound = bp1::edges(b, None)?.len().saturating_add(2);
        let mut passes = 0usize;
        let mut done_with_vertex = false;

        // while using this vertex to find glueable edge pairs
        while !done_with_vertex {
            passes += 1;
            if passes > bound {
                return Err(SolidError::invalid_input(
                    OP,
                    "edge glue rescan bound exceeded",
                ));
            }
            done_with_vertex = true;

            // get all edges connected to this vertex
            let mut v_edges: Vec<Option<EdgeId>> =
                vertex::edges(b, v, None)?.into_iter().map(Some).collect();

            // for every vertex->edge[jj]
            for jj in 0..v_edges.len() {
                // skip edges already processed
                let Some(mut e) = at(&v_edges, jj)? else {
                    continue;
                };

                // If we are making a manifold solid only stitch lamina and wire edges
                if self.making_manifold_solid && !(edge::is_lamina(b, e)? || edge::is_wire(b, e)?) {
                    continue;
                }

                // iter every other vertex->edge[kk]
                for kk in jj + 1..v_edges.len() {
                    // skip edges already processed
                    let Some(test_e) = at(&v_edges, kk)? else {
                        continue;
                    };

                    if self.making_manifold_solid
                        && !(edge::is_lamina(b, test_e)? || edge::is_wire(b, test_e)?)
                    {
                        continue;
                    }

                    // skip edges that don't share common vertices on both ends
                    if edge::other_vertex(b, e, Some(v))? != edge::other_vertex(b, test_e, Some(v))?
                    {
                        continue;
                    }

                    // Skip this one if there is a Candidate list and neither the edge
                    // nor its mate is on it.
                    if restricted && !cand_edges.contains(&test_e) && !cand_edges.contains(&e) {
                        continue;
                    }

                    let e_ivl = edge::interval(b, e)?;
                    let te_ivl = edge::interval(b, test_e)?;
                    let e_curve = curve_of(b, e)?;
                    let te_curve = curve_of(b, test_e)?;

                    // select new tolerance
                    let this_tol =
                        smos_max(tol_3d, edge::tolerance(b, test_e)? + edge::tolerance(b, e)?);

                    let mid_pnt = e_curve.evaluate_point(k, e_ivl.evaluate(0.5))?;

                    // If we're finding pdMinUnstitchedEdgeGap, collect every solution.
                    let guess = te_ivl.mid();
                    let drop_tol = if out.min_unstitched_edge_gap.is_none() {
                        this_tol
                    } else {
                        BIG_DOUBLE
                    };

                    // skip edge pair if pE->midPoint is not within tolerance of pTestE
                    let dropped = match te_curve.drop_point(
                        k,
                        &te_ivl,
                        mid_pnt,
                        None,
                        drop_tol,
                        Some(guess),
                        SolverOperation::Minimize,
                    ) {
                        Ok(d) if d.success => d,
                        Err(err) if err.is_unported() => return Err(err),
                        // no drop point, or a failing status
                        Ok(_) | Err(_) => continue,
                    };
                    let mut dist_to_curve = dropped.distance;

                    if dist_to_curve > this_tol {
                        if let Some(min_gap) = out.min_unstitched_edge_gap.as_mut() {
                            if dist_to_curve < *min_gap {
                                *min_gap = dist_to_curve;
                            }
                        }
                        continue;
                    }

                    // Check relative orientation of the edge curves.
                    let pv = e_curve.evaluate(k, e_ivl.evaluate(0.5), 1, true, true)?;
                    let pv_test = te_curve.evaluate(k, dropped.parameter, 1, true, true)?;

                    let mut orientation = OrientType::Same;
                    let mut t_start = te_ivl.min();
                    let mut t_end = te_ivl.max();
                    if tangent(&pv)?.dot(tangent(&pv_test)?) < 0.0 {
                        orientation = OrientType::Opposite;
                        core::mem::swap(&mut t_start, &mut t_end);
                    }

                    // skip edge pairs not within tolerance of one another
                    let edges_are_lines = (e_curve.degree()? == 1
                        && e_curve.number_natural_knots(k)? == 4)
                        && (te_curve.degree()? == 1 && te_curve.number_natural_knots(k)? == 4);
                    dist_to_curve = 0.0;
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
                            if let Some(min_gap) = out.min_unstitched_edge_gap.as_mut() {
                                if dist_to_curve < *min_gap {
                                    *min_gap = dist_to_curve;
                                }
                            }
                            continue;
                        }

                        // when asked - do a precise distance check
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
                                if let Some(min_gap) = out.min_unstitched_edge_gap.as_mut() {
                                    if dist_to_curve < *min_gap {
                                        *min_gap = dist_to_curve;
                                    }
                                }
                                continue;
                            }
                        }
                    }

                    // If make it here then we need to glue edges; save the maxEdgeGap
                    if dist_to_curve > out.max_edge_gap {
                        out.max_edge_gap = dist_to_curve;
                    }

                    // skip edges farther apart than the dMaxDistance
                    let choice = self.callback().select_topology_to_keep(
                        k,
                        b,
                        EntityRef::Edge(e),
                        EntityRef::Edge(test_e),
                        this_tol,
                    )?;
                    if choice.max_distance > this_tol {
                        if let Some(min_gap) = out.min_unstitched_edge_gap.as_mut() {
                            if choice.max_distance < *min_gap {
                                *min_gap = choice.max_distance;
                            }
                        }
                        continue;
                    }

                    // branch on which edge to keep
                    let (keep, delete, deleted_slot) = if choice.keep_first {
                        (e, test_e, kk)
                    } else {
                        (test_e, e, jj)
                    };
                    // place every deleted edge's face on ModifiedFaces list
                    for f in edge::faces(b, delete, None)? {
                        add_unique(modified_faces, f);
                    }
                    match self.edit_glue_edges(
                        k,
                        b,
                        keep,
                        orientation,
                        dist_to_curve,
                        self.do_region_nesting,
                        delete,
                        None, // header default pInsideFaceuses = NULL
                    ) {
                        Ok(()) => {
                            if !choice.keep_first {
                                *v_edges
                                    .get_mut(jj)
                                    .ok_or_else(|| deref("index outside the list"))? = Some(test_e);
                                e = test_e;
                            }
                            set_none(&mut v_edges, kk)?;
                            done_with_vertex = false;
                        }
                        Err(err) if err.is_unported() => return Err(err),
                        Err(_) if !self.ignore_problems => return Err(sm_err()),
                        Err(_) => {
                            // forget the deleted edge when the failed glue removed it
                            if let Some(slot) = at(&v_edges, deleted_slot)? {
                                if !bp1::edges(b, None)?.contains(&slot) {
                                    set_none(&mut v_edges, deleted_slot)?;
                                }
                            }
                        }
                    }

                    // add the surviving edge, pE, to list of candidate edges
                    if restricted {
                        cand_edges.push(e);
                    }

                    // increment the number of glue edge attempts
                    out.stitched_edges += 1;

                    if !done_with_vertex {
                        break;
                    }
                }

                if !done_with_vertex {
                    break;
                }
            }
        }
        Ok(())
    }

    /// `StitchVertexPairs` (one-list overload): glues (or, with
    /// [`Stitch::squeeze_small_edges`], squeezes) each vertex of `list` to the later
    /// ones within `max(stitch tol, summed vertex tolerances)` along the unit `(1,2,3)`
    /// direction, as `DoStitching` does; when making a manifold solid only lamina pairs
    /// and pairs with a wire vertex. Glued entries become `None`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `list` holds no vertex with a B-rep; those of the callback and
    /// the gluing (`SER`).
    pub fn stitch_vertex_list(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        list: &mut [Option<VertexId>],
    ) -> Result<VertexList> {
        // Init outputs.
        let mut out = VertexList {
            surviving: Vec::new(),
            deleted: Vec::new(),
            deleted_edges: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        };
        let tol_3d = self.stitch_tol_3d();

        let n = list.len();
        if n < 1 {
            return Ok(out);
        }

        // Get Brep pointer from the first non-null entry.
        let mut has_brep = false;
        if let Some(&v) = list.iter().flatten().next() {
            has_brep = vertex::brep(b, v)?.is_some();
        }
        if !has_brep {
            return Err(sm_err()); // we'll flag this as an error.
        }

        // Sort on (1,2,3), unitized so that parameter differences are 3d distances.
        // (upstream ignores Unitize's status here)
        let s_vec =
            ignored(Vec3::new(1.0, 2.0, 3.0).unitize())?.unwrap_or(Vec3::new(1.0, 2.0, 3.0));

        // Precompute lamina flag (read by nothing) and sort value; the min/max
        // parameters upstream keeps for boxing are never read either.
        let mut sort_param: Vec<f64> = Vec::new();
        for v in list.iter() {
            let Some(v) = *v else {
                sort_param.push(0.0);
                continue;
            };
            vertex::is_lamina_vertex(b, v)?;
            sort_param.push(s_vec.dot(vertex::point(b, v)?));
        }

        // Now sort the list by projection param.
        let sorted = sort_by_param(
            n,
            |ii| list.get(ii).copied().flatten().is_none(),
            &sort_param,
        )?;

        let mut coin_vertices: Vec<VertexId> = Vec::new();
        for ii in 0..sorted.len() {
            let s_ii = at(&sorted, ii)?;
            // skip vertices already processed
            let Some(mut v) = at(list, s_ii)? else {
                continue;
            };
            let v_param = at(&sort_param, s_ii)?;

            // load sCoinVertices with all vertices coincident with pV
            coin_vertices.clear();
            for jj in ii + 1..sorted.len() {
                let s_jj = at(&sorted, jj)?;
                let Some(test_v) = at(list, s_jj)? else {
                    continue;
                };
                let tv_param = at(&sort_param, s_jj)?;

                let test_tol = smos_max(
                    tol_3d,
                    vertex::tolerance(b, test_v)? + vertex::tolerance(b, v)?,
                );

                // done looking when pTestV is too far along the vector; since the
                // params are projected to (1,2,3), need 3x tol to ensure no coincidence
                if tv_param - v_param > test_tol * 3.0 {
                    break;
                }

                let dist = vertex::point(b, v)?.distance(vertex::point(b, test_v)?);

                // when vertices are within tolerance
                if dist < test_tol {
                    coin_vertices.push(test_v);
                    set_none(list, s_jj)?;
                    if dist > out.max_vertex_gap {
                        out.max_vertex_gap = dist;
                    }
                } else if dist < out.min_unstitched_vert_gap {
                    out.min_unstitched_vert_gap = dist;
                }
            }

            // Now glue (or squeeze) every vertex that is coincident with pV.
            for jj in 0..coin_vertices.len() {
                let cv = at(&coin_vertices, jj)?;

                if v == cv {
                    set_none(list, s_ii)?;
                    continue;
                }

                // when m_bMakingManifoldSolid - only glue pairs of Lamina vertices or
                // wire vertices; else glue all pairs of coincident vertices
                if self.making_manifold_solid
                    && !((vertex::is_lamina_vertex(b, v)? && vertex::is_lamina_vertex(b, cv)?)
                        || (vertex::is_wire_vertex(b, v)? || vertex::is_wire_vertex(b, cv)?))
                {
                    continue;
                }

                // select the vertex to keep
                let dist_to_v = vertex::point(b, v)?.distance(vertex::point(b, cv)?);
                let choice = self.callback().select_topology_to_keep(
                    k,
                    b,
                    EntityRef::Vertex(v),
                    EntityRef::Vertex(cv),
                    dist_to_v,
                )?;

                // skip vertices farther apart than allowed stitching tolerance + vertex
                // tolerances
                let this_tol =
                    smos_max(tol_3d, vertex::tolerance(b, v)? + vertex::tolerance(b, cv)?);
                if choice.max_distance > this_tol {
                    if dist_to_v < out.min_unstitched_vert_gap {
                        out.min_unstitched_vert_gap = dist_to_v;
                    }
                    continue; // Don't stitch it if distance gets too big.
                }

                let (do_glue, do_squeeze, squeeze_edge) = glue_or_squeeze(k, b, v, cv, dist_to_v)?;

                // skip squeezing vertices when m_bSqueezeSmallEdges is set to FALSE
                if do_squeeze && !self.squeeze_small_edges {
                    continue;
                }

                // squeeze or glue vertices together (make pV the retained vertex)
                if choice.keep_first {
                    if do_squeeze {
                        self.edit_squeeze_edge(k, b, squeeze_edge, v)?;
                    } else if do_glue {
                        self.edit_glue_vertices(k, b, v, cv, None)?;
                    }
                    out.deleted.push(cv);
                } else {
                    if do_squeeze {
                        self.edit_squeeze_edge(k, b, squeeze_edge, cv)?;
                    } else if do_glue {
                        self.edit_glue_vertices(k, b, cv, v, None)?;
                    }
                    out.deleted.push(v);
                    v = cv;
                }

                // update output (the first short edge is reported even when a second
                // one cancelled the squeeze)
                if let Some(se) = squeeze_edge {
                    out.deleted_edges.push(se);
                }
                set_none(list, s_ii)?;
            }

            out.surviving.push(v);
        }
        Ok(out)
    }

    /// `SplitEdgesWithVertices`: splits each edge of `edges` that passes within
    /// `max(stitch tol, vertex + edge tolerance)` of a vertex of `verts` in its
    /// interior, moves the vertex to the midpoint of itself and the split vertex and
    /// glues the two, raising the vertex's and edges' tolerances to the gap. New edges
    /// are appended to `edges` (and tested by the later vertices); vertices that split
    /// an edge become `None`. Making a manifold solid only uses lamina or wire vertices
    /// and edges.
    ///
    /// # Errors
    ///
    /// Those of [`super::p1::test_and_split_edge`] unless [`Stitch::ignore_problems`];
    /// [`SolidError::InvalidInput`] past the per-vertex split bound.
    pub fn split_edges_with_vertices(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        verts: &mut [Option<VertexId>],
        edges: &mut Vec<Option<EdgeId>>,
    ) -> Result<SplitEdges> {
        // Init outputs.
        let mut out = SplitEdges {
            processed: Vec::new(),
            split: Vec::new(),
            new_edges: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        };
        let tol_3d = self.stitch_tol_3d();

        let n_verts = verts.len();
        if n_verts < 1 {
            return Ok(out);
        }
        if edges.is_empty() {
            return Ok(out);
        }

        // For each vertex in the list, look for close edges.
        for ii in 0..n_verts {
            let Some(v) = at(verts, ii)? else {
                continue;
            };

            // Do not do Manifold vertices if indicated. We do allow Wire vertices though.
            if self.making_manifold_solid
                && !vertex::is_lamina_vertex(b, v)?
                && !vertex::is_wire_vertex(b, v)?
            {
                continue;
            }

            let pv_point = vertex::point(b, v)?;
            // (upstream builds a box around the vertex that nothing reads)

            let split_bound = edges.len();
            let mut splits = 0usize;
            let mut jj = 0usize;
            while jj < edges.len() {
                let Some(e) = at(edges, jj)? else {
                    jj += 1;
                    continue;
                };
                jj += 1;

                // Again, do only Lamina edges if indicated.
                if self.making_manifold_solid && !edge::is_lamina(b, e)? && !edge::is_wire(b, e)? {
                    continue;
                }

                let this_tol = smos_max(tol_3d, vertex::tolerance(b, v)? + edge::tolerance(b, e)?);

                let split = match self.edit_test_and_split_edge(k, b, e, pv_point, this_tol) {
                    Ok(s) => s,
                    Err(err) if err.is_unported() => return Err(err),
                    Err(_) if self.ignore_problems => continue,
                    Err(err) => return Err(err),
                };
                let dist_to_edge = split.dist_to_edge;

                // when split created a new vertex
                if let Some(new_v) = split.new_vertex {
                    splits += 1;
                    if splits > split_bound {
                        return Err(SolidError::invalid_input(OP, "edge split bound exceeded"));
                    }

                    // Glue the new vertex to pV, and switch lists.
                    if vertex::brep(b, v)?.is_none() {
                        if self.ignore_problems {
                            continue;
                        }
                        // upstream SER(SM_SUCCESS) falls through to a null B-rep
                        return Err(deref("vertex without B-rep"));
                    }

                    // First set the position of the surviving vertex to the midpoint
                    // of the two, to keep tolerances from growing excessively.
                    let mid = (vertex::point(b, v)? + vertex::point(b, new_v)?) * 0.5;
                    self.edit_set_point(k, b, v, mid)?;

                    // (status not checked)
                    ignore_status(self.edit_glue_vertices(k, b, v, new_v, None))?;
                    out.processed.push(v);
                    set_none(verts, ii)?;

                    // Set max distance.
                    out.max_vertex_gap = smos_max(out.max_vertex_gap, dist_to_edge);

                    // (statuses not checked)
                    ignore_status(update_object_tolerance(
                        k,
                        b,
                        Some(TopologyToUpdate::Vertex(v)),
                        dist_to_edge,
                        0.0,
                        false,
                    ))?;
                    ignore_status(update_object_tolerance(
                        k,
                        b,
                        Some(TopologyToUpdate::Edge(e)),
                        dist_to_edge,
                        0.0,
                        false,
                    ))?;
                } else if split.reason == SplitReason::FarFromGeom {
                    // Set min distance, if it didn't split due to being too far away
                    // (as opposed to being at a vertex).
                    out.min_unstitched_vert_gap =
                        smos_min(out.min_unstitched_vert_gap, dist_to_edge);
                }

                // Add the split edge and the new edge to the output lists.
                if let Some(new_e) = split.new_edge {
                    out.split.push(e);
                    out.new_edges.push(new_e);

                    let e_tol = edge::tolerance(b, e)?;
                    ignore_status(update_object_tolerance(
                        k,
                        b,
                        Some(TopologyToUpdate::Edge(new_e)),
                        dist_to_edge,
                        e_tol,
                        false,
                    ))?;

                    // Also add to main Edges list: might still need processing here.
                    edges.push(Some(new_e));
                }
            }
        }
        Ok(out)
    }
}

/// The squeeze/glue decision of `DoStitching` and `StitchVertexPairs` for two
/// coincident vertices `dist` apart: glue unless they share a short (shorter than
/// `2 * dist`) edge; squeeze that edge when it is the only short one and lamina or
/// wire; neither when it is manifold or a second short edge exists. Returns
/// `(glue, squeeze, first short lamina/wire edge)`.
fn glue_or_squeeze(
    k: &Kernel,
    b: &Brep,
    v: VertexId,
    other: VertexId,
    dist: f64,
) -> Result<(bool, bool, Option<EdgeId>)> {
    let mut do_glue = true;
    let mut do_squeeze = false;
    let mut squeeze_edge = None;

    // for every common edge between the vertices
    for e in vertex::common_edges(b, v, Some(other), None)? {
        let edge_length = edge_length(k, b, e, 10)?;

        // if edge is short
        if edge_length < 2.0 * dist {
            // A second short edge connecting the two vertices: don't squeeze or glue.
            if do_squeeze {
                do_squeeze = false;
                do_glue = false;
                break;
            }

            // squeeze lamina and wire edges - don't squeeze manifold edges
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
    Ok((do_glue, do_squeeze, squeeze_edge))
}

/// The start of `StitchIntoSolid`: its first stitch tolerance (a tenth of the B-rep
/// tolerance) and the bounding-box diagonal the doubling runs up to (`1/200` of it).
/// The doubling never ends when the tolerance starts at or below zero under that
/// bound (HANG-24 in `plan/upstream-issues.md`).
///
/// # Errors
///
/// Those of `CalculateBoundingBox`.
pub fn solid_stitch_start(k: &Kernel, b: &Brep) -> Result<(f64, f64)> {
    let bbox = qbrep::calculate_bounding_box(
        k,
        b,
        BoundingBoxOptions {
            approx: true,
            ..BoundingBoxOptions::default()
        },
    )?; // TRUE: Approx is fine [B259]
    let dist = bbox.size().length();
    // This tol will be iteratively increased:
    Ok((b.tolerance() / 10.0, dist))
}

/// `SmStitch::StitchIntoSolid`: stitches with the default callback, a fast edge
/// compare, no edge splitting and manifold gluing, from a tenth of the B-rep tolerance,
/// doubling it while lamina edges remain and it is below 1/200 of the bounding box
/// diagonal. The result is a solid when no lamina edge is left and the B-rep has more
/// than one region.
///
/// # Errors
///
/// Those of `CalculateBoundingBox` and [`Stitch::do_stitching`];
/// [`SolidError::InvalidInput`] past [`MAX_TOL_DOUBLINGS`].
pub fn stitch_into_solid(k: &mut Kernel, b: &mut Brep) -> Result<AdvancedStitch> {
    stitch_into_solid_with(k, b, None)
}

/// [`stitch_into_solid`] whose edits notify through `edits` (the caller's stores and
/// identities of `b`'s objects, as upstream's notify the objects themselves) instead
/// of the default call-local records.
///
/// # Errors
///
/// Those of [`stitch_into_solid`].
pub(crate) fn stitch_into_solid_with(
    k: &mut Kernel,
    b: &mut Brep,
    mut edits: Option<&mut dyn super::p1::StitchEditContext>,
) -> Result<AdvancedStitch> {
    let mut out = AdvancedStitch {
        produces_a_solid: false,
        stitched_edges: 0,
        lamina_edges: 0,
        max_vertex_gap: 0.0,
        max_edge_gap: 0.0,
    };

    let (mut stitch_tol_3d, dist) = solid_stitch_start(k, b)?;

    // First try stitching down to 1/10000 of the size of the part.
    let mut num_lamina = 0u64;
    let mut doublings = 0u32;
    while stitch_tol_3d < dist / 200.0 {
        doublings += 1;
        if doublings > MAX_TOL_DOUBLINGS {
            return Err(SolidError::invalid_input(
                OP,
                "stitch tolerance doubling bound exceeded",
            ));
        }
        let mut callback = DefaultStitchCallback::new();
        let mut stitch = Stitch::new(&mut callback, stitch_tol_3d, true, false);
        stitch.fast_edge_compare = true;
        stitch.split_edges_with_vertices = false;
        stitch.making_manifold_solid = true;
        stitch.edits = edits
            .as_mut()
            .map(|e| &mut **e as &mut dyn super::p1::StitchEditContext);
        let r = stitch.do_stitching(k, b, None, None, MinGapRequest::default())?;
        if r.max_vertex_gap > out.max_vertex_gap {
            out.max_vertex_gap = r.max_vertex_gap;
        }
        if r.max_edge_gap > out.max_edge_gap {
            out.max_edge_gap = r.max_edge_gap;
        }
        out.stitched_edges += r.stitched_edges;
        num_lamina = r.lamina_edges;
        if num_lamina == 0 {
            break;
        }
        stitch_tol_3d *= 2.0;
    }

    if num_lamina == 0 && bp1::regions(b, None)?.len() > 1 {
        out.produces_a_solid = true;
    }
    Ok(out)
}

/// `SmStitch::StitchIntoShell`: stitches with the default callback, no region nesting,
/// a fast edge compare and small-edge squeezing (ignoring glue problems unless
/// `shell_is_well_formed`), from 1/100 of `max_stitch_tol_3d`, doubling it while
/// lamina edges remain (the last pass uses `max_stitch_tol_3d` itself).
///
/// # Errors
///
/// Those of [`Stitch::do_stitching`]; [`SolidError::InvalidInput`] past
/// [`MAX_TOL_DOUBLINGS`].
pub fn stitch_into_shell(
    k: &mut Kernel,
    b: &mut Brep,
    shell_is_well_formed: bool,
    max_stitch_tol_3d: f64,
) -> Result<AdvancedStitch> {
    stitch_into_shell_with(k, b, shell_is_well_formed, max_stitch_tol_3d, None)
}

/// [`stitch_into_shell`] whose edits notify through `edits`, as
/// [`stitch_into_solid_with`].
///
/// # Errors
///
/// Those of [`stitch_into_shell`].
pub(crate) fn stitch_into_shell_with(
    k: &mut Kernel,
    b: &mut Brep,
    shell_is_well_formed: bool,
    max_stitch_tol_3d: f64,
    mut edits: Option<&mut dyn super::p1::StitchEditContext>,
) -> Result<AdvancedStitch> {
    // init output
    let mut out = AdvancedStitch {
        produces_a_solid: false,
        stitched_edges: 0,
        lamina_edges: 0,
        max_vertex_gap: 0.0,
        max_edge_gap: 0.0,
    };

    // First stitch with tighter tolerances: start with 1/100 and double each time.
    let mut stitch_tol_3d = max_stitch_tol_3d / 100.0;

    let mut callback = DefaultStitchCallback::new();
    let mut stitch = Stitch::new(&mut callback, stitch_tol_3d, true, false);
    stitch.do_region_nesting = false;
    stitch.fast_edge_compare = true;
    stitch.ignore_problems = !shell_is_well_formed;
    stitch.validate_result = false;
    stitch.squeeze_small_edges = true;
    stitch.edits = edits
        .as_mut()
        .map(|e| &mut **e as &mut dyn super::p1::StitchEditContext);

    let mut doublings = 0u32;
    while stitch_tol_3d < max_stitch_tol_3d {
        doublings += 1;
        if doublings > MAX_TOL_DOUBLINGS {
            return Err(SolidError::invalid_input(
                OP,
                "stitch tolerance doubling bound exceeded",
            ));
        }
        if stitch_tol_3d > max_stitch_tol_3d / 2.0 {
            stitch_tol_3d = max_stitch_tol_3d;
        }

        let r = stitch.do_stitching(k, b, None, None, MinGapRequest::default())?;

        if r.max_vertex_gap > out.max_vertex_gap {
            out.max_vertex_gap = r.max_vertex_gap;
        }
        if r.max_edge_gap > out.max_edge_gap {
            out.max_edge_gap = r.max_edge_gap;
        }
        out.stitched_edges += r.stitched_edges;

        if r.lamina_edges == 0 {
            break;
        }

        stitch_tol_3d *= 2.0;
        stitch.set_stitch_tol_3d(stitch_tol_3d);
    }
    Ok(out)
}

// ---- unported callees ----------------------------------------------------------------

/// `SmBrep::DeleteFace(face, bDeleteEdgesVertices, bDoRegionNesting,
/// pOptDeletedEdgesVertices, pOptRegionToKeep)` ([`crate::topology::brep::p3::delete_face`];
/// upstream defaults `FALSE`, `TRUE`, `NULL`, `NULL`), notifying through the kernel
/// context's callbacks ([`crate::topology::brep::p3::with_kernel_callbacks`], as
/// `super::p1::squeeze_edge_to`). `deleted_edges_vertices` is reset first, as upstream,
/// and gets the deleted edges and vertices when the deletion succeeds.
pub(super) fn delete_face(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    delete_edges_vertices: bool,
    do_region_nesting: bool,
    deleted_edges_vertices: Option<&mut Vec<EntityRef>>,
    region_to_keep: Option<RegionId>,
) -> Result<()> {
    use crate::topology::brep::p3 as brep_p3;
    let options = brep_p3::DeleteFaceOptions {
        delete_edges_vertices,
        do_region_nesting,
        region_to_keep,
    };
    let mut out = deleted_edges_vertices;
    if let Some(out) = out.as_deref_mut() {
        out.clear();
    }
    let deleted = brep_p3::with_kernel_callbacks(k, b, |k, b, env| {
        let mut marks = crate::topology::topology_traverser::Marks::default();
        brep_p3::delete_face(k, b, env, &mut marks, f, options)
    })?;
    if let Some(out) = out {
        *out = deleted;
    }
    Ok(())
}

/// `SmBrep::FindAndSetInfiniteRegion(pOptShellToTest, eVoidSettingFlag, pOptBrepBBox)`
/// (upstream defaults `NULL`, `SM_VS_INFINITEREGION_SWAP`, `NULL`), on a fresh
/// [`crate::query::brep::BrepQuery`] of the B-rep.
pub(crate) fn find_and_set_infinite_region(
    k: &mut Kernel,
    b: &mut Brep,
    shell_to_test: Option<ShellId>,
    void_setting: SetRegionVoidType,
    brep_bbox: Option<&Extent3d>,
) -> Result<()> {
    find_and_set_infinite_region_with_identity(k, b, None, shell_to_test, void_setting, brep_bbox)
}

fn find_and_set_infinite_region_with_identity(
    k: &mut Kernel,
    b: &mut Brep,
    identity: Option<crate::query::brep::BrepIdentity<'_>>,
    shell_to_test: Option<ShellId>,
    void_setting: SetRegionVoidType,
    brep_bbox: Option<&Extent3d>,
) -> Result<()> {
    let query = match identity {
        Some(identity) => crate::query::brep::BrepQuery::with_proven_owners(identity, b),
        None => crate::query::brep::BrepQuery::of_brep(b),
    };
    crate::topology::brep::p5::find_and_set_infinite_region(
        k,
        b,
        &query,
        shell_to_test,
        void_setting,
        brep_bbox,
    )
}

/// `SmFace::IsLamina` (unit `topology.face_p3`).
fn face_is_lamina(b: &Brep, f: FaceId) -> Result<bool> {
    crate::topology::face::p3::is_lamina(b, f)
}

/// `SmFace::IsDegenerate(bSkipQuickCheck, pOptZoneTol3d)` (unit `topology.face_p3`;
/// upstream defaults `FALSE`, `NULL`).
fn face_is_degenerate(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    skip_quick_check: bool,
    zone_tol_3d: Option<f64>,
) -> Result<bool> {
    crate::topology::face::p3::is_degenerate(k, b, f, skip_quick_check, zone_tol_3d)
}

/// `SmFace::Notify(eNotifyOperation, pData1, pData2, pData3)` (unit
/// `topology.face_p3`) with `pData2 = SM_NO_GET_BREP(pF)` (null), the only form used
/// here.
pub(super) fn face_notify(
    b: &mut Brep,
    f: FaceId,
    operation: NotifyOperation,
    data1: Option<EntityRef>,
    data3: Option<EntityRef>,
) -> Result<()> {
    crate::topology::loopuse::face_notify_data(b, f, operation, [data1, None, data3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glue_or_squeeze_defaults_to_glue() {
        let k = &Kernel::default();
        // no common edge: glue
        let mut b = Brep::new();
        let v1 = b
            .add_vertex(crate::math::Point3::new(0.0, 0.0, 0.0))
            .unwrap();
        let v2 = b
            .add_vertex(crate::math::Point3::new(1.0, 0.0, 0.0))
            .unwrap();
        let (glue, squeeze, e) = glue_or_squeeze(k, &b, v1, v2, 1.0).unwrap();
        assert!(glue && !squeeze && e.is_none());
    }

    #[test]
    fn empty_lists_return_at_once() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let mut cb = DefaultStitchCallback::new();
        let mut s = Stitch::new(&mut cb, 0.01, true, true);
        let r = s.stitch_vertex_list(&mut k, &mut b, &mut []).unwrap();
        assert!(r.surviving.is_empty() && r.min_unstitched_vert_gap == BIG_DOUBLE);
        let r = s
            .split_edges_with_vertices(&mut k, &mut b, &mut [], &mut Vec::new())
            .unwrap();
        assert!(r.processed.is_empty() && r.max_vertex_gap == 0.0);
    }

    #[test]
    fn shell_without_tolerance_range_is_a_no_op() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let r = stitch_into_shell(&mut k, &mut b, true, 0.0).unwrap();
        assert_eq!(r.stitched_edges, 0);
        let r = stitch_into_shell(&mut k, &mut b, false, f64::NAN).unwrap();
        assert_eq!(r.max_edge_gap, 0.0);
    }
}

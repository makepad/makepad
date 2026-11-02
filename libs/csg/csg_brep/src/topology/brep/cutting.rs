//! Cutting a B-rep with an infinite plane: the plane is merged into the B-rep as new
//! edges and vertices, then everything on the plane normal's side is removed
//! (`SmBrepCutting` with an `SmPlaneCutter`, and `SmApiCut`).
//!
//! Upstream: `source/SMLib/src/SmBrepCutting.cpp`, `source/SMLib/inc/SmBrepCutting.h`,
//! `source/SMLib/src/SmCutter.cpp`, `source/SMLib/inc/SmCutter.h`,
//! `source/SM_API/src/SmApiBrep.cpp` (`SmApiCut`).
//!
//! | Rust                         | Upstream                              |
//! |------------------------------|---------------------------------------|
//! | [`PlaneCutter`]              | `SmPlaneCutter`                       |
//! | [`PlaneCutter::new`]         | `SmPlaneCutter::SmPlaneCutter`        |
//! | [`PlaneCutter::merge_intersection`] | `SmPlaneCutter::MergeIntersection` |
//! | [`PlaneCutter::classification`], [`CutClassification`] | `SmPlaneCutter::Classification` |
//! | [`do_cut`]                   | `SmBrepCutting::DoCut` (`SmBrepCutting` holds only its arguments) |
//! | [`cut`], [`cut_notified`]    | `SmApiCut`                            |
//!
//! Deviations:
//! - **Cutter kinds.** Only the plane cutter is ported: `SmApiCut` is the only caller of
//!   `SmBrepCutting`, and it cuts with a plane. `SmCutter`'s base `MergeIntersection` and
//!   `Classification` (`SE(SM_ERR)`) and `SmSurfaceBiLinearCutter` (no caller in
//!   `SM_API`, `SMLib` or the upstream tests) are not; [`do_cut`] takes the plane cutter
//!   directly, so `DoCut`'s bilinear branches (`GetType() == SmSurfaceBiLinearCutter_TYPE`)
//!   are its plane branches. The `Draw` methods and the `SM_DEBUG_CODE` blocks are
//!   omitted.
//! - **Notifications.** The edits notify through the caller's [`NotifyEnv`] (its records,
//!   attributes and identities); `curve_ref` names each new edge curve. [`cut`] uses an
//!   arena-only store ([`ArenaNotifyState`]), as `SmApiCut`'s caller holds none.
//!   `StitchAndOrient` notifies through its own kernel-context edits
//!   ([`p5::stitch_and_orient`]).
//! - **Marks.** `SmNewMarkAndLock` advances and locks an unlocked context mark as
//!   upstream ([`MarkLock`], released on every return); the edges and faces it marks are
//!   sets owned by the call (a freshly advanced mark marks nothing).
//! - **Outputs.** `m_apEdgesInCutter` (`GetCutterEdges`) is the
//!   [`PlaneCutter::cutter_edges`] list. The removal lists are [`CutClassification`].
//! - **Statuses.** The statuses upstream ignores (a face's `CalculateBoundingBox`, which
//!   then keeps the previous face's box, `MergeCurvesOnSurface` in the face loop,
//!   `DeleteFace`, `DeleteEdge`, `DeleteVertex`, `StitchAndOrient`) are ignored here too
//!   ([`SolidError::Unported`] still propagates). `SmTemporaryChangeValue`s of the B-rep's
//!   editing flag and the context's doing-Boolean flag are a set and a restore, also on
//!   error.
//! - No recursion.

use std::collections::BTreeSet;

use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::CurvePropertyType;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::attribute::{ids, Attribute, Behavior};
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::geom::extent3d::Extent3d;
use crate::geom::object::ObjectRef;
use crate::geom::tol;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO_SQRT;
use crate::query::brep::{calculate_bounding_box, BoundingBoxOptions};
use crate::surface::surface::p4::NoObject;
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p10;
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::brep::p3::{self as bp3, DeleteFaceOptions};
use crate::topology::brep::p4::{self as bp4, DeleteEnv};
use crate::topology::brep::p5::{self, StitchAndOrientOptions};
use crate::topology::brep::p9;
use crate::topology::brep::planar_section::{create_plane_from_bbox, face_planar_section_curves};
use crate::topology::edge::{self, NotifyEnv};
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};
use crate::topology::face::p2 as face2;
use crate::topology::topology_traverser::Marks;
use crate::topology::{vertex, Brep, EdgeId, FaceId, VertexId};

const OP_MERGE: &str = "topology::brep::cutting::PlaneCutter::merge_intersection";
const OP_CLASSIFY: &str = "topology::brep::cutting::PlaneCutter::classification";
const OP_CUT: &str = "topology::brep::cutting::cut";

/// `SmPlaneCutter`: the infinite plane through [`PlaneCutter::plane_point`] with unit
/// normal [`PlaneCutter::plane_normal`]; the part of a B-rep on the normal's side is
/// cut away.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaneCutter {
    /// `m_vPlanePoint`: a point on the cutting plane.
    pub plane_point: Point3,
    /// `m_vPlaneNormal`: the plane's unit normal, pointing to the side removed.
    pub plane_normal: Vec3,
    /// `m_apEdgesInCutter`: the B-rep edges [`PlaneCutter::merge_intersection`] made of
    /// (or found for) the section curves.
    pub cutter_edges: Vec<EdgeId>,
}

/// The removal lists of [`PlaneCutter::classification`] (`rFacesToRemove`,
/// `rEdgesToRemove`, `rVerticesToRemove`), in the order found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CutClassification {
    /// The faces to delete.
    pub faces: Vec<FaceId>,
    /// The edges to delete.
    pub edges: Vec<EdgeId>,
    /// The vertices to delete.
    pub vertices: Vec<VertexId>,
}

impl PlaneCutter {
    /// `SmPlaneCutter(crPlanePoint, crPlaneNormal)`: the normal is unitized (a zero
    /// normal, which upstream only asserts against, stays zero: `Unitize` leaves it).
    #[must_use]
    pub fn new(plane_point: Point3, plane_normal: Vec3) -> Self {
        // SM_ASSERT(m_vPlaneNormal.LengthSquared() > SM_EFF_ZERO_SQ) is compiled out
        let plane_normal = plane_normal.unitize().unwrap_or(plane_normal);
        PlaneCutter {
            plane_point,
            plane_normal,
            cutter_edges: Vec::new(),
        }
    }

    /// `SmPlaneCutter::MergeIntersection(pTgtBrep, dXSectTol3d, pOptSubsetFaces)`:
    /// sections every face of `b` (or of `subset_faces`) whose box lies within
    /// `xsect_tol_3d` of the plane and merges the section curves into the face
    /// (`MergeCurvesOnSurface`, through the plane bounded to the B-rep's box), keeping
    /// their edges in [`PlaneCutter::cutter_edges`]; then splits every wire edge where it
    /// crosses the plane away from its ends.
    ///
    /// # Errors
    ///
    /// Those of the bounding boxes, [`create_plane_from_bbox`], the face sections,
    /// `GlobalPropertyAnalysis`, the curve evaluations and `MakeVertexSplitEdge` (its
    /// `SM_ERR` included); [`SolidError::Unported`] from the curve merge.
    pub fn merge_intersection(
        &mut self,
        k: &mut Kernel,
        b: &mut Brep,
        env: &mut NotifyEnv<'_>,
        curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
        xsect_tol_3d: f64,
        subset_faces: Option<&[FaceId]>,
    ) -> Result<()> {
        // init state
        self.cutter_edges.clear();

        // make temporary nurb plane which is larger than the Brep BBox.
        let bbox = calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?;
        let plane = create_plane_from_bbox(k, &bbox, self.plane_point, self.plane_normal, 0.0)?;

        // get target Brep faces
        let faces = match subset_faces {
            Some(subset) => subset.to_vec(),
            None => bp1::faces(b, None)?,
        };

        // (upstream's face box is declared outside the loop: a failed box keeps the last)
        let mut face_box = Extent3d::new();

        // split every target Brep face with this plane
        for f in faces {
            // locals for plane/surface intersections
            let surface = face2::surface(b, f)?
                .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?
                .clone_for_evaluation()?;
            let uv_domain = face2::uv_domain(b, f)?;

            // Skip Faces far from cutting plane
            if let Some(bx) = ignored(crate::query::face::p1::calculate_bounding_box(
                k, &mut *b, f, false,
            ))? {
                face_box = bx;
            }
            let dist = face_box.distance_to_plane(self.plane_point, self.plane_normal);
            if dist.abs() > xsect_tol_3d {
                continue;
            }

            // XSect plane with Face->Surface
            let approx_tol_3d = tol::approx_tol3d(k, tol::approx_tol3d(k, xsect_tol_3d) / 4.0);
            let section = face_planar_section_curves(
                k,
                b,
                f,
                &surface,
                &uv_domain,
                self.plane_point,
                self.plane_normal,
                Some(approx_tol_3d),
                None,
            )?;
            let mut curves_3d = section.curves_3d;
            // (one UV entry per 3D curve, as upstream indexes them; `None` where the
            // intersector made none, upstream's null entry)
            let mut uv_curves = section.uv_curves;
            uv_curves.resize_with(curves_3d.len(), || None);

            // no work - no cuts
            if curves_3d.is_empty() {
                continue;
            }

            // Now classify and merge intersections into the Brep
            let merged = p10::merge_curves_on_surface_through_surface(
                k,
                b,
                env,
                curve_ref,
                f,
                xsect_tol_3d,
                &mut curves_3d,
                Some(&uv_curves),
                plane.clone(),
            );

            // skip Merge calls that failed - Don't abort the entire operation because of a
            // failure here [070310]
            let Some((_new_faces, edges_in_brep)) = ignored(merged)? else {
                continue;
            };

            // accumulate list of Edges added to TgtBrp
            self.cutter_edges.extend(edges_in_brep);
        }

        // wire edges

        // get modified targetBrep edges
        let wire_edges = bp1::wire_edges(b, None)?;

        // split every targetBrep wireEdge with this plane
        for wire in wire_edges {
            let mut wire_edge = wire;

            // for wire edges
            if !edge::is_wire(b, wire_edge)? {
                continue;
            }
            let plane_d = -self.plane_point.dot(self.plane_normal);
            let vectors = [self.plane_normal];
            let solutions = {
                let e = b.edge(wire_edge)?;
                let curve = e
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?;
                // find wire/plane intersection points
                curve
                    .global_property_analysis::<NoObject>(
                        k,
                        &e.interval,
                        CurvePropertyType::PlaneIntersection,
                        xsect_tol_3d,
                        &PropertyAnalysisOptions {
                            property_value: Some(plane_d),
                            opt_vectors: &vectors,
                            want_comb: false,
                            handle: None,
                        },
                    )?
                    .solutions
            };

            // For every solution - merge points not on the cutplane into TgtBrep
            for sol in solutions.as_slice() {
                let param = sol.start.param(0)?;
                let v1 = edge::vertex(b, wire_edge)?
                    .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?;
                let v2 = edge::other_vertex(b, wire_edge, Some(v1))?
                    .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?;

                // get edge endPoints, Curve, and SolutionPoint
                let new_v_pnt = b
                    .edge(wire_edge)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?
                    .evaluate_point(k, param)?;

                // when solutionPoint is not close to endPoints
                if new_v_pnt.distance(vertex::point(b, v1)?)
                    > xsect_tol_3d + vertex::tolerance(b, v1)?
                    && new_v_pnt.distance(vertex::point(b, v2)?)
                        > xsect_tol_3d + vertex::tolerance(b, v2)?
                {
                    // split the wire edge with the solutionPoint
                    let split = p9::make_vertex_split_edge(k, b, wire_edge, param, env, curve_ref)?;
                    if !split.split {
                        return Err(SolidError::sm_err(OP_MERGE));
                    }
                    let (Some([_new_e1, new_e2]), Some(new_v)) = (split.edges, split.vertex) else {
                        return Err(SolidError::null_pointer(OP_MERGE));
                    };
                    // SM_OLDTOL_LINE pNewV->SetTolerance(SM_XSECT_TO_ZONETOL3D(dXSectTol3d))
                    let zone = tol::zone_tol3d(k, tol::zone_tol3d(k, xsect_tol_3d) / 2.0);
                    vertex::set_tolerance(b, new_v, zone, true, true)?;

                    // Change Edge to point to second edge in split - this is because
                    // solutions are sorted in ascending order.
                    wire_edge = new_e2;
                }
            }
        }

        // all done
        Ok(())
    }

    /// `SmPlaneCutter::Classification(pTgtBrep, dXSectTol3d, ..)`: the faces, edges and
    /// vertices of `b` to delete. A vertex above the plane (on the normal's side, beyond
    /// `xsect_tol_3d`) is removed with the edges and faces at it (the faces get an
    /// `SM_AI_BOOLEAN_DELETE` attribute); a vertex on the plane only when it bounds one
    /// face. An edge not reached from such a vertex is classified by points at `0.45678`
    /// (and `0.65432` when the first is near the plane) of its interval, with its faces;
    /// an edge in the plane only when its single face carries the delete attribute. A face
    /// reached by neither is classified by the edge use binormals of its upward face use.
    /// Increments and locks an unlocked context mark for the call.
    ///
    /// # Errors
    ///
    /// Those of the B-rep lists, the curve evaluations and `EvaluateBinormal`; of the mark
    /// lock; of the attribute store.
    pub fn classification(
        &self,
        k: &mut Kernel,
        b: &mut Brep,
        env: &mut NotifyEnv<'_>,
        xsect_tol_3d: f64,
    ) -> Result<CutClassification> {
        // Set a new mark increment and lock an unlocked mark - we are using marking to
        // prevent retracing things
        let mut mark = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
        let result = self.classify_marked(k, b, env, xsect_tol_3d);
        let released = mark.release(core::slice::from_mut(&mut k.context));
        let out = result?;
        released?;
        Ok(out)
    }

    /// [`PlaneCutter::classification`] once the mark is held; the marked edges and faces
    /// are local sets.
    fn classify_marked(
        &self,
        k: &Kernel,
        b: &mut Brep,
        env: &mut NotifyEnv<'_>,
        xsect_tol_3d: f64,
    ) -> Result<CutClassification> {
        // init output
        let mut out = CutClassification::default();
        let mut marked_edges: BTreeSet<EdgeId> = BTreeSet::new();
        let mut marked_faces: BTreeSet<FaceId> = BTreeSet::new();

        // for every vertex
        for v in bp1::vertices(b, None)? {
            // get vector from point to plane point
            let vec_to_point = vertex::point(b, v)? - self.plane_point;
            let signed_dist = vec_to_point.dot(self.plane_normal);

            // if the vertex is on the cutting plane AND ONLY on the one face then remove
            // the vertex. This happens when the cutting plane did not split a face
            if signed_dist.abs() < xsect_tol_3d {
                if vertex::faces(b, v)?.len() == 1 {
                    out.vertices.push(v);
                }
                continue;
            }

            // This vertex is definitely above or below the plane. Classify it and all of
            // its connected edges and faces.

            // Classify vertex:
            if signed_dist > 0.0 {
                // Above the plane: place vertex on remove list
                out.vertices.push(v);
            }

            // Classify all edges attached to pVertex:
            for e in vertex::edges(b, v, None)? {
                // skip already processed edges
                if !marked_edges.insert(e) {
                    continue;
                }

                // When edge is attached to vertex above the cutting plane - add edge to
                // remove list
                if signed_dist > 0.0 {
                    out.edges.push(e);
                }
            }

            // Classify all Faces attached to pVertex:
            for f in vertex::faces(b, v)? {
                if !marked_faces.insert(f) {
                    continue;
                }

                // When face is connected to a vertex above the cutting plane - mark plane
                // for deletion
                if signed_dist > 0.0 {
                    // Add attribute to mark it deleted for future use
                    let key = (env.key_of)(EntityRef::Face(f)).key;
                    let attribute = env.attributes.insert(Attribute::long(
                        ids::BOOLEAN_DELETE,
                        1,
                        Behavior::Reference,
                    ))?;
                    env.attributes.add_attribute(key, attribute)?;
                    out.faces.push(f);
                }
            }
        }

        // Now look at every edge that wasn't classified from vertex connections.
        for e in bp1::edges(b, None)? {
            // Skip marked edges: already classified in the Vertex loop.
            if !marked_edges.insert(e) {
                continue;
            }

            // evaluate a nearMid Edge Point - project that to the plane
            let edge_tol = edge::tolerance(b, e)?;
            let ivl = edge::interval(b, e)?;
            let point_at = |t: f64| -> Result<Point3> {
                b.edge(e)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP_CLASSIFY))?
                    .evaluate_point(k, ivl.evaluate(t))
            };
            let mut signed_dist = (point_at(0.45678)? - self.plane_point).dot(self.plane_normal);

            // Be loose with this tolerance, because they will be caught when checking
            // faces.
            if signed_dist.abs() < edge_tol + xsect_tol_3d {
                // Try one more interior point to make sure.
                signed_dist = (point_at(0.65432)? - self.plane_point).dot(self.plane_normal);

                // Edge is contained in plane: if edge has only one face then cutting
                // plane did not split a face so remove the edge
                if signed_dist.abs() < edge_tol + xsect_tol_3d {
                    let faces = edge::faces(b, e, None)?;
                    if let [face] = faces.as_slice() {
                        // Check for previously applied attribute on face to remove
                        let key = (env.key_of)(EntityRef::Face(*face)).key;
                        if env
                            .attributes
                            .find_attribute(key, ids::BOOLEAN_DELETE)
                            .is_some()
                        {
                            out.edges.push(e);
                        }
                    }
                    continue;
                }
            }

            // This Edge is definitely above or below the plane. Classify it and all of
            // its connected faces.

            // Classify this edge:
            if signed_dist > 0.0 {
                // Above the plane: remove this Edge.
                out.edges.push(e);
            }

            // Classify all Faces attached to this Edge:
            for f in edge::faces(b, e, None)? {
                if !marked_faces.insert(f) {
                    continue;
                }

                // Classify this face:
                if signed_dist > 0.0 {
                    out.faces.push(f);
                }
            }
        }

        // Finally we have to pick up faces which have no edge or vertices that are not
        // on the plane. These are the dome cases where a dome of a face is cut with the
        // plane. Since all of their edges and vertices are in the plane, we'll look at
        // the binormals of the edges: pointing up or down w.r.t the plane.
        for f in bp1::faces(b, None)? {
            // skip faces already marked; remember this face has been processed
            if !marked_faces.insert(f) {
                continue;
            }

            // for every edgeuse on the face's upward faceuse
            let fu = face2::upward_faceuse(b, f)?
                .ok_or_else(|| SolidError::null_pointer(OP_CLASSIFY))?;
            for eu in eu1::faceuse_edgeuses(b, fu, false, None)? {
                // get edgeuse midPoint binormal vector
                let e = eu1::edge(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP_CLASSIFY))?;
                let mid = edge::interval(b, e)?.evaluate(0.5);
                let binormal =
                    eu3::evaluate_binormal(k, b, eu, mid, false, &eu3::BinormalOptions::default())?
                        .binormal;

                // classify the binormal direction against the plane's normal direction
                let dot = binormal.dot(self.plane_normal);

                // skip edges where the binormal vector is nearly tangent to the plane
                if dot.abs() < EFF_ZERO_SQRT {
                    // too close to call try another edge
                    continue;
                }

                // when face is above plane
                if dot > 0.0 {
                    out.faces.push(f);
                }

                break; // go to next face
            }

            // arrive here when all edgesuse midPoint binormals are tangent to the plane.
        }

        // all done
        Ok(out)
    }
}

/// `SmBrepCutting(b).DoCut(pCutter, d3DTolerance, bRemoveCutPortions, pOptSubsetFaces)`
/// with a plane cutter: merges the cutter's intersection into `b`
/// ([`PlaneCutter::merge_intersection`]) and, when `remove_cut_portions`, deletes the
/// classified faces, then the classified edges and vertices still there, and stitches
/// and orients the result. The B-rep's editing flag and the context's doing-Boolean
/// flag are set for the call.
///
/// # Errors
///
/// Those of [`PlaneCutter::merge_intersection`] and
/// [`PlaneCutter::classification`]; [`SolidError::Unported`] from the deletions and the
/// stitching (whose other statuses are ignored, as upstream).
#[allow(clippy::too_many_arguments)]
pub fn do_cut(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    cutter: &mut PlaneCutter,
    tolerance_3d: f64,
    remove_cut_portions: bool,
    subset_faces: Option<&[FaceId]>,
) -> Result<()> {
    // set m_pBrepToCut EditingEnabled flag to TRUE for upcoming boolean
    let editing = b.editing_enabled();
    b.set_editing_enabled(true);
    let doing_boolean = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let result = do_cut_body(
        k,
        b,
        env,
        curve_ref,
        cutter,
        tolerance_3d,
        remove_cut_portions,
        subset_faces,
    );
    k.context.set_doing_boolean(doing_boolean);
    b.set_editing_enabled(editing);
    result
}

/// [`do_cut`] with the flags set.
#[allow(clippy::too_many_arguments)]
fn do_cut_body(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    cutter: &mut PlaneCutter,
    tolerance_3d: f64,
    remove_cut_portions: bool,
    subset_faces: Option<&[FaceId]>,
) -> Result<()> {
    // Do intersection of cutter with brep
    cutter.merge_intersection(k, b, env, curve_ref, tolerance_3d, subset_faces)?;

    // no work - when not removing cut portions
    if !remove_cut_portions {
        return Ok(());
    }

    // Now classify faces, edges, and vertices - return list of them which are to be
    // deleted from the brep.
    let to_delete = cutter.classification(k, b, env, tolerance_3d)?;

    // Now delete things which have been cutaway from the brep
    let mut deleted_edges_vertices: Vec<EntityRef> = Vec::new();

    // delete every face on sFacesToDel list from targetBrep
    for &f in &to_delete.faces {
        // (the planar cutter takes care of hanging wire edges and shell vertices)
        let mut marks = Marks::default();
        let options = DeleteFaceOptions {
            delete_edges_vertices: false,
            do_region_nesting: true,
            region_to_keep: None,
        };
        if let Some(deleted) = ignored(bp3::delete_face(k, b, env, &mut marks, f, options))? {
            deleted_edges_vertices.extend(deleted);
        }
    }

    // delete every edge on sEdgesToDel list from targetBrep
    for &e in &to_delete.edges {
        // skip already deleted edges
        if deleted_edges_vertices.contains(&EntityRef::Edge(e)) {
            continue;
        }
        let mut d = DeleteEnv::from_notify(env);
        ignore_status(bp4::delete_edge(k, b, &mut d, e, None))?;
    }

    // delete every vertex on sVerticesToDel list from targetBrep
    for &v in &to_delete.vertices {
        // skip already deleted vertices
        if deleted_edges_vertices.contains(&EntityRef::Vertex(v)) {
            continue;
        }
        let mut d = DeleteEnv::from_notify(env);
        ignore_status(bp4::delete_vertex(k, b, &mut d, v))?;
    }

    ignore_status(p5::stitch_and_orient(
        k,
        b,
        &StitchAndOrientOptions::default(),
    ))?;
    Ok(())
}

/// `SmApiCut(pBrep, rPlanePt, rPlaneNormal)` under the caller's notification
/// environment: cuts away the part of `b` on the side of `plane_normal` of the plane
/// through `plane_point` ([`do_cut`] with a [`PlaneCutter`], the B-rep's tolerance,
/// removing the cut portions).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` ([`SolidError::Upstream`]) for a normal `Unitize` rejects,
/// before `b` is touched; those of [`do_cut`].
pub fn cut_notified(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    plane_point: Point3,
    plane_normal: Vec3,
) -> Result<()> {
    let Ok(normal) = plane_normal.unitize() else {
        return Err(SolidError::Upstream {
            operation: OP_CUT,
            code: SM_ERR_INVALID_INPUT,
            entity: None,
        });
    };
    let mut cutter = PlaneCutter::new(plane_point, normal);
    let tolerance = b.tolerance();
    do_cut(k, b, env, curve_ref, &mut cutter, tolerance, true, None)
}

/// `SmApiCut(pBrep, rPlanePt, rPlaneNormal)`: [`cut_notified`] through an arena-only
/// notification store (the caller holds none).
///
/// # Errors
///
/// Those of [`cut_notified`].
pub fn cut(k: &mut Kernel, b: &mut Brep, plane_point: Point3, plane_normal: Vec3) -> Result<()> {
    let mut state = ArenaNotifyState::new(b);
    let mut env = state.env(None);
    let curve_ref =
        |e: EdgeId, _type_code: i64| crate::topology::loopuse::notify_ref(EntityRef::Edge(e));
    cut_notified(k, b, &mut env, &curve_ref, plane_point, plane_normal)
}

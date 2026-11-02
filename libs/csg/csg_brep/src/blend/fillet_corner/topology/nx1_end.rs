//! The end of an `NByOne` corner: the end rib and, at a mixed-convexity corner, the
//! surface extension and the cap topology around the original vertex.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (606–667, 6177–6443).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`find_surface_extension_dist_at_corner`] | `sm_FindSurfaceExtensionDistAtCorner` |
//! | `finish_nx1_end` (crate) | tail of `SmFilletNx1Corner::MakeCornerTopology` |
//!
//! Deviations: the solver is the caller's concrete solver object instead of a
//! `GetSolverType` cast. A failed variable-radius law evaluation yields its status
//! code as the radius, as upstream's `SER` inside the `double`
//! `SmVariableRadiusFS::GetFilletRadius` does (an `Unported` callee still
//! propagates). The gap checks of a multi-neighbour end face (vertex and side-edge
//! geometry, extended-surface coincidence), the last statements of
//! `MakeCornerTopology`, are returned as an [`Nx1GapCheck`] for the caller, which
//! holds the corner table and solver store they need, to run before anything else.
//! There is no recursion.

use std::collections::BTreeMap;

use crate::blend::fillet_corner::p1::face_by_two_edgeuses;
use crate::blend::fillet_corner::p3::FilletCorner;
use crate::blend::fillet_executive::session::{FilletBody, FilletObject, SolverObject};
use crate::blend::fillet_geom::p1::{
    FilletBrep, FilletCornerId, FilletEdgeType, FilletGeomId, FilletVertex, FilletVertexType,
};
use crate::error::{EntityRef, Result, SolidError, SM_ERR, SM_ERR_INVALID_INPUT};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::math::{Point3, EFF_ZERO};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, Vertex, VertexId};

const OP: &str = "blend::fillet_corner::topology::make_nx1_corner_topology";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// The `SmStatus` upstream's `SER` returns for a failed callee.
fn status_of(e: &SolidError) -> i32 {
    match e {
        SolidError::Upstream { code, .. } => *code,
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => SM_ERR_INVALID_INPUT,
        _ => SM_ERR,
    }
}

fn need<T>(value: Option<T>, reason: &'static str) -> Result<T> {
    value.ok_or_else(|| invalid(reason))
}

/// `sm_FindSurfaceExtensionDistAtCorner`: an extension distance for the end surface of
/// an Nx1 or 3x2 mixed-convexity corner, `7.5` times the largest fillet radius of a
/// constant-radius solver, the distance of a constant-distance solver or the radius a
/// variable-radius solver has at the corner end of `fillet_edgeuse` (`0` for any other
/// solver, whose `SE(SM_ERR)` is only reported).
///
/// # Errors
/// [`SolidError::InvalidInput`] for an edge use without vertex use or edge (upstream
/// dereferences them); [`SolidError::Unported`] from the variable-radius law evaluation.
pub fn find_surface_extension_dist_at_corner(
    target: &Brep,
    fillet_edgeuse: EdgeuseId,
    solver: &mut SolverObject,
    corner_vertex: VertexId,
) -> Result<f64> {
    let mut ext_dist = 0.0;
    match solver {
        // return largest fillet radius
        SolverObject::ConstantRadius(cr) => {
            for i in 0..2 {
                let rad = cr.fillet_radius(i)?;
                if rad > ext_dist {
                    ext_dist = rad;
                }
            }
        }
        // return constant distance
        SolverObject::ConstantDistance(cd) => ext_dist = cd.distance(),
        // return radius at vertex
        SolverObject::VariableRadius(vr) => {
            let e = need(eu1::edge(target, fillet_edgeuse)?, "edge use without edge")?;
            let ivl = edge::interval(target, e)?;
            let vu = need(
                eu1::vertexuse(target, fillet_edgeuse)?,
                "edge use without vertex use",
            )?;
            let vert = target.vertexuse(vu)?.vertex;
            let orient = eu1::orientation(target, fillet_edgeuse)?;
            let parameter = if (vert == Some(corner_vertex) && orient == OrientType::Opposite)
                || (vert != Some(corner_vertex) && orient == OrientType::Same)
            {
                ivl.max()
            } else {
                ivl.min()
            };
            let interval = vr.law_interval;
            let orientation = vr.law_orientation;
            // SmVariableRadiusFS::GetFilletRadius is a `double` function whose
            // `SER(m_rLaw->Evaluate(..))` returns the failed `SmStatus` itself, so a
            // failed law evaluation yields the status code as the radius.
            ext_dist = match vr.law.evaluate(parameter, &interval, orientation) {
                Ok(values) => values[0],
                Err(e) if e.is_unported() => return Err(e),
                Err(e) => f64::from(status_of(&e)),
            };
        }
        // SE(SM_ERR): reported only
        SolverObject::SurfaceSurface(_) => {}
    }

    // In prog_test, Fillet test my_test_suite_0 iter: 2 will create a surface
    // with a slight crease for extension values greater than about 7.5.
    let rad_factor = 7.5;
    Ok(rad_factor * ext_dist)
}

/// The live objects of the end of `SmFilletNx1Corner::MakeCornerTopology`.
pub(crate) struct Nx1End<'a> {
    pub k: &'a Kernel,
    pub target: &'a mut Brep,
    pub pseudo: &'a mut FilletBrep,
    pub corner: FilletCornerId,
    pub solver: &'a mut SolverObject,
    pub extended_surfaces: &'a mut BTreeMap<FaceId, Surface>,
    pub topo_edges: &'a mut Vec<Option<FilletObject>>,
    pub geometry: FilletGeomId,
    pub filleted: EdgeuseId,
    pub vertex: VertexId,
    pub total: usize,
    pub mixed: bool,
    pub same: [bool; 2],
    pub sides: [EdgeuseId; 2],
    pub end_face: Option<FaceId>,
    pub extend_edge: Option<EdgeId>,
}

/// The gap checks `SmFilletNx1Corner::MakeCornerTopology` ends with when the end face
/// of a mixed-convexity corner connects to more than the fillet surface and one other
/// face: `CalcCornerVertGeom`, `CalcCornerEdgeGeom` of the two new side edges (statuses
/// ignored) and, when the end face's extended surface was kept, its
/// `SimpleCoincidenceChecker` against the other side edge, clearing
/// `m_bExtendedSurfacePatch` when they are not coincident. The caller runs them right
/// after the topology returns, before anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nx1GapCheck {
    /// `pNewFilletEdge1` (rail-1 vertex side, along `pEU[0]`).
    pub edge1: EdgeId,
    /// `pNewFilletEdge2` (rail-2 vertex side, along `pEU[1]`).
    pub edge2: EdgeId,
    /// The end face whose new extended surface stayed in the extended-surface map
    /// (`pNewExtendSurface` not NULL), if any.
    pub extended_face: Option<FaceId>,
    /// `pOtherSideEdgeuse`.
    pub other_side_edgeuse: Option<EdgeuseId>,
}

fn pseudo_edge(edge: EdgeId) -> FilletObject {
    FilletObject::Topology(FilletBody::Pseudo, EntityRef::Edge(edge))
}

impl FilletCorner {
    /// From the extension distance on: the end rib, its end face, the extended end
    /// surface and, at a mixed-convexity corner, the vertex on the original vertex and
    /// the two side edges joining it to the rail vertices; the gap checks of a
    /// multi-neighbour end face are returned for the caller.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub(crate) fn finish_nx1_end(&mut self, env: Nx1End<'_>) -> Result<Option<Nx1GapCheck>> {
        let Nx1End {
            k,
            target,
            pseudo,
            corner,
            solver,
            extended_surfaces,
            topo_edges,
            geometry,
            filleted,
            vertex,
            total,
            mixed,
            same,
            sides,
            mut end_face,
            mut extend_edge,
        } = env;

        // compute how much to extend the endSurface to make sure it covers the 'hole'
        let mut ext_dist = if mixed {
            find_surface_extension_dist_at_corner(target, filleted, solver, vertex)?
        } else {
            0.0
        };

        let start = self.vertices.first().copied();
        let end = self.vertices.get(1).copied();
        let rib = pseudo.make_fillet_edge(start, end, Some(corner))?;
        pseudo
            .fillet_edge_mut(rib)?
            .set_fillet_edge_type(FilletEdgeType::FilletXSideFace);
        let primary = need(
            eu1::primary_edgeuse(pseudo.brep(), rib)?,
            "end rib without primary edge use",
        )?;
        pseudo.set_edgeuse_fillet_geom(primary, Some(geometry))?;
        self.edges.push(rib);

        // In the mixed-cvxty case, pEndFace will be NULL when we arrive here
        // if both side edges have opposite convexity to the filleted edge.
        let mut multi_neighbor_end_face = false;
        let mut other_side_edgeuse = None;
        if end_face.is_none() {
            let [first, second] = sides;
            end_face = face_by_two_edgeuses(target, Some(first), Some(second), None)?;
            if end_face.is_none() {
                multi_neighbor_end_face = true;
                if total > 3 {
                    // Use the side edge that is a cliff edge.
                    let (pick, other) = if same[0] {
                        (first, second)
                    } else if same[1] {
                        (second, first)
                    } else {
                        // both are cliff edges, just pick one.
                        (first, second)
                    };
                    end_face = eu1::face(target, pick)?;
                    extend_edge = eu1::edge(target, pick)?;
                    other_side_edgeuse = Some(other);
                } else {
                    // Invalid brep topology
                    return Err(SolidError::sm_err(OP));
                }
            }
        }
        pseudo.fillet_edge_mut(rib)?.set_original_face(end_face);

        // compute how much to extend the sideSurface to make sure it covers the 'hole'
        let mut do_extend = extend_edge.is_some();
        if ext_dist > EFF_ZERO && !do_extend && matches!(solver, SolverObject::ConstantRadius(_)) {
            ext_dist *= 0.1; // use only a small extension
            do_extend = true; // for constant radius only
        }

        // When a surface extension is needed to define the endFace shape,
        // build and save it.
        let mut new_extend_face = None;
        if do_extend {
            let face = need(end_face, "extended end face is null")?;
            let surface = need(
                target.face(face)?.surface.as_ref(),
                "end face without surface",
            )?;
            // create an extended surface large enough to fill in the endCap requirements
            let new_surface = surface.create_extended_surface(k, ext_dist, ContinuityType::G1)?;
            // Copy pNewSurface, when possible as an analytic surface
            let new_extend_surface = new_surface.copy_and_add_analytics(k)?;
            // If AddExtendedOriginalSurface() deleted pNewExtendSurface, then
            // pEndFace already had an extended surface, and we don't have to
            // deal with it anymore.
            if Self::add_extended_original_surface_with_map(
                extended_surfaces,
                face,
                new_extend_surface,
            ) {
                new_extend_face = Some(face);
            }
        }

        // When adding an endSurface to fill the 'hole',
        // set up the topology for the end cap.
        if mixed {
            // add a filletVertex at the origVertex location
            let fv = pseudo.brep_mut().add_unlinked_vertex_record(Vertex {
                point: Point3::ZERO,
                tolerance: 0.0,
                ..Vertex::default()
            })?;
            pseudo.insert_fillet_vertex(fv, FilletVertex::new(k, Some(corner)));
            pseudo
                .fillet_vertex_mut(fv)?
                .set_point_class(PointClassType::Vertex, Some(EntityRef::Vertex(vertex)));
            pseudo
                .fillet_vertex_mut(fv)?
                .set_fillet_vertex_type(FilletVertexType::OnVertex);
            self.vertices.push(fv);

            let v = |i: usize| -> Result<VertexId> {
                need(self.vertices.get(i).copied(), "corner vertex index")
            };
            let (v0, v1, v2) = (v(0)?, v(1)?, v(2)?);

            // Add 1st fillet edge from expected rail2/sideEdge2 intersection to origVertex
            let edge2 = pseudo.make_fillet_edge(Some(v1), Some(v2), Some(corner))?;
            if pseudo.fillet_vertex(v1)?.fillet_vertex_type()
                == FilletVertexType::RailXExtendedEdgeuse
            {
                let fe = pseudo.fillet_edge_mut(edge2)?;
                fe.set_fillet_edge_type(FilletEdgeType::OnExtendEdge);
                fe.set_original_edge(extend_edge);
            } else {
                pseudo
                    .fillet_edge_mut(edge2)?
                    .set_fillet_edge_type(FilletEdgeType::OnEdge);
                if total <= 3 {
                    // Register this edge and process it later
                    // when removing topological vertices
                    topo_edges.push(Some(pseudo_edge(edge2)));
                }
                let original = eu1::edge(target, sides[1])?;
                pseudo.fillet_edge_mut(edge2)?.set_original_edge(original);
            }
            let face1 = eu1::face(target, sides[1])?;
            pseudo.fillet_edge_mut(edge2)?.set_original_face(face1);
            self.edges.push(edge2);

            // Add 2nd fillet edge from origVertex to other expected rail1/sideEdge intersection
            let edge1 = pseudo.make_fillet_edge(Some(v2), Some(v0), Some(corner))?;
            if pseudo.fillet_vertex(v0)?.fillet_vertex_type()
                == FilletVertexType::RailXExtendedEdgeuse
            {
                let fe = pseudo.fillet_edge_mut(edge1)?;
                fe.set_fillet_edge_type(FilletEdgeType::OnExtendEdge);
                fe.set_original_edge(extend_edge);
            } else {
                pseudo
                    .fillet_edge_mut(edge1)?
                    .set_fillet_edge_type(FilletEdgeType::OnEdge);
                if total <= 3 {
                    topo_edges.push(Some(pseudo_edge(edge1)));
                }
                let original = eu1::edge(target, sides[0])?;
                pseudo.fillet_edge_mut(edge1)?.set_original_edge(original);
            }
            let face0 = eu1::face(target, sides[0])?;
            pseudo.fillet_edge_mut(edge1)?.set_original_face(face0);
            self.edges.push(edge1);

            // When endFace connects to more than the filletSurface and 1 other Face,
            // check for gaps
            if multi_neighbor_end_face {
                return Ok(Some(Nx1GapCheck {
                    edge1,
                    edge2,
                    extended_face: new_extend_face,
                    other_side_edgeuse,
                }));
            }
        }
        Ok(None)
    }
}

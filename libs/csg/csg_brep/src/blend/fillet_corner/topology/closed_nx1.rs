//! Topology at a corner where one closed filleted edge meets its side edges.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (4962–5129).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletCorner::make_nx1_closed_corner_topology`] | `SmFilletNx1ClosedCorner::MakeCornerTopology` |
//!
//! Deviations: the caller lends the actual target, pseudo-B-rep and solver (the
//! [`Nx1TopologyEnv`] of the open `Nx1` corner; its global-merge switch is not used).
//! A corner with no side edge use (upstream then reads `m_vVertices[0]` of an empty
//! list) and a closed corner with more than two edge uses but fewer than two fillet
//! vertices are input errors. There is no recursion.

use super::{need, new_corner_vertex, Nx1TopologyEnv};
use crate::blend::fillet_corner::p1::test_g1_continuity;
use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletCornerId, FilletEdgeType, FilletVertexType,
};
use crate::error::{EntityRef, Result, SolidError};
use crate::intersect::point_class::PointClassType;
use crate::math::rad_to_deg;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::edge;
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};

const OP: &str = "blend::fillet_corner::topology::make_nx1_closed_corner_topology";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

impl FilletCorner {
    /// Construct the rail vertices and the single cross edge of an actual
    /// `NByOneClosed` corner (`SmFilletNx1ClosedCorner::MakeCornerTopology`).
    ///
    /// Sets the corner's G1 flag and blending state, and the fillet geometry's split
    /// flag when the closed edge is not G1 at the vertex. Allocations and
    /// corner-list insertions occur in native order and remain visible if a later
    /// operation fails. Vertex geometry is computed later.
    ///
    /// # Errors
    /// Invalid topology or a different corner kind; those of the edge-use binormal,
    /// curve evaluation, solver rail lookups and pseudo-topology operations;
    /// `SM_ERR_NULL_POINTER` for a side edge use without a radial; `SM_ERR` when the
    /// new fillet edge does not have exactly two edge uses.
    #[allow(clippy::too_many_lines)]
    pub fn make_nx1_closed_corner_topology(&mut self, env: Nx1TopologyEnv<'_>) -> Result<()> {
        fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
            CornerLookup {
                this: Some((Some(corner), this)),
                ..Default::default()
            }
        }
        let Nx1TopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solver,
            ..
        } = env;
        let solver = solver.base_mut();
        if self.kind != FilletCornerKind::NByOneClosed {
            return Err(invalid("requires an NByOneClosed corner"));
        }

        // locals - 1st outward bound fillet edgeuse, its FilletSolver, and its 1st fillet geom
        let fil_edgeuse = need(
            self.solver_eus
                .as_deref()
                .and_then(|eus| eus.first())
                .copied(),
            "corner without filleted edge use",
        )?;
        let fillet_geom = solver.first_fillet_geom()?;
        let all = need(self.all_eus.clone(), "corner without outward edge uses")?;
        let total = all.len();
        let mut seam_edgeuse = None;

        // Test if the filleted edge is G1-closed
        let filleted_edge = eu1::edge(target, fil_edgeuse)?;
        self.g1_case = test_g1_continuity(
            k,
            target,
            self.vertex,
            filleted_edge,
            filleted_edge,
            rad_to_deg(self.tangency_tol_radians),
        )?
        .0;

        // for all edges connected to this corner
        for &eu in &all {
            // skip the filletEdge - process the others
            if eu == fil_edgeuse {
                continue;
            }
            let mut seam = eu;

            // get Seam edgeuse's binormal at vertex (seam is not the fillet edgeuse)
            let seam_edge = need(eu1::edge(target, seam)?, "side edge use without edge")?;
            let ivl = edge::interval(target, seam_edge)?;
            let t = if eu1::orientation(target, seam)? == OrientType::Same {
                ivl.min()
            } else {
                ivl.max()
            };
            let bin_vec = eu3::evaluate_binormal(
                k,
                target,
                seam,
                t,
                false,
                &eu3::BinormalOptions::default(),
            )?
            .binormal;

            // get filletEdge's tangent at vertex
            let fil_edge = need(filleted_edge, "filleted edge use without edge")?;
            let ivl1 = edge::interval(target, fil_edge)?;
            let t1 = if eu1::orientation(target, fil_edgeuse)? == OrientType::Same {
                ivl1.min()
            } else {
                ivl1.max()
            };
            let curve = need(
                edge::curve(target, fil_edge)?,
                "filleted edge without curve",
            )?;
            let pv1 = curve.evaluate(k, t1, 1, true, true)?;
            let tangent = *need(pv1.get(1), "curve evaluation without tangent")?;

            // when binormal opposes tangent - get radial edgeuse
            if tangent.dot(bin_vec) < 0.0 {
                seam = eu1::radial(target, seam)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            }
            seam_edgeuse = Some(seam);

            // Create a fillet vertex on the seam edge - label it as intersection of
            // railCurve and edge->Curve
            let fv = new_corner_vertex(k, pseudo, corner)?;
            {
                let v = pseudo.fillet_vertex_mut(fv)?;
                v.set_point_class(PointClassType::Edgeuse, Some(EntityRef::Edgeuse(seam)));
                v.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }
            self.vertices.push(fv);

            // Connect FilletVertex to the railCurve that intersects the seam edge use
            // with 2 new FilletEdgeuse->FilletVertexuse pairs (the rail is closed)
            let rail = solver.find_index_of_rail_x_side_edgeuse(target, seam)?;
            for orient in [OrientType::Same, OrientType::Opposite] {
                pseudo.make_rail_edgeuse_on(
                    fillet_geom,
                    rail,
                    Some(fv),
                    orient,
                    None,
                    lookup(corner, self),
                    solver,
                    target,
                )?;
            }
        }

        // create the filletVertex mate - the filletVertex on the other railCurve
        let fv0 = need(
            self.vertices.first().copied(),
            "corner without side edge use",
        )?;
        if total == 2 {
            let seam = need(seam_edgeuse, "corner without side edge use")?;
            // Create a fillet vertex on the side face
            let fv = new_corner_vertex(k, pseudo, corner)?;
            let mut face = eu1::face(target, fil_edgeuse)?;
            if face == eu1::face(target, seam)? {
                let radial = need(
                    eu1::radial(target, fil_edgeuse)?,
                    "filleted edge use without radial",
                )?;
                face = eu1::face(target, radial)?;
            }

            // remember that this vertex connects to the face
            pseudo
                .fillet_vertex_mut(fv)?
                .set_point_class(PointClassType::Face, face.map(EntityRef::Face));

            if self.g1_case {
                // for tangent closed-curves - vertex is mate to vertex on seam edge
                {
                    let v = pseudo.fillet_vertex_mut(fv)?;
                    v.set_fillet_vertex_type(FilletVertexType::Mate);
                    v.set_mate(0, Some(fv0));
                }
                pseudo.fillet_vertex_mut(fv0)?.set_mate(0, Some(fv));
            } else {
                // for non-g1 closed curves - vertex is located at fillet/fillet intersection
                pseudo
                    .fillet_vertex_mut(fv)?
                    .set_fillet_vertex_type(FilletVertexType::FilletXFillet);
            }

            // add the 2nd vertex to the FilletCorner list
            self.vertices.push(fv);
            let rail = solver.find_index_of_rail_on_face(target, face)?;

            // connect 2nd vertex to 2nd railCurve with 2 FilletEdgeuse->FilletVertexuse
            // pairs (the railCurve is closed)
            for orient in [OrientType::Same, OrientType::Opposite] {
                pseudo.make_rail_edgeuse_on(
                    fillet_geom,
                    rail,
                    Some(fv),
                    orient,
                    None,
                    lookup(corner, self),
                    solver,
                    target,
                )?;
            }
        }

        // Corner consists of one fillet edge only (joining fv0 & fv1)
        let fv1 = need(
            self.vertices.get(1).copied(),
            "corner with one fillet vertex",
        )?;
        let fillet_edge = pseudo.make_fillet_edge(Some(fv0), Some(fv1), Some(corner))?;
        if self.g1_case {
            // edge shape is same as filletSurface crossSection shape
            pseudo
                .fillet_edge_mut(fillet_edge)?
                .set_fillet_edge_type(FilletEdgeType::CrossSection);
            self.blending = true;
        } else {
            // Split the fillet so that we don't self-intersect
            pseudo.geom_mut(fillet_geom)?.need_split = true;
            pseudo
                .fillet_edge_mut(fillet_edge)?
                .set_fillet_edge_type(FilletEdgeType::FilletXFillet);
            self.blending = false;
        }

        // get edgeuses marking filletEdge sector
        if edge::edgeuses(pseudo.brep(), fillet_edge, None, None)?.len() != 2 {
            return Err(SolidError::sm_err(OP));
        }

        // set FilletEdgeuse->FilletGeom pointers
        let prim = need(
            eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
            "fillet edge without primary edge use",
        )?;
        pseudo.set_edgeuse_fillet_geom(prim, Some(fillet_geom))?;
        let mate = need(
            eu1::mate(pseudo.brep(), prim)?,
            "fillet edge use without mate",
        )?;
        pseudo.set_edgeuse_fillet_geom(mate, Some(fillet_geom))?;

        // add this edge to the corner list of edges
        self.edges.push(fillet_edge);
        Ok(())
    }
}

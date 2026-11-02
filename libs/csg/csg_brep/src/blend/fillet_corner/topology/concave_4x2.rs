//! The mixed-convexity 4x2 corner: two opposite filleted edges at a vertex with four
//! edges, one of the two side edges concave. The rails cross the convex side edge at
//! one fillet vertex; at the concave side edge one rail stops on that edge and the
//! other on the face it shares with the concave fillet, and a three-sided corner face
//! closes the gap.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (548–604, 6905–7062).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `FilletCorner::make_4x2_concave_corner_topology` (crate) | `SmFilletNx2Corner::Make4x2ConcaveCornerTopology` |
//! | `find_angle_between_edgeuses` (private) | `sm_FindAngleBetweenEdgeuses` |
//!
//! Deviations: the two solvers, their first geometries, the corner vertex and the
//! outward edge uses come from the caller (`make_nx2_corner_topology`, which has already
//! read them; upstream reads them again, without effects). A failed end tangent makes
//! the angle that status code, as upstream's `SER` inside a `double` function does
//! (an `Unported` callee still propagates); a zero tangent makes it `0`, as
//! `AngleBetween` reports. Edge uses, vertex uses and edges that upstream dereferences
//! are input errors when missing. There is no recursion.

use super::{edge_of, find_side_edgeuse, need, new_corner_vertex, radial, vertex_of, OP_NX2};
use crate::blend::fillet_corner::p1::{
    check_face_complimentary_corner_concavity, face_by_two_edgeuses,
};
use crate::blend::fillet_corner::p3::FilletCorner;
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletBrep, FilletCornerId, FilletEdgeType, FilletGeomId, FilletVertexType,
};
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::error::{EntityRef, Result, SolidError, SM_ERR, SM_ERR_INVALID_INPUT};
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{Brep, EdgeuseId, VertexId};

/// The `SmStatus` upstream's `SER` returns, as the `double` result.
fn status_angle(e: &SolidError) -> f64 {
    let code = match e {
        SolidError::Upstream { code, .. } => *code,
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => SM_ERR_INVALID_INPUT,
        _ => SM_ERR,
    };
    f64::from(code)
}

/// The tangent of `eu`'s edge at the corner end, pointing away from the corner (the
/// `bAtEdgeStart` test and negation of `sm_FindAngleBetweenEdgeuses`).
fn outward_tangent(
    k: &Kernel,
    target: &Brep,
    corner_vertex: VertexId,
    eu: EdgeuseId,
) -> Result<std::result::Result<crate::math::Vec3, f64>> {
    let vert = vertex_of(target, eu)?;
    let orient = eu1::orientation(target, eu)?;
    let at_start = !((Some(corner_vertex) == vert && orient == OrientType::Opposite)
        || (Some(corner_vertex) != vert && orient == OrientType::Same));
    let e = edge_of(target, eu)?;
    match edge::end_tangent(k, target, e, at_start) {
        Ok(t) => Ok(Ok(if at_start { t } else { -t })),
        Err(err) if err.is_unported() => Err(err),
        Err(err) => Ok(Err(status_angle(&err))),
    }
}

/// `sm_FindAngleBetweenEdgeuses`: the angle between the two edges' tangents pointing
/// away from `corner_vertex`.
fn find_angle_between_edgeuses(
    k: &Kernel,
    target: &Brep,
    corner_vertex: VertexId,
    eu1: EdgeuseId,
    eu2: EdgeuseId,
) -> Result<f64> {
    let tangent1 = match outward_tangent(k, target, corner_vertex, eu1)? {
        Ok(t) => t,
        Err(status) => return Ok(status),
    };
    let tangent2 = match outward_tangent(k, target, corner_vertex, eu2)? {
        Ok(t) => t,
        Err(status) => return Ok(status),
    };
    // SE: only reported; AngleBetween reports 0 for a zero vector.
    Ok(tangent1.angle_between(tangent2).unwrap_or(0.0))
}

/// The objects `make_4x2_concave_corner_topology` reads.
pub(crate) struct Concave4x2<'a> {
    pub k: &'a Kernel,
    pub target: &'a mut Brep,
    pub pseudo: &'a mut FilletBrep,
    pub corner: FilletCornerId,
    pub solvers: [&'a FilletSolver; 2],
    pub geoms: [FilletGeomId; 2],
    pub vertex: VertexId,
    pub base_eus: [EdgeuseId; 2],
    pub all_eus: &'a [EdgeuseId],
}

impl FilletCorner {
    /// `SmFilletNx2Corner::Make4x2ConcaveCornerTopology`: for each side edge, the
    /// fillet vertex where the rails cross it (at the concave side edge, the rail of
    /// the concave fillet ends on a face vertex instead, mate of the edge vertex), the
    /// two rail edge uses, then the three-sided corner face: two cross sections, one
    /// for each fillet, and a rail/rail interpolation inside the common face.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub(crate) fn make_4x2_concave_corner_topology(&mut self, env: Concave4x2<'_>) -> Result<()> {
        fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
            CornerLookup {
                this: Some((Some(corner), this)),
                ..Default::default()
            }
        }
        let Concave4x2 {
            k,
            target,
            pseudo,
            corner,
            solvers: [fil_solver1, fil_solver2],
            geoms: [fillet_geom1, fillet_geom2],
            vertex,
            base_eus: [base_eu1, base_eu2],
            all_eus,
        } = env;
        let unknown = OrientType::Unknown;

        let mut common_face = None;
        let mut fv_on_common_face = None;
        let mut fv_on_convex_edge = None;
        let mut convex_fg = None;
        let mut concave_fg = None;

        // Make topology for all (both) of the non-filleted edges at this vertex.
        for ii in 0..4 {
            let curr_eu = *need(all_eus.get(ii), "4x2 corner with fewer than 4 edge uses")?;
            if curr_eu == base_eu1 || curr_eu == base_eu2 {
                continue; // Do only the non-filleted ones.
            }

            // This edgeuse is in between two filletted edgeuses
            let mut fv = new_corner_vertex(k, pseudo, corner)?;
            self.vertices.push(fv);

            let side_eu = find_side_edgeuse(target, base_eu1, curr_eu)?;
            {
                let v = pseudo.fillet_vertex_mut(fv)?;
                v.set_point_class(PointClassType::Edgeuse, side_eu.map(EntityRef::Edgeuse));
                v.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }
            let side_eu = need(side_eu, "no side edge use intersects rail")?;

            let fv2;
            if check_face_complimentary_corner_concavity(
                k,
                target,
                vertex,
                base_eu1,
                base_eu2,
                Some(curr_eu),
            )? {
                let angle1 = find_angle_between_edgeuses(k, target, vertex, curr_eu, base_eu1)?;
                let angle2 = find_angle_between_edgeuses(k, target, vertex, curr_eu, base_eu2)?;

                if angle1 > angle2 {
                    common_face =
                        face_by_two_edgeuses(target, Some(curr_eu), Some(base_eu1), None)?;
                    let side_radial = radial(target, side_eu)?;
                    pseudo.fillet_vertex_mut(fv)?.set_point_class(
                        PointClassType::Edgeuse,
                        Some(EntityRef::Edgeuse(side_radial)),
                    );

                    let on_face = new_corner_vertex(k, pseudo, corner)?;
                    pseudo
                        .fillet_vertex_mut(on_face)?
                        .set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                    fv_on_common_face = Some(on_face);

                    pseudo
                        .fillet_vertex_mut(on_face)?
                        .set_fillet_vertex_type(FilletVertexType::Mate);
                    // SM_SWAP_PTR(pFV, pFV2)
                    fv2 = fv;
                    fv = on_face;
                    concave_fg = Some(fillet_geom1);
                    convex_fg = Some(fillet_geom2);
                } else {
                    common_face =
                        face_by_two_edgeuses(target, Some(curr_eu), Some(base_eu2), None)?;

                    let on_face = new_corner_vertex(k, pseudo, corner)?;
                    pseudo
                        .fillet_vertex_mut(on_face)?
                        .set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                    fv_on_common_face = Some(on_face);

                    fv2 = on_face;
                    pseudo
                        .fillet_vertex_mut(fv2)?
                        .set_fillet_vertex_type(FilletVertexType::Mate);
                    concave_fg = Some(fillet_geom2);
                    convex_fg = Some(fillet_geom1);
                }
            } else {
                fv2 = fv;
                fv_on_convex_edge = Some(fv);
            }

            let rail_index = fil_solver1.find_index_of_rail_x_side_edgeuse(target, side_eu)?;
            pseudo.make_rail_edgeuse_on(
                fillet_geom1,
                rail_index,
                Some(fv),
                unknown,
                None,
                lookup(corner, self),
                fil_solver1,
                target,
            )?;
            let other_rail_index =
                fil_solver2.find_index_of_rail_x_side_edgeuse(target, radial(target, side_eu)?)?;
            pseudo.make_rail_edgeuse_on(
                fillet_geom2,
                other_rail_index,
                Some(fv2),
                unknown,
                None,
                lookup(corner, self),
                fil_solver2,
                target,
            )?;
        } // end loop on all (both) non-filleted edges.

        let fv_on_common_face =
            fv_on_common_face.ok_or_else(|| SolidError::null_pointer(OP_NX2))?;
        let fv_on_convex_edge =
            fv_on_convex_edge.ok_or_else(|| SolidError::null_pointer(OP_NX2))?;
        pseudo
            .fillet_vertex_mut(fv_on_common_face)?
            .set_mate(0, Some(fv_on_convex_edge));
        pseudo
            .fillet_vertex_mut(fv_on_convex_edge)?
            .set_mate(0, Some(fv_on_common_face));
        self.vertices.push(fv_on_common_face);

        // Will create a 3-sided corner face
        let mut prev_fv = fv_on_common_face; // i.e. m_vVertices[2]
        for ii in 0..3 {
            let curr_fv = *need(self.vertices.get(ii), "4x2 corner vertex index")?;
            let fillet_edge =
                pseudo.make_fillet_edge(Some(prev_fv), Some(curr_fv), Some(corner))?;
            if ii == 1 || curr_fv == fv_on_convex_edge || prev_fv == fv_on_convex_edge {
                pseudo
                    .fillet_edge_mut(fillet_edge)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let prim_eu = need(
                    eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
                    "cross edge without primary edge use",
                )?;
                let geom = if ii == 1 { convex_fg } else { concave_fg };
                pseudo.set_edgeuse_fillet_geom(prim_eu, geom)?;
            } else {
                let fe = pseudo.fillet_edge_mut(fillet_edge)?;
                fe.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
                fe.set_original_face(common_face);
            }
            self.edges.push(fillet_edge);
            prev_fv = curr_fv;
        }

        Ok(())
    }
}

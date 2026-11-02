//! The 3x2 mixed-convexity corner: one convex and one concave filleted edge at a
//! vertex with three edges. The concave fillet's rails stop on the side edge and on the
//! common face; the convex fillet runs on to the extended side edge, and the corner is
//! closed by an on-extended-edge and an on-edge fillet edge through the original
//! vertex, with the concave side face extended to cover it.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (4755–4897).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletCorner::make_3x2_mixed_corner_topology`] | `SmFillet3x2MixedConvexityCorner::MakeCornerTopology` |
//!
//! Deviations: the solvers of the corner's two filleted edge uses are resolved by the
//! caller (`GetFilletSolverOfEdgeuse`, which has no effects) and swapped with them.
//! Edge uses, faces and edges that upstream dereferences are input errors when
//! missing. There is no recursion. With `Kernel::fixes.fillet_3x2_mixed_concave_side_edge`
//! (WR-524, off by default) the roles of the two fillets follow the unfilleted edge's
//! convexity, as source fork@1255993: where that edge is concave, the convex
//! fillet's rail crosses it and the concave one's crosses its continuation.

use std::collections::BTreeMap;

use super::nx1_end::find_surface_extension_dist_at_corner;
use super::{edge_of, find_side_edgeuse, need, new_corner_vertex};
use crate::blend::fillet_corner::p1::face_by_two_edgeuses;
use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
use crate::blend::fillet_executive::session::SolverObject;
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletBrep, FilletCornerId, FilletEdgeType, FilletSolverId, FilletVertexType,
};
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::{p1 as eu1, p2 as eu2};
use crate::topology::{Brep, FaceId};

const OP: &str = "blend::fillet_corner::topology::make_3x2_mixed_corner_topology";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// Actual objects used by `SmFillet3x2MixedConvexityCorner::MakeCornerTopology`.
#[derive(Debug)]
pub struct Mixed3x2TopologyEnv<'a> {
    /// The executive's kernel.
    pub k: &'a Kernel,
    /// The target whose edge uses the corner stores.
    pub target: &'a mut Brep,
    /// The executive's actual pseudo-B-rep.
    pub pseudo: &'a mut FilletBrep,
    /// This corner's identity in the executive's corner store.
    pub corner: FilletCornerId,
    /// The executive's solver store.
    pub solvers: &'a mut [Option<SolverObject>],
    /// The solvers of the corner's first and second filleted edge use
    /// (`GetFilletSolverOfEdgeuse`).
    pub solver_ids: [FilletSolverId; 2],
    /// The executive's `m_vExtendedSurfacesMap`.
    pub extended_surfaces: &'a mut BTreeMap<FaceId, Surface>,
}

fn live(solvers: &[Option<SolverObject>], id: FilletSolverId) -> Result<&FilletSolver> {
    solvers
        .get(id.0 as usize)
        .and_then(Option::as_ref)
        .map(SolverObject::base)
        .ok_or_else(|| invalid("corner solver is not live"))
}

impl FilletCorner {
    /// `SmFillet3x2MixedConvexityCorner::MakeCornerTopology`: four fillet vertices (rail
    /// on the side edge, rail/rail on the common face, rail on the extended side edge,
    /// the original vertex), the rail edge uses of both fillets, two cross sections, the
    /// on-extended-edge and on-edge fillet edges, and the extended concave side surface.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR` unless three edge uses meet at the corner; `SM_ERR_NULL_POINTER` without
    /// a side edge use or a side face surface; [`SolidError::InvalidInput`] for a
    /// different corner kind or topology upstream dereferences; those of the convexity,
    /// solver, surface-extension and pseudo-topology operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_3x2_mixed_corner_topology(&mut self, env: Mixed3x2TopologyEnv<'_>) -> Result<()> {
        fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
            CornerLookup {
                this: Some((Some(corner), this)),
                ..Default::default()
            }
        }
        let Mixed3x2TopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            solver_ids,
            extended_surfaces,
        } = env;
        if self.kind != FilletCornerKind::ThreeByTwoMixed {
            return Err(invalid("requires a 3x2 mixed-convexity corner"));
        }
        let all = self
            .all_eus
            .clone()
            .ok_or_else(|| invalid("corner without outward edge uses"))?;
        if all.len() != 3 {
            return Err(SolidError::sm_err(OP));
        }
        let (mut convex_eu, mut concave_eu) = match self.solver_eus.as_deref() {
            Some([first, second, ..]) => (*first, *second),
            _ => return Err(invalid("corner without two filleted edge uses")),
        };
        let [mut convex_id, mut concave_id] = solver_ids;
        if !eu2::is_convex_radial_sector(k, target, concave_eu, 5, false, 0.0)? {
            std::mem::swap(&mut concave_eu, &mut convex_eu);
            std::mem::swap(&mut concave_id, &mut convex_id);
        }
        let vertex = self
            .vertex
            .ok_or_else(|| invalid("corner without vertex"))?;
        let unknown = OrientType::Unknown;

        // The edge use 'in-between' the two filleted edge uses (pCurrEU).
        let curr_eu_found = all
            .iter()
            .copied()
            .find(|&eu| eu != convex_eu && eu != concave_eu);
        // By default the concave fillet's rail crosses the unfilleted edge (pCrossEU) and
        // the convex one's its continuation (pExtEU), as in an L-shaped box. With
        // `fillet_3x2_mixed_concave_side_edge` (WR-524) the roles follow the unfilleted
        // edge's convexity, as source fork@1255993: that fork resolves the
        // in-between edge use and its convexity before the solvers.
        let (cross_eu, ext_eu, cross_id, ext_id) = if k.fixes.fillet_3x2_mixed_concave_side_edge {
            let curr_eu = curr_eu_found.ok_or_else(|| SolidError::null_pointer(OP))?;
            if eu2::is_convex_radial_sector(k, target, curr_eu, 5, false, 0.0)? {
                (convex_eu, concave_eu, convex_id, concave_id)
            } else {
                (concave_eu, convex_eu, concave_id, convex_id)
            }
        } else {
            (concave_eu, convex_eu, concave_id, convex_id)
        };

        {
            // Get Solvers
            let ext_solver = live(solvers, ext_id)?;
            let cross_solver = live(solvers, cross_id)?;
            let ext_fg = ext_solver.first_fillet_geom()?;
            let cross_fg = cross_solver.first_fillet_geom()?;

            let curr_eu = curr_eu_found.ok_or_else(|| SolidError::null_pointer(OP))?;

            // This edgeuse is 'in-between' two filletted edgeuses
            let fv0 = new_corner_vertex(k, pseudo, corner)?;
            let side_eu = find_side_edgeuse(target, cross_eu, curr_eu)?;
            let side = need(side_eu, "no side edge use intersects rail")?;
            let rail_index = cross_solver.find_index_of_rail_x_side_edgeuse(target, side)?;
            pseudo.make_rail_edgeuse_on(
                cross_fg,
                rail_index,
                Some(fv0),
                unknown,
                None,
                lookup(corner, self),
                cross_solver,
                target,
            )?;
            {
                let v = pseudo.fillet_vertex_mut(fv0)?;
                v.set_point_class(PointClassType::Edgeuse, side_eu.map(EntityRef::Edgeuse));
                v.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }
            self.vertices.push(fv0);

            let common_face =
                face_by_two_edgeuses(target, Some(ext_eu), Some(cross_eu), Some(curr_eu))?;
            let fv1 = new_corner_vertex(k, pseudo, corner)?;
            {
                let v = pseudo.fillet_vertex_mut(fv1)?;
                v.set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                v.set_fillet_vertex_type(FilletVertexType::RailXRail);
            }
            self.vertices.push(fv1);

            // Setup rails of fillet
            let rail_index = cross_solver.find_index_of_rail_on_face(target, common_face)?;
            pseudo.make_rail_edgeuse_on(
                cross_fg,
                rail_index,
                Some(fv1),
                unknown,
                None,
                lookup(corner, self),
                cross_solver,
                target,
            )?;
            let other_rail_index = ext_solver.find_index_of_rail_on_face(target, common_face)?;
            pseudo.make_rail_edgeuse_on(
                ext_fg,
                other_rail_index,
                Some(fv1),
                unknown,
                None,
                lookup(corner, self),
                ext_solver,
                target,
            )?;

            // Make a cross section on the crossing (by default the concave) fillet
            let fe0 = pseudo.make_fillet_edge(Some(fv0), Some(fv1), Some(corner))?;
            pseudo
                .fillet_edge_mut(fe0)?
                .set_fillet_edge_type(FilletEdgeType::CrossSection);
            let prim_eu = need(
                eu1::primary_edgeuse(pseudo.brep(), fe0)?,
                "cross edge without primary edge use",
            )?;
            pseudo.set_edgeuse_fillet_geom(prim_eu, Some(cross_fg))?;
            self.edges.push(fe0);

            let fv2 = new_corner_vertex(k, pseudo, corner)?;
            pseudo
                .fillet_vertex_mut(fv2)?
                .set_fillet_vertex_type(FilletVertexType::RailXExtendedEdgeuse);
            let side_eu = find_side_edgeuse(target, ext_eu, curr_eu)?;
            let side = need(side_eu, "no side edge use intersects rail")?;
            let rail_index = ext_solver.find_index_of_rail_x_side_edgeuse(target, side)?;
            pseudo.make_rail_edgeuse_on(
                ext_fg,
                rail_index,
                Some(fv2),
                unknown,
                None,
                lookup(corner, self),
                ext_solver,
                target,
            )?;
            pseudo
                .fillet_vertex_mut(fv2)?
                .set_point_class(PointClassType::Edgeuse, side_eu.map(EntityRef::Edgeuse));
            self.vertices.push(fv2);

            // Make a cross section on the extended (by default the convex) fillet
            let fe1 = pseudo.make_fillet_edge(Some(fv1), Some(fv2), Some(corner))?;
            pseudo
                .fillet_edge_mut(fe1)?
                .set_fillet_edge_type(FilletEdgeType::CrossSection);
            let prim_eu1 = need(
                eu1::primary_edgeuse(pseudo.brep(), fe1)?,
                "cross edge without primary edge use",
            )?;
            pseudo.set_edgeuse_fillet_geom(prim_eu1, Some(ext_fg))?;
            self.edges.push(fe1);

            let fv3 = new_corner_vertex(k, pseudo, corner)?;
            {
                let v = pseudo.fillet_vertex_mut(fv3)?;
                v.set_point_class(PointClassType::Vertex, Some(EntityRef::Vertex(vertex)));
                v.set_fillet_vertex_type(FilletVertexType::OnVertex);
            }
            self.vertices.push(fv3);

            // Find the original face
            let orig_face = face_by_two_edgeuses(target, Some(curr_eu), Some(ext_eu), None)?;
            let curr_edge = edge_of(target, curr_eu)?;
            // Make an extended edge
            let fe2 = pseudo.make_fillet_edge(Some(fv2), Some(fv3), Some(corner))?;
            {
                let fe = pseudo.fillet_edge_mut(fe2)?;
                fe.set_fillet_edge_type(FilletEdgeType::OnExtendEdge);
                fe.set_original_edge(Some(curr_edge));
                fe.set_original_face(orig_face);
            }
            self.edges.push(fe2);

            // Add the second edge
            let fe3 = pseudo.make_fillet_edge(Some(fv3), Some(fv0), Some(corner))?;
            {
                let fe = pseudo.fillet_edge_mut(fe3)?;
                fe.set_fillet_edge_type(FilletEdgeType::OnEdge);
                fe.set_original_edge(Some(curr_edge));
                fe.set_original_face(orig_face);
            }
            self.edges.push(fe3);
        }

        // Extend the Side Surface if necessary
        let ext_solver = solvers
            .get_mut(ext_id.0 as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("corner solver is not live"))?;
        let ext_dist = find_surface_extension_dist_at_corner(target, ext_eu, ext_solver, vertex)?;
        let curr_eu = all
            .iter()
            .copied()
            .find(|&eu| eu != convex_eu && eu != concave_eu)
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let orig_face1 = need(
            face_by_two_edgeuses(target, Some(curr_eu), Some(cross_eu), None)?,
            "no concave side face",
        )?;
        let surface = target
            .face(orig_face1)?
            .surface
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let new_surface = surface.create_extended_surface(k, ext_dist, ContinuityType::G1)?;

        // Copy pNewSurface, when possible as an analytic surface
        let new_analytic_surface = new_surface.copy_and_add_analytics(k)?;
        Self::add_extended_original_surface_with_map(
            extended_surfaces,
            orig_face1,
            new_analytic_surface,
        );
        Ok(())
    }
}

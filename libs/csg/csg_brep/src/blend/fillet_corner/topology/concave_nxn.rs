//! Topology of a concave `NxN` corner (all edges filleted, two of them concave).
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (5160–5329).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletCorner::make_concave_nxn_corner_topology`] | `SmFilletConcaveNxNCorner::MakeCornerTopology` |
//!
//! Deviations: the caller lends the actual target, pseudo-B-rep and the solver of
//! each outward edge use, resolved before any allocation (upstream looks each one up
//! inside its loop, after allocating the corner vertices; the lookups have no effects).
//! A setback corner is delegated by the caller to
//! [`FilletCorner::make_convex_nxn_corner_topology`], as upstream calls the base
//! class. Missing topology that native dereferences is an input error. There is no
//! recursion.

use super::{need, new_corner_vertex, NxNTopologyEnv};
use crate::blend::fillet_corner::p1::face_by_two_edgeuses;
use crate::blend::fillet_corner::p2::find_concave_edgeuses;
use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletCornerId, FilletEdgeType, FilletVertexType,
};
use crate::error::{EntityRef, Result, SolidError};
use crate::intersect::point_class::PointClassType;
use crate::topology::edgeuse::p1 as eu1;

const OP: &str = "blend::fillet_corner::topology::make_concave_nxn_corner_topology";

impl FilletCorner {
    /// `SmFilletConcaveNxNCorner::MakeCornerTopology` for a corner without setback:
    /// one fillet vertex per face between consecutive filleted edge uses (a rail/rail
    /// intersection, the rail edge uses of both fillets on that face and a cross
    /// section of the previous fillet), except on the face between the two concave
    /// edge uses, which gets two mate vertices joined by a rail/rail interpolation
    /// edge. The corner is blending.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails. Vertex geometry is computed later.
    ///
    /// # Errors
    /// `SM_ERR` unless exactly two concave edge uses are found;
    /// [`SolidError::NullPointer`] when two consecutive edge uses share no face;
    /// [`SolidError::InvalidInput`] for a different corner kind, a setback corner (the
    /// caller delegates it), a solver list that does not match the edge uses, or
    /// missing topology upstream dereferences; those of the convexity, solver and
    /// pseudo-topology operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_concave_nxn_corner_topology(&mut self, env: NxNTopologyEnv<'_>) -> Result<()> {
        fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
            CornerLookup {
                this: Some((Some(corner), this)),
                ..Default::default()
            }
        }
        let NxNTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
        } = env;
        let invalid = |reason: &'static str| SolidError::invalid_input(OP, reason);
        if self.kind != FilletCornerKind::ConcaveNByN {
            return Err(invalid("requires a ConcaveNByN corner"));
        }
        // Determine if this is a setback corner
        if self.set_back_dist > self.approx_tol_3d * 100.0 {
            // Let SmFilletConvexNxNCorner handle setbacks (the caller dispatches it)
            return Err(invalid(
                "setback corner is built by the convex NxN topology",
            ));
        }

        // All the edges of corner are filletted
        let all = self
            .all_eus
            .clone()
            .ok_or_else(|| invalid("corner without outward edge uses"))?;
        let vertex = need(self.vertex, "corner without vertex")?;
        let concave = find_concave_edgeuses(k, target, vertex, &all)?;
        let [concave_eu1, concave_eu2] = concave[..] else {
            return Err(SolidError::sm_err(OP));
        };
        let total_eus = all.len();
        if solvers.len() != total_eus {
            return Err(invalid("one solver per edge use required"));
        }
        let mut total = total_eus;
        // Allocate 'lTotalEUs' Fillet Vertices
        for _ in 0..total {
            let fv = new_corner_vertex(k, pseudo, corner)?;
            self.vertices.push(fv);
        }
        let vertex_at = |this: &Self, index: usize| {
            this.vertices
                .get(index)
                .copied()
                .ok_or_else(|| invalid("corner vertex index out of range"))
        };
        let (Some(&last_eu), Some(&last_solver)) = (all.last(), solvers.last()) else {
            return Err(invalid("corner without edge uses"));
        };
        let unknown = crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Unknown;

        let mut prev_eu = last_eu;
        let mut prev_solver = last_solver;
        let mut prev_fv = vertex_at(self, total - 1)?;
        let mut jj = 0;
        for (&curr_eu, &curr_solver) in all.iter().zip(&solvers) {
            let curr_fv = vertex_at(self, jj)?;

            let common_face = face_by_two_edgeuses(target, Some(prev_eu), Some(curr_eu), None)?;
            let Some(common_face) = common_face else {
                //Unrecoverable error
                return Err(SolidError::null_pointer(OP));
            };
            pseudo
                .fillet_vertex_mut(curr_fv)?
                .set_point_class(PointClassType::Face, Some(EntityRef::Face(common_face)));

            let rail_index = prev_solver.find_index_of_rail_on_face(target, Some(common_face))?;
            let other_rail_index =
                curr_solver.find_index_of_rail_on_face(target, Some(common_face))?;
            let prev_geom = prev_solver.first_fillet_geom()?;
            let curr_geom = curr_solver.first_fillet_geom()?;
            if (prev_eu == concave_eu1 && curr_eu == concave_eu2)
                || (prev_eu == concave_eu2 && curr_eu == concave_eu1)
            {
                // Process concave corner of pCommonFace
                // Add one more Fillet Point
                let extra = new_corner_vertex(k, pseudo, corner)?;
                if jj > self.vertices.len() {
                    return Err(invalid("corner vertex index out of range"));
                }
                self.vertices.insert(jj, extra);
                jj += 1;
                total += 1;
                {
                    let fv = pseudo.fillet_vertex_mut(extra)?;
                    fv.set_point_class(PointClassType::Face, Some(EntityRef::Face(common_face)));
                    fv.set_fillet_vertex_type(FilletVertexType::Mate);
                }
                pseudo.make_rail_edgeuse_on(
                    prev_geom,
                    rail_index,
                    Some(extra),
                    unknown,
                    None,
                    lookup(corner, self),
                    prev_solver,
                    target,
                )?;
                pseudo.fillet_vertex_mut(extra)?.set_mate(0, Some(prev_fv));
                pseudo.fillet_vertex_mut(prev_fv)?.set_mate(0, Some(extra));
                // Make fillet edge
                let edge = pseudo.make_fillet_edge(Some(prev_fv), Some(extra), Some(corner))?;
                pseudo
                    .fillet_edge_mut(edge)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), edge)?,
                    "cross edge without primary edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(prev_geom))?;
                self.edges.push(edge);

                // Setup curr vert
                pseudo
                    .fillet_vertex_mut(curr_fv)?
                    .set_fillet_vertex_type(FilletVertexType::Mate);
                pseudo.make_rail_edgeuse_on(
                    curr_geom,
                    other_rail_index,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    curr_solver,
                    target,
                )?;
                let next_fv = vertex_at(self, (jj + 1) % total)?;
                pseudo
                    .fillet_vertex_mut(curr_fv)?
                    .set_mate(0, Some(next_fv));
                pseudo
                    .fillet_vertex_mut(next_fv)?
                    .set_mate(0, Some(curr_fv));
                // Make fillet edge between extra vert and current vert
                let edge = pseudo.make_fillet_edge(Some(extra), Some(curr_fv), Some(corner))?;
                {
                    let edge = pseudo.fillet_edge_mut(edge)?;
                    edge.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
                    edge.set_original_face(Some(common_face));
                }
                self.edges.push(edge);
            } else {
                pseudo
                    .fillet_vertex_mut(curr_fv)?
                    .set_fillet_vertex_type(FilletVertexType::RailXRail);
                // Setup rails of fillet
                pseudo.make_rail_edgeuse_on(
                    prev_geom,
                    rail_index,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    prev_solver,
                    target,
                )?;
                pseudo.make_rail_edgeuse_on(
                    curr_geom,
                    other_rail_index,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    curr_solver,
                    target,
                )?;

                // Make fillet edge
                let edge = pseudo.make_fillet_edge(Some(prev_fv), Some(curr_fv), Some(corner))?;
                pseudo
                    .fillet_edge_mut(edge)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), edge)?,
                    "cross edge without primary edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(prev_geom))?;
                self.edges.push(edge);
            }
            prev_solver = curr_solver;
            prev_fv = curr_fv;
            prev_eu = curr_eu;
            jj += 1;
        }
        self.blending = true;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_geom::p1::FilletBrep;
    use crate::blend::fillet_solver::p1::FilletSolver;
    use crate::kernel::Kernel;
    use crate::topology::Brep;

    fn run(kind: FilletCornerKind, set_back: f64) -> Result<()> {
        let k = Kernel::default();
        let mut target = Brep::new();
        let mut pseudo = FilletBrep::new();
        let solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        let mut corner = FilletCorner::new(kind, None, None, None, 1e-3, 0.1);
        corner.set_back_dist = set_back;
        corner.make_concave_nxn_corner_topology(NxNTopologyEnv {
            k: &k,
            target: &mut target,
            pseudo: &mut pseudo,
            corner: FilletCornerId(0),
            solvers: vec![&solver; 3],
        })
    }

    #[test]
    fn concave_nxn_topology_rejects_other_kinds_setbacks_and_missing_input() {
        for (kind, set_back) in [
            (FilletCornerKind::ConvexNByN, 0.0),
            (FilletCornerKind::ConcaveNByN, 1.0),
            (FilletCornerKind::ConcaveNByN, 0.0),
        ] {
            assert!(matches!(
                run(kind, set_back),
                Err(SolidError::InvalidInput { .. })
            ));
        }
    }
}

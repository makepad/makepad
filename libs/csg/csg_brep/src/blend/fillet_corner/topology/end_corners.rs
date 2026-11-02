//! Topology of the tangent (1x1, 2x2), 4x3, open-end and degenerate corners.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (4372–4436, 4494–4561, 8570–8669,
//! 8699–8748, 8779–8861).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletCorner::make_1x1_corner_topology`] | `SmFillet1x1Corner::MakeCornerTopology` |
//! | [`FilletCorner::make_2x2_corner_topology`] | `SmFillet2x2Corner::MakeCornerTopology` |
//! | [`FilletCorner::make_4x3_corner_topology`] | `SmFillet4x3Corner::MakeCornerTopology` |
//! | [`FilletCorner::make_open_corner_topology`] | `SmFilletOpenCorner::MakeCornerTopology` |
//! | [`FilletCorner::make_degenerate_corner_topology`] | `SmFilletDegenerateCorner::MakeCornerTopology` |
//!
//! Deviations: the caller resolves `GetFilletSolverOfEdgeuse` of the corner's edge uses
//! before the call ([`EndCornerTopologyEnv::solver_eu_solvers`],
//! [`EndCornerTopologyEnv::all_eu_solvers`]; the lookup has no effects). A missing
//! solver or surface upstream dereferences (`NULL`) is `SM_ERR_NULL_POINTER` at the
//! dereference; other missing topology upstream dereferences is an input error. The
//! unused `GetEdges` reads of the open and degenerate corners are omitted. The
//! degenerate corner's `GlobalPointSolve` runs on the solver's base surface, which has
//! no trim test, as upstream with `m_bPointTestEnabled` turned off. There is no
//! recursion.

use super::{find_side_edgeuse, need, new_corner_vertex, radial};
use crate::blend::fillet_corner::p1::face_by_two_edgeuses;
use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
use crate::blend::fillet_executive::session::SolverObject;
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilStatus, FilletBrep, FilletCornerId, FilletEdgeType, FilletSolverId,
    FilletVertexType,
};
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::intersect::point_class::PointClassType;
use crate::intersect::surface_intersector::p1::{IntersectionPointType, SrfIndex};
use crate::kernel::Kernel;
use crate::math::Point2;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{vertex, Brep, EdgeuseId, ObjectHandle};

const OP: &str = "blend::fillet_corner::topology::end_corners";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// Actual objects used by the 1x1, 2x2, 4x3, open and degenerate corners'
/// `MakeCornerTopology`.
#[derive(Debug)]
pub struct EndCornerTopologyEnv<'a> {
    /// The executive's kernel.
    pub k: &'a Kernel,
    /// The target whose edge uses the corner stores.
    pub target: &'a mut Brep,
    /// The executive's actual pseudo-B-rep.
    pub pseudo: &'a mut FilletBrep,
    /// This corner's identity in the executive's corner store.
    pub corner: FilletCornerId,
    /// The executive's solver store.
    pub solvers: &'a [Option<SolverObject>],
    /// `GetFilletSolverOfEdgeuse` of each `m_pSolverEUs` entry, in order (`None`:
    /// `NULL`).
    pub solver_eu_solvers: Vec<Option<FilletSolverId>>,
    /// `GetFilletSolverOfEdgeuse` of each `m_pAllEUs` entry, in order (`None`: `NULL`).
    pub all_eu_solvers: Vec<Option<FilletSolverId>>,
}

/// The live solver `id` (`NULL`: `SM_ERR_NULL_POINTER`).
fn live(solvers: &[Option<SolverObject>], id: Option<FilletSolverId>) -> Result<&FilletSolver> {
    let id = id.ok_or_else(|| SolidError::null_pointer(OP))?;
    solvers
        .get(id.0 as usize)
        .and_then(Option::as_ref)
        .map(SolverObject::base)
        .ok_or_else(|| invalid("corner solver is not live"))
}

fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
    CornerLookup {
        this: Some((Some(corner), this)),
        ..Default::default()
    }
}

/// `SrfIndex` of a rail index (`0` or `1`).
fn srf(i: usize) -> Result<SrfIndex> {
    match i {
        0 => Ok(SrfIndex::First),
        1 => Ok(SrfIndex::Second),
        _ => Err(invalid("rail index past 1")),
    }
}

impl FilletCorner {
    fn filleted_eu(&self, index: usize) -> Result<EdgeuseId> {
        need(
            self.solver_eus
                .as_deref()
                .and_then(|eus| eus.get(index))
                .copied(),
            "corner without filleted edge use",
        )
    }

    /// The cross-section vertex `fv0` on the face of `base_eu` and its mate `fv1` on the
    /// radial face (the shared start of the 1x1 and 2x2 corners).
    fn make_cross_section_vertices(
        &mut self,
        k: &Kernel,
        target: &Brep,
        pseudo: &mut FilletBrep,
        corner: FilletCornerId,
        base_eu: EdgeuseId,
    ) -> Result<(
        crate::topology::VertexId,
        crate::topology::VertexId,
        Option<crate::topology::FaceId>,
    )> {
        // Create a fillet vertex on one side of the edge
        let fv0 = new_corner_vertex(k, pseudo, corner)?;
        let face0 = eu1::face(target, base_eu)?;
        {
            let v = pseudo.fillet_vertex_mut(fv0)?;
            v.set_point_class(PointClassType::Face, face0.map(EntityRef::Face));
            v.set_fillet_vertex_type(FilletVertexType::OnCrossSection);
        }
        self.vertices.push(fv0);

        // Create another fillet vertex on the other side of the edge
        let fv1 = new_corner_vertex(k, pseudo, corner)?;
        let face1 = eu1::face(target, radial(target, base_eu)?)?;
        {
            let v = pseudo.fillet_vertex_mut(fv1)?;
            v.set_point_class(PointClassType::Face, face1.map(EntityRef::Face));
            v.set_fillet_vertex_type(FilletVertexType::Mate);
            v.set_mate(0, Some(fv0));
        }
        pseudo.fillet_vertex_mut(fv0)?.set_mate(0, Some(fv1));
        self.vertices.push(fv1);
        Ok((fv0, fv1, face0))
    }

    /// `SmFillet1x1Corner::MakeCornerTopology`: the cross-section vertex on the
    /// filleted edge use's face, its mate on the radial face, the cross-section fillet
    /// edge between them and two rail edge uses (same and opposite) on each closed rail.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without the solver; `SM_ERR` unless the new fillet edge has
    /// two edge uses; invalid topology or corner kind; those of the solver rail lookups
    /// and pseudo-topology operations.
    pub fn make_1x1_corner_topology(&mut self, env: EndCornerTopologyEnv<'_>) -> Result<()> {
        if self.kind != FilletCornerKind::OneByOne {
            return Err(invalid("requires a 1x1 corner"));
        }
        let EndCornerTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            solver_eu_solvers,
            ..
        } = env;
        let fil_edgeuse = self.filleted_eu(0)?;
        let solver = live(solvers, solver_eu_solvers.first().copied().flatten())?;

        let (fv0, fv1, face0) =
            self.make_cross_section_vertices(k, target, pseudo, corner, fil_edgeuse)?;

        // Corner consists of one fillet edge only (joining fv0 & fv1)
        let fillet_edge = pseudo.make_fillet_edge(Some(fv0), Some(fv1), Some(corner))?;
        pseudo
            .fillet_edge_mut(fillet_edge)?
            .set_fillet_edge_type(FilletEdgeType::CrossSection);

        if crate::topology::edge::edgeuses(pseudo.brep(), fillet_edge, None, None)?.len() != 2 {
            return Err(SolidError::sm_err(OP));
        }

        let prim = need(
            eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
            "fillet edge without primary edge use",
        )?;
        let fillet_geom = solver.first_fillet_geom()?;
        let mate = need(
            eu1::mate(pseudo.brep(), prim)?,
            "fillet edge use without mate",
        )?;
        pseudo.set_edgeuse_fillet_geom(prim, Some(fillet_geom))?;
        pseudo.set_edgeuse_fillet_geom(mate, Some(fillet_geom))?;
        self.edges.push(fillet_edge);

        // Setup rails of fillet
        let rail = solver.find_index_of_rail_on_face(target, face0)?;
        let other = 1usize
            .checked_sub(rail)
            .ok_or_else(|| invalid("rail index past 1"))?;

        // Need two edgeuses for each (closed-)rail
        for (rail, fv) in [(rail, fv0), (other, fv1)] {
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

        self.blending = true;
        Ok(())
    }

    /// `SmFillet2x2Corner::MakeCornerTopology`: as the 1x1 corner, but the cross-section
    /// edge's two edge uses take the two G1-continuous fillets' geometries and each
    /// fillet gets one rail edge use at each of the two vertices.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without either solver; invalid topology or corner kind;
    /// those of the solver rail lookups and pseudo-topology operations.
    pub fn make_2x2_corner_topology(&mut self, env: EndCornerTopologyEnv<'_>) -> Result<()> {
        if self.kind != FilletCornerKind::TwoByTwo {
            return Err(invalid("requires a 2x2 corner"));
        }
        let EndCornerTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            solver_eu_solvers,
            ..
        } = env;
        let base_edgeuse1 = self.filleted_eu(0)?;
        self.filleted_eu(1)?;
        let solver1 = live(solvers, solver_eu_solvers.first().copied().flatten())?;
        let solver2 = live(solvers, solver_eu_solvers.get(1).copied().flatten())?;

        let (fv0, fv1, face0) =
            self.make_cross_section_vertices(k, target, pseudo, corner, base_edgeuse1)?;

        // Corner consists of one fillet edge only (joining fv0 & fv1)
        let fillet_edge = pseudo.make_fillet_edge(Some(fv0), Some(fv1), Some(corner))?;
        pseudo
            .fillet_edge_mut(fillet_edge)?
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        let prim = need(
            eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
            "fillet edge without primary edge use",
        )?;
        let fillet_geom1 = solver1.first_fillet_geom()?;
        pseudo.set_edgeuse_fillet_geom(prim, Some(fillet_geom1))?;
        let mate = need(
            eu1::mate(pseudo.brep(), prim)?,
            "fillet edge use without mate",
        )?;
        let fillet_geom2 = solver2.first_fillet_geom()?;
        pseudo.set_edgeuse_fillet_geom(mate, Some(fillet_geom2))?;
        self.edges.push(fillet_edge);

        // Setup rails of fillet
        let unknown = OrientType::Unknown;
        for (solver, geom) in [(solver1, fillet_geom1), (solver2, fillet_geom2)] {
            let rail = solver.find_index_of_rail_on_face(target, face0)?;
            let other = 1usize
                .checked_sub(rail)
                .ok_or_else(|| invalid("rail index past 1"))?;
            for (rail, fv) in [(rail, fv0), (other, fv1)] {
                pseudo.make_rail_edgeuse_on(
                    geom,
                    rail,
                    Some(fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    solver,
                    target,
                )?;
            }
        }

        self.blending = true;
        Ok(())
    }

    /// `SmFillet4x3Corner::MakeCornerTopology`: one fillet vertex per filleted edge use
    /// (rail/side-edge where the unfilleted edge use lies between two fillets, else
    /// rail/rail on the common face), the rail edge uses of both neighbouring fillets at
    /// each, and a cross-section fillet edge between consecutive vertices.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without a neighbouring solver or common face; invalid
    /// topology or corner kind; those of the solver rail lookups and pseudo-topology
    /// operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_4x3_corner_topology(&mut self, env: EndCornerTopologyEnv<'_>) -> Result<()> {
        if self.kind != FilletCornerKind::FourByThree {
            return Err(invalid("requires a 4x3 corner"));
        }
        let EndCornerTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            all_eu_solvers,
            ..
        } = env;
        let all = need(self.all_eus.clone(), "corner without outward edge uses")?;
        let total_eus = all.len();
        let total_solver_eus = need(
            self.solver_eus.as_deref().map(<[EdgeuseId]>::len),
            "corner without filleted edge uses",
        )?;
        if total_eus == 0 || total_solver_eus == 0 || all_eu_solvers.len() != total_eus {
            return Err(invalid("4x3 corner without matching edge uses"));
        }
        let solver_of = |i: usize| all_eu_solvers.get(i % total_eus).copied().flatten();
        let eu_at = |i: usize| -> Result<EdgeuseId> {
            need(
                all.get(i % total_eus).copied(),
                "corner edge use out of range",
            )
        };

        // Allocate three Fillet Vertices
        for _ in 0..total_solver_eus {
            let fv = new_corner_vertex(k, pseudo, corner)?;
            self.vertices.push(fv);
        }

        let mut curr_eu = eu_at(0)?;
        let mut curr_solver = solver_of(0);
        let mut curr_index = 0usize;
        let mut curr_fv = need(self.vertices.first().copied(), "corner without vertices")?;
        let unknown = OrientType::Unknown;
        for ii in 0..total_eus {
            let next_eu = eu_at(ii + 1)?;
            let Some(next_id) = solver_of(ii + 1) else {
                curr_solver = None;
                curr_eu = next_eu;
                continue;
            };
            let next_solver = live(solvers, Some(next_id))?;
            let next_fillet_geom = next_solver.first_fillet_geom()?;

            if let Some(curr_id) = curr_solver {
                let common_face = face_by_two_edgeuses(target, Some(curr_eu), Some(next_eu), None)?;
                if common_face.is_none() {
                    return Err(SolidError::null_pointer(OP)); // Unrecoverable error
                }
                {
                    let v = pseudo.fillet_vertex_mut(curr_fv)?;
                    v.set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                    v.set_fillet_vertex_type(FilletVertexType::RailXRail);
                }

                // Create rail edgeuses
                let curr = live(solvers, Some(curr_id))?;
                let curr_fillet_geom = curr.first_fillet_geom()?;
                let rail = curr.find_index_of_rail_on_face(target, common_face)?;
                let other_rail = next_solver.find_index_of_rail_on_face(target, common_face)?;
                pseudo.make_rail_edgeuse_on(
                    curr_fillet_geom,
                    rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    curr,
                    target,
                )?;
                pseudo.make_rail_edgeuse_on(
                    next_fillet_geom,
                    other_rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    next_solver,
                    target,
                )?;
            } else {
                let prev_eu = eu_at(ii + total_eus - 1)?;
                let prev_solver = live(solvers, solver_of(ii + total_eus - 1))?;
                let prev_fillet_geom = prev_solver.first_fillet_geom()?;
                let side_eu = find_side_edgeuse(target, prev_eu, curr_eu)?;
                {
                    let v = pseudo.fillet_vertex_mut(curr_fv)?;
                    v.set_point_class(PointClassType::Edgeuse, side_eu.map(EntityRef::Edgeuse));
                    v.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
                }

                // Create rail edgeuses
                let side = need(side_eu, "no side edge use intersects rail")?;
                let rail = prev_solver.find_index_of_rail_x_side_edgeuse(target, side)?;
                let other_rail =
                    next_solver.find_index_of_rail_x_side_edgeuse(target, radial(target, side)?)?;
                pseudo.make_rail_edgeuse_on(
                    prev_fillet_geom,
                    rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    prev_solver,
                    target,
                )?;
                pseudo.make_rail_edgeuse_on(
                    next_fillet_geom,
                    other_rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    next_solver,
                    target,
                )?;
            }

            // Make fillet edge
            curr_index += 1;
            let next_fv = need(
                self.vertices.get(curr_index % total_solver_eus).copied(),
                "corner vertex out of range",
            )?;
            let fillet_edge =
                pseudo.make_fillet_edge(Some(curr_fv), Some(next_fv), Some(corner))?;
            pseudo
                .fillet_edge_mut(fillet_edge)?
                .set_fillet_edge_type(FilletEdgeType::CrossSection);
            let prim = need(
                eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
                "fillet edge without primary edge use",
            )?;
            pseudo.set_edgeuse_fillet_geom(prim, Some(next_fillet_geom))?;
            self.edges.push(fillet_edge);

            curr_solver = Some(next_id);
            curr_eu = next_eu;
            curr_fv = next_fv;
        }
        Ok(())
    }

    /// `SmFilletOpenCorner::MakeCornerTopology`: a rail/side-edge vertex on each edge
    /// use next to the filleted edge use (its CW neighbour and its radial's CCW
    /// neighbour), a rail edge use at each and the fillet-end edge between them.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without the solver; invalid topology or corner kind; those
    /// of the solver rail lookups and pseudo-topology operations.
    pub fn make_open_corner_topology(&mut self, env: EndCornerTopologyEnv<'_>) -> Result<()> {
        if self.kind != FilletCornerKind::Open {
            return Err(invalid("requires an open corner"));
        }
        let EndCornerTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            solver_eu_solvers,
            ..
        } = env;
        let fil_edgeuse = self.filleted_eu(0)?;
        // Adjacent edgeuses of filleted edgeuse
        let eus = [
            eu1::cw_edgeuse(target, fil_edgeuse)?,
            eu1::ccw_edgeuse(target, radial(target, fil_edgeuse)?)?,
        ];
        let solver = live(solvers, solver_eu_solvers.first().copied().flatten())?;
        let fillet_geom = solver.first_fillet_geom()?;
        for eu in eus {
            let fv = new_corner_vertex(k, pseudo, corner)?;
            {
                let v = pseudo.fillet_vertex_mut(fv)?;
                v.set_point_class(PointClassType::Edgeuse, eu.map(EntityRef::Edgeuse));
                v.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }
            self.vertices.push(fv);
            let eu = need(eu, "filleted edge use without neighbour")?;
            let rail = solver.find_index_of_rail_x_side_edgeuse(target, eu)?;
            // Create rail-edgeuse using this vertex
            pseudo.make_rail_edgeuse_on(
                fillet_geom,
                rail,
                Some(fv),
                OrientType::Unknown,
                None,
                lookup(corner, self),
                solver,
                target,
            )?;
        }
        let fv0 = self.vertices.first().copied();
        let fv1 = self.vertices.get(1).copied();
        let fillet_edge = pseudo.make_fillet_edge(fv0, fv1, Some(corner))?;
        pseudo
            .fillet_edge_mut(fillet_edge)?
            .set_fillet_edge_type(FilletEdgeType::FilletEnd);
        let prim = need(
            eu1::primary_edgeuse(pseudo.brep(), fillet_edge)?,
            "fillet edge without primary edge use",
        )?;
        pseudo.set_edgeuse_fillet_geom(prim, Some(fillet_geom))?;
        self.edges.push(fillet_edge);
        Ok(())
    }

    /// `SmFilletDegenerateCorner::MakeCornerTopology`: one processed fillet vertex at
    /// the corner vertex, the vertex dropped onto the solver's two base surfaces, and a
    /// rail edge use of each rail at it whose intersection point holds the two UV points
    /// as a tangent point.
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without the solver or a solver surface; `SM_ERR` unless each
    /// drop has exactly one solution; invalid topology or corner kind; those of the
    /// drops, the solver rail lookups and pseudo-topology operations.
    pub fn make_degenerate_corner_topology(&mut self, env: EndCornerTopologyEnv<'_>) -> Result<()> {
        if self.kind != FilletCornerKind::Degenerate {
            return Err(invalid("requires a degenerate corner"));
        }
        let EndCornerTopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers,
            solver_eu_solvers,
            ..
        } = env;
        let fil_edgeuse = self.filleted_eu(0)?;
        // Adjacent edgeuses of filleted edgeuse
        let eus = [
            eu1::cw_edgeuse(target, fil_edgeuse)?,
            eu1::ccw_edgeuse(target, radial(target, fil_edgeuse)?)?,
        ];
        let solver_id = solver_eu_solvers.first().copied().flatten();
        let cp_vertex = need(self.vertex, "corner without vertex")?;
        let fv = new_corner_vertex(k, pseudo, corner)?;
        let corner_pnt = target.vertex(cp_vertex)?.point;
        {
            let v = pseudo.fillet_vertex_mut(fv)?;
            v.set_point_class(PointClassType::Vertex, Some(EntityRef::Vertex(cp_vertex)));
            v.set_fillet_vertex_type(FilletVertexType::OnVertex);
        }
        pseudo.set_vertex_point(fv, corner_pnt)?;
        pseudo
            .fillet_vertex_mut(fv)?
            .set_status(FilStatus::Processed);
        self.vertices.push(fv);

        // Drop sCornerPnt onto 2 surfaces
        let solver = live(solvers, solver_id)?;
        let tol = vertex::tolerance(target, cp_vertex)? * 10.0;
        let mut uv = [Point2::ZERO; 2];
        for (ii, slot) in uv.iter_mut().enumerate() {
            let base = solver
                .surface(ii)?
                .ok_or_else(|| SolidError::null_pointer(OP))?
                .surface
                .base_surface();
            // Point testing off: GlobalPointSolve keeps every point solution.
            let solutions = base.global_point_solve_point_test_off::<ObjectHandle>(
                k,
                None,
                &base.natural_uv_domain_with_nurb(k)?,
                SolverOperation::Intersect,
                corner_pnt,
                tol,
                None,
                SolutionRequested::Single,
            )?;
            let [solution] = solutions.as_slice() else {
                return Err(SolidError::sm_err(OP));
            };
            *slot = Point2::new(solution.start.param(0)?, solution.start.param(1)?);
        }

        let fillet_geom = solver.first_fillet_geom()?;
        for eu in eus {
            let eu = need(eu, "filleted edge use without neighbour")?;
            let rail = solver.find_index_of_rail_x_side_edgeuse(target, eu)?;
            let other = 1usize
                .checked_sub(rail)
                .ok_or_else(|| invalid("rail index past 1"))?;
            // Create rail-edgeuse using this vertex
            let new_edgeuse = pseudo.make_rail_edgeuse_on(
                fillet_geom,
                rail,
                Some(fv),
                OrientType::Unknown,
                None,
                lookup(corner, self),
                solver,
                target,
            )?;
            let vu = need(
                eu1::vertexuse(pseudo.brep(), new_edgeuse)?,
                "rail edge use without vertex use",
            )?;
            let mut tsect = pseudo.fillet_vertexuse(vu)?.tsect_pnt().clone();
            for i in [rail, other] {
                *tsect.uv_pos_mut(srf(i)?) =
                    *uv.get(i).ok_or_else(|| invalid("rail index past 1"))?;
            }
            tsect.point_type = IntersectionPointType::TangentPoint;
            pseudo.fillet_vertexuse_mut(vu)?.set_tsect_pnt(&tsect);
        }
        Ok(())
    }
}

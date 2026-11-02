//! Topology at the end of one filleted edge.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (44–119, 443–506, 5642–5815,
//! 5990–6443, 6487–6859).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletCorner::make_nx1_corner_topology`] | `SmFilletNx1Corner::MakeCornerTopology` |
//! | [`FilletCorner::make_nx2_corner_topology`] | `SmFilletNx2Corner::MakeCornerTopology` |
//! | [`FilletCorner::make_convex_nxn_corner_topology`] | `SmFilletConvexNxNCorner::MakeCornerTopology` |
//! | `find_side_edgeuse`, `find_closed_side_edgeuse` | `sm_FindSideEdgeuse`, `sm_FindClosedSideEdgeuse` |
//! | `same_convexity_edges` | `same_convexity_edges` |
//! | [`FilletCorner::make_nx1_closed_corner_topology`] ([`closed_nx1`]) | `SmFilletNx1ClosedCorner::MakeCornerTopology` |
//! | [`FilletCorner::make_3x2_mixed_corner_topology`] ([`mixed_3x2`]) | `SmFillet3x2MixedConvexityCorner::MakeCornerTopology` |
//! | `make_4x2_concave_corner_topology` (crate, `concave_4x2`) | `SmFilletNx2Corner::Make4x2ConcaveCornerTopology` |
//! | 1x1, 2x2, 4x3, open and degenerate corners ([`end_corners`]) | `SmFillet1x1Corner`, `SmFillet2x2Corner`, `SmFillet4x3Corner`, `SmFilletOpenCorner`, `SmFilletDegenerateCorner` `::MakeCornerTopology` |
//!
//! Deviations: the caller lends the actual target, pseudo-B-rep and solver.
//! Missing topology that native dereferences is an input error. The Nx1 end rib,
//! surface extension and cap topology are in [`nx1_end`]; the mixed-convexity 4x2
//! corner's `Make4x2ConcaveCornerTopology` is in `concave_4x2`. The Nx2 corner's two solvers are resolved by the
//! caller (upstream looks them up after the 4x2 checks; the lookup has no effects).
//! An unset `sm_TestG1Continuity` angle (upstream reads it uninitialised) is an input
//! error where the mixed-convexity tests read it. The convex `NxN` corner's solvers are
//! resolved by the caller before any allocation (upstream resolves each inside its
//! vertex loop; the lookups have no effects), and its unused `GetEdges` read is
//! omitted. There is no recursion.

pub mod closed_nx1;
mod concave_4x2;
pub mod concave_nxn;
pub mod end_corners;
pub mod mixed_3x2;

use crate::blend::fillet_corner::p1::{
    check_face_complimentary_corner_concavity, face_by_two_edgeuses, test_g1_continuity,
};
use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
use crate::blend::fillet_executive::session::{FilletObject, SolverObject};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletBrep, FilletCornerId, FilletEdgeType, FilletVertex, FilletVertexType,
};
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::error::{EntityRef, Result, SolidError};
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::surface::surface::Surface;
use crate::topology::edge;
use crate::topology::edgeuse::{p1 as eu1, p2 as eu2, p3 as eu3};
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, Vertex, VertexId};
use std::collections::BTreeMap;

pub mod nx1_end;

const OP: &str = "blend::fillet_corner::topology::make_nx1_corner_topology";
const OP_NX2: &str = "blend::fillet_corner::topology::make_nx2_corner_topology";
const OP_NXN: &str = "blend::fillet_corner::topology::make_convex_nxn_corner_topology";

/// Actual objects used by `SmFilletConvexNxNCorner::MakeCornerTopology`.
#[derive(Debug)]
pub struct NxNTopologyEnv<'a> {
    /// The executive's kernel.
    pub k: &'a Kernel,
    /// The target whose edge uses the corner stores.
    pub target: &'a mut Brep,
    /// The executive's actual pseudo-B-rep.
    pub pseudo: &'a mut FilletBrep,
    /// This corner's identity in the executive's corner store.
    pub corner: FilletCornerId,
    /// The loaded solver of each outward edge use, in `m_pAllEUs` order
    /// (`GetFilletSolverOfEdgeuse` of the edge use, else of its mate).
    pub solvers: Vec<&'a FilletSolver>,
}

/// Actual objects used by `SmFilletNx2Corner::MakeCornerTopology`.
#[derive(Debug)]
pub struct Nx2TopologyEnv<'a> {
    /// The executive's kernel.
    pub k: &'a Kernel,
    /// The target whose edge uses the corner stores.
    pub target: &'a mut Brep,
    /// The executive's actual pseudo-B-rep.
    pub pseudo: &'a mut FilletBrep,
    /// This corner's identity in the executive's corner store.
    pub corner: FilletCornerId,
    /// The loaded solvers of the corner's first and second filleted edge use
    /// (`GetFilletSolverOfEdgeuse`).
    pub solvers: [&'a FilletSolver; 2],
}

/// Actual objects used by `SmFilletNx1Corner::MakeCornerTopology`.
#[derive(Debug)]
pub struct Nx1TopologyEnv<'a> {
    /// The executive's kernel.
    pub k: &'a Kernel,
    /// The target whose edge uses the corner stores.
    pub target: &'a mut Brep,
    /// The executive's actual pseudo-B-rep.
    pub pseudo: &'a mut FilletBrep,
    /// This corner's identity in the executive's corner store.
    pub corner: FilletCornerId,
    /// The loaded concrete solver of the corner's filleted edge use.
    pub solver: &'a mut SolverObject,
    /// The executive's live global-merge switch.
    pub do_global_merge: &'a mut bool,
    /// The executive's `m_vExtendedSurfacesMap`.
    pub extended_surfaces: &'a mut BTreeMap<FaceId, Surface>,
    /// The executive's `m_vTopoEdges`.
    pub topo_edges: &'a mut Vec<Option<FilletObject>>,
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn need<T>(value: Option<T>, reason: &'static str) -> Result<T> {
    value.ok_or_else(|| invalid(reason))
}

fn edge_of(target: &Brep, eu: EdgeuseId) -> Result<EdgeId> {
    need(eu1::edge(target, eu)?, "edge use without edge")
}

fn radial(target: &Brep, eu: EdgeuseId) -> Result<EdgeuseId> {
    need(eu1::radial(target, eu)?, "edge use without radial")
}

fn vertex_of(target: &Brep, eu: EdgeuseId) -> Result<Option<VertexId>> {
    let vu = need(eu1::vertexuse(target, eu)?, "edge use without vertex use")?;
    Ok(target.vertexuse(vu)?.vertex)
}

fn find_side_edgeuse(
    target: &Brep,
    filleted: EdgeuseId,
    side: EdgeuseId,
) -> Result<Option<EdgeuseId>> {
    let filleted_edge = eu1::edge(target, filleted)?;
    let cw = need(
        eu1::cw_edgeuse(target, side)?,
        "side edge use without CW neighbour",
    )?;
    if filleted_edge == eu1::edge(target, cw)? {
        return Ok(Some(side));
    }
    let side = radial(target, side)?;
    let ccw = need(
        eu1::ccw_edgeuse(target, side)?,
        "side radial without CCW neighbour",
    )?;
    Ok((filleted_edge == eu1::edge(target, ccw)?).then_some(side))
}

fn find_closed_side_edgeuse(
    target: &Brep,
    solver: &FilletSolver,
    side: EdgeuseId,
    rail: usize,
) -> Result<EdgeuseId> {
    let filleted = need(solver.edgeuse(0)?, "solver without first edge use")?;
    let same_vertex = vertex_of(target, side)? == vertex_of(target, filleted)?;
    let neighbour = if same_vertex {
        need(
            eu1::cw_edgeuse(target, filleted)?,
            "filleted edge use without CW neighbour",
        )?
    } else {
        need(
            eu1::ccw_edgeuse(target, filleted)?,
            "filleted edge use without CCW neighbour",
        )?
    };
    if (rail == 0) == same_vertex {
        need(eu1::mate(target, neighbour)?, "side edge use without mate")
    } else {
        Ok(neighbour)
    }
}

fn same_convexity_edges(
    k: &Kernel,
    target: &mut Brep,
    first: EdgeuseId,
    second: EdgeuseId,
    vertex: VertexId,
) -> Result<bool> {
    let concave = if eu2::is_convex_radial_sector(k, target, first, 5, false, 0.0)? {
        if edge::is_manifold(target, edge_of(target, first)?)? {
            let mate = need(eu1::mate(target, first)?, "filleted edge use without mate")?;
            Some(radial(target, mate)?)
        } else {
            None
        }
    } else {
        Some(first)
    };
    // Native SER(SM_ERR) returns a nonzero status through this SmBoolean.
    let Some(concave) = concave else {
        return Ok(true);
    };
    let second_edge = eu1::edge(target, second)?;
    let mut side = need(
        eu1::corner_mate_edgeuse(target, concave, Some(vertex))?,
        "concave edge use without corner mate",
    )?;
    if eu1::edge(target, side)? != second_edge {
        side = need(
            eu1::corner_mate_edgeuse(target, radial(target, concave)?, Some(vertex))?,
            "concave radial without corner mate",
        )?;
    }
    Ok(!eu2::is_convex_radial_sector(
        k, target, side, 5, false, 0.0,
    )?)
}

/// A failed callee makes the fork's `FALSE`-returning helpers answer `FALSE`; an
/// `Unported` one still propagates.
fn or_false(r: Result<bool>) -> Result<bool> {
    match r {
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
        Ok(v) => Ok(v),
    }
}

/// `sm_CheckTangentialEdgeuseAtVertex` (the fork's source fork@d5ebcb3): whether
/// the two faces of `eu`'s edge meet tangentially (the same way round) at `vertex`.
fn check_tangential_edgeuse_at_vertex(
    k: &Kernel,
    target: &mut Brep,
    eu: EdgeuseId,
    vertex: VertexId,
    ang_tol_deg: f64,
) -> Result<bool> {
    or_false((|| {
        let e = edge_of(target, eu)?;
        if edge::is_closed(target, e)? || !edge::is_manifold(target, e)? {
            return Ok(false);
        }
        let Some(curve) = edge::curve(target, e)? else {
            return Ok(false);
        };
        let ivl = edge::interval(target, e)?;
        let (lo, hi) = (
            curve.evaluate_point(k, ivl.min())?,
            curve.evaluate_point(k, ivl.max())?,
        );
        let at = target.vertex(vertex)?.point;
        let t = if lo.distance_squared(at) <= hi.distance_squared(at) {
            ivl.min()
        } else {
            ivl.max()
        };
        let opts = eu3::BinormalOptions::default();
        let n1 = eu3::evaluate_binormal(k, target, eu, t, false, &opts)?.face_normal;
        let rad = radial(target, eu)?;
        let n2 = eu3::evaluate_binormal(k, target, rad, t, false, &opts)?.face_normal;
        Ok(n1.dot(n2) > 0.0 && n1.is_parallel_to(n2, ang_tol_deg))
    })())
}

/// `same_convexity_edges_round_vertex` (the fork's source fork@d5ebcb3): whether
/// `edge2`, anywhere round the vertex, has the convexity of `first`'s edge.
fn same_convexity_edges_round_vertex(
    k: &Kernel,
    target: &mut Brep,
    first: EdgeuseId,
    edge2: EdgeId,
) -> Result<bool> {
    let concave = if eu2::is_convex_radial_sector(k, target, first, 5, false, 0.0)? {
        if edge::is_manifold(target, edge_of(target, first)?)? {
            let mate = need(eu1::mate(target, first)?, "filleted edge use without mate")?;
            Some(radial(target, mate)?)
        } else {
            None
        }
    } else {
        Some(first)
    };
    // Native SE(SM_ERR) returns FALSE.
    let Some(concave) = concave else {
        return Ok(false);
    };
    let Some(region) = eu1::shell(target, concave)?
        .map(|s| eu1::shell_region(target, s))
        .transpose()?
    else {
        return Ok(false);
    };
    for eu in edge::edgeuses(target, edge2, None, None)? {
        if let Some(shell) = eu1::shell(target, eu)? {
            if eu1::shell_region(target, shell)? == region {
                return Ok(!eu2::is_convex_radial_sector(k, target, eu, 5, false, 0.0)?);
            }
        }
    }
    Ok(false)
}

fn is_fillet_side_face(target: &Brep, filleted: EdgeuseId, face: Option<FaceId>) -> Result<bool> {
    if face == eu1::face(target, filleted)? {
        return Ok(true);
    }
    Ok(face == eu1::face(target, radial(target, filleted)?)?)
}

fn new_corner_vertex(
    k: &Kernel,
    pseudo: &mut FilletBrep,
    corner: FilletCornerId,
) -> Result<VertexId> {
    let vertex = pseudo.brep_mut().add_unlinked_vertex_record(Vertex {
        point: Point3::ZERO,
        tolerance: 0.0,
        ..Vertex::default()
    })?;
    pseudo.insert_fillet_vertex(vertex, FilletVertex::new(k, Some(corner)));
    Ok(vertex)
}

impl FilletCorner {
    /// Construct the rails and end rib of an actual `NByOne` corner and, at a
    /// mixed-convexity corner, the extended end surface and the cap topology.
    ///
    /// Allocations and corner-list insertions occur in native order and remain
    /// visible if a later operation fails. Vertex geometry is computed later, except
    /// for the gap checks of a mixed-convexity end face spanning several faces: they
    /// end upstream's routine and are returned for the caller to run at once.
    ///
    /// # Errors
    /// Invalid topology or a different corner kind; those of the actual solver,
    /// convexity, surface-extension and pseudo-topology operations.
    pub fn make_nx1_corner_topology(
        &mut self,
        env: Nx1TopologyEnv<'_>,
    ) -> Result<Option<nx1_end::Nx1GapCheck>> {
        let Nx1TopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solver: solver_object,
            do_global_merge,
            extended_surfaces,
            topo_edges,
        } = env;
        let solver = solver_object.base();
        if self.kind != FilletCornerKind::NByOne {
            return Err(invalid("requires an NByOne corner"));
        }
        let filleted = need(
            self.solver_eus
                .as_deref()
                .and_then(|eus| eus.first())
                .copied(),
            "corner without filleted edge use",
        )?;
        let geometry = solver.first_fillet_geom()?;
        let all = need(self.all_eus.as_deref(), "corner without outward edge uses")?;
        let total = all.len();
        let index = need(
            all.iter().position(|&eu| eu == filleted),
            "filleted edge use not in corner",
        )?;
        let mut sides = [
            *need(
                all.get((index + total - 1) % total),
                "preceding side edge use",
            )?,
            *need(all.get((index + 1) % total), "following side edge use")?,
        ];
        let [side0, side1] = sides;
        let closed_side = side0 == side1;
        let vertex = need(self.vertex, "corner without vertex")?;
        let same = [
            same_convexity_edges(k, target, filleted, side0, vertex)?,
            same_convexity_edges(k, target, filleted, side1, vertex)?,
        ];
        let mut same = same;
        // A side edge whose faces meet tangentially at the vertex (a flat top running
        // smoothly onto a round) is no edge of the corner: the corner's side edge is
        // the one reached past it round the vertex (source fork@d5ebcb3, WR-457).
        let mut corner_sides = sides;
        let mut corner_total = total;
        if k.fixes.fillet_nx1_tangent_side_edge && !closed_side && total > 3 {
            let tol_deg = crate::math::rad_to_deg(self.tangency_tol_radians);
            for kk in 0..2 {
                let other = 1 - kk;
                if *need(same.get(other), "convexity index")?
                    || check_tangential_edgeuse_at_vertex(
                        k,
                        target,
                        *need(sides.get(other), "side index")?,
                        vertex,
                        tol_deg,
                    )?
                {
                    continue;
                }
                let behind = (index + total - 1) % total;
                let ahead = (index + 1) % total;
                let (mut idx, other_idx) = if kk == 0 {
                    (behind, ahead)
                } else {
                    (ahead, behind)
                };
                let mut skipped = 0;
                let mut passed_continuation = false;
                let mut next = *need(sides.get(kk), "side index")?;
                while total - skipped > 3 {
                    if !check_tangential_edgeuse_at_vertex(k, target, next, vertex, tol_deg)? {
                        // The filleted edge's continuation across the vertex, of the
                        // opposite convexity, is passed once as well: the overhang's rim
                        // continuing a boss's base circle past a wall. The rail on the side
                        // face runs alongside it and never meets it; the round ends on
                        // the end face past it (source fork@420e47d, WR-499).
                        if !k.fixes.fillet_nx1_opposite_continuation_side_edge {
                            break;
                        }
                        let filleted_edge = edge_of(target, filleted)?;
                        let next_edge = edge_of(target, next)?;
                        if passed_continuation
                            || next_edge == filleted_edge
                            || !test_g1_continuity(
                                k,
                                target,
                                Some(vertex),
                                Some(filleted_edge),
                                Some(next_edge),
                                tol_deg,
                            )?
                            .0
                            || same_convexity_edges_round_vertex(k, target, filleted, next_edge)?
                        {
                            break;
                        }
                        passed_continuation = true;
                    }
                    idx = if kk == 0 {
                        (idx + total - 1) % total
                    } else {
                        (idx + 1) % total
                    };
                    if idx == index || idx == other_idx {
                        skipped = 0;
                        break;
                    }
                    next = *need(all.get(idx), "side edge use")?;
                    skipped += 1;
                }
                if skipped > 0 {
                    let next_edge = edge_of(target, next)?;
                    if same_convexity_edges_round_vertex(k, target, filleted, next_edge)? {
                        *need(corner_sides.get_mut(kk), "side index")? = next;
                        *need(same.get_mut(kk), "convexity index")? = true;
                        corner_total -= skipped;
                        break;
                    }
                }
            }
        }
        let [same0, same1] = same;
        let mixed = !same0 || !same1;
        let mut end_face = None;
        let mut extend_edge = None;

        for rail in 0..2 {
            let side = *need(sides.get(rail), "side index")?;
            let fv = new_corner_vertex(k, pseudo, corner)?;
            self.vertices.push(fv);
            let (side_use, solver_rail) = if closed_side {
                (find_closed_side_edgeuse(target, solver, side, rail)?, rail)
            } else {
                let found = need(
                    find_side_edgeuse(target, filleted, side)?,
                    "no side edge use intersects rail",
                )?;
                (
                    found,
                    solver.find_index_of_rail_x_side_edgeuse(target, found)?,
                )
            };
            pseudo
                .fillet_vertex_mut(fv)?
                .set_point_class(PointClassType::Edgeuse, Some(EntityRef::Edgeuse(side_use)));
            pseudo.make_rail_edgeuse_on(
                geometry,
                solver_rail,
                Some(fv),
                crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Unknown,
                None,
                CornerLookup {
                    this: Some((Some(corner), self)),
                    ..Default::default()
                },
                solver,
                target,
            )?;

            let face = eu1::face(target, side)?;
            if mixed && *need(same.get(rail), "convexity index")? {
                pseudo
                    .fillet_vertex_mut(fv)?
                    .set_fillet_vertex_type(FilletVertexType::RailXExtendedEdgeuse);
                // (Past a tangent side edge, the corner's side edge.)
                let extend = *need(corner_sides.get(rail), "side index")?;
                let other = *need(corner_sides.get(1 - rail), "other side index")?;
                end_face = face_by_two_edgeuses(target, Some(extend), Some(other), None)?;
                extend_edge = eu1::edge(target, extend)?;
                if !is_fillet_side_face(target, filleted, face)? {
                    *need(sides.get_mut(rail), "side index")? = radial(target, side)?;
                }
            } else {
                pseudo
                    .fillet_vertex_mut(fv)?
                    .set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
                if is_fillet_side_face(target, filleted, face)? {
                    *need(sides.get_mut(rail), "side index")? = radial(target, side)?;
                }
            }
        }
        if !mixed && corner_total > 3 {
            *do_global_merge = true;
            return Ok(None);
        }
        self.finish_nx1_end(nx1_end::Nx1End {
            k,
            target,
            pseudo,
            corner,
            solver: solver_object,
            extended_surfaces,
            topo_edges,
            geometry,
            filleted,
            vertex,
            total: corner_total,
            mixed,
            same,
            sides,
            end_face,
            extend_edge,
        })
    }
}

impl FilletCorner {
    /// `SmFilletNx2Corner::MakeCornerTopology`: the fillet vertices where the rails of
    /// the corner's two fillets cross its side edges, the face vertex (rail/rail
    /// intersection of a bevel, mate of a tangent corner) or the two mates of a
    /// mixed-convexity 3x2 corner, their rail edge uses and the corner's cross edge(s).
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails. Vertex geometry is computed later.
    ///
    /// # Errors
    /// `SM_ERR` for more than four edge uses around the corner, filleted edge uses not
    /// among them or adjacent in a 4x2 corner, or other than two vertices before the
    /// cross edge; [`SolidError::InvalidInput`] for a different corner kind or missing
    /// topology upstream dereferences; those of the convexity, continuity, solver and
    /// pseudo-topology operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_nx2_corner_topology(&mut self, env: Nx2TopologyEnv<'_>) -> Result<()> {
        fn lookup(corner: FilletCornerId, this: &FilletCorner) -> CornerLookup<'_> {
            CornerLookup {
                this: Some((Some(corner), this)),
                ..Default::default()
            }
        }
        let Nx2TopologyEnv {
            k,
            target,
            pseudo,
            corner,
            solvers: [fil_solver1, fil_solver2],
        } = env;
        let invalid = |reason: &'static str| SolidError::invalid_input(OP_NX2, reason);
        if self.kind != FilletCornerKind::NByTwo {
            return Err(invalid("requires an NByTwo corner"));
        }
        let all = self
            .all_eus
            .clone()
            .ok_or_else(|| invalid("corner without outward edge uses"))?;
        let total = all.len();

        // no work - unhandled case - Currently we handle only 3x2 and 4x2
        if total > 4 {
            return Err(SolidError::sm_err(OP_NX2));
        }

        // Nx2 corner, get the two filleted edgeuses from the base brep
        let (base_eu1, base_eu2) = match self.solver_eus.as_deref() {
            Some([first, second, ..]) => (*first, *second),
            _ => return Err(invalid("corner without two filleted edge uses")),
        };
        let vertex = self
            .vertex
            .ok_or_else(|| invalid("corner without vertex"))?;

        // for 4 EU case
        if total == 4 {
            // check state: to be filleted edgeuses are on list of outward edgeuses
            let (Some(found1), Some(found2)) = (
                all.iter().position(|&eu| eu == base_eu1),
                all.iter().position(|&eu| eu == base_eu2),
            ) else {
                return Err(SolidError::sm_err(OP_NX2));
            };
            // the two filleted edgeuses must not be adjacent to each other
            if (found1 + found2) % 2 != 0 {
                return Err(SolidError::sm_err(OP_NX2));
            }
            // Check the non-filleted (side) edges for mixed convexity
            for &side in &all {
                if side == base_eu1 || side == base_eu2 {
                    continue;
                }
                if check_face_complimentary_corner_concavity(
                    k,
                    target,
                    vertex,
                    base_eu1,
                    base_eu2,
                    Some(side),
                )? {
                    return self.make_4x2_concave_corner_topology(concave_4x2::Concave4x2 {
                        k,
                        target,
                        pseudo,
                        corner,
                        solvers: [fil_solver1, fil_solver2],
                        geoms: [
                            fil_solver1.first_fillet_geom()?,
                            fil_solver2.first_fillet_geom()?,
                        ],
                        vertex,
                        base_eus: [base_eu1, base_eu2],
                        all_eus: &all,
                    });
                }
            }
            self.bevel = true;
        }

        // Determine if two edges are tangent at corner vertex.
        let tangency_tol_deg = crate::math::rad_to_deg(self.tangency_tol_radians);
        let (is_tangent_case, angle_deg) = test_g1_continuity(
            k,
            target,
            Some(vertex),
            Some(edge_of(target, base_eu1)?),
            Some(edge_of(target, base_eu2)?),
            tangency_tol_deg,
        )?;
        // Store this value for later use.
        self.g1_case = is_tangent_case;

        let mixed_convexity =
            check_face_complimentary_corner_concavity(k, target, vertex, base_eu1, base_eu2, None)?;
        let angle = || angle_deg.ok_or_else(|| invalid("edge angle at the corner is unset"));

        // Get Solvers and FilletGeoms for the two filleted edges at this vertex
        let geom1 = fil_solver1.first_fillet_geom()?;
        let geom2 = fil_solver2.first_fillet_geom()?;
        let unknown = crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Unknown;

        // For the non-filleted edge(s), we expect the rails to hit it (them).
        // Create a (fillet) vertex there, along with two rail edgeuses.
        for &curr in &all {
            if curr == base_eu1 || curr == base_eu2 {
                continue;
            }
            // This edgeuse is in between two filletted edgeuses
            let side = find_side_edgeuse(target, base_eu1, curr)?;
            let fv = new_corner_vertex(k, pseudo, corner)?;
            {
                let vertex = pseudo.fillet_vertex_mut(fv)?;
                vertex.set_point_class(PointClassType::Edgeuse, side.map(EntityRef::Edgeuse));
                vertex.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }
            self.vertices.push(fv);
            let side = side.ok_or_else(|| invalid("no side edge use intersects rail"))?;
            let rail = fil_solver1.find_index_of_rail_x_side_edgeuse(target, side)?;
            let other_rail =
                fil_solver2.find_index_of_rail_x_side_edgeuse(target, radial(target, side)?)?;
            pseudo.make_rail_edgeuse_on(
                geom1,
                rail,
                Some(fv),
                unknown,
                None,
                lookup(corner, self),
                fil_solver1,
                target,
            )?;
            pseudo.make_rail_edgeuse_on(
                geom2,
                other_rail,
                Some(fv),
                unknown,
                None,
                lookup(corner, self),
                fil_solver2,
                target,
            )?;
        }

        // Create the topology for the fillet corner.
        if total == 3 {
            // Find the side EU -- the un-filleted one.
            let side = all
                .iter()
                .copied()
                .find(|&eu| eu != base_eu1 && eu != base_eu2);
            let common_face = face_by_two_edgeuses(target, Some(base_eu1), Some(base_eu2), side)?;
            let rail = fil_solver1.find_index_of_rail_on_face(target, common_face)?;
            let other_rail = fil_solver2.find_index_of_rail_on_face(target, common_face)?;
            let side_fv = *self
                .vertices
                .first()
                .ok_or_else(|| invalid("no side fillet vertex"))?;
            let face_fv = new_corner_vertex(k, pseudo, corner)?;
            pseudo
                .fillet_vertex_mut(face_fv)?
                .set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
            self.vertices.push(face_fv);

            // mixed convexity case
            if mixed_convexity && !self.bevel && angle()? > 5.0 {
                // Mixed-convexity 3x2 corner, will create a 3-sided corner face.
                self.blending = true;
                {
                    let fv = pseudo.fillet_vertex_mut(face_fv)?;
                    fv.set_fillet_vertex_type(FilletVertexType::Mate);
                    fv.set_mate(0, Some(side_fv));
                }
                pseudo
                    .fillet_vertex_mut(side_fv)?
                    .set_mate(0, Some(face_fv));
                pseudo.make_rail_edgeuse_on(
                    geom1,
                    rail,
                    Some(face_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    fil_solver1,
                    target,
                )?;

                let face_fv2 = new_corner_vertex(k, pseudo, corner)?;
                self.vertices.push(face_fv2);
                {
                    let fv = pseudo.fillet_vertex_mut(face_fv2)?;
                    fv.set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                    fv.set_fillet_vertex_type(FilletVertexType::Mate);
                    fv.set_mate(0, Some(side_fv));
                }
                pseudo
                    .fillet_vertex_mut(side_fv)?
                    .set_mate(1, Some(face_fv2));
                pseudo.make_rail_edgeuse_on(
                    geom2,
                    other_rail,
                    Some(face_fv2),
                    unknown,
                    None,
                    lookup(corner, self),
                    fil_solver2,
                    target,
                )?;

                // a straight-across rib to end filletGeom1
                let rib = pseudo.make_fillet_edge(Some(side_fv), Some(face_fv), Some(corner))?;
                pseudo
                    .fillet_edge_mut(rib)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), rib)?,
                    "cross edge without primary edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(fil_solver1.first_fillet_geom()?))?;
                self.edges.push(rib);

                // the 3-sided face's third edge, inside the common face
                let rib = pseudo.make_fillet_edge(Some(face_fv), Some(face_fv2), Some(corner))?;
                {
                    let edge = pseudo.fillet_edge_mut(rib)?;
                    edge.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
                    edge.set_original_face(common_face);
                }
                self.edges.push(rib);

                // a straight-across rib to end filletGeom2
                let rib = pseudo.make_fillet_edge(Some(face_fv2), Some(side_fv), Some(corner))?;
                pseudo
                    .fillet_edge_mut(rib)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), rib)?,
                    "cross edge without primary edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(fil_solver2.first_fillet_geom()?))?;
                self.edges.push(rib);

                // all done
                return Ok(());
            }

            if is_tangent_case || (mixed_convexity && angle()? <= 5.0) {
                self.blending = true;
                // The new face-vertex will just be straight across from the
                // rail-side edge intersection vertex.
                {
                    let fv = pseudo.fillet_vertex_mut(face_fv)?;
                    fv.set_fillet_vertex_type(FilletVertexType::Mate);
                    fv.set_mate(0, Some(side_fv));
                }
                pseudo
                    .fillet_vertex_mut(side_fv)?
                    .set_mate(0, Some(face_fv));
            } else {
                // The new face vertex will be the intersection of the rails.
                pseudo
                    .fillet_vertex_mut(face_fv)?
                    .set_fillet_vertex_type(FilletVertexType::RailXRail);
                self.bevel = true;
            }

            // Setup rails of fillet
            pseudo.make_rail_edgeuse_on(
                geom1,
                rail,
                Some(face_fv),
                unknown,
                None,
                lookup(corner, self),
                fil_solver1,
                target,
            )?;
            pseudo.make_rail_edgeuse_on(
                geom2,
                other_rail,
                Some(face_fv),
                unknown,
                None,
                lookup(corner, self),
                fil_solver2,
                target,
            )?;
        } else if total == 4 {
            if is_tangent_case || (mixed_convexity && angle()? <= 5.0) {
                // Two filleted edgeuses are tangent to each other
                self.blending = true;
                self.bevel = false;
            } else {
                // Regular BEVEL corner
                self.bevel = true;
                self.blending = false;
            }
        }

        // At this point, we have only one fillet (cross) edge at this corner.
        if self.vertices.len() != 2 {
            return Err(SolidError::sm_err(OP_NX2));
        }
        let cross = pseudo.make_fillet_edge(
            self.vertices.first().copied(),
            self.vertices.get(1).copied(),
            Some(corner),
        )?;
        let primary = need(
            eu1::primary_edgeuse(pseudo.brep(), cross)?,
            "cross edge without primary edge use",
        )?;
        let mate = need(
            eu1::mate(pseudo.brep(), primary)?,
            "cross edge without mate edge use",
        )?;
        pseudo.set_edgeuse_fillet_geom(primary, Some(geom1))?;
        pseudo.set_edgeuse_fillet_geom(mate, Some(geom2))?;
        let edge_type = if is_tangent_case || self.blending {
            FilletEdgeType::CrossSection
        } else {
            FilletEdgeType::FilletXFillet
        };
        pseudo
            .fillet_edge_mut(cross)?
            .set_fillet_edge_type(edge_type);
        self.edges.push(cross);

        // all done
        Ok(())
    }
}

impl FilletCorner {
    /// `SmFilletConvexNxNCorner::MakeCornerTopology`: one fillet vertex per face
    /// between consecutive filleted edge uses (rail/rail intersection, or a mate and a
    /// setback vertex for a setback corner), the rail edge uses of both fillets on that
    /// face, and the corner's cross edges (iso-curves of a blending corner, edges to a
    /// shared centre vertex of a three-edge bevel corner, plus the in-face edges of a
    /// setback corner).
    ///
    /// Allocations and corner-list insertions occur in native order and remain visible
    /// if a later operation fails. Vertex geometry is computed later.
    ///
    /// # Errors
    /// [`SolidError::NullPointer`] when two consecutive edge uses share no face;
    /// `SM_ERR` for a bevel corner other than three edge uses;
    /// [`SolidError::InvalidInput`] for a different corner kind, a solver list that does
    /// not match the edge uses, or missing topology upstream dereferences; those of the
    /// solver and pseudo-topology operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_convex_nxn_corner_topology(&mut self, env: NxNTopologyEnv<'_>) -> Result<()> {
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
        let invalid = |reason: &'static str| SolidError::invalid_input(OP_NXN, reason);
        // SmFilletConcaveNxNCorner delegates its setback corners here.
        if !matches!(
            self.kind,
            FilletCornerKind::ConvexNByN | FilletCornerKind::ConcaveNByN
        ) {
            return Err(invalid("requires a convex or concave NxN corner"));
        }
        // Determine if this is a setback corner
        let is_set_back = self.set_back_dist > self.approx_tol_3d * 100.0;

        // All the edges of corner are filletted
        let all = self
            .all_eus
            .clone()
            .ok_or_else(|| invalid("corner without outward edge uses"))?;
        let total_eus = all.len();
        if solvers.len() != total_eus {
            return Err(invalid("one solver per edge use required"));
        }
        let (Some(&last_eu), Some(&last_solver)) = (all.last(), solvers.last()) else {
            return Err(invalid("corner without edge uses"));
        };
        let unknown = crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Unknown;

        let mut prev_eu = last_eu;
        let mut prev_solver = last_solver;
        // Create all fillet vertices
        for (&curr_eu, &curr_solver) in all.iter().zip(&solvers) {
            let common_face = face_by_two_edgeuses(target, Some(prev_eu), Some(curr_eu), None)?;
            if common_face.is_none() {
                //Unrecoverable error
                return Err(SolidError::null_pointer(OP_NXN));
            }
            let prev_rail = prev_solver.find_index_of_rail_on_face(target, common_face)?;
            let curr_rail = curr_solver.find_index_of_rail_on_face(target, common_face)?;

            let curr_fv = new_corner_vertex(k, pseudo, corner)?;
            pseudo
                .fillet_vertex_mut(curr_fv)?
                .set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));

            let prev_geom = prev_solver.first_fillet_geom()?;
            let curr_geom = curr_solver.first_fillet_geom()?;
            if is_set_back {
                // Will insert a new vertex before curr vert
                let new_v = new_corner_vertex(k, pseudo, corner)?;
                {
                    let fv = pseudo.fillet_vertex_mut(new_v)?;
                    fv.set_point_class(PointClassType::Face, common_face.map(EntityRef::Face));
                    fv.set_fillet_vertex_type(FilletVertexType::Mate);
                }
                self.vertices.push(new_v);
                self.vertices.push(curr_fv);
                // Setup rails of fillets
                pseudo.make_rail_edgeuse_on(
                    prev_geom,
                    prev_rail,
                    Some(new_v),
                    unknown,
                    None,
                    lookup(corner, self),
                    prev_solver,
                    target,
                )?;
                pseudo
                    .fillet_vertex_mut(curr_fv)?
                    .set_fillet_vertex_type(FilletVertexType::Setback);
                pseudo.make_rail_edgeuse_on(
                    curr_geom,
                    curr_rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    curr_solver,
                    target,
                )?;
            } else {
                // Regular corner, no setbacks
                pseudo
                    .fillet_vertex_mut(curr_fv)?
                    .set_fillet_vertex_type(FilletVertexType::RailXRail);
                // Setup rails of fillets
                pseudo.make_rail_edgeuse_on(
                    prev_geom,
                    prev_rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    prev_solver,
                    target,
                )?;
                pseudo.make_rail_edgeuse_on(
                    curr_geom,
                    curr_rail,
                    Some(curr_fv),
                    unknown,
                    None,
                    lookup(corner, self),
                    curr_solver,
                    target,
                )?;
                self.vertices.push(curr_fv);
            }
            prev_solver = curr_solver;
            prev_eu = curr_eu;
        }

        // Define center vert for Bevel cases
        let center_fv = if self.bevel {
            self.blending = false;
            if total_eus != 3 {
                // Will not handle other cases for now
                return Err(SolidError::sm_err(OP_NXN));
            }
            let center = new_corner_vertex(k, pseudo, corner)?;
            pseudo
                .fillet_vertex_mut(center)?
                .set_fillet_vertex_type(FilletVertexType::FilletX2Fillets);
            Some(center)
        } else {
            // This is a blending corner (it blends all edge-fillets)
            self.blending = true;
            None
        };

        // Now, create all fillet edges
        let total = self.vertices.len();
        let vertex_at = |this: &Self, index: usize| {
            this.vertices
                .get(index)
                .copied()
                .ok_or_else(|| invalid("corner vertex index out of range"))
        };
        let mut prev_solver = last_solver;
        let mut count = 0;
        let mut curr_fv = vertex_at(self, count)?;
        for &curr_solver in &solvers {
            let prev_geom = prev_solver.first_fillet_geom()?;
            let curr_geom = curr_solver.first_fillet_geom()?;
            // Get next vertex
            count = (count + 1) % total;
            let mut next_fv = vertex_at(self, count)?;
            if is_set_back {
                // Insert a rail-rail-interpo edge
                let edge = pseudo.make_fillet_edge(Some(curr_fv), Some(next_fv), Some(corner))?;
                let Some(EntityRef::Face(common_face)) =
                    pseudo.fillet_vertex(next_fv)?.point_class_object()
                else {
                    return Err(SolidError::null_pointer(OP_NXN));
                };
                {
                    let edge = pseudo.fillet_edge_mut(edge)?;
                    edge.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
                    edge.set_original_face(Some(common_face));
                }
                self.edges.push(edge);
                curr_fv = next_fv;
                count = (count + 1) % total;
                next_fv = vertex_at(self, count)?;
            }
            let fillet_edge = if self.bevel {
                // Make edge from curr vert to center
                let edge = pseudo.make_fillet_edge(Some(curr_fv), center_fv, Some(corner))?;
                pseudo
                    .fillet_edge_mut(edge)?
                    .set_fillet_edge_type(FilletEdgeType::FilletXFillet);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), edge)?,
                    "cross edge without primary edge use",
                )?;
                let mate = need(
                    eu1::mate(pseudo.brep(), primary)?,
                    "cross edge without mate edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(prev_geom))?;
                pseudo.set_edgeuse_fillet_geom(mate, Some(curr_geom))?;
                edge
            } else {
                // Make a cross-section (iso-curve) on fillet
                let edge = pseudo.make_fillet_edge(Some(curr_fv), Some(next_fv), Some(corner))?;
                pseudo
                    .fillet_edge_mut(edge)?
                    .set_fillet_edge_type(FilletEdgeType::CrossSection);
                let primary = need(
                    eu1::primary_edgeuse(pseudo.brep(), edge)?,
                    "cross edge without primary edge use",
                )?;
                pseudo.set_edgeuse_fillet_geom(primary, Some(curr_geom))?;
                if is_set_back {
                    pseudo
                        .fillet_vertex_mut(curr_fv)?
                        .set_mate(0, Some(next_fv));
                    pseudo
                        .fillet_vertex_mut(next_fv)?
                        .set_mate(0, Some(curr_fv));
                }
                edge
            };
            self.edges.push(fillet_edge);

            curr_fv = next_fv;
            prev_solver = curr_solver;
        }

        // Add center vertex for BEVEL case
        if let Some(center) = center_fv {
            self.vertices.push(center);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: FilletCornerKind, all: usize) -> Result<()> {
        let k = Kernel::default();
        let mut target = Brep::new();
        let mut pseudo = FilletBrep::new();
        let solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        let mut corner = FilletCorner::new(kind, None, None, None, 1e-3, 0.1);
        let eus = (0..all)
            .map(|_| {
                target
                    .add_edgeuse_record(crate::topology::Edgeuse::default())
                    .unwrap()
            })
            .collect();
        corner.all_eus = Some(eus);
        corner.make_nx2_corner_topology(Nx2TopologyEnv {
            k: &k,
            target: &mut target,
            pseudo: &mut pseudo,
            corner: FilletCornerId(0),
            solvers: [&solver, &solver],
        })
    }

    fn run_nxn(kind: FilletCornerKind, all: usize, solvers: usize) -> Result<()> {
        let k = Kernel::default();
        let mut target = Brep::new();
        let mut pseudo = FilletBrep::new();
        let solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        let mut corner = FilletCorner::new(kind, None, None, None, 1e-3, 0.1);
        let eus = (0..all)
            .map(|_| {
                target
                    .add_edgeuse_record(crate::topology::Edgeuse::default())
                    .unwrap()
            })
            .collect();
        corner.all_eus = Some(eus);
        corner.make_convex_nxn_corner_topology(NxNTopologyEnv {
            k: &k,
            target: &mut target,
            pseudo: &mut pseudo,
            corner: FilletCornerId(0),
            solvers: vec![&solver; solvers],
        })
    }

    #[test]
    fn convex_nxn_topology_rejects_other_kinds_and_mismatched_solvers() {
        assert!(matches!(
            run_nxn(FilletCornerKind::NByTwo, 3, 3),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            run_nxn(FilletCornerKind::ConvexNByN, 3, 2),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            run_nxn(FilletCornerKind::ConvexNByN, 0, 0),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn nx2_topology_rejects_other_kinds_and_more_than_four_edge_uses() {
        assert!(matches!(
            run(FilletCornerKind::NByOne, 3),
            Err(SolidError::InvalidInput { .. })
        ));
        let err = run(FilletCornerKind::NByTwo, 5).unwrap_err();
        assert!(!matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
        assert!(matches!(
            run(FilletCornerKind::NByTwo, 3),
            Err(SolidError::InvalidInput { .. })
        ));
    }
}

//! The live big-radius conversion of a fillet between two tangent `Nx2` corners.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp` (4239–4471).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `test_and_convert_big_rad_fillet` (crate) | `SmFilletExecutive::TestAndConvertBigRadFillet` |
//!
//! Deviations: the session's corner and solver slots are never reused, so the two
//! deleted corners keep their (emptied) records and the deleted solver's slot becomes
//! empty; `~SmFilletSolver`'s deletion of its fillet geometries clears their centre
//! line, offset surfaces and owned vertices, as the co-port
//! [`FilletExecutiveData::test_and_convert_big_rad_fillet`](crate::blend::fillet_corner::p3::FilletExecutiveData::test_and_convert_big_rad_fillet)
//! does. The executive's edge-use-to-solver map keeps naming the deleted solver (a
//! dangling pointer upstream); a later lookup through it is an input error. The new
//! `SmFilletConvexNxNCorner` is added to the corner slots only on success (upstream's
//! `SmObjDelete` deletes it, and the vertices it already lists, on a failure return;
//! the executive then fails). Self-aliasing topology that would be a use after free
//! upstream is an input error. There is no recursion.

use crate::blend::fillet_corner::p3::{
    modify_corneredge_fillet_relation, move_vertexuse, reverse_fillet_edge, FilletCorner,
    FilletCornerKind,
};
use crate::blend::fillet_executive::p3::{FilletExecutive, VertexEntry};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilStatus, FilletBrep, FilletCornerId, FilletCornerType, FilletEdgeType,
    FilletGeomId, FilletSolverId, FilletVertexType,
};
use crate::error::{EntityRef, Result, SolidError};
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{Brep, EdgeId, VertexId};

use super::{live_vertex_solvers, FilletBody, FilletObject, SolverObject};

const OP: &str = "blend::fillet_executive::session::big_radius";

fn null() -> SolidError {
    SolidError::invalid_input(OP, "upstream dereferences NULL")
}

/// The live objects `TestAndConvertBigRadFillet` reaches.
pub(super) struct BigRadEnv<'a> {
    pub(super) k: &'a Kernel,
    pub(super) target: &'a mut Brep,
    pub(super) pseudo: &'a mut FilletBrep,
    pub(super) solvers: &'a mut [Option<SolverObject>],
    /// The session's corner slots; the running corner's slot holds a placeholder.
    pub(super) corners: &'a mut Vec<FilletCorner>,
    pub(super) executive: &'a mut FilletExecutive<FilletObject>,
}

fn corner<'c>(
    corners: &'c [FilletCorner],
    running: (FilletCornerId, &'c FilletCorner),
    id: FilletCornerId,
) -> Result<&'c FilletCorner> {
    if running.0 == id {
        return Ok(running.1);
    }
    corners
        .get(id.0 as usize)
        .ok_or_else(|| SolidError::invalid_input(OP, "corner is not live"))
}

fn corner_mut<'c>(
    corners: &'c mut [FilletCorner],
    running_id: FilletCornerId,
    running: &'c mut FilletCorner,
    id: FilletCornerId,
) -> Result<&'c mut FilletCorner> {
    if running_id == id {
        return Ok(running);
    }
    corners
        .get_mut(id.0 as usize)
        .ok_or_else(|| SolidError::invalid_input(OP, "corner is not live"))
}

/// `SmFilletExecutive::TestAndConvertBigRadFillet(fg, rbConvertedToCorner)` called from
/// the running corner `running` (its id `running_id`): when `fg`'s two rails between
/// two `Nx2` corners run in opposite directions (a fillet radius too big for the
/// edge), squeezes the rail with two mate vertices into a rail/rail vertex, turns the
/// other rail and the two corners' cross sections into a new convex `NxN` corner that
/// takes the lower of the two corners' places, and deletes the two corners and `fg`'s
/// solver. Returns whether it converted.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a vertex without corner or a corner without filleted
/// vertex; `SM_ERR` when neither rail has two mate vertices, a corner does not have
/// exactly one edge or the corners or the solver are not in the executive's lists;
/// [`SolidError::InvalidInput`] where upstream dereferences `NULL`, for dead slots and
/// for self-aliasing topology; those of the pseudo-topology edits and the solver
/// lookups of `CalcRailIntRail` ([`SolidError::Unported`] propagates; its other
/// statuses are ignored, as upstream).
#[allow(clippy::too_many_lines)] // one upstream routine
pub(super) fn test_and_convert_big_rad_fillet(
    env: &mut BigRadEnv<'_>,
    running_id: FilletCornerId,
    running: &mut FilletCorner,
    fg: FilletGeomId,
) -> Result<bool> {
    let BigRadEnv {
        k,
        target,
        pseudo,
        solvers,
        corners,
        executive,
    } = env;
    let k: &Kernel = k;

    // Collect info about the ends of the two rails.
    let mut start_v = [VertexId::from_index(0); 2];
    let mut end_v = [VertexId::from_index(0); 2];
    let mut rails = [EdgeId::from_index(0); 2];
    let mut start_corner = FilletCornerId(0);
    let mut end_corner = FilletCornerId(0);
    let mut rail_dir = [Vec3::ZERO; 2];

    for ii in 0..2usize {
        let rail = pseudo.geom(fg)?.rail(ii).ok_or_else(null)?;
        let prim_eu = eu1::primary_edgeuse(pseudo.brep(), rail)?.ok_or_else(null)?;
        let mate_eu = eu1::mate(pseudo.brep(), prim_eu)?.ok_or_else(null)?;
        let prim_vu = eu1::vertexuse(pseudo.brep(), prim_eu)?.ok_or_else(null)?;
        let mate_vu = eu1::vertexuse(pseudo.brep(), mate_eu)?.ok_or_else(null)?;
        let sv = pseudo.brep().vertexuse(prim_vu)?.vertex.ok_or_else(null)?;
        let ev = pseudo.brep().vertexuse(mate_vu)?.vertex.ok_or_else(null)?;
        *rails.get_mut(ii).ok_or_else(null)? = rail;
        *start_v.get_mut(ii).ok_or_else(null)? = sv;
        *end_v.get_mut(ii).ok_or_else(null)? = ev;
        if ii == 0 {
            start_corner = pseudo
                .fillet_vertex(sv)?
                .fillet_corner()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            end_corner = pseudo
                .fillet_vertex(ev)?
                .fillet_corner()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            if corner(corners, (running_id, &*running), start_corner)?.topology_adjusted
                || corner(corners, (running_id, &*running), end_corner)?.topology_adjusted
            {
                // Unable to determine if the conversion can be achieved
                return Ok(false);
            }
        }
        if corner(corners, (running_id, &*running), start_corner)?.corner_type()
            != FilletCornerType::NByTwo
            || corner(corners, (running_id, &*running), end_corner)?.corner_type()
                != FilletCornerType::NByTwo
            || !pseudo.fillet_vertex(sv)?.is_processed()
            || !pseudo.fillet_vertex(ev)?.is_processed()
        {
            return Ok(false);
        }
        let pnt1 = pseudo.vertex_point(sv)?;
        let pnt2 = pseudo.vertex_point(ev)?;
        *rail_dir.get_mut(ii).ok_or_else(null)? = pnt2 - pnt1;
    }

    // Check whether the ends of the rails go in generally the same direction; if not,
    // then it's twisting back on itself, as in a too-tight corner. Also, if one rail
    // is stationary, that's not the large-radius case.
    let [dir1, dir2] = rail_dir;
    let len1 = dir1.length();
    let len2 = dir2.length();
    let fs_id = pseudo.geom(fg)?.fillet_solver().ok_or_else(null)?;
    let (tol, tan_tol) = {
        let fs = solvers
            .get(fs_id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(null)?
            .base();
        (fs.approx_tol_3d, fs.tangency_tolerance)
    };
    if len1 <= tol || len2 <= tol || dir1.dot(dir2) > 0.0 {
        // Valid FG, two rails
        return Ok(false);
    }

    // Two rails go in opposite directions: squeeze one of the rails and recompute its
    // vertex geometry
    let is_mate = |pseudo: &FilletBrep, v: VertexId| -> Result<bool> {
        Ok(pseudo.fillet_vertex(v)?.fillet_vertex_type() == FilletVertexType::Mate)
    };
    let mut squeeze = 0usize;
    if !is_mate(pseudo, start_v[0])? || !is_mate(pseudo, end_v[0])? {
        squeeze = 1;
        if !is_mate(pseudo, start_v[1])? || !is_mate(pseudo, end_v[1])? {
            return Err(SolidError::sm_err(OP));
        }
    }
    let other = 1 - squeeze;
    let sv_sq = *start_v.get(squeeze).ok_or_else(null)?;
    let ev_sq = *end_v.get(squeeze).ok_or_else(null)?;
    let sv_ot = *start_v.get(other).ok_or_else(null)?;
    let ev_ot = *end_v.get(other).ok_or_else(null)?;
    // Self-aliasing topology would be a use after free or double delete upstream.
    if rails[0] == rails[1]
        || sv_sq == ev_sq
        || ev_sq == sv_ot
        || ev_sq == ev_ot
        || start_corner == end_corner
    {
        return Err(null());
    }

    let squeezed_rail = *rails.get(squeeze).ok_or_else(null)?;
    pseudo.delete_fillet_edge(squeezed_rail)?;
    pseudo.geom_mut(fg)?.set_rail(squeeze, None);

    let vus = pseudo.brep().vertex(ev_sq)?.vertexuses.clone();
    for vu in vus {
        move_vertexuse(pseudo, vu, ev_sq, sv_sq)?;
    }
    pseudo.delete_fillet_vertex(ev_sq)?;

    pseudo
        .fillet_vertex_mut(sv_sq)?
        .set_fillet_vertex_type(FilletVertexType::RailXRail);
    for ii in 0..2usize {
        if let Some(mate) = pseudo.fillet_vertex(sv_sq)?.mate(ii) {
            let mv = pseudo.fillet_vertex_mut(mate)?;
            if mv.mate(0) == Some(sv_sq) {
                mv.set_mate(0, None);
            } else if mv.mate(1) == Some(sv_sq) {
                mv.set_mate(1, None);
            }
        }
    }
    {
        let v = pseudo.fillet_vertex_mut(sv_sq)?;
        v.set_mate(0, None);
        v.set_mate(1, None);
        v.set_status(FilStatus::Unprocessed);
    }

    // Compute vertex geometry (the status is ignored upstream)
    {
        let live = live_vertex_solvers(pseudo, solvers, sv_sq)?;
        let lookup = CornerLookup {
            table: Some(corners.as_slice()),
            this: Some((Some(running_id), &*running)),
            exec: None,
        };
        match pseudo.calc_rail_int_rail_with_solver_ops(k, sv_sq, lookup, live, target) {
            Err(e) if e.is_unported() => return Err(e),
            _ => {}
        }
    }

    // Make rail an edge for the new corner
    let kept_rail = *rails.get(other).ok_or_else(null)?;
    pseudo
        .fillet_edge_mut(kept_rail)?
        .set_fillet_edge_type(FilletEdgeType::CrossSection);
    let prim_eu1 = eu1::primary_edgeuse(pseudo.brep(), kept_rail)?.ok_or_else(null)?;
    let mate_eu1 = eu1::mate(pseudo.brep(), prim_eu1)?.ok_or_else(null)?;
    pseudo.set_edgeuse_fillet_geom(prim_eu1, None)?;
    pseudo.set_edgeuse_fillet_geom(mate_eu1, None)?;
    pseudo.geom_mut(fg)?.set_rail(other, None);

    let new_id = FilletCornerId(
        u32::try_from(corners.len())
            .map_err(|_| SolidError::invalid_input(OP, "too many corners"))?,
    );
    let mut new_corner =
        FilletCorner::new(FilletCornerKind::ConvexNByN, None, None, None, tol, tan_tol);

    // Insert start & end vertices to the new corner
    for v in [sv_sq, sv_ot, ev_ot] {
        pseudo.fillet_vertex_mut(v)?.set_fillet_corner(Some(new_id));
    }
    new_corner.vertices.extend([sv_ot, ev_ot, sv_sq]);

    // Insert remaining rail & 2 cross-sections to the new corner in an oriented order
    pseudo
        .fillet_edge_mut(kept_rail)?
        .set_fillet_corner(Some(new_id));
    new_corner.edges.push(kept_rail);
    for (old, first_vertex) in [(end_corner, ev_ot), (start_corner, sv_sq)] {
        let old_corner = corner_mut(corners, running_id, &mut *running, old)?;
        let &[cross_section] = old_corner.edges.as_slice() else {
            return Err(SolidError::sm_err(OP));
        };
        if edge::vertex(pseudo.brep(), cross_section)? != Some(first_vertex) {
            // Reverse the edge
            reverse_fillet_edge(pseudo, cross_section, OP)?;
        }
        modify_corneredge_fillet_relation(pseudo, cross_section, fg, OP)?;
        old_corner.vertices.clear();
        pseudo
            .fillet_edge_mut(cross_section)?
            .set_fillet_corner(Some(new_id));
        new_corner.edges.push(cross_section);
    }

    // Delete the two original corners and remove them from the corners array; insert
    // the new corner in the empty slot with the lower index (so the caller does not
    // revisit the same corner).
    let position = |executive: &FilletExecutive<FilletObject>, id: FilletCornerId| {
        executive
            .corners
            .iter()
            .position(|&c| c == FilletObject::Corner(id))
            .ok_or_else(|| SolidError::sm_err(OP))
    };
    let mut index1 = position(executive, start_corner)?;
    let mut index2 = position(executive, end_corner)?;
    if index1 > index2 {
        std::mem::swap(&mut index1, &mut index2);
    }
    *executive.corners.get_mut(index1).ok_or_else(null)? = FilletObject::Corner(new_id);
    if index2 < executive.corners.len() {
        executive.corners.remove(index2);
    }
    let orig_vert_s = corner(corners, (running_id, &*running), start_corner)?
        .filleted_vertex()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let orig_vert_e = corner(corners, (running_id, &*running), end_corner)?
        .filleted_vertex()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    for vertex in [orig_vert_s, orig_vert_e] {
        executive.vertex_corner_map.insert(
            Some(FilletObject::Topology(
                FilletBody::Target,
                EntityRef::Vertex(vertex),
            )),
            VertexEntry::Corner(FilletObject::Corner(new_id)),
        );
    }
    // ~SmFilletCorner: its vertex list is empty; the edge-use arrays are deleted.
    for old in [start_corner, end_corner] {
        let old_corner = corner_mut(corners, running_id, &mut *running, old)?;
        old_corner.solver_eus = None;
        old_corner.all_eus = None;
    }

    // Delete fillet solver
    let solver_index = executive
        .solvers
        .iter()
        .position(|&s| s == FilletObject::Solver(fs_id))
        .ok_or_else(|| SolidError::sm_err(OP))?;
    executive.solvers.remove(solver_index);
    delete_solver(pseudo, solvers, fs_id)?;

    corners.push(new_corner);
    Ok(true)
}

/// `delete pFS`: `~SmFilletSolver` deletes its fillet geometries (their centre line,
/// owned vertices and offset surfaces) and its offset surfaces; the slot is emptied.
fn delete_solver(
    pseudo: &mut FilletBrep,
    solvers: &mut [Option<SolverObject>],
    id: FilletSolverId,
) -> Result<()> {
    let slot = solvers.get_mut(id.0 as usize).ok_or_else(null)?;
    let solver = slot.take().ok_or_else(null)?;
    for geom in solver.base().fillet_geoms.clone() {
        let g = pseudo.geom_mut(geom)?;
        g.center_line_curve = None;
        let vertices = std::mem::take(&mut g.vertices);
        g.off_surfs = [None, None];
        for v in vertices {
            pseudo.delete_fillet_vertex(v)?;
        }
    }
    Ok(())
}

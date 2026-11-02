//! Live concave `NxN` corner topology over the session's objects.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp` (`SmFilletConcaveNxNCorner::MakeCornerTopology`,
//! 5196–5329).
//!
//! Deviations: each outward edge use's solver is resolved before the corner builds
//! anything (upstream looks them up inside its loop, without the mate fallback the
//! convex corner uses; the lookups have no effects). A setback corner goes to the
//! convex `NxN` session path, as upstream calls `SmFilletConvexNxNCorner::MakeCornerTopology`.
//! Its mate vertices are computed by the session's vertex dispatch
//! (`CalcMateGeom` through `FilletBrep::calc_mate_geom_with`, bounded by
//! `MAX_MATE_DEPTH`).

use super::{invalid, FilletBody, FilletObject, SessionObjects, SolverObject};
use crate::blend::fillet_corner::topology::NxNTopologyEnv;
use crate::blend::fillet_executive::p3::FilletExecutive;
use crate::blend::fillet_geom::p1::FilletCornerId;
use crate::error::{EntityRef, Result, SolidError};
use crate::kernel::Kernel;

impl SessionObjects<'_> {
    pub(super) fn make_concave_nxn_corner_topology(
        &mut self,
        k: &Kernel,
        id: FilletCornerId,
        executive: &mut FilletExecutive<FilletObject>,
    ) -> Result<()> {
        const NATIVE: &str = "SmFilletConcaveNxNCorner::MakeCornerTopology";
        let corner = self
            .corners
            .get(id.0 as usize)
            .ok_or_else(|| invalid("corner is not live"))?;
        // Determine if this is a setback corner
        if corner.set_back_dist > corner.approx_tol_3d * 100.0 {
            // Let SmFilletConvexNxNCorner handle setbacks
            return self.make_convex_nxn_corner_topology(k, id, executive);
        }
        let edgeuses = corner
            .all_eus
            .as_deref()
            .ok_or_else(|| invalid("corner has no outward edge uses"))?;
        let mut solver_ids = Vec::new();
        for &eu in edgeuses {
            let object = FilletObject::Topology(FilletBody::Target, EntityRef::Edgeuse(eu));
            match executive.fillet_solver_of_edgeuse(object) {
                Some(FilletObject::Solver(solver)) => solver_ids.push(solver),
                None => return Err(SolidError::null_pointer(NATIVE)),
                Some(_) => return Err(invalid("edge use does not name an actual solver")),
            }
        }
        let SessionObjects {
            target,
            pseudo,
            solvers,
            corners,
            ..
        } = self;
        let solvers = solver_ids
            .into_iter()
            .map(|solver| {
                solvers
                    .get(solver.0 as usize)
                    .and_then(Option::as_ref)
                    .map(SolverObject::base)
                    .ok_or_else(|| invalid("corner solver is not live"))
            })
            .collect::<Result<Vec<_>>>()?;
        let corner = corners
            .get_mut(id.0 as usize)
            .ok_or_else(|| invalid("corner is not live"))?;
        corner.make_concave_nxn_corner_topology(NxNTopologyEnv {
            k,
            target,
            pseudo,
            corner: id,
            solvers,
        })
    }
}

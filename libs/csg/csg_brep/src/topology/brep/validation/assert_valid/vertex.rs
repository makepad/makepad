//! The vertex and vertex-use checks.
//!
//! Upstream: `source/SMLib/src/SmVertex.cpp`, `SmVertexuse.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `vertex` | `SmVertex::AssertValid` |
//! | `vertexuse` | `SmVertexuse::AssertValid` |
//!
//! Deviations: none beyond the walk's (see the parent module).

use super::{base, class, seams, Cx};
use crate::error::{EntityRef, Result};
use crate::kernel::Kernel;
use crate::topology::{vertexuse as tvu, Brep, VertexId, VertexuseId, VertexuseOwner};

/// `SmVertex::AssertValid(SM_NO_WALK)`.
pub(super) fn vertex(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, v: VertexId) -> Result<bool> {
    const C: i64 = class::VERTEX;
    let this = EntityRef::Vertex(v);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::owning(b, cx, this)?;
    }

    // 1: Vertex location must be tightly located on FaceCorner (surf/surf/surf XSect);
    // the check runs at every level, its report is level 2
    let (located, max_gap) = seams::assert_test_vertex1(k, b, v)?;
    if !located {
        // SmTol::GetXSectTol3d(GetBrep())
        let xsect_tol = crate::topology::vertex::corner::xsect_tol_3d_of_brep(k, b);
        if max_gap > xsect_tol {
            let ok = !cx.rep.need(2);
            rtn &= cx
                .rep
                .this_value(b, C, this, 1, ok, xsect_tol, || max_gap)?;
        }
    }

    // 2: Vertex has multiple Vertexuse Vertex/Face connections one of which is a
    // VertexLoop
    let ok = !cx.rep.need(0) || seams::vertex_check_pointers(k, b, v)?;
    rtn &= cx.rep.this_bool(b, C, this, 2, ok)?;

    // Check all connected Vertexuses
    for vu in b.vertex(v)?.vertexuses.clone() {
        rtn &= vertexuse(k, b, cx, vu)?;
    }

    // all done
    Ok(rtn)
}

/// `SmVertexuse::AssertValid(SM_NO_WALK)`.
fn vertexuse(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, vu: VertexuseId) -> Result<bool> {
    const C: i64 = class::VERTEXUSE;
    let this = EntityRef::Vertexuse(vu);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::topology(b, cx, this)?;
    }

    // 0: SmVertexuse and the objects it attaches to need to share common contexts;
    // 1: its type matches its owner's (intrinsic: one enum)
    let owner = b.vertexuse(vu)?.owner;
    let has_owner = matches!(
        owner,
        VertexuseOwner::Edgeuse(Some(_))
            | VertexuseOwner::Loopuse(Some(_))
            | VertexuseOwner::Shell(Some(_))
    );
    if has_owner {
        rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
        rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
    }

    // 5: Vertexuse->Sector contains a FaceSrf->Seam not marked by SeamEdge
    let missing_seam = tvu::is_sector_missing_seam(k, b, vu, true)?.missing;
    rtn &= cx
        .rep
        .this_bool(b, C, this, 5, !cx.rep.need(0) || !missing_seam)?;

    // 6: Vertexuse connects to Faceuse for LoopuseTypes and EdgeuseTypes not connected
    // to wires
    if matches!(
        owner,
        VertexuseOwner::Loopuse(_) | VertexuseOwner::Edgeuse(_)
    ) {
        let edge = match tvu::edgeuse(b, vu)? {
            Some(eu) => b.edgeuse(eu)?.edge,
            None => None,
        };
        let ok = !cx.rep.need(0)
            || tvu::faceuse(b, vu)?.is_some()
            || match edge {
                Some(e) => tvu::edge_is_wire(b, e)?,
                None => false,
            };
        rtn &= cx.rep.this_bool(b, C, this, 6, ok)?;
    }

    // all done
    Ok(rtn)
}

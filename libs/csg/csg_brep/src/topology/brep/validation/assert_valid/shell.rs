//! The region and shell checks.
//!
//! Upstream: `source/SMLib/src/SmRegion.cpp`, `SmShell.cpp`, `SmEdgeuse.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `region` | `SmRegion::AssertValid` |
//! | `shell` | `SmShell::AssertValid` |
//! | `ShellIndex` | `sm_GetShellVertices`, `sm_GetShellFaceuses` |
//! | `connected_edgeuses` | `SmEdgeuse::GetConnectedEdgeuses` |
//!
//! Deviations: the shell vertices and face uses of every shell come from one pass over
//! the B-rep's vertices and faces (upstream scans them again for every shell); each
//! shell's lists keep the native order. The debug neighbour collection on a face-use
//! count mismatch only feeds drawing upstream and is omitted.

use std::collections::BTreeMap;

use super::{base, class, Cx};
use crate::error::{EntityRef, Result};
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::kernel::Kernel;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::shell::{wire_edges, ShellMembers};
use crate::topology::topology_traverser::{self as tt, Marks};
use crate::topology::{
    vertex as tv, vertexuse as tvu, Brep, EdgeuseId, EdgeuseOwner, FaceuseId, LoopuseStart,
    RegionId, ShellId, VertexId, VertexuseOwner,
};

/// `SmRegion::AssertValid(SM_NO_WALK)`: the base classes only.
pub(super) fn region(b: &Brep, cx: &mut Cx<'_>, r: RegionId) -> Result<bool> {
    let mut rtn = true;
    if cx.bases() {
        rtn &= base::owning(b, cx, EntityRef::Region(r))?;
    }
    Ok(rtn)
}

/// The vertices and face uses pointing back to each shell (`sm_GetShellVertices`,
/// `sm_GetShellFaceuses`), in B-rep vertex and face order.
pub(super) struct ShellIndex {
    vertices: BTreeMap<ShellId, Vec<VertexId>>,
    faceuses: BTreeMap<ShellId, Vec<FaceuseId>>,
}

impl ShellIndex {
    /// Indexes the shell vertices and face uses of `b`.
    pub(super) fn new(b: &Brep) -> Result<Self> {
        let mut vertices: BTreeMap<ShellId, Vec<VertexId>> = BTreeMap::new();
        // for every Brep vertex: add ShellVertices to their shell's list
        for &v in b.vertex_list() {
            if tv::is_shell_vertex(b, v)? {
                let vu = super::super::required(tv::primary_vertexuse(b, v)?)?;
                if let Some(s) = tvu::shell(b, vu)? {
                    vertices.entry(s).or_default().push(v);
                }
            }
        }
        let mut faceuses: BTreeMap<ShellId, Vec<FaceuseId>> = BTreeMap::new();
        // for every Brep face: add both faceuses to their shell's list
        for &f in b.face_list() {
            let (fu1, fu2) = crate::topology::face::p2::faceuses(b, f)?;
            for fu in [fu1, fu2].into_iter().flatten() {
                if let Some(s) = b.faceuse(fu)?.shell {
                    faceuses.entry(s).or_default().push(fu);
                }
            }
        }
        Ok(Self { vertices, faceuses })
    }

    fn vertices(&self, s: ShellId) -> &[VertexId] {
        self.vertices.get(&s).map_or(&[], Vec::as_slice)
    }

    fn faceuses(&self, s: ShellId) -> &[FaceuseId] {
        self.faceuses.get(&s).map_or(&[], Vec::as_slice)
    }
}

/// `SmEdgeuse::GetConnectedEdgeuses(rEdgeuses, eMarkType)`: a loop edge use's loop
/// edge uses, else the edge uses of every wire connected to its edge
/// (`CollectWireEdges` under a new mark). The status is ignored by the caller.
fn connected_edgeuses(k: &mut Kernel, b: &Brep, eu: EdgeuseId) -> Result<Vec<EdgeuseId>> {
    // when Edgeuse is part of a loop
    if eu1::is_loop_edgeuse(b, eu)? {
        let lu = super::super::required(eu1::loopuse(b, eu)?)?;
        return eu1::loopuse_edgeuses(b, lu);
    }

    // else edgeuse is part of a wire network
    let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
    let mark = lock.mark_type();
    let mut marks = Marks::default();
    let mut edges = Vec::new();
    let collected =
        tt::collect_wire_edges(k, b, &mut marks, eu1::edge(b, eu)?, mark, &mut edges, None);
    lock.release(core::slice::from_mut(&mut k.context))?;
    crate::error::ignored(collected)?;

    // for every wire
    let mut out = Vec::new();
    for e in edges {
        out.extend(b.edge(e)?.edgeuses.iter().copied());
    }
    Ok(out)
}

/// The edge use or vertex use a loop use starts with (`m_pEUorVU`).
fn loopuse_start_edgeuse(b: &Brep, fu: FaceuseId) -> Result<Option<(bool, Option<EdgeuseId>)>> {
    let Some(&lu) = b.faceuse(fu)?.loopuses.first() else {
        return Ok(None);
    };
    let is_edge = crate::topology::loopuse::is_edge_loopuse(b, lu)?;
    let start = match b.loopuse(lu)?.start {
        LoopuseStart::Edgeuse(eu) => eu,
        // upstream casts the vertex use to an edge use
        LoopuseStart::Vertexuse(_) | LoopuseStart::Unknown => None,
    };
    Ok(Some((is_edge, start)))
}

/// `SmShell::AssertValid(SM_NO_WALK)`.
#[allow(clippy::too_many_lines)]
pub(super) fn shell(
    k: &mut Kernel,
    b: &Brep,
    cx: &mut Cx<'_>,
    index: &ShellIndex,
    s: ShellId,
) -> Result<bool> {
    const C: i64 = class::SHELL;
    let this = EntityRef::Shell(s);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::owning(b, cx, this)?;
    }

    // check m_pList for consistent pointers of (faceuses, edgeuses, or vertexuse)
    let members = b.shell(s)?.members.clone();
    let has_list = !matches!(&members, ShellMembers::Empty)
        && !matches!(&members, ShellMembers::Faceuses(f) if f.is_empty());
    let ok = !cx.rep.need(0) || has_list;
    rtn &= cx.rep.this_bool(b, C, this, 0, ok)?;
    if !has_list {
        return Ok(false);
    }

    // search all brep vertices, edges, and faces for direct back pointers to this shell
    let shell_vertices = index.vertices(s);
    let wires = wire_edges(k, b, s)?;
    let shell_faceuses = index.faceuses(s);
    let need = cx.rep.need(0);

    // check for pointer/geometric consistency
    match b.shell(s)?.shell_type.code() {
        class::VERTEXUSE => {
            // check vertexuse back pointers to shell
            let ShellMembers::Vertexuse(vu) = members else {
                return Err(super::super::invalid(
                    "vertex shell list is not a vertexuse",
                ));
            };
            let shell_vertex = b.shell(s)?.vertex(b)?;
            let ok = !need
                || (shell_vertices.len() == 1 && shell_vertices.first().copied() == shell_vertex);
            rtn &= cx.rep.this_bool(b, C, this, 1, ok)?;

            // there should be no wire edges or shell faceuses
            rtn &= cx.rep.this_bool(b, C, this, 2, !need || wires.is_empty())?;
            rtn &= cx
                .rep
                .this_bool(b, C, this, 3, !need || shell_faceuses.is_empty())?;
            rtn &= cx.rep.this_bool(b, C, this, 4, true)?;
            let ok = !need || tvu::is_shell_vertexuse(b, vu)?;
            rtn &= cx.rep.this_bool(b, C, this, 5, ok)?;
            let ok = !need || b.vertexuse(vu)?.owner == VertexuseOwner::Shell(Some(s));
            rtn &= cx.rep.this_bool(b, C, this, 6, ok)?;
        }
        class::EDGEUSE => {
            // check edgeuse back pointers to shell
            let ShellMembers::Edgeuse(eu) = members else {
                return Err(super::super::invalid("wire shell list is not an edgeuse"));
            };
            rtn &= cx.rep.this_bool(b, C, this, 7, true)?;
            let ok = !need || eu1::is_shell_edgeuse(b, eu)?;
            rtn &= cx.rep.this_bool(b, C, this, 8, ok)?;
            let ok = !need || b.edgeuse(eu)?.owner == EdgeuseOwner::Shell(Some(s));
            rtn &= cx.rep.this_bool(b, C, this, 9, ok)?;

            // collect all connected edges in two ways - each should be in this shell
            let edgeuses1 = connected_edgeuses(k, b, eu)?;

            // there should be no ShellVertices nor ShellFaceuses
            rtn &= cx
                .rep
                .this_bool(b, C, this, 10, !need || shell_vertices.is_empty())?;
            rtn &= cx
                .rep
                .this_bool(b, C, this, 11, !need || shell_faceuses.is_empty())?;

            // get all WireEdge edgeuses
            let mut edgeuses2 = Vec::new();
            for &e in &wires {
                edgeuses2.extend(b.edge(e)?.edgeuses.iter().copied());
            }

            // number of edgeuses should be same
            let ok = !need || edgeuses1.len() == edgeuses2.len();
            rtn &= cx.rep.this_bool(b, C, this, 12, ok)?;

            // check every edgeuse back pointers to shell
            for (&eu1_, &eu2_) in edgeuses1.iter().zip(edgeuses2.iter()) {
                rtn &= cx.rep.this_bool(b, C, this, 13, true)?;
                rtn &= cx.rep.this_bool(b, C, this, 14, true)?;
                let ok = !need || eu1::is_shell_edgeuse(b, eu1_)?;
                rtn &= cx.rep.this_bool(b, C, this, 15, ok)?;
                let ok = !need || b.edgeuse(eu1_)?.owner == EdgeuseOwner::Shell(Some(s));
                rtn &= cx.rep.this_bool(b, C, this, 16, ok)?;
                let ok = !need || eu1::is_shell_edgeuse(b, eu2_)?;
                rtn &= cx.rep.this_bool(b, C, this, 17, ok)?;
                let ok = !need || b.edgeuse(eu2_)?.owner == EdgeuseOwner::Shell(Some(s));
                rtn &= cx.rep.this_bool(b, C, this, 18, ok)?;
            }
        }
        class::FACEUSE => {
            // check faceuse back pointers to shell
            let ShellMembers::Faceuses(fus) = members else {
                return Err(super::super::invalid("faceuse shell list is not faceuses"));
            };
            let first = super::super::required(fus.first().copied())?;
            rtn &= cx.rep.this_bool(b, C, this, 19, true)?;
            let ok = !need || b.faceuse(first)?.shell == Some(s);
            rtn &= cx.rep.this_bool(b, C, this, 20, ok)?;

            // 1: our topology list; 2: a topology traversal; 3: all faceuses pointing
            // back to this shell (already collected)
            let faceuses1 = b.shell(s)?.get_faceuses()?;
            let mut faceuses2: Vec<FaceuseId> = Vec::new();

            // get 1st shell faceuse's first loopuse
            let fu = super::super::required(b.shell(s)?.first_faceuse()?)?;
            let Some((is_edge_loopuse, start)) = loopuse_start_edgeuse(b, fu)? else {
                rtn &= cx.rep.this_bool(b, C, this, 34, !need)?;
                return Ok(rtn);
            };
            rtn &= cx.rep.this_bool(b, C, this, 21, !need || is_edge_loopuse)?;
            let start = super::super::required(start)?;

            // get faceuses through a shell traversal (increments an unused Mark value)
            if let Some(vu) = eu1::vertexuse(b, start)? {
                let v = super::super::required(tvu::vertex(b, vu)?)?;
                let mut marks = Marks::default();
                let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
                let traversed =
                    tt::shell_traversal(k, b, &mut marks, v, s, None, Some(&mut faceuses2), &lock);
                lock.release(core::slice::from_mut(&mut k.context))?;
                traversed?;
            }

            // number of faceuses collected all ways should be the same
            let ok = !need || faceuses1.len() == faceuses2.len();
            rtn &= cx.rep.this_bool(b, C, this, 22, ok)?;
            let ok = !need || shell_faceuses.len() == faceuses2.len();
            rtn &= cx.rep.this_bool(b, C, this, 23, ok)?;

            // there can be wire edges but no shell vertices
            rtn &= cx
                .rep
                .this_bool(b, C, this, 24, !need || shell_vertices.is_empty())?;

            // for every faceuse pair (should be same number)
            for (&fu1, &fu2) in faceuses1.iter().zip(faceuses2.iter()) {
                // every faceuse should point to this shell
                rtn &= cx.rep.this_bool(b, C, this, 24, true)?;
                let ok = !need || b.faceuse(fu1)?.shell == Some(s);
                rtn &= cx.rep.this_bool(b, C, this, 25, ok)?;
                let ok = !need || b.faceuse(fu2)?.shell == Some(s);
                rtn &= cx.rep.this_bool(b, C, this, 26, ok)?;
            }
        }
        _ => rtn = false,
    }

    // all done
    Ok(rtn)
}

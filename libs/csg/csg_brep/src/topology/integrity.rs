//! Structural integrity of a B-rep's arenas: no live entity names a dead one, and every
//! field the port caches where upstream derives agrees with its derivation.
//!
//! Upstream: none. This is not a port: `SmBrep::ValidatePointers`
//! ([`crate::topology::brep::validation::validate_pointers`]) is upstream's pointer check
//! and walks the topology through its own links. This check is the arena form of "no
//! dangling pointer": every typed id stored in a record the B-rep holds (its lists and
//! what their links reach) must name a live record.
//! It also compares the port's own cached fields (no upstream member) with what upstream
//! reads through the use links: [`Edge::vertices`] (`GetVertices`), [`Face::shell`]
//! (`GetShell`, the primary face use's shell), [`Face::loops`] (the primary face use's
//! loop uses' loops), [`Loop::face`] (`GetFace`, through the loop use and face use),
//! [`Loop::edges`] (the primary loop use's edge-use cycle) and [`Shell::faces`] (the
//! faces of the shell's face uses). [`Brep::refresh_cached_topology`] rewrites them
//! from the same derivations. It reads every record once, so it is for tests and
//! debugging, not for hot paths.
//!
//! Deviations: not an upstream port (see above).

use std::collections::BTreeSet;

use crate::error::{EntityRef, Result, SolidError};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::shell::ShellMembers;
use crate::topology::{
    edge, Arena, ArenaId, Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, LoopEdge,
    LoopId, LoopuseId, LoopuseStart, RegionId, ShellId, VertexId, VertexuseId, VertexuseOwner,
};
#[cfg(doc)]
use crate::topology::{Edge, Face, Loop, Shell};

/// The shell of `f` as `SmFace::GetShell` reads it: its primary face use's owner.
/// `None` without a primary face use or shell.
pub(crate) fn derived_face_shell(b: &Brep, f: FaceId) -> Option<ShellId> {
    let fu = b.faces().get(f)?.faceuse?;
    b.faceuses().get(fu)?.shell
}

/// The loops of `f` through its primary face use's loop uses, in their list order
/// (outer first). `None` without a primary face use or with a loop use without loop.
pub(crate) fn derived_face_loops(b: &Brep, f: FaceId) -> Option<Vec<LoopId>> {
    let fu = b.faces().get(f)?.faceuse?;
    let mut out = Vec::new();
    for &lu in &b.faceuses().get(fu)?.loopuses {
        out.push(b.loopuses().get(lu)?.loop_?);
    }
    Some(out)
}

/// The face of `l` as `SmLoop::GetFace` reads it: through its primary loop use and that
/// loop use's face use. `None` where a link is missing.
pub(crate) fn derived_loop_face(b: &Brep, l: LoopId) -> Option<FaceId> {
    let lu = b.loops().get(l)?.loopuse?;
    let fu = b.loopuses().get(lu)?.faceuse?;
    b.faceuses().get(fu)?.face
}

/// The edges of `l` as its primary loop use runs them: from the loop use's start edge
/// use around the counter-clockwise links, each forward when its edge use has the
/// edge's orientation. Empty for a vertex loop; `None` for a loop use not yet built, a
/// broken link or a cycle that does not close within the B-rep's edge uses.
pub(crate) fn derived_loop_edges(b: &Brep, l: LoopId) -> Option<Vec<LoopEdge>> {
    let lu = b.loops().get(l)?.loopuse?;
    let first = match b.loopuses().get(lu)?.start {
        LoopuseStart::Edgeuse(eu) => eu?,
        LoopuseStart::Vertexuse(_) => return Some(Vec::new()),
        LoopuseStart::Unknown => return None,
    };
    let mut out = Vec::new();
    let mut eu = first;
    for _ in 0..b.edgeuses().len() {
        let rec = b.edgeuses().get(eu)?;
        out.push(LoopEdge {
            edge: rec.edge?,
            forward: rec.orientation == OrientType::Same,
        });
        eu = rec.ccw?;
        if eu == first {
            return Some(out);
        }
    }
    None
}

/// The faces of `s` through its face uses in list order, each once.
pub(crate) fn derived_shell_faces(b: &Brep, s: ShellId) -> Option<Vec<FaceId>> {
    let mut out = Vec::new();
    if let ShellMembers::Faceuses(list) = &b.shells().get(s)?.members {
        for &fu in list {
            let f = b.faceuses().get(fu)?.face?;
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    Some(out)
}

/// `id` names a live record of `arena`, else [`SolidError::NotFound`] naming it, with
/// `relation` (which link held it) as the operation.
fn live<I: ArenaId, T>(
    arena: &Arena<I, T>,
    id: I,
    relation: &'static str,
    entity: impl Fn(I) -> EntityRef,
) -> Result<()> {
    if arena.get(id).is_some() {
        Ok(())
    } else {
        Err(SolidError::NotFound {
            operation: relation,
            entity: entity(id),
        })
    }
}

/// [`live`] for an optional link (`None` is not checked).
fn live_opt<I: ArenaId, T>(
    arena: &Arena<I, T>,
    id: Option<I>,
    relation: &'static str,
    entity: impl Fn(I) -> EntityRef,
) -> Result<()> {
    match id {
        Some(id) => live(arena, id, relation, entity),
        None => Ok(()),
    }
}

/// The entities `b` holds: its regions on the region list (and the infinite region),
/// faces on the face list, edges on the edge list and vertices on the vertex list, and
/// everything their links reach (shells, face uses, loop uses, loops, edge uses, vertex
/// uses). A record left in an arena that nothing in the B-rep reaches (a use the port's
/// construction dropped without freeing) is not part of the B-rep, as upstream's freed
/// object is not. Ids are collected only when live; dead ones are reported by the link
/// checks.
#[derive(Default)]
struct Reach {
    regions: BTreeSet<RegionId>,
    shells: BTreeSet<ShellId>,
    faces: BTreeSet<FaceId>,
    faceuses: BTreeSet<FaceuseId>,
    loops: BTreeSet<LoopId>,
    loopuses: BTreeSet<LoopuseId>,
    edges: BTreeSet<EdgeId>,
    edgeuses: BTreeSet<EdgeuseId>,
    vertices: BTreeSet<VertexId>,
    vertexuses: BTreeSet<VertexuseId>,
}

impl Reach {
    fn of(b: &Brep) -> Self {
        let mut r = Self::default();
        for (id, rec) in b.regions().iter() {
            if rec.in_brep || b.infinite_region() == Some(id) {
                r.regions.insert(id);
                r.shells
                    .extend(rec.shells.iter().filter(|&&s| b.shells().get(s).is_some()));
            }
        }
        for &s in &r.shells {
            if let Some(rec) = b.shells().get(s) {
                match &rec.members {
                    ShellMembers::Faceuses(list) => r.faceuses.extend(list.iter().copied()),
                    ShellMembers::Edgeuse(eu) => {
                        r.edgeuses.insert(*eu);
                    }
                    ShellMembers::Vertexuse(vu) => {
                        r.vertexuses.insert(*vu);
                    }
                    ShellMembers::Empty => {}
                }
            }
        }
        r.faces.extend(b.face_list().iter().copied());
        r.edges.extend(b.edge_list().iter().copied());
        r.vertices.extend(b.vertex_list().iter().copied());
        for &f in &r.faces.clone() {
            if let Some(fu) = b.faces().get(f).and_then(|x| x.faceuse) {
                r.faceuses.insert(fu);
            }
        }
        for &fu in &r.faceuses.clone() {
            if let Some(rec) = b.faceuses().get(fu) {
                r.faceuses.extend(rec.mate);
                r.faces.extend(rec.face);
            }
        }
        for &fu in &r.faceuses {
            if let Some(rec) = b.faceuses().get(fu) {
                r.loopuses.extend(rec.loopuses.iter().copied());
            }
        }
        for &lu in &r.loopuses.clone() {
            if let Some(rec) = b.loopuses().get(lu) {
                r.loopuses.extend(rec.mate);
                r.loops.extend(rec.loop_);
                match rec.start {
                    LoopuseStart::Edgeuse(eu) => r.edgeuses.extend(eu),
                    LoopuseStart::Vertexuse(vu) => r.vertexuses.extend(vu),
                    LoopuseStart::Unknown => {}
                }
            }
        }
        for &e in &r.edges.clone() {
            if let Some(rec) = b.edges().get(e) {
                r.edgeuses.extend(rec.edgeuses.iter().copied());
            }
        }
        for &eu in &r.edgeuses.clone() {
            if let Some(rec) = b.edgeuses().get(eu) {
                r.edges.extend(rec.edge);
                r.vertexuses.extend(rec.vertexuse);
            }
        }
        for &v in &r.vertices.clone() {
            if let Some(rec) = b.vertices().get(v) {
                r.vertexuses.extend(rec.vertexuses.iter().copied());
            }
        }
        for &vu in &r.vertexuses.clone() {
            if let Some(v) = b.vertexuses().get(vu).and_then(|x| x.vertex) {
                r.vertices.insert(v);
            }
        }
        // keep only live ids (dead ones are the link checks' to report)
        r.shells.retain(|&x| b.shells().get(x).is_some());
        r.faces.retain(|&x| b.faces().get(x).is_some());
        r.faceuses.retain(|&x| b.faceuses().get(x).is_some());
        r.loops.retain(|&x| b.loops().get(x).is_some());
        r.loopuses.retain(|&x| b.loopuses().get(x).is_some());
        r.edges.retain(|&x| b.edges().get(x).is_some());
        r.edgeuses.retain(|&x| b.edgeuses().get(x).is_some());
        r.vertices.retain(|&x| b.vertices().get(x).is_some());
        r.vertexuses.retain(|&x| b.vertexuses().get(x).is_some());
        r
    }
}

/// Checks `b`, asking no more of the links than upstream's `ValidatePointers` walk
/// dereferences: every upstream link (`m_p...` member) held by a region, shell, face,
/// face use, loop, loop use, edge, edge use, vertex use or vertex the B-rep holds (on its lists or reached through their
/// links; see `Reach`) names a live entity. The port's own cached fields (see the module
/// docs; no upstream member) are not checked for liveness but must equal their
/// derivation wherever the links they derive from exist (they are refreshed when a
/// public operation returns).
///
/// # Errors
///
/// [`SolidError::NotFound`] for the first dead id found (the operation names the link
/// that held it) or for the first stale cached field, naming its entity (the operation
/// names the field, e.g. `"topology::integrity::check: cached edge ends"`); those of
/// [`edge::start_end_vertices`] for an edge whose uses cannot be read.
pub fn check(b: &Brep) -> Result<()> {
    let reach = Reach::of(b);
    for (_, r) in b
        .regions()
        .iter()
        .filter(|(id, _)| reach.regions.contains(id))
    {
        for &s in &r.shells {
            live(
                b.shells(),
                s,
                "topology::integrity::check: region shell",
                EntityRef::Shell,
            )?;
        }
    }
    for (_, s) in b
        .shells()
        .iter()
        .filter(|(id, _)| reach.shells.contains(id))
    {
        let rel = "topology::integrity::check: shell region";
        live_opt(b.regions(), s.region, rel, EntityRef::Region)?;
    }
    for (_, f) in b.faces().iter().filter(|(id, _)| reach.faces.contains(id)) {
        let rel = "topology::integrity::check: face use of a face";
        live_opt(b.faceuses(), f.faceuse, rel, EntityRef::Faceuse)?;
    }
    for (_, fu) in b
        .faceuses()
        .iter()
        .filter(|(id, _)| reach.faceuses.contains(id))
    {
        let rel = "topology::integrity::check: face use shell";
        live_opt(b.shells(), fu.shell, rel, EntityRef::Shell)?;
        let rel = "topology::integrity::check: face use face";
        live_opt(b.faces(), fu.face, rel, EntityRef::Face)?;
        let rel = "topology::integrity::check: face use mate";
        live_opt(b.faceuses(), fu.mate, rel, EntityRef::Faceuse)?;
        for &lu in &fu.loopuses {
            let rel = "topology::integrity::check: face use loop use";
            live(b.loopuses(), lu, rel, EntityRef::Loopuse)?;
        }
    }
    for (_, l) in b.loops().iter().filter(|(id, _)| reach.loops.contains(id)) {
        let rel = "topology::integrity::check: loop use of a loop";
        live_opt(b.loopuses(), l.loopuse, rel, EntityRef::Loopuse)?;
    }
    for (_, lu) in b
        .loopuses()
        .iter()
        .filter(|(id, _)| reach.loopuses.contains(id))
    {
        let rel = "topology::integrity::check: loop use face use";
        live_opt(b.faceuses(), lu.faceuse, rel, EntityRef::Faceuse)?;
        let rel = "topology::integrity::check: loop use loop";
        live_opt(b.loops(), lu.loop_, rel, EntityRef::Loop)?;
        let rel = "topology::integrity::check: loop use mate";
        live_opt(b.loopuses(), lu.mate, rel, EntityRef::Loopuse)?;
        match lu.start {
            LoopuseStart::Edgeuse(eu) => {
                let rel = "topology::integrity::check: loop use start edge use";
                live_opt(b.edgeuses(), eu, rel, EntityRef::Edgeuse)?;
            }
            LoopuseStart::Vertexuse(vu) => {
                let rel = "topology::integrity::check: loop use start vertex use";
                live_opt(b.vertexuses(), vu, rel, EntityRef::Vertexuse)?;
            }
            LoopuseStart::Unknown => {}
        }
    }
    for (_, e) in b.edges().iter().filter(|(id, _)| reach.edges.contains(id)) {
        for &eu in &e.edgeuses {
            let rel = "topology::integrity::check: edge's edge use";
            live(b.edgeuses(), eu, rel, EntityRef::Edgeuse)?;
        }
    }
    for (_, eu) in b
        .edgeuses()
        .iter()
        .filter(|(id, _)| reach.edgeuses.contains(id))
    {
        let rel = "topology::integrity::check: edge use edge";
        live_opt(b.edges(), eu.edge, rel, EntityRef::Edge)?;
        let rel = "topology::integrity::check: edge use ccw";
        live_opt(b.edgeuses(), eu.ccw, rel, EntityRef::Edgeuse)?;
        let rel = "topology::integrity::check: edge use cw";
        live_opt(b.edgeuses(), eu.cw, rel, EntityRef::Edgeuse)?;
        let rel = "topology::integrity::check: edge use vertex use";
        live_opt(b.vertexuses(), eu.vertexuse, rel, EntityRef::Vertexuse)?;
        match eu.owner {
            EdgeuseOwner::Loopuse(lu) => {
                let rel = "topology::integrity::check: edge use owner loop use";
                live_opt(b.loopuses(), lu, rel, EntityRef::Loopuse)?;
            }
            EdgeuseOwner::Shell(s) => {
                let rel = "topology::integrity::check: edge use owner shell";
                live_opt(b.shells(), s, rel, EntityRef::Shell)?;
            }
            EdgeuseOwner::Unknown => {}
        }
    }
    for (_, vu) in b
        .vertexuses()
        .iter()
        .filter(|(id, _)| reach.vertexuses.contains(id))
    {
        let rel = "topology::integrity::check: vertex use vertex";
        live_opt(b.vertices(), vu.vertex, rel, EntityRef::Vertex)?;
        match vu.owner {
            VertexuseOwner::Edgeuse(eu) => {
                let rel = "topology::integrity::check: vertex use owner edge use";
                live_opt(b.edgeuses(), eu, rel, EntityRef::Edgeuse)?;
            }
            VertexuseOwner::Loopuse(lu) => {
                let rel = "topology::integrity::check: vertex use owner loop use";
                live_opt(b.loopuses(), lu, rel, EntityRef::Loopuse)?;
            }
            VertexuseOwner::Shell(s) => {
                let rel = "topology::integrity::check: vertex use owner shell";
                live_opt(b.shells(), s, rel, EntityRef::Shell)?;
            }
            VertexuseOwner::Unknown => {}
        }
    }
    for (_, v) in b
        .vertices()
        .iter()
        .filter(|(id, _)| reach.vertices.contains(id))
    {
        for &vu in &v.vertexuses {
            let rel = "topology::integrity::check: vertex's vertex use";
            live(b.vertexuses(), vu, rel, EntityRef::Vertexuse)?;
        }
    }
    // the port's cached fields against their derivations
    let stale =
        |operation: &'static str, entity: EntityRef| SolidError::NotFound { operation, entity };
    for (f, rec) in b.faces().iter().filter(|(id, _)| reach.faces.contains(id)) {
        if derived_face_shell(b, f).is_some_and(|s| s != rec.shell) {
            let op = "topology::integrity::check: cached face shell";
            return Err(stale(op, EntityRef::Face(f)));
        }
        if derived_face_loops(b, f).is_some_and(|ls| ls != rec.loops) {
            let op = "topology::integrity::check: cached face loops";
            return Err(stale(op, EntityRef::Face(f)));
        }
    }
    for (l, rec) in b.loops().iter().filter(|(id, _)| reach.loops.contains(id)) {
        if derived_loop_face(b, l).is_some_and(|f| f != rec.face) {
            let op = "topology::integrity::check: cached loop face";
            return Err(stale(op, EntityRef::Loop(l)));
        }
        if derived_loop_edges(b, l).is_some_and(|es| es != rec.edges) {
            let op = "topology::integrity::check: cached loop edges";
            return Err(stale(op, EntityRef::Loop(l)));
        }
    }
    for (s, rec) in b
        .shells()
        .iter()
        .filter(|(id, _)| reach.shells.contains(id))
    {
        if derived_shell_faces(b, s).is_some_and(|fs| fs != rec.faces) {
            let op = "topology::integrity::check: cached shell faces";
            return Err(stale(op, EntityRef::Shell(s)));
        }
    }
    for (e, rec) in b.edges().iter().filter(|(id, _)| reach.edges.contains(id)) {
        if rec.edgeuses.is_empty() {
            continue;
        }
        let (start, end) = edge::start_end_vertices(b, e)?;
        if let (Some(start), Some(end)) = (start, end) {
            if rec.vertices != [start, end] {
                return Err(SolidError::NotFound {
                    operation: "topology::integrity::check: cached edge ends",
                    entity: EntityRef::Edge(e),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Kernel;
    use crate::math::Point3;
    use crate::primitive::api_primitives::create_box;

    #[test]
    fn a_box_is_intact_and_a_stale_end_pair_is_reported() {
        let mut k = Kernel::default();
        let mut b = create_box(&mut k, Point3::new(0.0, 0.0, 0.0), 1.0, 2.0, 3.0).unwrap();
        b.refresh_cached_topology();
        check(&b).unwrap();
        let (e, rec) = b.edges().iter().next().unwrap();
        let swapped = [rec.vertices[1], rec.vertices[0]];
        b.edge_mut(e).unwrap().vertices = swapped;
        assert!(matches!(
            check(&b),
            Err(SolidError::NotFound {
                entity: EntityRef::Edge(_),
                ..
            })
        ));
        b.refresh_cached_topology();
        check(&b).unwrap();
    }
}

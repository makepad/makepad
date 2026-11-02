//! Connected face, wire, face-use and shell traversals with caller-owned marks.
//!
//! Upstream: `source/SMLib/inc/SmTopologyTraverser.h`,
//! `source/SMLib/src/SmTopologyTraverser.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`collect_faces`] | `SmTopologyTraverser::CollectFaces`, `sm_CollectFaces` |
//! | [`collect_wire_edges`] | `SmTopologyTraverser::CollectWireEdges`, `sm_CollectWires` |
//! | [`closure_traversal`], [`faceuse_traversal`] | `ClosureTraversal`, `FaceuseTraversal` |
//! | [`remove_shell`], [`faceuse_shell_replacement`] | `RemoveShell`, `FaceuseShellReplacement` |
//! | [`shell_traversal`], `shell_connectivity` (private) | `ShellTraversal`, `sm_ShellConnectivity` |
//! | [`is_inner_shell`] | `IsInnerShell`, `IsOdd` |
//! | [`has_lamina_edge`] | `SmFace::HasLaminaEdge` (CO-PORTED, `topology.face_p3`; re-exported there) |
//! | [`face_edges`] | `SmFace::GetEdges` (CO-PORTED, `topology.face_p4`, default arguments) |
//!
//! Deviations: stateless constructor/destructor become free functions. Marks are
//! [`Marks`] records alongside one B-rep, one slot per arena index of each kind
//! (upstream keeps the record in the object itself), using the kernel context's counters;
//! pointers become typed ids. Null dereferences and `NoMark` on cyclic traversals
//! return errors. Wire recursion becomes explicit continuation frames, in exactly
//! the upstream order (no input-dependent call-stack growth). Outputs remain mutable
//! arrays where early returns preserve optional arrays upstream. Shell ownership
//! moves through `topology::shell` (`transfer_children`, `move_faceuse`: the
//! `Remove` / `PostInsert` statuses that `FaceuseShellReplacement` ignores cannot
//! fail on a consistent B-rep, so an inconsistent one is an error). `IsInnerShell`'s
//! face samples and ray/face intersections (`SmFace::GetPointsInFace`,
//! `SmFace::GlobalLineIntersect`) run on the caller's B-rep, which keeps the UV trim
//! curves they make, as upstream's `const` queries leave them. `IsInnerShell` keeps each target face surface's solve cache for the whole call
//! (upstream's surface keeps its cache across the rays); the port otherwise builds one
//! per intersection, and a failed build is kept too instead of being retried on every
//! ray (the build is deterministic, so a retry fails the same way).
//! Existing co-ported accessors are reused.

use std::collections::BTreeMap;

use crate::error::{Result, SolidError};
use crate::geom::context::{MarkLock, MarkType};
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::edge::edge_faces;
use crate::surface::surface::p8::HeldSolveCache;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::face::p2 as face;
use crate::topology::topology::Topology;
use crate::topology::{
    shell, vertex, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, ShellId, VertexId,
    VertexuseId,
};

const OP: &str = "topology::topology_traverser";

/// A topology object carrying a traversal mark, scoped to one B-rep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Entity {
    /// A face.
    Face(FaceId),
    /// An edge.
    Edge(EdgeId),
    /// A vertex.
    Vertex(VertexId),
    /// A face use.
    Faceuse(FaceuseId),
    /// A vertex use.
    Vertexuse(VertexuseId),
    /// A shell (`SmShell::Mark`, as `SmShell::GetMatedShells` uses it).
    Shell(ShellId),
}

/// Topology mark records for one B-rep; retain across calls using the same context.
///
/// One optional record per arena slot of each entity kind, as upstream keeps the
/// record in the object: lookups are indexed, not searched.
#[derive(Debug, Clone, Default)]
pub struct Marks([Vec<Option<Topology>>; 6]);

/// Mark access shared by face traversal and a production topology record store.
/// Implementations must retain marks across calls under the same kernel context.
pub trait FaceTraversalMarks {
    /// Query the current context's mark on an entity.
    fn is_marked(&self, k: &Kernel, entity: Entity, mark: MarkType) -> bool;
    /// Set the current context's mark on an entity.
    fn mark(&mut self, k: &Kernel, entity: Entity, mark: MarkType);
}

impl FaceTraversalMarks for Marks {
    fn is_marked(&self, k: &Kernel, entity: Entity, mark: MarkType) -> bool {
        Self::is_marked(self, k, entity, mark)
    }
    fn mark(&mut self, k: &Kernel, entity: Entity, mark: MarkType) {
        Self::mark(self, k, entity, mark);
    }
}

impl Marks {
    /// The kind table and arena index of `entity`.
    fn slot(entity: Entity) -> (usize, usize) {
        match entity {
            Entity::Face(x) => (0, x.index() as usize),
            Entity::Edge(x) => (1, x.index() as usize),
            Entity::Vertex(x) => (2, x.index() as usize),
            Entity::Faceuse(x) => (3, x.index() as usize),
            Entity::Vertexuse(x) => (4, x.index() as usize),
            Entity::Shell(x) => (5, x.index() as usize),
        }
    }

    /// The record of `entity`, made default first if it has none.
    fn record_mut(&mut self, entity: Entity) -> Option<&mut Topology> {
        let (kind, i) = Self::slot(entity);
        let table = self.0.get_mut(kind)?;
        if table.len() <= i {
            table.resize(i + 1, None);
        }
        Some(table.get_mut(i)?.get_or_insert_with(Topology::default))
    }

    /// Import the caller's persistent topology record before traversal.
    pub fn set_record(&mut self, entity: Entity, record: Topology) {
        if let Some(r) = self.record_mut(entity) {
            *r = record;
        }
    }

    /// The updated topology record, including marks written before an error.
    #[must_use]
    pub fn record(&self, entity: Entity) -> Option<&Topology> {
        let (kind, i) = Self::slot(entity);
        self.0.get(kind)?.get(i)?.as_ref()
    }

    /// Whether `entity` carries the kernel's current `mark` value.
    #[must_use]
    pub fn is_marked(&self, k: &Kernel, entity: Entity, mark: MarkType) -> bool {
        self.record(entity)
            .copied()
            .unwrap_or_default()
            .is_marked(&k.context, mark)
    }

    /// Set `entity` to the kernel's current `mark` value.
    pub fn mark(&mut self, k: &Kernel, entity: Entity, mark: MarkType) {
        if let Some(r) = self.record_mut(entity) {
            r.mark(Some(&k.context), mark);
        }
    }
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}

fn check_mark(mark: MarkType) -> Result<()> {
    if mark == MarkType::NoMark {
        return Err(SolidError::invalid_input(
            OP,
            "traversal requires a real mark",
        ));
    }
    Ok(())
}

/// `SmFace::GetEdges(defaults)`: append each loop's unique edges, including repeats
/// between loops ([`crate::topology::face::p4::edges`]).
///
/// # Errors
///
/// Those of [`crate::topology::face::p4::edges`].
pub fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

fn edge_vertices(b: &Brep, e: EdgeId) -> Result<(VertexId, VertexId)> {
    let primary = required(eu::primary_edgeuse(b, e)?)?;
    let first = required(b.edgeuse(primary)?.vertexuse)?;
    let mate = required(eu::mate(b, primary)?)?;
    let second = required(b.edgeuse(mate)?.vertexuse)?;
    Ok((
        required(b.vertexuse(first)?.vertex)?,
        required(b.vertexuse(second)?.vertex)?,
    ))
}

/// Collect unmarked faces across unmarked edges, marking their edges and vertices.
/// A null or marked seed clears only `faces`; otherwise all requested outputs reset.
/// Returns errors for dangling ids, missing links, or a non-progressing `NoMark`.
pub fn collect_faces(
    k: &Kernel,
    b: &Brep,
    marks: &mut impl FaceTraversalMarks,
    start: Option<FaceId>,
    mark: MarkType,
    faces: &mut Vec<FaceId>,
    mut edges: Option<&mut Vec<EdgeId>>,
    mut vertices: Option<&mut Vec<VertexId>>,
) -> Result<()> {
    faces.clear();
    let Some(start) = start else {
        return Ok(());
    };
    b.face(start)?;
    if marks.is_marked(k, Entity::Face(start), mark) {
        return Ok(());
    }
    check_mark(mark)?;
    if let Some(out) = &mut edges {
        out.clear();
    }
    if let Some(out) = &mut vertices {
        out.clear();
    }
    let mut stack = vec![start];
    while let Some(f) = stack.pop() {
        if marks.is_marked(k, Entity::Face(f), mark) {
            continue;
        }
        faces.push(f);
        marks.mark(k, Entity::Face(f), mark);
        for e in face_edges(b, f)? {
            if marks.is_marked(k, Entity::Edge(e), mark) {
                continue;
            }
            if let Some(out) = &mut edges {
                out.push(e);
            }
            marks.mark(k, Entity::Edge(e), mark);
            let (v1, v2) = edge_vertices(b, e)?;
            for v in [v1, v2] {
                if !marks.is_marked(k, Entity::Vertex(v), mark) {
                    if let Some(out) = &mut vertices {
                        out.push(v);
                    }
                }
                marks.mark(k, Entity::Vertex(v), mark);
            }
            for neighbor in edge_faces(b, e)? {
                if !marks.is_marked(k, Entity::Face(neighbor), mark) && !stack.contains(&neighbor) {
                    stack.push(neighbor);
                }
            }
        }
    }
    Ok(())
}

/// Collect connected wire edges in upstream depth-first order. Clears `edges`;
/// appends to optional `vertices` (including when the seed is null).
/// Returns errors for dangling ids, missing links, or `NoMark`.
pub fn collect_wire_edges(
    k: &Kernel,
    b: &Brep,
    marks: &mut Marks,
    start: Option<EdgeId>,
    mark: MarkType,
    edges: &mut Vec<EdgeId>,
    mut vertices: Option<&mut Vec<VertexId>>,
) -> Result<()> {
    enum Work {
        Edge(EdgeId),
        Vertex(VertexId, EdgeId),
    }
    edges.clear();
    let Some(start) = start else {
        return Ok(());
    };
    check_mark(mark)?;
    let mut stack = vec![Work::Edge(start)];
    while let Some(work) = stack.pop() {
        match work {
            Work::Edge(e) => {
                if marks.is_marked(k, Entity::Edge(e), mark) || !vertexuse::edge_is_wire(b, e)? {
                    continue;
                }
                edges.push(e);
                marks.mark(k, Entity::Edge(e), mark);
                let (v1, v2) = edge_vertices(b, e)?;
                stack.push(Work::Vertex(v2, e));
                stack.push(Work::Vertex(v1, e));
            }
            Work::Vertex(v, e) => {
                if marks.is_marked(k, Entity::Vertex(v), mark) || !vertex::is_wire_vertex(b, v)? {
                    continue;
                }
                if let Some(out) = &mut vertices {
                    out.push(v);
                }
                marks.mark(k, Entity::Vertex(v), mark);
                for neighbor in vertex::edges(b, v, None)?.into_iter().rev() {
                    if neighbor != e {
                        stack.push(Work::Edge(neighbor));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Traverse radial face-use connections. The mark is not incremented.
/// Without `faceuses`, stop before marking the first mate of the seed.
/// Returns whether the shell is closed, or a missing-link/id/`NoMark` error.
pub fn faceuse_traversal(
    k: &Kernel,
    b: &Brep,
    marks: &mut Marks,
    start: FaceuseId,
    mark: MarkType,
    mut faceuses: Option<&mut Vec<FaceuseId>>,
) -> Result<bool> {
    check_mark(mark)?;
    if let Some(out) = &mut faceuses {
        out.clear();
    }
    let mut closed = true;
    let mut stack = vec![start];
    while let Some(fu) = stack.pop() {
        if marks.is_marked(k, Entity::Faceuse(fu), mark) {
            continue;
        }
        if b.faceuse(fu)?.mate == Some(start) {
            closed = false;
            if faceuses.is_none() {
                return Ok(false);
            }
        }
        if let Some(out) = &mut faceuses {
            out.push(fu);
        }
        marks.mark(k, Entity::Faceuse(fu), mark);
        for &lu in &b.faceuse(fu)?.loopuses {
            for edgeuse in eu::loopuse_edgeuses(b, lu)? {
                let radial = required(eu::radial(b, edgeuse)?)?;
                let neighbor = required(eu::faceuse(b, radial)?)?;
                if !marks.is_marked(k, Entity::Faceuse(neighbor), mark) {
                    stack.push(neighbor);
                }
            }
        }
    }
    Ok(closed)
}

/// `SmFace::HasLaminaEdge` (CO-PORTED from `topology.face_p3`): whether any edge of the
/// face is lamina.
///
/// # Errors
///
/// Those of [`face_edges`] and the lamina test.
pub fn has_lamina_edge(b: &Brep, f: FaceId) -> Result<bool> {
    for e in face_edges(b, f)? {
        if eu::edge_is_lamina(b, e)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Check lamina edges before advancing the mark and traversing the face uses.
/// Returns false immediately for a lamina edge, without advancing the mark.
/// Errors propagate from topology access and the mark lock.
pub fn closure_traversal(
    k: &mut Kernel,
    b: &Brep,
    marks: &mut Marks,
    start: FaceuseId,
    lock: &MarkLock,
    mut faceuses: Option<&mut Vec<FaceuseId>>,
) -> Result<bool> {
    if let Some(out) = &mut faceuses {
        out.clear();
    }
    let f = required(b.faceuse(start)?.face)?;
    if has_lamina_edge(b, f)? {
        return Ok(false);
    }
    lock.new_mark(std::slice::from_mut(&mut k.context))?;
    faceuse_traversal(k, b, marks, start, lock.mark_type(), faceuses)
}

/// Transfer a shell's children to its replacement (`RemoveShell`: `SER` on
/// `SmShell::TransferChildrenTo` with no wire-edge list).
///
/// # Errors
/// Those of [`shell::transfer_children`].
pub fn remove_shell(k: &mut Kernel, b: &mut Brep, remove: ShellId, replace: ShellId) -> Result<()> {
    shell::transfer_children(k, b, remove, replace, None)
}

/// Traverse connected face uses, then move each to `new_shell` in visit order
/// (`Remove` / `PostInsert`, statuses ignored upstream; [`shell::move_faceuse`]).
pub fn faceuse_shell_replacement(
    k: &mut Kernel,
    b: &mut Brep,
    marks: &mut Marks,
    start: FaceuseId,
    new_shell: ShellId,
    replaced: &mut Vec<FaceuseId>,
    lock: &MarkLock,
) -> Result<()> {
    lock.new_mark(std::slice::from_mut(&mut k.context))?;
    faceuse_traversal(k, b, marks, start, lock.mark_type(), Some(replaced))?;
    for &fu in replaced.iter() {
        if b.faceuse(fu)?.shell != Some(new_shell) {
            shell::move_faceuse(b, fu, new_shell)?;
        }
    }
    Ok(())
}

fn shell_connectivity(
    k: &Kernel,
    b: &Brep,
    marks: &mut (impl FaceTraversalMarks + ?Sized),
    start: VertexuseId,
    shell: ShellId,
    mark: MarkType,
    edgeuses: &mut Option<&mut Vec<EdgeuseId>>,
    faceuses: &mut Option<&mut Vec<FaceuseId>>,
) -> Result<()> {
    marks.mark(k, Entity::Vertexuse(start), mark);
    let mut stack = vec![start];
    while let Some(vu) = stack.pop() {
        let mut propagate = Vec::new();
        let fu = if vertexuse::is_edge_vertexuse(b, vu)? {
            let e = required(vertexuse::edgeuse(b, vu)?)?;
            if let Some(out) = edgeuses {
                out.push(e);
            }
            let mate = required(eu::mate(b, e)?)?;
            if eu::shell(b, mate)? == Some(shell) {
                let mate_vu = required(b.edgeuse(mate)?.vertexuse)?;
                if !marks.is_marked(k, Entity::Vertexuse(mate_vu), mark) {
                    marks.mark(k, Entity::Vertexuse(mate_vu), mark);
                    stack.push(mate_vu);
                }
            }
            if eu::is_loop_edgeuse(b, e)? {
                vertexuse::faceuse(b, vu)?
            } else {
                None
            }
        } else if vertexuse::is_loop_vertexuse(b, vu)? {
            vertexuse::faceuse(b, vu)?
        } else {
            None
        };
        if let Some(fu) = fu {
            if !marks.is_marked(k, Entity::Faceuse(fu), mark) {
                marks.mark(k, Entity::Faceuse(fu), mark);
                if let Some(out) = faceuses {
                    out.push(fu);
                }
            }
            let f = required(b.faceuse(fu)?.face)?;
            if !marks.is_marked(k, Entity::Face(f), mark) {
                marks.mark(k, Entity::Face(f), mark);
                propagate.push(fu);
            }
        }
        let v = required(b.vertexuse(vu)?.vertex)?;
        if !marks.is_marked(k, Entity::Vertex(v), mark) {
            marks.mark(k, Entity::Vertex(v), mark);
            for next in vertex::vertexuses(b, v)? {
                if !marks.is_marked(k, Entity::Vertexuse(next), mark)
                    && vertexuse::shell(b, next)? == Some(shell)
                {
                    marks.mark(k, Entity::Vertexuse(next), mark);
                    stack.push(next);
                }
            }
        }
        for fu in propagate {
            for &lu in &b.faceuse(fu)?.loopuses {
                for next in face::loopuse_vertexuses(b, lu)? {
                    let next = required(next)?;
                    if !marks.is_marked(k, Entity::Vertexuse(next), mark)
                        && vertexuse::shell(b, next)? == Some(shell)
                    {
                        marks.mark(k, Entity::Vertexuse(next), mark);
                        stack.push(next);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Advance the mark and traverse shell connectivity through the first vertex use
/// attached to `shell`. Clears optional outputs even when no seed use matches.
/// Errors propagate from topology access or the mark lock.
pub fn shell_traversal(
    k: &mut Kernel,
    b: &Brep,
    marks: &mut Marks,
    start: VertexId,
    shell: ShellId,
    edgeuses: Option<&mut Vec<EdgeuseId>>,
    faceuses: Option<&mut Vec<FaceuseId>>,
    lock: &MarkLock,
) -> Result<()> {
    shell_traversal_with_marks(k, b, marks, start, shell, edgeuses, faceuses, lock)
}

/// Traverse shell connectivity through borrowed retained topology marks.
///
/// # Errors
/// Those of [`shell_traversal`]; successful mark writes remain on failure.
#[allow(clippy::too_many_arguments)]
pub fn shell_traversal_with_marks(
    k: &mut Kernel,
    b: &Brep,
    marks: &mut (impl FaceTraversalMarks + ?Sized),
    start: VertexId,
    shell: ShellId,
    mut edgeuses: Option<&mut Vec<EdgeuseId>>,
    mut faceuses: Option<&mut Vec<FaceuseId>>,
    lock: &MarkLock,
) -> Result<()> {
    if let Some(out) = &mut edgeuses {
        out.clear();
    }
    if let Some(out) = &mut faceuses {
        out.clear();
    }
    lock.new_mark(std::slice::from_mut(&mut k.context))?;
    check_mark(lock.mark_type())?;
    for vu in vertex::vertexuses(b, start)? {
        if vertexuse::shell(b, vu)? == Some(shell) {
            shell_connectivity(
                k,
                b,
                marks,
                vu,
                shell,
                lock.mark_type(),
                &mut edgeuses,
                &mut faceuses,
            )?;
            break;
        }
    }
    Ok(())
}

/// The surface of a ray's target face as the solutions' object (upstream's surface
/// pointer), answering its natural domain.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RaySurface(crate::geom::extent2d::Extent2d);

impl crate::geom::solution_array::SolutionObject for RaySurface {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        crate::geom::solution_array::ObjectKind::Surface
    }
    fn natural_uv_domain(&self) -> Option<crate::geom::extent2d::Extent2d> {
        Some(self.0)
    }
}

/// `pTargetFace->GlobalLineIntersect(GetUVDomain(), origin, direction, NULL, FALSE,
/// GetTolerance(), sols)`: the line parameter of each solution. `held` is the face
/// surface's cache, kept across the rays as upstream keeps it on the surface.
fn line_parameters(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    held: &mut HeldSolveCache<RaySurface>,
    origin: Vec3,
    direction: Vec3,
) -> Result<Vec<f64>> {
    let face = b.face(f)?;
    let (uv_domain, tol) = (face.uv_domain, face.tolerance);
    let surface = RaySurface(
        face.surface
            .as_ref()
            .ok_or_else(|| SolidError::invalid_input(OP, "ray target face without surface"))?
            .natural_uv_domain_with_nurb(k)?,
    );
    let solutions = crate::topology::face::p4::global_line_intersect_held(
        k, b, f, surface, held, &uv_domain, origin, direction, None, false, tol,
    )?;
    Ok(solutions
        .as_slice()
        .iter()
        .map(|s| s.start.parameters[0])
        .collect())
}

/// Classify a closed shell by parity of forward and backward ray intersections.
/// Errors propagate from face sampling and the ray/face intersections (whose other
/// failures skip the face, as upstream).
/// An empty or inconclusive set returns the upstream `SM_ERR`.
///
/// The rays and face samples run on `b` itself, so the UV trim curves their face
/// classifications create stay on the B-rep, as upstream's `const` queries leave them.
pub fn is_inner_shell(k: &mut Kernel, b: &mut Brep, faceuses: &[FaceuseId]) -> Result<bool> {
    if faceuses.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    let mut samples = 3;
    if faceuses.len() < 6 {
        samples *= 2;
    }
    if faceuses.len() < 3 {
        samples *= 2;
    }
    let mut forward = 0_u64;
    let mut backward = 0_u64;
    // each target face surface's cache, built on its first ray (upstream's
    // smsurf_GetSurfaceCache keeps one per surface; the rays do not change surfaces)
    let mut caches: BTreeMap<FaceId, HeldSolveCache<RaySurface>> = BTreeMap::new();
    for &source in faceuses {
        let source = b.faceuse(source)?;
        let (f, orientation) = (required(source.face)?, source.orientation);
        let mut points = face::points_in_face(k, b, f, samples)?.pv;
        if orientation == OrientType::Opposite {
            for pair in points.chunks_exact_mut(2) {
                if let Some(normal) = pair.get_mut(1) {
                    *normal *= -1.0;
                }
            }
        }
        for pair in points.chunks_exact(2) {
            let (Some(&origin), Some(&direction)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            forward = 0;
            backward = 0;
            for &target in faceuses {
                let f = required(b.faceuse(target)?.face)?;
                let tol = b.face(f)?.tolerance;
                let held = caches.entry(f).or_default();
                match line_parameters(k, b, f, held, origin, direction) {
                    Ok(parameters) => {
                        for t in parameters {
                            if t < -tol {
                                backward += 1;
                            } else if t > tol {
                                forward += 1;
                            }
                        }
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => (),
                }
            }
            if forward == 1 && backward == 0 {
                return Ok(true);
            }
            if forward == 0 && backward == 1 {
                return Ok(false);
            }
            if forward % 2 != 0 && backward % 2 == 0 {
                return Ok(true);
            }
            if forward % 2 == 0 && backward % 2 != 0 {
                return Ok(false);
            }
        }
    }
    if forward > backward && backward == 0 && forward % 2 != 0 {
        return Ok(true);
    }
    if backward > forward && forward == 0 && backward % 2 != 0 {
        return Ok(false);
    }
    Err(SolidError::sm_err(OP))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::context::MarkType;
    use crate::topology::{Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};

    #[test]
    fn face_edges_deduplicates_within_each_loop_but_retains_cross_loop_repeats() {
        use crate::topology::{Faceuse, Loopuse, LoopuseStart};
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let face = b.add_face(shell).unwrap();
        let fu = b
            .add_faceuse_record(Faceuse {
                face: Some(face),
                ..Faceuse::default()
            })
            .unwrap();
        b.face_mut(face).unwrap().faceuse = Some(fu);
        let v = b.add_vertex(Vec3::ZERO).unwrap();
        let edge = b.add_edge(v, v).unwrap();
        for _ in 0..2 {
            let lu = b
                .add_loopuse_record(Loopuse {
                    faceuse: Some(fu),
                    ..Loopuse::default()
                })
                .unwrap();
            let a = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    owner: EdgeuseOwner::Loopuse(Some(lu)),
                    ..Edgeuse::default()
                })
                .unwrap();
            let z = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    owner: EdgeuseOwner::Loopuse(Some(lu)),
                    ccw: Some(a),
                    cw: Some(a),
                    ..Edgeuse::default()
                })
                .unwrap();
            b.edgeuse_mut(a).unwrap().ccw = Some(z);
            b.edgeuse_mut(a).unwrap().cw = Some(z);
            b.loopuse_mut(lu).unwrap().start = LoopuseStart::Edgeuse(Some(a));
            b.faceuse_mut(fu).unwrap().loopuses.push(lu);
        }
        assert_eq!(face_edges(&b, face).unwrap(), [edge, edge]);
    }

    #[test]
    fn long_wire_chain_uses_no_recursion_and_keeps_depth_first_order() {
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let mut previous = b.add_vertex(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let n = 6000;
        for i in 0..n {
            let next = b.add_vertex(Vec3::new(f64::from(i + 1), 0.0, 0.0)).unwrap();
            let e = b.add_edge(previous, next).unwrap();
            for (v, orientation) in [(previous, OrientType::Same), (next, OrientType::Opposite)] {
                let eu = b
                    .add_edgeuse_record(Edgeuse {
                        edge: Some(e),
                        owner: EdgeuseOwner::Shell(Some(shell)),
                        orientation,
                        ..Edgeuse::default()
                    })
                    .unwrap();
                let vu = b
                    .add_vertexuse_record(Vertexuse {
                        vertex: Some(v),
                        owner: VertexuseOwner::Edgeuse(Some(eu)),
                        ..Vertexuse::default()
                    })
                    .unwrap();
                b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
                b.vertex_mut(v).unwrap().vertexuses.push(vu);
                b.edge_mut(e).unwrap().edgeuses.push(eu);
            }
            previous = next;
        }
        let mut k = Kernel::default();
        let lock = MarkLock::new(
            std::slice::from_mut(&mut k.context),
            0,
            MarkType::Mark2.code(),
        )
        .unwrap();
        let mut marks = Marks::default();
        let mut edges = Vec::new();
        let mut vertices = Vec::new();
        collect_wire_edges(
            &k,
            &b,
            &mut marks,
            Some(EdgeId::from_index(0)),
            lock.mark_type(),
            &mut edges,
            Some(&mut vertices),
        )
        .unwrap();
        assert_eq!(edges.len(), n as usize);
        assert_eq!(vertices.len(), n as usize + 1);
        assert!(edges
            .iter()
            .enumerate()
            .all(|(i, e)| e.index() as usize == i));
        assert!(vertices
            .iter()
            .enumerate()
            .all(|(i, v)| v.index() as usize == i));
    }

    #[test]
    fn no_mark_is_rejected_and_dangling_ids_do_not_panic() {
        let k = Kernel::default();
        let b = Brep::new();
        let mut marks = Marks::default();
        assert!(collect_wire_edges(
            &k,
            &b,
            &mut marks,
            Some(EdgeId::from_index(u32::MAX)),
            MarkType::NoMark,
            &mut Vec::new(),
            None
        )
        .is_err());
        assert!(collect_faces(
            &k,
            &b,
            &mut marks,
            Some(FaceId::from_index(u32::MAX)),
            MarkType::Mark2,
            &mut Vec::new(),
            None,
            None
        )
        .is_err());
    }
}

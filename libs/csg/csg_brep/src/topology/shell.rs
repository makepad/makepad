//! Shell and region topology, with typed arena links and native traversal order.
//!
//! Upstream: `source/SMLib/inc/SmShell.h`, `source/SMLib/src/SmShell.cpp`,
//! `source/SMLib/inc/SmRegion.h`, `source/SMLib/src/SmRegion.cpp`.
//! The `SmEdge`/`SmVertex` callees of wire-edge traversal are called through their
//! owners' ports (`topology::edge`, `topology::vertex`), not re-ported here;
//! `source/SMLib/inc/SmTopology.h` supplies literal sibling links and
//! `source/SMLib/inc/SmObject.h` defines the checked `SM_CAST_PTR` semantics.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`ShellType`], [`ShellMembers`] | `m_tShellType`, typed `m_pList` contents |
//! | [`Shell::new`], [`Region::new`], implicit `Drop` | constructors, destructors |
//! | [`Shell::type_string`], [`Shell::class_string`], [`Shell::is_kind_of`], [`Region::type_string`], [`Region::class_string`], [`Region::is_kind_of`] | `SM_COMMON` names and full base-class chain |
//! | [`Shell::get_faceuses`], [`Shell::first_faceuse`] | `GetFaceuses`, `GetFirstFaceuse` |
//! | [`Shell::wire_edge`], [`Shell::vertex`] | `GetWireEdge`, `GetVertex` |
//! | [`Shell::is_closed`], [`Shell::mated_shells`] | `IsClosed`, `GetMatedShells` |
//! | [`shell_region`], [`shell_brep`] | `GetRegion`, `GetBrep` |
//! | [`RegionLookup`], [`RegionRef`] | caller-supplied resolution of implicit native region pointers |
//! | [`wire_edges`] | `SmShell::GetWireEdges` |
//! | [`ray_fire`] | `SmShell::RayFire` (partial output state retained) |
//! | [`transfer_children`] | `SmShell::TransferChildrenTo` |
//! | [`move_faceuse`] | `SmOwningTopology::Remove` + `PostInsert` of a face use (typed list) |
//! | [`post_insert_faceuse`] | `SmOwningTopology::PostInsert` of a new face use (typed list) |
//! | [`shell_is_degenerate`], [`region_is_degenerate`] | `IsDegenerate` (native default tolerance) |
//! | `shell_bounds_for_degeneracy`, `region_bounds_for_degeneracy` | default-tight `CalculateBoundingBox` (`query::face::p1`), status kept beside the partial box |
//! | [`faceuse_sibling_links`] | typed `SmTopology::GetNext` / `GetLast` for face uses |
//! | `topology::edge::{vertices, edgeuses, is_wire}`, `topology::vertex::edges` | `SmEdge::GetVertices`, `GetEdgeuses`, `IsWire`, `SmVertex::GetEdges` (callees, no filters) |
//! | [`Region::get_shells`], [`Region::is_void`], [`Region::set_is_void`] | `GetShells`, `IsVoid`, `SetIsVoid` |
//! | [`Region::is_infinite_region`] | `IsInfiniteRegion` |
//! | [`Shell::is_inner_shell`], [`order_region_shells`] | `IsInnerShell`, `OrderShells` |
//! | [`region_remove_shell`] | `SmOwningTopology::Remove` of a shell from its region |
//! | [`region_post_insert_shell`] | `SmOwningTopology::PostInsert` of a shell into a region (CO-PORTED, `topology.brep_p3`) |
//! | [`shell_remove_faceuse`], [`shell_post_insert_faceuse`] | `SmOwningTopology::Remove` / `PostInsert` of one face use (CO-PORTED, `topology.brep_p3`) |
//! | [`shell_list_size`] | `SmShell::GetSize` (`m_lListSize`; CO-PORTED, `topology.brep_p3`) |
//! | [`is_inner_shell_of`], [`SM_TRUE`], [`SM_FALSE`], [`SM_UNSURE`] | `SmShell::IsInnerShellOf` (its `SmBoolean`) |
//!
//! Deviations: circular lists are typed vectors in head-first order, as in the
//! shared Brep arena model. `GetWireEdges`, `GetMatedShells` and `RayFire` claim and
//! lock a context mark (`SmNewMarkAndLock`) and mark through
//! [`crate::topology::topology_traverser::Marks`] (per-call, as the mark values on the
//! objects are unique per claim), advancing the kernel context's mark counters as upstream. Incompatible array reinterpret casts return `InvalidInput`;
//! checked member casts retain native NULL. Native
//! notification/logging side effects are not represented by these arena records.
//! Face-use sibling links survive a cleared owner head, as in native self-transfer.
//! `SmOwningTopology::Remove` / `PostInsert` of a face use are CO-PORTED on the
//! typed face-use list (`move_faceuse`; unit `topology.proto_topology_p1` works on
//! object-key records). `SmBrep::GetEdges` (unit `topology.brep_p1`) is its owner's
//! port `topology::brep::p1::edges` (the `m_pEdgeListHead` list order), which replaced
//! the earlier co-port.
//! `TransferChildrenTo` ignores the status of `Remove`/`PostInsert` (`SE(...)`) and goes on;
//! here a face use whose owner is not the source (or a `Remove` that cannot find it)
//! returns `InvalidInput` instead. That state needs an inconsistent graph (a face use
//! listed in a shell that its back link does not name); native code would corrupt
//! the lists. Face-use sibling links, once `Some` (`Faceuse::sibling_links`), win
//! over `Shell::members`: code that edits `members` directly (e.g. a later unit that
//! rebuilds face-use lists) must keep them in step or reset them to `None`.
//! There is no recursion. Missing external callees return explicit `Unported` errors.
//! `RayFire` reaches the B-rep's geometry through a [`ShellRayWorld`] (upstream: the
//! faces' own pointers); the B-rep's is [`crate::query::brep::BrepQuery`]. Its view
//! borrows the B-rep for the shape solve only, since the sector step after it may add
//! UV trim curves.
//! The production B-rep world keeps one solve in flight, with immutable evaluation
//! geometry and live topology services for trims, caches and tolerance reads. A
//! read-only world's default adapter may instead request preparation and restart;
//! each edge-use or face request is attempted at most once.
//! The single-member `Remove` / `PostInsert` co-ports (`topology.brep_p3`, PR #204)
//! return upstream's `SM_ERR` of `Remove` (a member not in this list) as
//! [`SolidError::sm_err`], which callers that ignore it can tell apart. `PostInsert`
//! does no checks upstream; here it returns `InvalidInput` for a member that some list
//! still holds (upstream would splice it into a second list), or for a face use
//! going into a shell whose head is a wire or vertex use.

use crate::error::{Result, SolidError};
use crate::geom::context::{with_mark_lock, ALL_MARKS};
use crate::geom::object::Shape;
use crate::kernel::Kernel;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::topology::proto_topology::p1::owning_topology;
use crate::topology::topology_traverser::{Entity, Marks};
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceuseId, Region, RegionId, Shell, ShellId, VertexId, VertexuseId,
};

/// The native shell member class, independently stored from its list head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellType {
    /// `SmUnknown_TYPE` (16999), the constructor value.
    #[default]
    Unknown,
    /// `SmVertexuse_TYPE` (16006).
    Vertex,
    /// `SmEdgeuse_TYPE` (16005).
    Wire,
    /// `SmFaceuse_TYPE` (16022).
    Faceuse,
    /// Other values preserved by `SetShellType` without reinterpretation.
    Other(i64),
}

impl ShellType {
    /// The exact native type code.
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::Unknown => 16999,
            Self::Vertex => 16006,
            Self::Wire => 16005,
            Self::Faceuse => 16022,
            Self::Other(code) => code,
        }
    }
    /// Retain an arbitrary native caller's shell type.
    #[must_use]
    pub const fn from_code(code: i64) -> Self {
        match code {
            16999 => Self::Unknown,
            16006 => Self::Vertex,
            16005 => Self::Wire,
            16022 => Self::Faceuse,
            other => Self::Other(other),
        }
    }
}

/// A checked representation of the shell's native list head and members.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShellMembers {
    /// NULL list head.
    #[default]
    Empty,
    /// Face-use circular list, from its current head.
    Faceuses(Vec<FaceuseId>),
    /// First wire edge use (the edge owns its radial list).
    Edgeuse(EdgeuseId),
    /// Vertex use of a vertex shell.
    Vertexuse(VertexuseId),
}

const fn invalid_members() -> SolidError {
    SolidError::InvalidInput {
        operation: "topology::shell",
        reason: "shell type and native member class disagree",
    }
}

impl Shell {
    /// Empty, unknown shell (`SmShell()`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Native class type (`SmShell_TYPE`).
    #[must_use]
    pub const fn type_code() -> i64 {
        16013
    }
    /// Native class name.
    #[must_use]
    pub const fn class_string() -> &'static str {
        "SmShell"
    }
    /// Native type name.
    #[must_use]
    pub const fn type_string() -> &'static str {
        "SmShell_TYPE"
    }
    /// Native type/base-class membership, using the shared owning-topology port.
    #[must_use]
    pub const fn is_kind_of(code: i64) -> bool {
        code == Self::type_code() || owning_topology::is_kind_of(code)
    }
    /// Whether this is a vertex-use shell.
    #[must_use]
    pub const fn is_vertex_shell(&self) -> bool {
        self.shell_type.code() == 16006
    }
    /// Whether this is a wire-edge shell.
    #[must_use]
    pub const fn is_wire_shell(&self) -> bool {
        self.shell_type.code() == 16005
    }
    /// Whether this contains face uses.
    #[must_use]
    pub const fn is_faceuse_shell(&self) -> bool {
        self.shell_type.code() == 16022
    }
    /// Whether its type is the native unknown value.
    #[must_use]
    pub const fn is_unknown_shell(&self) -> bool {
        self.shell_type.code() == 16999
    }
    /// Change only the native type field, retaining its existing list head.
    pub fn set_shell_type(&mut self, code: i64) {
        self.shell_type = ShellType::from_code(code);
    }
    /// Face uses in native list order; empty for every other shell type.
    ///
    /// # Errors
    /// A native list head of an incompatible class.
    pub fn get_faceuses(&self) -> Result<Vec<FaceuseId>> {
        if !self.is_faceuse_shell() {
            return Ok(Vec::new());
        }
        match &self.members {
            ShellMembers::Empty => Ok(Vec::new()),
            ShellMembers::Faceuses(ids) => Ok(ids.clone()),
            _ => Err(invalid_members()),
        }
    }
    /// First face use; native logging-only errors retain the NULL result.
    ///
    /// A head-class mismatch retains native NULL rather than becoming an error.
    ///
    /// # Errors
    /// This compatibility result succeeds; native logging-only errors yield `None`.
    pub fn first_faceuse(&self) -> Result<Option<FaceuseId>> {
        if !self.is_faceuse_shell() {
            return Ok(None);
        }
        // SM_CAST_PTR checks the actual head class and returns NULL on mismatch.
        // Unlike the reinterpret-cast GetFaceuses array, this is a defined path.
        Ok(match &self.members {
            ShellMembers::Faceuses(ids) => ids.first().copied(),
            _ => None,
        })
    }
    /// First wire edge, preserving NULL for empty or other-kind shells.
    ///
    /// # Errors
    /// Dangling arena references; a head-class mismatch retains native NULL.
    pub fn wire_edge(&self, brep: &Brep) -> Result<Option<EdgeId>> {
        if !self.is_wire_shell() {
            return Ok(None);
        }
        match self.members {
            ShellMembers::Edgeuse(id) => Ok(brep.edgeuse(id)?.edge),
            _ => Ok(None),
        }
    }
    /// Vertex of a vertex shell, preserving NULL for empty or other-kind shells.
    ///
    /// # Errors
    /// Dangling arena references; a head-class mismatch retains native NULL.
    pub fn vertex(&self, brep: &Brep) -> Result<Option<VertexId>> {
        if !self.is_vertex_shell() {
            return Ok(None);
        }
        match self.members {
            ShellMembers::Vertexuse(id) => Ok(brep.vertexuse(id)?.vertex),
            _ => Ok(None),
        }
    }
    /// Whether a face in this shell has a mate belonging to a different shell.
    ///
    /// # Errors
    /// Invalid list classes or dangling face-use/mate references.
    pub fn is_closed(&self, brep: &Brep, id: ShellId) -> Result<bool> {
        if !self.is_faceuse_shell() {
            return Ok(false);
        }
        for faceuse in self.get_faceuses()? {
            if let Some(mate) = brep.faceuse(faceuse)?.mate {
                if brep.faceuse(mate)?.shell != Some(id) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// Distinct mate shells in first-seen face-use order. Like `SmShell::GetMatedShells`
    /// this claims and locks a context mark (`SmNewMarkAndLock`, all marks) for the
    /// duration of the call, so the context's mark counters advance.
    ///
    /// # Errors
    /// Missing native mates/owners or dangling arena references; every mark is still
    /// released.
    pub fn mated_shells(&self, kernel: &mut Kernel, brep: &Brep) -> Result<Vec<ShellId>> {
        let mut output = Vec::new();
        if !self.is_faceuse_shell() {
            return Ok(output);
        }
        with_mark_lock(kernel, ALL_MARKS, |kernel, mark| {
            let mut marks = Marks::default();
            for faceuse in self.get_faceuses()? {
                let mate = brep.faceuse(faceuse)?.mate.ok_or(invalid_members())?;
                let shell = brep.faceuse(mate)?.shell.ok_or(invalid_members())?;
                brep.shell(shell)?;
                if marks.is_marked(kernel, Entity::Shell(shell), mark) {
                    continue;
                }
                marks.mark(kernel, Entity::Shell(shell), mark);
                output.push(shell);
            }
            Ok(())
        })??;
        Ok(output)
    }
    /// Classify whether shell `id` of `brep` is an inner shell:
    /// `SmTopologyTraverser::IsInnerShell` on its face uses (`GetFaceuses`), on `brep`
    /// itself (the UV trim curves its face queries make stay, as upstream's).
    ///
    /// # Errors
    /// A dangling id, invalid member classes and the errors of
    /// [`crate::topology::topology_traverser::is_inner_shell`] (its `SM_ERR` for an
    /// empty or inconclusive set, `Unported` from its ray intersections).
    pub fn is_inner_shell(kernel: &mut Kernel, brep: &mut Brep, id: ShellId) -> Result<bool> {
        let faceuses = brep.shell(id)?.get_faceuses()?;
        crate::topology::topology_traverser::is_inner_shell(kernel, brep, &faceuses)
    }
}

/// Owning region (`SmShell::GetRegion`).
///
/// # Errors
/// Dangling shell id.
pub fn shell_region(brep: &Brep, id: ShellId) -> Result<Option<RegionId>> {
    Ok(brep.shell(id)?.region)
}

/// B-rep of a shell's owning region (`SmShell::GetBrep`).
///
/// # Errors
/// Dangling shell or region id.
pub fn shell_brep(brep: &Brep, id: ShellId) -> Result<Option<&Brep>> {
    match shell_region(brep, id)? {
        None => Ok(None),
        Some(region) => Ok(brep.region(region)?.in_brep.then_some(brep)),
    }
}

impl Region {
    /// Empty non-void region (`SmRegion()`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Native region type.
    #[must_use]
    pub const fn type_code() -> i64 {
        16012
    }
    /// Native class name.
    #[must_use]
    pub const fn class_string() -> &'static str {
        "SmRegion"
    }
    /// Native type name.
    #[must_use]
    pub const fn type_string() -> &'static str {
        "SmRegion_TYPE"
    }
    /// Native type/base-class membership.
    #[must_use]
    pub const fn is_kind_of(code: i64) -> bool {
        code == Self::type_code() || owning_topology::is_kind_of(code)
    }
    /// Native void state, preserving `FALSE` (0), `TRUE` (1), and `UNSURE` (2).
    #[must_use]
    pub const fn is_void(&self) -> i32 {
        self.is_void
    }
    /// Set the native void flag.
    pub fn set_is_void(&mut self, value: i32) {
        self.is_void = value;
    }
    /// Owning B-rep, or NULL when the region is detached (`GetBrep`).
    #[must_use]
    pub const fn get_brep<'a>(&self, brep: &'a Brep) -> Option<&'a Brep> {
        if self.in_brep {
            Some(brep)
        } else {
            None
        }
    }

    /// Whether this is the owning B-rep's designated infinite region.
    #[must_use]
    pub fn is_infinite_region(&self, brep: &Brep, id: RegionId) -> bool {
        self.in_brep && brep.infinite_region() == Some(id)
    }
    /// Region shell list, retaining optional attribute filtering via the caller's
    /// actual attribute lookup rather than dropping or defaulting that parameter.
    ///
    /// # Errors
    /// Those of the attribute lookup.
    pub fn get_shells<F>(
        &self,
        attribute_id: Option<u64>,
        mut has_attribute: F,
    ) -> Result<Vec<ShellId>>
    where
        F: FnMut(ShellId, u64) -> Result<bool>,
    {
        let mut output = Vec::new();
        for &shell in &self.shells {
            if let Some(attribute) = attribute_id {
                if !has_attribute(shell, attribute)? {
                    continue;
                }
            }
            output.push(shell);
        }
        Ok(output)
    }
}

/// Select the first shell according to `SmRegion::OrderShells`, rotating the
/// circular head while preserving every other link's native relative order.
///
/// # Errors
/// Invalid arena references or an unported inner-shell classification.
pub fn order_region_shells(kernel: &mut Kernel, brep: &mut Brep, id: RegionId) -> Result<()> {
    let shells = brep.region(id)?.shells.clone();
    if shells.len() < 2 {
        return Ok(());
    }
    let first = shells.first().copied().ok_or(invalid_members())?;
    if brep.shell(first)?.is_faceuse_shell() {
        return Ok(());
    }
    let mut faceuse_shells = Vec::new();
    for &shell in &shells {
        if brep.shell(shell)?.is_faceuse_shell() {
            faceuse_shells.push(shell);
        }
    }
    let mut chosen = faceuse_shells.first().copied();
    if faceuse_shells.len() > 1 {
        for &shell in &faceuse_shells {
            if brep.shell(shell)?.is_closed(brep, shell)? {
                match Shell::is_inner_shell(kernel, brep, shell) {
                    Ok(true) => {
                        chosen = Some(shell);
                        break;
                    }
                    Err(error) if error.is_unported() => return Err(error),
                    Ok(false) | Err(_) => {}
                }
            }
        }
    }
    if let Some(chosen) = chosen {
        let position = shells
            .iter()
            .position(|&shell| shell == chosen)
            .ok_or(invalid_members())?;
        brep.region_mut(id)?.shells.rotate_left(position);
    }
    Ok(())
}

/// Wire edges of a shell, preserving native stack and first-seen order. A wire
/// shell claims and locks a context mark (`SmNewMarkAndLock`, all marks) for its
/// traversal, so the context's mark counters advance as upstream's do.
///
/// # Errors
/// Dangling topology links, native undefined pointer accesses, or the missing
/// vertex edge-list traversal; the mark is released on every path. There is no
/// recursion.
pub fn wire_edges(kernel: &mut Kernel, brep: &Brep, id: ShellId) -> Result<Vec<EdgeId>> {
    const WIRE_OP: &str = "topology::shell::wire_edges";
    let shell = brep.shell(id)?;
    let mut output = Vec::new();
    if shell.is_vertex_shell() {
        return Ok(output);
    }
    if shell.is_wire_shell() {
        with_mark_lock(kernel, ALL_MARKS, |kernel, mark| {
            let first = shell.wire_edge(brep)?.ok_or(invalid_members())?;
            let mut marks = Marks::default();
            let mut stack = vec![first];
            while let Some(edge) = stack.pop() {
                for vertex in crate::topology::edge::vertices(brep, edge, None)? {
                    // Upstream marks every listed vertex: a NULL start vertex is a
                    // NULL dereference.
                    let vertex = vertex.ok_or_else(|| SolidError::null_pointer(WIRE_OP))?;
                    if marks.is_marked(kernel, Entity::Vertex(vertex), mark) {
                        continue;
                    }
                    marks.mark(kernel, Entity::Vertex(vertex), mark);
                    for adjacent in crate::topology::vertex::edges(brep, vertex, None)? {
                        if marks.is_marked(kernel, Entity::Edge(adjacent), mark) {
                            continue;
                        }
                        marks.mark(kernel, Entity::Edge(adjacent), mark);
                        output.push(adjacent);
                        stack.push(adjacent);
                    }
                }
            }
            Ok(())
        })??;
        return Ok(output);
    }
    // GetBrep is required even when the edge list happens to be empty.
    let owner = shell_brep(brep, id)?.ok_or(invalid_members())?;
    // SmBrep::GetEdges (unit topology.brep_p1, which replaced the co-port)
    for edge in crate::topology::brep::p1::edges(owner, None)? {
        if crate::topology::edge::is_wire(owner, edge)? {
            let primary = crate::topology::edgeuse::p1::primary_edgeuse(owner, edge)?
                .ok_or(invalid_members())?;
            if owner.edgeuse(primary)?.owner == crate::topology::EdgeuseOwner::Shell(Some(id)) {
                output.push(edge);
            }
        }
    }
    Ok(output)
}

/// `delete pShell`: `~SmShell` notifies `SM_NO_DESTRUCTION` (`Notify(op, this, NULL,
/// NULL)`, which `SmShell` passes to `SmTopology::Notify`, through the shell's context,
/// `GetContext()`: the environment's, else `k`'s), whose status it ignores
/// ([`SolidError::Unported`] propagates); `~SmAObject` (`ClearAttributes`) removes it
/// from every attribute it uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; the notification's
/// [`SolidError::Unported`]; those of `ClearAttributes`.
pub fn destroy(
    k: &mut Kernel,
    brep: &mut Brep,
    id: ShellId,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<()> {
    brep.shell(id)?;
    let this = (env.key_of)(crate::error::EntityRef::Shell(id));
    crate::topology::edge::with_object_context(k, env, |n| {
        crate::error::ignore_status(crate::topology::topology::notify(
            &mut *n.records,
            &mut *n.attributes,
            n.context.as_deref_mut(),
            crate::geom::object::NotifyOperation::Destruction,
            this,
            Some(this),
            None,
            None,
        ))
    })?;
    brep.remove_shell_record(id)?;
    env.attributes.clear_attributes(this.key)
}

/// `pRegion->Remove(pShell)` (`SmOwningTopology::Remove`, inline in
/// `SmOwningTopology.h`): takes the shell off the region's list and clears its
/// owner, leaving every other shell's relative order.
///
/// # Errors
/// Native `SM_ERR` (as `InvalidInput`) when the shell is not in this region's list,
/// and dangling ids.
pub fn region_remove_shell(brep: &mut Brep, region: RegionId, shell: ShellId) -> Result<()> {
    if brep.shell(shell)?.region != Some(region) {
        return Err(invalid_members());
    }
    let list = &mut brep.region_mut(region)?.shells;
    let index = list
        .iter()
        .position(|id| *id == shell)
        .ok_or(invalid_members())?;
    list.remove(index);
    brep.shell_mut(shell)?.region = None;
    Ok(())
}

/// `pRegion->PostInsert(pShell)` (`SmOwningTopology::PostInsert`): appends the shell
/// to the tail of the region's list and makes the region its owner.
///
/// # Errors
/// `InvalidInput` when a region's list still holds the shell (this one's, or that of
/// the region it names), and dangling ids.
pub fn region_post_insert_shell(brep: &mut Brep, region: RegionId, shell: ShellId) -> Result<()> {
    let owner = brep.shell(shell)?.region;
    if brep.region(region)?.shells.contains(&shell) {
        return Err(invalid_members());
    }
    if let Some(owner) = owner {
        if brep.region(owner)?.shells.contains(&shell) {
            return Err(invalid_members());
        }
    }
    brep.region_mut(region)?.shells.push(shell);
    brep.shell_mut(shell)?.region = Some(region);
    Ok(())
}

/// `pShell->Remove(pFU)` (`SmOwningTopology::Remove`): takes the face use off the
/// shell's face-use list, clears its owner and links, and closes the list over the
/// gap (the head moves to the next member when it was the head; a list left
/// without members gets a NULL head).
///
/// # Errors
/// [`SolidError::sm_err`] (upstream's `SM_ERR` return) when the face use's owner is
/// not this shell or it is missing from the list; `InvalidInput` for a shell whose
/// head is not a face use; dangling ids.
pub fn shell_remove_faceuse(brep: &mut Brep, shell: ShellId, faceuse: FaceuseId) -> Result<()> {
    const OP: &str = "topology::shell::shell_remove_faceuse";
    if brep.faceuse(faceuse)?.shell != Some(shell) {
        return Err(SolidError::sm_err(OP));
    }
    let members = &mut brep.shell_mut(shell)?.members;
    match members {
        ShellMembers::Faceuses(list) => {
            let index = list
                .iter()
                .position(|id| *id == faceuse)
                .ok_or_else(|| SolidError::sm_err(OP))?;
            list.remove(index);
            if list.is_empty() {
                *members = ShellMembers::Empty;
            }
        }
        ShellMembers::Empty => return Err(SolidError::sm_err(OP)),
        _ => return Err(invalid_members()),
    }
    let record = brep.faceuse_mut(faceuse)?;
    record.shell = None;
    record.sibling_links = None;
    refresh_faceuse_sibling_links(brep, shell)
}

/// `pShell->PostInsert(pFU)` (`SmOwningTopology::PostInsert`): appends the face use
/// to the tail of the shell's face-use list and makes the shell its owner. The
/// shell type is left as it is, as upstream.
///
/// # Errors
/// `InvalidInput` when a shell's face-use list still holds the face use, or when the
/// shell's head is a wire or vertex use; dangling ids.
pub fn shell_post_insert_faceuse(
    brep: &mut Brep,
    shell: ShellId,
    faceuse: FaceuseId,
) -> Result<()> {
    if let Some(owner) = brep.faceuse(faceuse)?.shell {
        if let ShellMembers::Faceuses(list) = &brep.shell(owner)?.members {
            if list.contains(&faceuse) {
                return Err(invalid_members());
            }
        }
    }
    let target = &mut brep.shell_mut(shell)?.members;
    match target {
        ShellMembers::Empty => *target = ShellMembers::Faceuses(vec![faceuse]),
        ShellMembers::Faceuses(list) => list.push(faceuse),
        _ => return Err(invalid_members()),
    }
    brep.faceuse_mut(faceuse)?.shell = Some(shell);
    refresh_faceuse_sibling_links(brep, shell)
}

/// Move a shell's vertex, wire edges and face uses to another shell.
///
/// `wire_candidates` is the native optional list of B-rep wire edges. When it
/// is present, ownership filters it in caller order; otherwise `GetWireEdges`
/// gathers the source's wires. It is never replaced by an empty default list.
/// Nullable list entries are only rejected if the native branch reads them.
///
/// # Errors
/// Native `SM_ERR` for an illegal vertex-shell move, dangling graph links, or
/// incompatible native list classes. The caller must supply connected topology.
pub fn transfer_children(
    kernel: &mut Kernel,
    brep: &mut Brep,
    source: ShellId,
    recipient: ShellId,
    wire_candidates: Option<&[Option<EdgeId>]>,
) -> Result<()> {
    use crate::topology::edgeuse::p1;
    use crate::topology::{EdgeuseOwner, VertexuseOwner};
    const OP: &str = "topology::shell::transfer_children";
    brep.shell(source)?;
    if shell_list_size(brep, recipient)? == 0 {
        brep.shell_mut(recipient)?.shell_type = ShellType::Unknown;
        if brep.shell(source)?.is_vertex_shell() {
            let ShellMembers::Vertexuse(vertexuse) = brep.shell(source)?.members else {
                return Err(invalid_members());
            };
            brep.vertexuse(vertexuse)?;
            let target = brep.shell_mut(recipient)?;
            target.shell_type = ShellType::Vertex;
            target.members = ShellMembers::Vertexuse(vertexuse);
            brep.vertexuse_mut(vertexuse)?.owner = VertexuseOwner::Shell(Some(recipient));
            clear_shell_members(brep, source)?;
            return Ok(());
        }
    } else {
        if brep.shell(recipient)?.is_vertex_shell() {
            return if shell_list_size(brep, source)? == 0 {
                Ok(())
            } else {
                Err(SolidError::sm_err(OP))
            };
        }
        if brep.shell(source)?.is_vertex_shell() {
            return Err(SolidError::sm_err(OP));
        }
    }

    let wires = if let Some(candidates) = wire_candidates {
        let mut wires = Vec::new();
        for &edge in candidates {
            let edge = edge.ok_or(invalid_members())?;
            let primary = p1::primary_edgeuse(brep, edge)?.ok_or(invalid_members())?;
            if brep.edgeuse(primary)?.owner == EdgeuseOwner::Shell(Some(source)) {
                wires.push(edge);
            }
        }
        wires
    } else {
        wire_edges(kernel, brep, source)?
    };
    if let Some(&first) = wires.first() {
        if brep.shell(recipient)?.is_unknown_shell() {
            let primary = p1::primary_edgeuse(brep, first)?.ok_or(invalid_members())?;
            let target = brep.shell_mut(recipient)?;
            target.members = ShellMembers::Edgeuse(primary);
            target.shell_type = ShellType::Wire;
            // UpdateListSize counts this edge's radial list; the shared arena
            // model retains that list on Edge::edgeuses rather than separately.
        }
        for edge in wires {
            // SmEdge::GetEdgeuses with neither optional filter: the native
            // circular radial list order.
            let edgeuses = crate::topology::edge::edgeuses(brep, edge, None, None)?;
            for edgeuse in edgeuses {
                brep.edgeuse_mut(edgeuse)?.owner = EdgeuseOwner::Shell(Some(recipient));
            }
        }
    }

    let faceuses = brep.shell(source)?.get_faceuses()?;
    if !faceuses.is_empty() {
        if !brep.shell(recipient)?.is_faceuse_shell() {
            let target = brep.shell_mut(recipient)?;
            target.shell_type = ShellType::Faceuse;
            target.members = ShellMembers::Empty;
        }
        for faceuse in faceuses {
            if brep.faceuse(faceuse)?.shell != Some(source) {
                return Err(invalid_members());
            }
            remove_and_post_insert(brep, faceuse, source, recipient)?;
        }
        // Self-transfer clears the shell head but leaves this face-use cycle
        // connected. Preserve those native links independently of the head.
        refresh_faceuse_sibling_links(brep, recipient)?;
    }
    clear_shell_members(brep, source)
}

// `source->Remove(faceuse)` then `recipient->PostInsert(faceuse)`
// (`SmOwningTopology`, inline in `SmOwningTopology.h`): the member moves to the
// recipient's tail without changing either list's remaining relative order; a
// list left without members gets a NULL head. Sibling links are not refreshed.
fn remove_and_post_insert(
    brep: &mut Brep,
    faceuse: FaceuseId,
    source: ShellId,
    recipient: ShellId,
) -> Result<()> {
    let members = &mut brep.shell_mut(source)?.members;
    match members {
        ShellMembers::Faceuses(list) => {
            let index = list
                .iter()
                .position(|id| *id == faceuse)
                .ok_or(invalid_members())?;
            list.remove(index);
            if list.is_empty() {
                *members = ShellMembers::Empty;
            }
        }
        _ => return Err(invalid_members()),
    }
    let target = &mut brep.shell_mut(recipient)?.members;
    match target {
        ShellMembers::Empty => *target = ShellMembers::Faceuses(vec![faceuse]),
        ShellMembers::Faceuses(list) => list.push(faceuse),
        _ => return Err(invalid_members()),
    }
    brep.faceuse_mut(faceuse)?.shell = Some(recipient);
    Ok(())
}

/// Move one face use from its shell to the tail of `recipient`
/// (`pOldShell->Remove(pFU); pNewShell->PostInsert(pFU)` with
/// `pOldShell = pFU->GetShell()`, as `SmTopologyTraverser::FaceuseShellReplacement`
/// does). Both shells' face-use sibling links are refreshed. The caller skips a
/// face use already in `recipient`.
///
/// # Errors
/// A face use without a shell (a NULL dereference upstream), a face use missing
/// from its shell's face-use list, a recipient holding wire/vertex members, or
/// dangling ids.
pub fn move_faceuse(brep: &mut Brep, faceuse: FaceuseId, recipient: ShellId) -> Result<()> {
    let source = brep
        .faceuse(faceuse)?
        .shell
        .ok_or_else(|| SolidError::null_pointer("topology::shell::move_faceuse"))?;
    brep.shell(recipient)?;
    remove_and_post_insert(brep, faceuse, source, recipient)?;
    refresh_faceuse_sibling_links(brep, source)?;
    refresh_faceuse_sibling_links(brep, recipient)
}

/// `pShell->PostInsert(pFU)` (`SmOwningTopology::PostInsert`, inline in
/// `SmOwningTopology.h`) of a face use not on any list: it joins the tail of the
/// shell's face-use list (a NULL head makes it the head) and gets the shell as owner.
/// Literal sibling links of the members, when any are held, are refreshed.
///
/// # Errors
/// A shell holding wire/vertex members, or dangling ids.
pub fn post_insert_faceuse(brep: &mut Brep, shell: ShellId, faceuse: FaceuseId) -> Result<()> {
    brep.faceuse(faceuse)?;
    let members = &mut brep.shell_mut(shell)?.members;
    match members {
        ShellMembers::Empty => *members = ShellMembers::Faceuses(vec![faceuse]),
        ShellMembers::Faceuses(list) => list.push(faceuse),
        _ => return Err(invalid_members()),
    }
    brep.faceuse_mut(faceuse)?.shell = Some(shell);
    let mut linked = false;
    for member in brep.shell(shell)?.get_faceuses()? {
        linked |= member != faceuse && brep.faceuse(member)?.sibling_links.is_some();
    }
    if linked {
        refresh_faceuse_sibling_links(brep, shell)?;
    }
    Ok(())
}

fn clear_shell_members(brep: &mut Brep, id: ShellId) -> Result<()> {
    let shell = brep.shell_mut(id)?;
    shell.shell_type = ShellType::Unknown;
    shell.members = ShellMembers::Empty;
    Ok(())
}

/// `SmShell::GetSize`: the shell's `m_lListSize`, recomputed from its typed members
/// (`UpdateListSize`; a wire shell counts its head edge's radial edge uses).
///
/// # Errors
/// A wire head without an edge, and dangling ids.
pub fn shell_list_size(brep: &Brep, id: ShellId) -> Result<usize> {
    match &brep.shell(id)?.members {
        ShellMembers::Empty => Ok(0),
        ShellMembers::Faceuses(members) => Ok(members.len()),
        ShellMembers::Vertexuse(_) => Ok(1),
        ShellMembers::Edgeuse(id) => {
            let edge = brep.edgeuse(*id)?.edge.ok_or(invalid_members())?;
            Ok(brep.edge(edge)?.edgeuses.len())
        }
    }
}

/// Native next/previous face-use links, independently of a shell's list head.
pub type FaceuseSiblingLinks = (Option<FaceuseId>, Option<FaceuseId>);

/// Typed adapter for a face use's native `GetNext` and `GetLast` links.
///
/// # Errors
/// A dangling id or a constructed owner vector that does not contain its member.
pub fn faceuse_sibling_links(brep: &Brep, id: FaceuseId) -> Result<FaceuseSiblingLinks> {
    let faceuse = brep.faceuse(id)?;
    if let Some(links) = faceuse.sibling_links {
        return Ok(links);
    }
    let Some(owner) = faceuse.shell else {
        return Ok((None, None));
    };
    let ShellMembers::Faceuses(members) = &brep.shell(owner)?.members else {
        return Err(invalid_members());
    };
    let mut previous = members.last().copied();
    for (&member, &next) in members.iter().zip(members.iter().cycle().skip(1)) {
        if member == id {
            return Ok((Some(next), previous));
        }
        previous = Some(member);
    }
    Err(invalid_members())
}

fn refresh_faceuse_sibling_links(brep: &mut Brep, owner: ShellId) -> Result<()> {
    let members = brep.shell(owner)?.get_faceuses()?;
    let mut previous = members.last().copied();
    for (&member, &next) in members.iter().zip(members.iter().cycle().skip(1)) {
        brep.faceuse_mut(member)?.sibling_links = Some((Some(next), previous));
        previous = Some(member);
    }
    Ok(())
}

/// Whether a shell fits within `tolerance` (`SmShell::IsDegenerate`).
/// `None` retains the native default `EFF_ZERO`, independently of zone tolerance.
///
/// # Errors
/// Invalid graph references, an undefined native pointer access, or an unported
/// bounding-box dependency. Ordinary native box errors retain the partial box.
pub fn shell_is_degenerate(
    kernel: &mut Kernel,
    brep: &Brep,
    id: ShellId,
    tolerance: Option<f64>,
) -> Result<bool> {
    if brep.shell(id)?.is_vertex_shell() {
        return Ok(true);
    }
    let bounds = shell_bounds_for_degeneracy(kernel, brep, id)?;
    // Native IsDegenerate deliberately ignores CalculateBoundingBox's status.
    Ok(bounds.is_point_sized(tolerance.unwrap_or(crate::math::EFF_ZERO)))
}

/// Whether a region fits within `tolerance` (`SmRegion::IsDegenerate`).
/// `None` retains the native default `EFF_ZERO`.
///
/// # Errors
/// Invalid graph references or an unported bounding-box dependency. Ordinary
/// native box errors are ignored while retaining the initialized/partial box.
pub fn region_is_degenerate(
    kernel: &mut Kernel,
    brep: &Brep,
    id: RegionId,
    tolerance: Option<f64>,
) -> Result<bool> {
    let bounds = region_bounds_for_degeneracy(kernel, brep, id)?;
    Ok(bounds.is_point_sized(tolerance.unwrap_or(crate::math::EFF_ZERO)))
}

// SmRegion/SmShell::CalculateBoundingBox (unit query.face_p1) with the default
// bTight = FALSE. The IsDegenerate callers ignore a native failure status and read
// the partial union of the children before the failing one; missing dependencies
// and unsafe native graph accesses still propagate.
fn region_bounds_for_degeneracy(
    kernel: &mut Kernel,
    brep: &Brep,
    id: RegionId,
) -> Result<crate::geom::extent3d::Extent3d> {
    let mut bounds = crate::geom::extent3d::Extent3d::new();
    let status = crate::query::face::p1::calculate_region_bounding_box_into(
        kernel,
        brep,
        id,
        false,
        &mut bounds,
    );
    native_bounds(bounds, status)
}

fn shell_bounds_for_degeneracy(
    kernel: &mut Kernel,
    brep: &Brep,
    id: ShellId,
) -> Result<crate::geom::extent3d::Extent3d> {
    let mut bounds = crate::geom::extent3d::Extent3d::new();
    let status = crate::query::face::p1::calculate_shell_bounding_box_into(
        kernel,
        brep,
        id,
        false,
        &mut bounds,
    );
    native_bounds(bounds, status)
}

fn native_bounds(
    bounds: crate::geom::extent3d::Extent3d,
    status: Result<()>,
) -> Result<crate::geom::extent3d::Extent3d> {
    match status {
        // Native SER returns the box unioned so far.
        Ok(()) | Err(SolidError::Upstream { .. } | SolidError::Nlib { .. }) => Ok(bounds),
        // Missing dependencies and safety errors must not become ordinary native
        // status results or disappear into an initialized extent.
        Err(error) => Err(error),
    }
}

/// Outputs of a shell ray query, including writes made before an error.
#[derive(Debug, Clone)]
pub struct ShellRayState<O> {
    /// Region containing the ray start, or no region after a missed shell.
    pub region: Option<RegionId>,
    /// Classification of the selected intersection.
    pub classification: crate::intersect::point_class::PointClassification<O>,
}

/// Caller parameters and initial output values for `SmShell::RayFire`.
pub struct ShellRayInput<O> {
    /// Ray start.
    pub start: crate::math::Point3,
    /// Caller direction, before native sizing and normalization.
    pub direction: crate::math::Vec3,
    /// Intersection search tolerance.
    pub zone_tolerance: f64,
    /// Optional radius, bypassing B-rep bounding-box construction.
    pub sphere_bound: Option<f64>,
    /// Initial output values, retained on paths that do not write them.
    pub state: ShellRayState<O>,
}

/// Status and observable output state of a ray query.
pub struct ShellRayAttempt<O> {
    /// Native operation status or an explicit missing dependency.
    pub status: Result<()>,
    /// Outputs after all writes preceding that status.
    pub state: ShellRayState<O>,
}

/// Borrowed inputs for the B-rep's local sector classification.
pub struct ShellRaySector<'a, O> {
    /// Actual owner arena key.
    pub brep: crate::topology::BrepId,
    /// Actual line segment from ray start to intersection.
    pub curve: &'a crate::nurbs::b_spline_curve::BSplineCurve,
    /// Natural interval of that curve.
    pub interval: crate::geom::extent1d::Extent1d,
    /// Native opposite orientation for the incoming segment.
    pub orientation: crate::nurbs::b_spline_surface_n_lib::p2::OrientType,
    /// Classification produced by the selected intersection.
    pub classification: &'a crate::intersect::point_class::PointClassification<O>,
}

/// Sector result retaining a possible region write even on error.
pub struct ShellRaySectorAttempt {
    /// Native sector status.
    pub status: Result<()>,
    /// `None` leaves the region unchanged; `Some(None)` writes NULL.
    pub region_update: Option<Option<RegionId>>,
}

/// Actual geometry and owner context needed by shell ray queries.
/// No global owner registry or duplicate geometry metadata is introduced.
/// The production world is [`crate::query::brep::BrepQuery`].
pub trait ShellRayWorld<O> {
    /// The topology solver's view of a B-rep's geometry.
    type Objects<'a>: crate::intersect::topology_solver::p1::TopologyObjects<O>
    where
        Self: 'a;

    /// The view of `brep` the ray's shape solve reads. It borrows the B-rep only
    /// for that solve: the sector step after it may edit the B-rep.
    ///
    /// # Errors
    /// A world that cannot view `brep`.
    fn objects<'a>(
        &'a self,
        brep: (crate::topology::BrepId, &'a Brep),
    ) -> Result<Self::Objects<'a>>;

    /// A read-only view that may request topology preparation from the mutable ray caller.
    fn objects_for_preparation<'a>(
        &'a self,
        brep: (crate::topology::BrepId, &'a Brep),
        _preparation: &'a crate::query::brep::RayPreparation,
    ) -> Result<Self::Objects<'a>> {
        self.objects(brep)
    }

    /// Solves the shell's ray. Mutable topology services may keep one solve in flight;
    /// the default releases a read-only view to service explicit preparation requests.
    fn solve_shape_ray(
        &self,
        k: &Kernel,
        brep: &mut Brep,
        brep_id: crate::topology::BrepId,
        shape: &Shape,
        point: crate::math::Point3,
        options: crate::intersect::topology_solver::p2::TopologySolveOptions<'_>,
    ) -> Result<crate::geom::solution_array::SolutionArray<O>>
    where
        O: crate::geom::solution_array::SolutionObject,
    {
        let mut preparation = crate::query::brep::RayPreparation::default();
        loop {
            let result = crate::intersect::topology_solver::p1::TopologySolver::shape_point_solve(
                k,
                &self.objects_for_preparation((brep_id, &*brep), &preparation)?,
                shape,
                point,
                options,
            );
            match (result, preparation.take_request()) {
                (Err(SolidError::PreparationRequired { .. }), Some(request)) => {
                    preparation.prepare(k, brep, request)?;
                }
                (result, _) => return result,
            }
        }
    }

    /// The shape of the shell's boundary faces (`SmShape(sBreps, sFaces, ...)`) of
    /// `brep`. Worlds that cannot build the real shape (a harness without the B-rep's
    /// cached geometry) defer here instead of failing later.
    fn shell_ray_shape(
        &self,
        kernel: &mut Kernel,
        brep: (crate::topology::BrepId, &Brep),
        faces: &[crate::geom::object::FaceKey],
    ) -> Result<Shape> {
        Shape::new(kernel, &brep, &[], faces, &[], &[])
    }

    /// The topology entity behind a handle the shape solve returned (a vertex, edge
    /// or face of the B-rep), as `LocalCurveSector` reads its classification.
    fn class_entity(&self, object: O) -> Option<crate::EntityRef>;

    /// Classify the incoming line segment at its known intersection
    /// (`pBrep->LocalCurveSector(curve, ivl, orientation, sPC, pRegion)`, unit
    /// `topology.brep_p2`): [`crate::topology::brep::p2::local_curve_sector`] on `brep`
    /// (the sectoring may add missing UV trim curves, as upstream's `const` method does),
    /// the classification's objects mapped by [`ShellRayWorld::class_entity`]. A failure
    /// leaves the region unchanged.
    fn shell_ray_sector(
        &self,
        kernel: &Kernel,
        brep: &mut Brep,
        request: ShellRaySector<'_, O>,
    ) -> ShellRaySectorAttempt
    where
        O: crate::intersect::point_class::ClassObject,
    {
        let sector = (|| {
            let hit = request.classification.map_objects(&mut |o| {
                self.class_entity(o).ok_or(SolidError::invalid_input(
                    "topology::shell::ShellRayWorld::shell_ray_sector",
                    "intersection without a topology entity",
                ))
            })?;
            let curve = crate::curve::curve::Curve::from(request.curve.clone());
            crate::topology::brep::p2::local_curve_sector(
                kernel,
                brep,
                &curve,
                &request.interval,
                request.orientation,
                &hit,
            )
        })();
        match sector {
            Ok(region) => ShellRaySectorAttempt {
                status: Ok(()),
                region_update: Some(region),
            },
            Err(e) => ShellRaySectorAttempt {
                status: Err(e),
                region_update: None,
            },
        }
    }
}

/// Fire a sized ray at this shell's distinct boundary faces (`SmShell::RayFire`).
/// Native partial output writes survive an error in the returned attempt.
/// Missing geometry queries are explicit dependency errors.
pub fn ray_fire<O, W>(
    kernel: &mut Kernel,
    brep: &mut Brep,
    brep_id: crate::topology::BrepId,
    shell_id: ShellId,
    world: &W,
    input: ShellRayInput<O>,
) -> ShellRayAttempt<O>
where
    O: crate::geom::solution_array::SolutionObject + crate::intersect::point_class::ClassObject,
    W: ShellRayWorld<O>,
{
    use crate::geom::global_solver::{SolutionRequested, SolverOperation};
    use crate::intersect::point_class::PointClassType;
    use crate::intersect::topology_solver::p2::TopologySolveOptions;
    use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    const OP: &str = "topology::shell::ray_fire";
    let mut state = input.state;
    let status = (|| {
        let radius = if let Some(radius) = input.sphere_bound {
            radius
        } else {
            shell_brep(brep, shell_id)?.ok_or(SolidError::null_pointer(OP))?;
            // pBrep->CalculateBoundingBox(sBBox) (unit `query.brep`, default arguments)
            qbrep::calculate_bounding_box(kernel, &mut *brep, BoundingBoxOptions::default())?
                .sphere_bound()
                .radius
        };
        let end = input.start + input.direction * (3.01 * radius);
        let direction = (end - input.start)
            .unitize()
            .map_err(|_| SolidError::Upstream {
                operation: OP,
                code: 1007, // SmVector3d::Unitize returns SM_ERR_INVALID_INPUT.
                entity: None,
            })?;
        // `SmNewMarkAndLock` with the default (all marks) mask, released on every path.
        let faces = with_mark_lock(kernel, ALL_MARKS, |kernel, mark| {
            let mut marks = Marks::default();
            let mut faces = Vec::new();
            for id in brep.shell(shell_id)?.get_faceuses()? {
                let faceuse = brep.faceuse(id)?;
                let face = faceuse.face.ok_or(SolidError::null_pointer(OP))?;
                if marks.is_marked(kernel, Entity::Face(face), mark) {
                    continue;
                }
                marks.mark(kernel, Entity::Face(face), mark);
                let mate = brep.faceuse(faceuse.mate.ok_or(SolidError::null_pointer(OP))?)?;
                if mate.shell != Some(shell_id) {
                    faces.push((brep_id, face));
                }
            }
            Ok::<_, SolidError>(faces)
        })??;
        let shape = world.shell_ray_shape(kernel, (brep_id, brep), &faces)?;
        let vectors = [direction];
        let solutions = world.solve_shape_ray(
            &*kernel,
            brep,
            brep_id,
            &shape,
            input.start,
            TopologySolveOptions {
                operation: SolverOperation::Rayfire,
                requested: SolutionRequested::All,
                tolerance_3d: input.zone_tolerance,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &vectors,
            },
        )?;
        let mut iter = solutions.as_slice().iter();
        let Some(mut chosen) = iter.next() else {
            state.region = None;
            state
                .classification
                .set_class_object(PointClassType::Unknown, None);
            return Ok(());
        };
        // Upstream compares every candidate against the INITIAL ray parameter,
        // even after selecting a different solution.
        let initial_parameter = chosen.start.solution_value;
        for candidate in iter {
            if candidate.start.solution_value - initial_parameter < EFF_ZERO_SQRT {
                if candidate.num_variables < chosen.num_variables {
                    chosen = candidate;
                    continue;
                }
                if candidate.num_variables > chosen.num_variables {
                    continue;
                }
            }
            if candidate.start.param(0)? < chosen.start.param(0)? {
                chosen = candidate;
            }
        }
        let parameter = chosen.start.solution_value;
        if parameter < -EFF_ZERO {
            state.region = None;
            state
                .classification
                .set_class_object(PointClassType::Unknown, None);
        }
        if parameter < EFF_ZERO * (1.0 + input.start.max_dimension()) {
            return Err(SolidError::sm_err(OP));
        }
        let object = chosen
            .objects
            .first()
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP))?;
        state.classification.load_from_intersection(
            input.zone_tolerance,
            object,
            &chosen.start,
            0,
            parameter,
        )?;
        let endpoint = input.start + direction * parameter;
        shell_brep(brep, shell_id)?.ok_or(SolidError::null_pointer(OP))?;
        let curve = BSplineCurve::create_line_segment(3, input.start, endpoint)?;
        let sector = world.shell_ray_sector(
            &*kernel,
            brep,
            ShellRaySector {
                brep: brep_id,
                curve: &curve,
                interval: curve.natural_interval(),
                orientation: OrientType::Opposite,
                classification: &state.classification,
            },
        );
        if let Some(region) = sector.region_update {
            state.region = region;
        }
        sector.status
    })();
    ShellRayAttempt { status, state }
}

/// A region resolved to its actual typed arena and B-rep identity.
/// The caller supplies the identity of that arena; detached regions retain no
/// owning B-rep even though their records still need storage.
#[derive(Clone, Copy, Debug)]
pub struct RegionRef<'a> {
    /// Arena containing the live region record.
    pub brep: &'a Brep,
    /// Identity of that B-rep in the caller's topology context.
    pub brep_id: crate::topology::BrepId,
    /// Actual region key in the arena.
    pub region_id: RegionId,
}

impl RegionRef<'_> {
    /// Resolve the native owner, including NULL for a detached region.
    ///
    /// # Errors
    /// A dangling region key.
    pub fn owning_brep(&self) -> Result<Option<crate::topology::BrepId>> {
        Ok(self
            .brep
            .region(self.region_id)?
            .get_brep(self.brep)
            .map(|_| self.brep_id))
    }

    /// The raw native void flag, including UNSURE (2).
    ///
    /// # Errors
    /// A dangling region key.
    pub fn void_state(&self) -> Result<i32> {
        Ok(self.brep.region(self.region_id)?.is_void())
    }

    /// Whether this is its owner's infinite region.
    ///
    /// # Errors
    /// A dangling region key.
    pub fn is_infinite(&self) -> Result<bool> {
        Ok(self
            .brep
            .region(self.region_id)?
            .is_infinite_region(self.brep, self.region_id))
    }
}

/// Borrowed resolution of opaque attribute object keys to live typed regions.
/// Implementations must preserve actual B-rep identity across arenas; this
/// interface stores no duplicate region flags and introduces no global registry.
pub trait RegionLookup {
    /// Resolve the actual region record for this object.
    ///
    /// # Errors
    /// An unknown object, wrong object class or dangling topology key.
    fn resolve_region(&self, object: crate::geom::attribute::ObjectKey) -> Result<RegionRef<'_>>;
}

// ---- SmShell::IsInnerShellOf ------------------------------------------------------------

/// `TRUE` of an `SmBoolean`.
pub const SM_TRUE: i32 = 1;
/// `FALSE` of an `SmBoolean`.
pub const SM_FALSE: i32 = 0;
/// `UNSURE` (`2`) of an `SmBoolean`.
pub const SM_UNSURE: i32 = 2;

/// The geometry behind the handles of [`is_inner_shell_of`]'s ray classification: the
/// ray line (`CacheObject::Other`) and the B-rep's edges, faces and vertices.
struct RayGeometry<'a> {
    b: &'a Brep,
    line: &'a crate::curve::curve::Curve,
}

impl crate::intersect::point_class::ClassGeometry<crate::topology::brep_cache::CacheObject>
    for RayGeometry<'_>
{
    fn curve(
        &self,
        o: crate::topology::brep_cache::CacheObject,
    ) -> Option<&crate::curve::curve::Curve> {
        use crate::topology::brep_cache::CacheObject;
        match o {
            CacheObject::Other(_) => Some(self.line),
            CacheObject::Edge(e) | CacheObject::Curve(e) => self.b.edge(e).ok()?.curve.as_ref(),
            _ => None,
        }
    }
    fn surface(
        &self,
        o: crate::topology::brep_cache::CacheObject,
    ) -> Option<&crate::surface::surface::Surface> {
        use crate::topology::brep_cache::CacheObject;
        match o {
            CacheObject::Face(f) | CacheObject::Surface(f) => self.b.face(f).ok()?.surface.as_ref(),
            _ => None,
        }
    }
    fn volume(
        &self,
        _o: crate::topology::brep_cache::CacheObject,
    ) -> Option<&crate::surface::volume::Volume> {
        None
    }
    fn point(&self, o: crate::topology::brep_cache::CacheObject) -> Option<crate::math::Point3> {
        match o {
            crate::topology::brep_cache::CacheObject::Vertex(v) => {
                Some(self.b.vertex(v).ok()?.point)
            }
            _ => None,
        }
    }
    fn classification_vertex_point(
        &self,
        vertex: crate::topology::brep_cache::CacheObject,
    ) -> Option<crate::math::Point3> {
        self.point(vertex)
    }
    fn topology_zone_tol_3d(
        &self,
        object: crate::topology::brep_cache::CacheObject,
    ) -> Option<f64> {
        use crate::topology::brep_cache::CacheObject;
        match object {
            CacheObject::Vertex(v) => Some(self.b.vertex(v).ok()?.tolerance),
            CacheObject::Edge(e) => Some(self.b.edge(e).ok()?.tolerance),
            CacheObject::Face(f) => Some(self.b.face(f).ok()?.tolerance),
            _ => None,
        }
    }
    fn topology_entity(
        &self,
        object: crate::topology::brep_cache::CacheObject,
    ) -> Option<(&Brep, crate::error::EntityRef)> {
        use crate::error::EntityRef;
        use crate::topology::brep_cache::CacheObject;
        match object {
            CacheObject::Vertex(v) => Some((self.b, EntityRef::Vertex(v))),
            CacheObject::Edge(e) => Some((self.b, EntityRef::Edge(e))),
            CacheObject::Face(f) => Some((self.b, EntityRef::Face(f))),
            _ => None,
        }
    }
    fn topology_handle(
        &self,
        entity: crate::error::EntityRef,
    ) -> Option<crate::topology::brep_cache::CacheObject> {
        use crate::error::EntityRef;
        use crate::topology::brep_cache::CacheObject;
        match entity {
            EntityRef::Vertex(v) => Some(CacheObject::Vertex(v)),
            EntityRef::Edge(e) => Some(CacheObject::Edge(e)),
            EntityRef::Face(f) => Some(CacheObject::Face(f)),
            _ => None,
        }
    }
}

/// `SmTopologySolver::BrepCurveSolve(pBrep, line, line.GetNaturalInterval(),
/// SM_SO_INTERSECT, SM_SR_ALL, tol, 0.0, NULL, solutions)` on the B-rep's own cache.
fn ray_brep_solve(
    k: &Kernel,
    b: &mut Brep,
    line: &crate::curve::curve::Curve,
    tol: f64,
) -> Result<crate::geom::solution_array::SolutionArray<crate::topology::brep_cache::CacheObject>> {
    use crate::geom::attribute::ObjectKey;
    use crate::geom::global_solver::{SolutionRequested, SolverOperation};
    use crate::geom::object::ObjectRef;
    use crate::intersect::topology_solver::p1::{CurveRef, TopologySolver};
    use crate::intersect::topology_solver::p2::TopologySolveOptions;
    use crate::topology::brep_cache::CacheObject;
    let query = crate::query::brep::BrepQuery::of_brep(b);
    let cache = query.cache(k, b)?;
    let identity = query.effective_identity();
    let objects = crate::query::brep::LiveBrepObjects::new(b, &identity, Some(cache))?;
    TopologySolver::brep_curve_solve(
        k,
        &objects,
        Some(CacheObject::Brep),
        CurveRef {
            curve: line,
            handle: CacheObject::Other(ObjectRef {
                type_code: crate::curve::line::Line::TYPE,
                key: ObjectKey(0),
            }),
        },
        &line.natural_interval(),
        TopologySolveOptions {
            operation: SolverOperation::Intersect,
            requested: SolutionRequested::All,
            tolerance_3d: tol,
            best_answer_so_far_sq: 0.0,
            opt_vectors: &[],
        },
    )
}

/// The outcome of a call whose status upstream ignores (see [`checked`]).
enum Checked<T> {
    /// It succeeded.
    Value(T),
    /// It failed and the status is ignored, as upstream.
    Ignored,
    /// It failed and, with
    /// [`crate::kernel::UpstreamFixes::shell_inner_shell_of_statuses`], decides.
    Decides,
}

/// The status-checked variant of an ignored upstream call: with
/// [`crate::kernel::UpstreamFixes::shell_inner_shell_of_statuses`] an `Err` decides;
/// without it the error is ignored, except [`SolidError::Unported`].
fn checked<T>(k: &Kernel, r: Result<T>) -> Result<Checked<T>> {
    match r {
        Ok(v) => Ok(Checked::Value(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) if k.fixes.shell_inner_shell_of_statuses => Ok(Checked::Decides),
        Err(_) => Ok(Checked::Ignored),
    }
}

/// `SmShell::IsInnerShellOf(pOptOuterShell)`: whether shell `id` lies directly inside
/// `outer` (simple parent-child containment, not nested deeper), or with `outer`
/// `None` is an inner shell of the infinite region. `outer` must be a closed face-use
/// shell (`ClosureTraversal` of its first face use; else [`SM_FALSE`]). Rays are cast
/// from three sample points of each face use of the shell (along its normal), or from
/// a wire shell's first edge or a vertex shell's vertex in random directions
/// (`kernel.rng`, ND-1), against the whole B-rep; the first ray whose crossings are
/// all simple face crossings decides, by the stack of shells it exits and enters.
/// Returns [`SM_TRUE`], [`SM_FALSE`] or [`SM_UNSURE`] (no ray decided).
///
/// Upstream ignores the statuses of the closure traversal, the bounding box, the
/// sample points, the ray solve, the classification insert and the crossing face's
/// evaluations; the opt-in
/// [`crate::kernel::UpstreamFixes::shell_inner_shell_of_statuses`] (fork 4038552)
/// returns [`SM_UNSURE`] when the closure, the box, the sample points or a crossing
/// face's point or normal cannot be computed, and goes on to the next ray when the ray
/// cannot be solved or classified.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (a wire shell without edge or edge curve, a vertex shell
/// without vertex, a crossing face without surface or face uses); the errors upstream
/// does not ignore (default) or every error of the classification walk
/// ([`SolidError::Unported`] always propagates).
#[allow(clippy::too_many_lines)] // one upstream routine
pub fn is_inner_shell_of(
    k: &mut Kernel,
    b: &mut Brep,
    id: ShellId,
    outer: Option<ShellId>,
) -> Result<i32> {
    use crate::curve::curve::Curve;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::geom::extent1d::Extent1d;
    use crate::geom::extent3d::Extent3d;
    use crate::geom::solution_array::{ObjectKind, SolutionObject, SolutionType};
    use crate::intersect::curve_class::p1::CurveClassification;
    use crate::intersect::curve_class::p4::ClassificationInit;
    use crate::math::{Point3, Vec3, EFF_ZERO};
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::nurbs::knots::Side;
    use crate::topology::brep_cache::CacheObject;
    const OP_ISO: &str = "topology::shell::is_inner_shell_of";
    let null = |reason: &'static str| SolidError::invalid_input(OP_ISO, reason);

    // check state - if given pOptOuterShell needs to be a Faceuse_TYPE shell that is
    // closed
    if let Some(outer) = outer {
        let mut is_closed = false;
        let outer_shell = b.shell(outer)?;
        if outer_shell.is_faceuse_shell() {
            let first = outer_shell.first_faceuse()?;
            // increment and lock any unlocked mark
            let lock = crate::geom::context::MarkLock::new(
                std::slice::from_mut(&mut k.context),
                0,
                ALL_MARKS,
            )?;
            let mut marks = Marks::default();
            let r = match first {
                Some(fu) => crate::topology::topology_traverser::closure_traversal(
                    k, b, &mut marks, fu, &lock, None,
                ),
                None => Err(SolidError::null_pointer(OP_ISO)),
            };
            let mut lock = lock;
            lock.release(std::slice::from_mut(&mut k.context))?;
            match checked(k, r)? {
                Checked::Value(c) => is_closed = c,
                Checked::Ignored => (),
                Checked::Decides => return Ok(SM_UNSURE),
            }
        }
        if !is_closed {
            return Ok(SM_FALSE);
        }
    }

    // get size based on Approx Brep bounding box
    let bbox = checked(
        k,
        qbrep::calculate_bounding_box(
            k,
            &mut *b,
            BoundingBoxOptions {
                approx: true,
                ..BoundingBoxOptions::default()
            },
        ),
    )?;
    let mut bbox = match bbox {
        Checked::Value(x) => x,
        Checked::Ignored => Extent3d::new(),
        Checked::Decides => return Ok(SM_UNSURE),
    };
    bbox.expand_absolute(1.0);
    let size = 2.0 * bbox.size().length();
    let tol = EFF_ZERO * (1.0 + size);

    // search the faceuses of the shells for likely ray casts (those that hit only faces)
    let smp_cnt = 3u64;
    let shell = b.shell(id)?.clone();
    let inner_vertex = shell.vertex(b)?;
    let inner_faceuses = shell.get_faceuses()?;
    let inner_wire_edges = wire_edges(k, b, id)?;
    let is_fu_shell = shell.is_faceuse_shell();
    let is_wire_shell = shell.is_wire_shell();
    let is_vertex_shell = shell.is_vertex_shell();

    let mut found_solution = false;
    let cnt = if is_fu_shell { inner_faceuses.len() } else { 1 };
    let mut smp_pv: Vec<Vec3> = Vec::new();
    let mut ii = 0;
    while ii < cnt && !found_solution {
        // sPV[0,2,4,...] = point to start ray for each SmpPoint,
        // sPV[1,3,5,...] = direction to run ray for each SmpPoint
        if is_fu_shell {
            let fu = *inner_faceuses
                .get(ii)
                .ok_or_else(|| null("face use index"))?;
            let fu_rec = b.faceuse(fu)?;
            let (f, orientation) = (
                fu_rec.face.ok_or_else(|| null("face use without face"))?,
                fu_rec.orientation,
            );
            // SmFaceuse::GetPointsInFace (which resets its outputs first; a failure
            // here leaves them empty)
            let r = crate::topology::face::p2::points_in_face(k, b, f, smp_cnt);
            match checked(k, r)? {
                Checked::Value(points) => {
                    let mut pv = points.pv;
                    if orientation == OrientType::Opposite {
                        for n in pv.iter_mut().skip(1).step_by(2) {
                            *n = -*n;
                        }
                    }
                    smp_pv = pv;
                }
                Checked::Ignored => smp_pv.clear(),
                Checked::Decides => return Ok(SM_UNSURE),
            }
        } else if is_wire_shell {
            let e = *inner_wire_edges
                .first()
                .ok_or_else(|| null("wire shell without edge"))?;
            let edge = b.edge(e)?;
            let ivl = edge.interval;
            let curve = edge
                .curve
                .clone()
                .ok_or_else(|| null("wire edge without curve"))?;
            let mut point = Point3::UNDEF;
            for jj in 0..smp_cnt {
                // sample the curve and pick varying directions for the vector
                match checked(
                    k,
                    curve.evaluate_point(k, ivl.evaluate((jj + 1) as f64 / (smp_cnt + 1) as f64)),
                )? {
                    Checked::Value(p) => point = p,
                    Checked::Ignored => (),
                    Checked::Decides => return Ok(SM_UNSURE),
                }
                let mut dir = Vec3::new(
                    k.rng.next_signed_unit(),
                    k.rng.next_signed_unit(),
                    k.rng.next_signed_unit(),
                );
                dir = dir.unitize_or_z();
                smp_pv.push(point);
                smp_pv.push(dir);
            }
        } else if is_vertex_shell {
            let v = inner_vertex.ok_or_else(|| null("vertex shell without vertex"))?;
            let point = b.vertex(v)?.point;
            for _ in 0..smp_cnt {
                // pick varying directions for the vector
                let mut dir = Vec3::new(
                    k.rng.next_signed_unit(),
                    k.rng.next_signed_unit(),
                    k.rng.next_signed_unit(),
                );
                dir = dir.unitize_or_z();
                smp_pv.push(point);
                smp_pv.push(dir);
            }
        } else {
            break;
        }

        // for every sample point (a position and a direction vector)
        let mut jj = 0;
        while jj < smp_pv.len() {
            found_solution = true;
            let line_pnt = *smp_pv.get(jj).ok_or_else(|| null("sample point"))?;
            let line_vec = *smp_pv.get(jj + 1).ok_or_else(|| null("sample direction"))?;
            jj += 2;

            // build a ray line
            let line = Curve::from(Line::bounded(
                line_pnt,
                line_vec,
                &Extent1d::from_min_max(0.0, size),
                &BoundedLineOptions {
                    scale: 1.0,
                    dim: 3,
                    ..BoundedLineOptions::default()
                },
            ));

            // intersect line with Brep
            let mut solutions = match checked(k, ray_brep_solve(k, b, &line, tol))? {
                Checked::Value(s) => s,
                Checked::Ignored => crate::geom::solution_array::SolutionArray::new(),
                Checked::Decides => continue,
            };

            // keep things simple - only use classifications which have point
            // intersections with faces
            let mut kk = 0;
            while kk < solutions.len() {
                let sol = *solutions.get(kk).ok_or_else(|| null("solution index"))?;
                let object = sol.objects.get(1).copied().flatten();
                let is_kind = |kind: ObjectKind| object.is_some_and(|o| o.kind() == kind);

                // keep the first solution - but ignore (by removing from the solutions
                // array) any subseqent wireedge or vertexshell intersections
                let ignore = sol.solution_type == SolutionType::SingleValue
                    && match object {
                        Some(CacheObject::Edge(e)) => crate::topology::edge::is_wire(b, e)?,
                        Some(CacheObject::Vertex(v)) => {
                            crate::topology::vertex::is_shell_vertex(b, v)?
                                || crate::topology::vertex::is_wire_vertex(b, v)?
                        }
                        _ => false,
                    };
                if ignore {
                    // remove this solution (upstream's `continue` then also steps past
                    // the solution that moves into its slot)
                    solutions.remove_at(kk, 1);
                    kk += 1;
                    continue;
                }

                // skip range solutions and intersections with anything but faces
                if sol.solution_type == SolutionType::RangeOfValues
                    || (kk == 0 && is_fu_shell && !is_kind(ObjectKind::Face))
                    || (kk == 0 && is_wire_shell && !is_kind(ObjectKind::Edge))
                    || (kk == 0 && is_vertex_shell && !is_kind(ObjectKind::Vertex))
                    || (kk > 0 && !is_kind(ObjectKind::Face))
                {
                    // skip this solution
                    found_solution = false;
                    break;
                }
                kk += 1;
            }

            // try again
            if !found_solution {
                continue;
            }

            // load solution into a CurveClassification
            let line_handle = CacheObject::Other(crate::geom::object::ObjectRef {
                type_code: Line::TYPE,
                key: crate::geom::attribute::ObjectKey(0),
            });
            let mut curve_class = CurveClassification::with_curve(
                k,
                &ClassificationInit {
                    curve: Some(line_handle),
                    interval: line.natural_interval(),
                    uv_curve: None,
                    src_zone_tol_3d: crate::geom::tol::zone_tol3d(k, tol),
                    opt_data_size: 0,
                    is_curve_owned: false,
                    is_curve_current: true,
                    surface1: None,
                    surface2: None,
                    uv_curve2: None,
                    ti: None,
                },
            )?;
            {
                let geometry = RayGeometry { b, line: &line };
                let r = curve_class.insert_topology_intersections(
                    k,
                    &geometry,
                    solutions.as_slice(),
                    0,
                );
                if matches!(checked(k, r)?, Checked::Decides) {
                    continue;
                }
            }

            // when the CurveClassification has only one interval then the ray moved into
            // the infinite region and sailed on out into space - we're done
            if curve_class.size() == 1 {
                // shell is an inner shell of the infinite region
                return Ok(if outer.is_none() { SM_TRUE } else { SM_FALSE });
            }

            // classify by the unbalanced ray/shell crossings
            let mut shell_stack: Vec<Option<ShellId>> = Vec::new();

            // for every classification but the first - it starts on the current faceuse
            for i in 1..curve_class.size() {
                let interval = curve_class.at(i)?;

                // get the face intersection point
                let face = match interval.start.face_object() {
                    Some(CacheObject::Face(f) | CacheObject::Surface(f)) => Some(f),
                    _ => None,
                };
                let uv = interval.start.uv_param();
                let Some(face) = face else {
                    // pFace->GetUpwardFaceuse() on NULL
                    return Err(null("crossing without face"));
                };
                let up = b
                    .face(face)?
                    .faceuse
                    .ok_or_else(|| null("face without face use"))?;
                let down = b
                    .faceuse(up)?
                    .mate
                    .ok_or_else(|| null("face use without mate"))?;
                let up_shell = b.faceuse(up)?.shell;
                let down_shell = b.faceuse(down)?.shell;

                // skip faces which are interior to a single shell
                if up_shell == down_shell {
                    continue;
                }

                // Evaluate the pFace->Surface
                let surface = b
                    .face(face)?
                    .surface
                    .as_ref()
                    .ok_or_else(|| null("crossing face without surface"))?;
                let pv0 = checked(k, surface.evaluate_point(k, uv))?;
                if matches!(pv0, Checked::Decides) {
                    return Ok(SM_UNSURE);
                }
                let normal =
                    match checked(k, surface.evaluate_normal(k, uv, Side::Left, Side::Left))? {
                        Checked::Value(n) => n,
                        Checked::Ignored => Vec3::UNDEF,
                        Checked::Decides => return Ok(SM_UNSURE),
                    };

                // figure out which shell is being exited and which shell is being
                // entered
                let ordered = if normal.dot(line_vec) < 0.0 {
                    [up_shell, down_shell]
                } else {
                    [down_shell, up_shell]
                };

                // parse the ordered shells into the shell stack
                for s in ordered {
                    if shell_stack.last().is_some_and(|last| *last == s) {
                        // this is a balanced intersection - remove the last stack member
                        shell_stack.pop();
                    } else {
                        // this is an unbalanced intersection - add it to the stack
                        shell_stack.push(s);
                    }
                }
            }

            // check the stack to pick the return value
            if shell_stack.is_empty() {
                // this is an inner shell of the infinite region
                return Ok(if outer.is_none() { SM_TRUE } else { SM_FALSE });
            }

            // when pOptOuterShell == NULL we were testing for an infinite region inner
            // shell
            let Some(outer) = outer else {
                return Ok(SM_FALSE);
            };

            // when pOptOuterShell == sShellStack[0] then this was an inner shell of the
            // given outer shell
            return Ok(if shell_stack.first().copied().flatten() == Some(outer) {
                SM_TRUE
            } else {
                SM_FALSE
            });
        }
        ii += 1;
    }

    // arrive here when no candidate ray produced a simple set of intersections
    Ok(SM_UNSURE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::Faceuse;

    #[test]
    fn region_order_rotates_the_circular_head_without_moving_other_links() {
        let mut brep = Brep::new();
        let ids = [
            ShellType::Wire,
            ShellType::Unknown,
            ShellType::Faceuse,
            ShellType::Vertex,
        ]
        .into_iter()
        .map(|shell_type| {
            brep.add_shell_record(Shell {
                shell_type,
                ..Shell::default()
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
        let region = brep
            .add_region_record(Region {
                shells: ids.clone(),
                ..Region::default()
            })
            .unwrap();
        order_region_shells(&mut Kernel::default(), &mut brep, region).unwrap();
        assert_eq!(
            brep.region(region).unwrap().shells,
            vec![ids[2], ids[3], ids[0], ids[1]]
        );
    }

    #[test]
    fn mated_shells_keep_first_seen_order_when_a_shell_reappears() {
        let mut brep = Brep::new();
        let source = brep.add_shell_record(Shell::default()).unwrap();
        let a = brep.add_shell_record(Shell::default()).unwrap();
        let b = brep.add_shell_record(Shell::default()).unwrap();
        let mut sources = Vec::new();
        for target in [b, a, b] {
            let mate = brep
                .add_faceuse_record(Faceuse {
                    shell: Some(target),
                    ..Faceuse::default()
                })
                .unwrap();
            sources.push(
                brep.add_faceuse_record(Faceuse {
                    shell: Some(source),
                    mate: Some(mate),
                    ..Faceuse::default()
                })
                .unwrap(),
            );
        }
        let shell = brep.shell_mut(source).unwrap();
        shell.shell_type = ShellType::Faceuse;
        shell.members = ShellMembers::Faceuses(sources);
        assert_eq!(
            brep.shell(source)
                .unwrap()
                .mated_shells(&mut Kernel::default(), &brep)
                .unwrap(),
            vec![b, a]
        );
        assert!(brep
            .shell(source)
            .unwrap()
            .is_closed(&brep, source)
            .unwrap());
    }

    #[test]
    fn changing_shell_type_preserves_the_list_head_and_members() {
        let members = vec![FaceuseId::from_index(2), FaceuseId::from_index(0)];
        let mut shell = Shell {
            shell_type: ShellType::Faceuse,
            members: ShellMembers::Faceuses(members.clone()),
            ..Shell::default()
        };
        shell.set_shell_type(16999);
        assert!(shell.get_faceuses().unwrap().is_empty());
        shell.set_shell_type(16022);
        assert_eq!(shell.get_faceuses().unwrap(), members);
    }

    #[test]
    fn move_faceuse_removes_then_post_inserts_and_relinks_siblings() {
        let mut brep = Brep::new();
        let from = brep.add_shell_record(Shell::default()).unwrap();
        let to = brep.add_shell_record(Shell::default()).unwrap();
        let mut ids = Vec::new();
        for shell in [from, from, to] {
            ids.push(
                brep.add_faceuse_record(Faceuse {
                    shell: Some(shell),
                    ..Faceuse::default()
                })
                .unwrap(),
            );
        }
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        for (shell, list) in [(from, vec![a, b]), (to, vec![c])] {
            let s = brep.shell_mut(shell).unwrap();
            s.shell_type = ShellType::Faceuse;
            s.members = ShellMembers::Faceuses(list);
        }
        // the head moves: the source list keeps its order, the recipient gains a tail
        move_faceuse(&mut brep, a, to).unwrap();
        assert_eq!(brep.shell(from).unwrap().get_faceuses().unwrap(), vec![b]);
        assert_eq!(brep.shell(to).unwrap().get_faceuses().unwrap(), vec![c, a]);
        assert_eq!(brep.faceuse(a).unwrap().shell, Some(to));
        assert_eq!(faceuse_sibling_links(&brep, a).unwrap(), (Some(c), Some(c)));
        assert_eq!(faceuse_sibling_links(&brep, b).unwrap(), (Some(b), Some(b)));
        // the last member leaves a NULL head
        move_faceuse(&mut brep, b, to).unwrap();
        assert_eq!(brep.shell(from).unwrap().members, ShellMembers::Empty);
        assert_eq!(
            brep.shell(to).unwrap().get_faceuses().unwrap(),
            vec![c, a, b]
        );
        // a face use missing from its shell's list is an inconsistent B-rep
        brep.faceuse_mut(c).unwrap().shell = Some(from);
        assert!(move_faceuse(&mut brep, c, to).is_err());
    }

    #[test]
    fn single_member_remove_and_post_insert() {
        let mut brep = Brep::new();
        let s = brep.add_shell_record(Shell::default()).unwrap();
        let t = brep.add_shell_record(Shell::default()).unwrap();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(
                brep.add_faceuse_record(Faceuse {
                    shell: Some(s),
                    ..Faceuse::default()
                })
                .unwrap(),
            );
        }
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        {
            let sh = brep.shell_mut(s).unwrap();
            sh.shell_type = ShellType::Faceuse;
            sh.members = ShellMembers::Faceuses(vec![a, b, c]);
        }
        assert_eq!(shell_list_size(&brep, s).unwrap(), 3);
        // removing the head moves it to the next member and closes the ring
        shell_remove_faceuse(&mut brep, s, a).unwrap();
        assert_eq!(brep.shell(s).unwrap().get_faceuses().unwrap(), vec![b, c]);
        assert_eq!(brep.faceuse(a).unwrap().shell, None);
        assert_eq!(faceuse_sibling_links(&brep, a).unwrap(), (None, None));
        assert_eq!(faceuse_sibling_links(&brep, b).unwrap(), (Some(c), Some(c)));
        // Remove of a member that is not in the list is upstream's SM_ERR status
        let e = shell_remove_faceuse(&mut brep, s, a).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
        // PostInsert into an empty shell makes its head; the type is not touched
        shell_post_insert_faceuse(&mut brep, t, a).unwrap();
        assert_eq!(
            brep.shell(t).unwrap().members,
            ShellMembers::Faceuses(vec![a])
        );
        assert_eq!(brep.shell(t).unwrap().shell_type, ShellType::Unknown);
        assert_eq!(brep.faceuse(a).unwrap().shell, Some(t));
        assert_eq!(faceuse_sibling_links(&brep, a).unwrap(), (Some(a), Some(a)));
        // a member still listed elsewhere is refused
        assert!(shell_post_insert_faceuse(&mut brep, t, b).is_err());
        // the last member leaves a NULL head
        shell_remove_faceuse(&mut brep, s, b).unwrap();
        shell_remove_faceuse(&mut brep, s, c).unwrap();
        assert_eq!(brep.shell(s).unwrap().members, ShellMembers::Empty);
        assert_eq!(shell_list_size(&brep, s).unwrap(), 0);
    }

    #[test]
    fn region_post_insert_appends_and_owns() {
        let mut brep = Brep::new();
        let r = brep.add_region_record(Region::default()).unwrap();
        let q = brep.add_region_record(Region::default()).unwrap();
        let s1 = brep.add_shell_record(Shell::default()).unwrap();
        let s2 = brep.add_shell_record(Shell::default()).unwrap();
        region_post_insert_shell(&mut brep, r, s1).unwrap();
        region_post_insert_shell(&mut brep, r, s2).unwrap();
        assert_eq!(brep.region(r).unwrap().shells, vec![s1, s2]);
        assert_eq!(brep.shell(s2).unwrap().region, Some(r));
        // listed twice, or still in another region's list: refused
        assert!(region_post_insert_shell(&mut brep, r, s1).is_err());
        assert!(region_post_insert_shell(&mut brep, q, s1).is_err());
        region_remove_shell(&mut brep, r, s1).unwrap();
        region_post_insert_shell(&mut brep, q, s1).unwrap();
        assert_eq!(brep.region(r).unwrap().shells, vec![s2]);
        assert_eq!(brep.region(q).unwrap().shells, vec![s1]);
    }
}

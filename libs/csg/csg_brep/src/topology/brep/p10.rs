//! `SmBrep` part 10: building a B-rep's topology from serialized B-rep data,
//! projecting and trimming with a curve, removing all topology, and testing a spine
//! edge for region closure.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                              | Upstream                                         |
//! |-----------------------------------|--------------------------------------------------|
//! | [`make_topology_from_data`]       | `SmBrep::MakeTopologyFromData`                   |
//! | [`MakeTopologyOptions`]           | its `bMakeUVTrimCurves` .. `bHealerIsEnabled` arguments |
//! | [`TopologyEnv`]                   | `rAllAttributes`, the objects' addresses         |
//! | [`project_and_trim`]              | `SmBrep::ProjectAndTrim`                         |
//! | [`TrimType`]                      | `SmTrimType`                                     |
//! | [`remove_all_topology`]           | `SmBrep::RemoveAllTopology`                      |
//! | [`remove_all_topology_notified`]  | `SmBrep::RemoveAllTopology` (caller's records)  |
//! | [`test_closure_split_region`]     | `SmBrep::TestClosureSplitRegion(SmBoolean, SmEdge*, ...)` |
//! | [`HEALER_VERSION`]                | `SM_HEALER_VERSION`                              |
//!
//! Deviations:
//! - **Out-params -> return values.** `rpNewShells` / `rpNewRegions` are returned in a
//!   [`ClosureSplit`].
//! - **Arena model.** Upstream stores the new objects' addresses in the B-rep data
//!   records (`m_pRegion`, `m_pShell1`, `m_pEdgeuse1`, ...); the port keeps them in
//!   local tables indexed like the records. `GetRegions` is
//!   [`super::p1::regions`]; `GetShells` reads the [`crate::topology::Region`] lists. Circular lists are vectors
//!   from their heads: `PostInsert` appends. For a non-manifold B-rep, upstream
//!   rewrites each edge use's `m_pNext` / `m_pLast` from the record's next indices;
//!   the port simulates those links and rebuilds each edge's list by following
//!   `m_pNext` from the head.
//! - **Malformed data.** Upstream indexes the record arrays with stored indices
//!   unchecked and dereferences the null object of a record never built (undefined
//!   behaviour, usually a crash). The port returns [`SolidError::InvalidInput`] there:
//!   an index past its array, an edge use whose loop belongs to no face, an edge use
//!   record linked as a loop or radial neighbour that built no edge use, the first
//!   edge use of a loop that is a wire edge use, a negative index whose negation
//!   overflows. It also returns [`SolidError::InvalidInput`] for graphs upstream
//!   would build but the arena model cannot hold (and upstream itself would loop on):
//!   a face use put on two shells, an edge use's loop use that is a vertex loop use,
//!   and a rewritten radial list that does not close at its head, passes through
//!   another edge's edge use, leaves out one of the edge's edge uses or has
//!   `m_pLast` disagreeing with `m_pNext`.
//! - **Identities.** Upstream objects are known by address. [`TopologyEnv::key_of`]
//!   names the new topology objects and [`TopologyEnv::next_key`] hands out the
//!   identities of the geometry copies (their attributes are listed under them in
//!   [`TopologyEnv::attributes`]). Each B-rep data curve or surface gets its copy as
//!   owner, as upstream; the copy is found again through a local table (a data
//!   object owned by anything else is [`SolidError::InvalidInput`]).
//! - **Notifications.** The `SM_NO_COPY` of an analytic edge curve and the
//!   `SM_NO_CHANGE_OWNER` of `SetOwner` go to the kernel's context; the
//!   construction and destruction notifications of new and deleted temporary objects
//!   are not delivered (as in the other topology ports).
//! - **Topology records.** Each new topology object gets its `SmTopology` record in
//!   [`TopologyEnv::records`] under its identity; `SetAllFlags`, `SetUserIndex1/2`
//!   and `SetUserPtr1` write it (B-rep data has no user pointer, so it is null).
//! - **Skeleton fields.** [`crate::topology::Face::shell`] is the shell the face's
//!   first placed face use joins (shell id 0 until then), the shell's `faces` list
//!   gains each face once, [`crate::topology::Edge::vertices`] are the edge record's
//!   vertices (vertex id 0 for an index past the array, which upstream never reads
//!   for an edge without uses) and `Loop::edges` stays empty.
//! - `SmPendingTopologyDelete`: an error leaves the objects built so far in the
//!   B-rep (upstream deletes the one being built).
//! - **Bounds.** Every loop is bounded by its record counts; the edge use walks of
//!   [`test_closure_split_region`] stop with [`SolidError::InvalidInput`] after
//!   visiting more edge uses than the B-rep holds (upstream would loop forever).
//!   There is no recursion.
//! - **Face-use closure.** The edge version calls the face use version of
//!   `TestClosureSplitRegion` ([`super::p6::test_closure_split_region`]) with a fresh
//!   [`Marks`] record per call (each call claims a new mark upstream); a NULL face use
//!   (upstream dereferences it) is [`SolidError::InvalidInput`].
//! - **UV trim curves.** `m_pUVTrimCurve = SM_CAST_PTR(SmBSplineCurve, pNewUV)`: a copy
//!   that is not a B-spline curve is made and owned but not stored.

use crate::curve::curve::p2::{BSPLINE_CURVE_TYPE, LINE_TYPE};
use crate::curve::curve::Curve;
use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::a_object;
use crate::geom::attribute::{AttributeHandle, AttributeStore, ObjectKey};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef, Shape};
use crate::geom::solution_array::Solution;
use crate::heal::face_props::HealerOpType;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::curve_class::p6::{MergeContext, MergeIntervalBrep};
use crate::intersect::point_class::ClassGeometry;
use crate::intersect::topology_solver::p1::{CurveRef, TopologySolver};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{max2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::query::face::p2::{CurveOnClassifyBrep, CurveOnClassifyObjects, FaceBrep};
use crate::surface::surface::Surface;
use crate::topology::brep::p1;
use crate::topology::brep::p4;
use crate::topology::brep::p6;
use crate::topology::brep_cache::{make_brep_cache, CacheObject, CacheSource};
use crate::topology::brep_data::{BrepData, TopologyData, NO_OBJECT, UNSURE};
use crate::topology::edge::NotifyEnv;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p3::{IntersectObject, IntersectObjects};
use crate::topology::shell::{ShellMembers, ShellType};
use crate::topology::topology::Topology;
use crate::topology::topology_traverser::Marks;
use crate::topology::{
    edge, vertex, Brep, BrepId, Edge, EdgeId, EdgeuseId, EdgeuseOwner, Face, FaceId, Faceuse,
    FaceuseId, Loop, LoopId, Loopuse, LoopuseId, LoopuseStart, Region, RegionId, Shell, ShellId,
    Vertex, VertexId, Vertexuse, VertexuseId, VertexuseOwner,
};

const OP_MAKE: &str = "topology::brep::p10::make_topology_from_data";
const OP_PROJECT: &str = "topology::brep::p10::project_and_trim";
const OP_REMOVE: &str = "topology::brep::p10::remove_all_topology";
const OP_CLOSURE: &str = "topology::brep::p10::test_closure_split_region";

/// `SM_HEALER_VERSION`: B-rep data written by an older healer is healed when read.
pub const HEALER_VERSION: u64 = 10;

/// `m_lUVCurve` values from this one up mean "no UV trim curve" (upstream tests
/// `< 999999`).
const NO_UV_CURVE: u64 = 999_999;

/// Shell map offset marking the inner (second) shell of an IGES solid's shell.
const INSIDE_SHELL: u64 = 20_000;

/// The optional arguments of [`make_topology_from_data`] (upstream defaults).
#[allow(clippy::struct_excessive_bools)] // upstream's six SmBoolean arguments
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakeTopologyOptions {
    /// `bMakeUVTrimCurves` (default `TRUE`): passed on to the healer.
    pub make_uv_trim_curves: bool,
    /// `bAddAnalytics` (default `TRUE`): copy surfaces with analytic information and
    /// replace NURBS edge and UV curves that are lines by lines.
    pub add_analytics: bool,
    /// `bForceSameTol` (default `FALSE`): use the data's positive zone tolerance as is.
    pub force_same_tol: bool,
    /// `bCheckUVTrimCurves` (default `FALSE`): not used upstream.
    pub check_uv_trim_curves: bool,
    /// `bAlwaysHeal` (default `FALSE`): heal whatever the data's healer version.
    pub always_heal: bool,
    /// `bHealerIsEnabled` (default `TRUE`): `FALSE` never heals.
    pub healer_enabled: bool,
}

impl Default for MakeTopologyOptions {
    fn default() -> Self {
        Self {
            make_uv_trim_curves: true,
            add_analytics: true,
            force_same_tol: false,
            check_uv_trim_curves: false,
            always_heal: false,
            healer_enabled: true,
        }
    }
}

/// What [`make_topology_from_data`] needs besides the B-rep and its data: object
/// identities and the attribute store.
pub struct TopologyEnv<'a> {
    /// `rAllAttributes`: the attributes the records' attribute indices refer to.
    pub all_attributes: &'a [AttributeHandle],
    /// The attribute lists of the B-rep data's curves and surfaces (by their
    /// [`crate::topology::brep_data::Keyed::key`]) and of the copies made from them.
    pub attributes: &'a mut AttributeStore,
    /// The identity (an upstream address) of each topology entity of the B-rep.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// The `SmTopology` records (flags, user values) of the topology objects, by
    /// identity; one is added for each object built.
    pub records: &'a mut crate::topology::topology::NotifyRecords,
    /// The next free identity for a geometry copy; incremented per copy.
    pub next_key: u64,
}

impl TopologyEnv<'_> {
    fn new_key(&mut self) -> Result<ObjectKey> {
        let key = ObjectKey(self.next_key);
        self.next_key = self
            .next_key
            .checked_add(1)
            .ok_or(SolidError::invalid_input(OP_MAKE, "object keys exhausted"))?;
        Ok(key)
    }
}

/// Upstream `SmTArray::operator[]` on a stored index: [`SolidError::InvalidInput`]
/// past the end (undefined behaviour upstream).
fn index(i: u64, len: usize, what: &'static str) -> Result<usize> {
    usize::try_from(i)
        .ok()
        .filter(|&i| i < len)
        .ok_or(SolidError::invalid_input(OP_MAKE, what))
}

fn at<T: Copy>(v: &[Option<T>], i: usize, what: &'static str) -> Result<T> {
    v.get(i)
        .copied()
        .flatten()
        .ok_or(SolidError::invalid_input(OP_MAKE, what))
}

/// `sm_AddAttributes` (unit `topology.brep_p1`): adds the attributes listed by index
/// in `rAllAttributes` to the object `entity`.
fn add_attributes(env: &mut TopologyEnv<'_>, entity: EntityRef, indices: &[u64]) -> Result<()> {
    let key = (env.key_of)(entity).key;
    add_attributes_to(env, key, indices)
}

fn add_attributes_to(env: &mut TopologyEnv<'_>, key: ObjectKey, indices: &[u64]) -> Result<()> {
    if indices.is_empty() {
        return Ok(());
    }
    // an index that does not fit a usize is past `rAllAttributes`
    let indices: Vec<usize> = indices
        .iter()
        .map(|&i| usize::try_from(i).unwrap_or(usize::MAX))
        .collect();
    p1::add_attributes(env.attributes, key, &indices, env.all_attributes)
}

/// The `SmTopology` record of a new object (`SmTopology()`).
fn new_record(env: &mut TopologyEnv<'_>, entity: EntityRef) {
    let key = (env.key_of)(entity).key;
    env.records.insert(key, Topology::new());
}

/// `SetAllFlags`, `SetUserIndex1`, `SetUserIndex2`, `SetUserPtr1` from a record.
fn set_topology(env: &mut TopologyEnv<'_>, entity: EntityRef, t: &TopologyData) {
    set_flags(env, entity, t.flags, t.user_index1, t.user_index2);
    let key = (env.key_of)(entity).key;
    env.records.or_default(key).set_user_ptr1(0);
}

/// `SetAllFlags`, `SetUserIndex1`, `SetUserIndex2`.
fn set_flags(env: &mut TopologyEnv<'_>, entity: EntityRef, flags: u64, user1: u64, user2: u64) {
    let key = (env.key_of)(entity).key;
    let r = env.records.or_default(key);
    r.set_all_flags(flags);
    r.set_user_index1(user1);
    r.set_user_index2(user2);
}

// ---- new topology objects (with their SmTopology records) ------------------------------

fn new_region(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Region) -> Result<RegionId> {
    let id = b.add_region_record(rec)?;
    new_record(env, EntityRef::Region(id));
    Ok(id)
}

fn new_shell(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Shell) -> Result<ShellId> {
    let id = b.add_shell_record(rec)?;
    new_record(env, EntityRef::Shell(id));
    Ok(id)
}

fn new_face(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Face) -> Result<FaceId> {
    let id = b.add_face_record(rec)?;
    new_record(env, EntityRef::Face(id));
    Ok(id)
}

fn new_faceuse(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Faceuse) -> Result<FaceuseId> {
    let id = b.add_faceuse_record(rec)?;
    new_record(env, EntityRef::Faceuse(id));
    Ok(id)
}

fn new_loop(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Loop) -> Result<LoopId> {
    let id = b.add_loop_record(rec)?;
    new_record(env, EntityRef::Loop(id));
    Ok(id)
}

fn new_loopuse(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Loopuse) -> Result<LoopuseId> {
    let id = b.add_loopuse_record(rec)?;
    new_record(env, EntityRef::Loopuse(id));
    Ok(id)
}

fn new_edge(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Edge) -> Result<EdgeId> {
    let id = b.add_edge_record(rec)?;
    new_record(env, EntityRef::Edge(id));
    Ok(id)
}

fn new_edgeuse(
    b: &mut Brep,
    env: &mut TopologyEnv<'_>,
    rec: crate::topology::Edgeuse,
) -> Result<EdgeuseId> {
    let id = b.add_edgeuse_record(rec)?;
    new_record(env, EntityRef::Edgeuse(id));
    // the edge use owns its UV trim curve under the caller's key (`SetOwner(m_pEdgeuse)`
    // here and in `SetUVTrimCurve`), recorded for the trims made later
    let owner = (env.key_of)(EntityRef::Edgeuse(id));
    b.set_geometry_owner_key(EntityRef::Edgeuse(id), Some(owner));
    Ok(id)
}

fn new_vertex(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Vertex) -> Result<VertexId> {
    let id = b.add_vertex_record(rec)?;
    new_record(env, EntityRef::Vertex(id));
    Ok(id)
}

fn new_vertexuse(b: &mut Brep, env: &mut TopologyEnv<'_>, rec: Vertexuse) -> Result<VertexuseId> {
    let id = b.add_vertexuse_record(rec)?;
    new_record(env, EntityRef::Vertexuse(id));
    Ok(id)
}

/// `pRegion->PostInsert(pShell)`.
fn post_insert_shell(b: &mut Brep, region: RegionId, shell: ShellId) -> Result<()> {
    b.region_mut(region)?.shells.push(shell);
    b.shell_mut(shell)?.region = Some(region);
    Ok(())
}

/// `SetOwner` of a geometry copy to its new topology owner, with the
/// `SM_NO_CHANGE_OWNER` notification upstream sends for a face (surface) or an edge
/// (3D curve). The new owner is not on the B-rep's lists yet, so its B-rep is null.
fn notify_owner_change(
    k: &mut Kernel,
    env: &mut TopologyEnv<'_>,
    this: ObjectRef,
    new_owner: ObjectRef,
    old_owner: Option<ObjectRef>,
) -> Result<()> {
    a_object::notify(
        env.attributes,
        Some(&mut k.context),
        NotifyOperation::ChangeOwner,
        this,
        Some(new_owner),
        None,
        old_owner,
    )
}

/// Moves the attributes of a B-rep data object (`GetAttributes`, then
/// `RemoveAttribute(attr, TRUE)` for each) and returns them for the copy.
fn take_attributes(env: &mut TopologyEnv<'_>, key: ObjectKey) -> Result<Vec<AttributeHandle>> {
    let attrs = env.attributes.attributes(key);
    for &h in &attrs {
        env.attributes.remove_attribute(key, h, true)?;
    }
    Ok(attrs)
}

fn give_attributes(
    env: &mut TopologyEnv<'_>,
    key: ObjectKey,
    attrs: &[AttributeHandle],
) -> Result<()> {
    for &h in attrs {
        env.attributes.add_attribute(key, h)?;
    }
    Ok(())
}

/// `SmBrep::MakeTopologyFromData`: builds the regions, shells, faces, loops, edges,
/// vertices and their uses described by `data` into `b` (normally a new
/// [`Brep::new_with_infinite_region`]), copying the data's geometry (with analytic
/// replacements when [`MakeTopologyOptions::add_analytics`]), then heals the B-rep
/// when the data's healer version is old (or always).
///
/// The B-rep's zone tolerance is the data's when positive and
/// [`MakeTopologyOptions::force_same_tol`], else the data's clamped to
/// `[1e-8, 1e-4]` times the vertex box size (`1e-5` times it without a positive data
/// tolerance). A data manifold flag of `UNSURE` is set to `FALSE` and recomputed at
/// the end. The kernel context's "doing Boolean" flag is set while the topology is
/// built.
///
/// # Errors
///
/// `SM_ERR` for a shell type above `2`, a loop whose edge uses built nothing;
/// `SM_ERR_NULL_POINTER` for a vertex edge use of a vertex without uses; those of
/// the geometry copies and analytic conversions; [`SolidError::InvalidInput`] for
/// malformed data (see the module notes); [`SolidError::Unported`] for indexed
/// attributes (`sm_AddAttributes`), a manifold flag to recompute
/// (`IsManifoldSolid`) and healing (`HealBrep`).
pub fn make_topology_from_data(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    options: MakeTopologyOptions,
) -> Result<()> {
    // the B-rep's topology owns its geometry under the caller's identities
    // (`TopologyEnv::key_of`)
    b.set_naming(crate::topology::NamingScheme::Caller);
    // tell context Brep->TopologyGraph is under construction
    let previous = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let result = make_topology(k, b, data, env, options);
    k.context.set_doing_boolean(previous);
    result
}

/// The ids built for the B-rep data records (upstream's `m_p*` record members).
struct Built {
    shell1: Vec<Option<ShellId>>,
    vertex: Vec<Option<VertexId>>,
    face: Vec<Option<FaceId>>,
    loop_: Vec<Option<LoopId>>,
    edge: Vec<Option<EdgeId>>,
    edgeuse1: Vec<Option<EdgeuseId>>,
    edgeuse2: Vec<Option<EdgeuseId>>,
    /// The face whose surface is the copy owning each data surface.
    surface_owner: Vec<Option<FaceId>>,
    /// The edge whose curve is the copy owning each data 3D curve.
    curve_owner: Vec<Option<EdgeId>>,
}

fn make_topology(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    options: MakeTopologyOptions,
) -> Result<()> {
    // when compiled without analytics - don't make them (the reference build defines
    // USE_ANALYTICS)
    let add_analytics = options.add_analytics;

    let mut calculate_is_manifold = false;
    if data.manifold == UNSURE {
        calculate_is_manifold = true;
        data.manifold = 0; // default to false until we can calculate it
    }

    // In pBrepData: copy all borrowed curve and surface structures into their own
    // memory
    if data.is_geom_borrowed {
        for c in &mut data.uv_curves {
            c.object = c.object.copy(k)?;
            c.key = env.new_key()?;
        }
        for c in &mut data.curves_3d {
            c.object = c.object.copy(k)?;
            c.key = env.new_key()?;
        }
        for s in &mut data.surfaces {
            s.object = s.object.copy(k)?;
            s.key = env.new_key()?;
        }
        data.is_geom_borrowed = false;
    }

    // attach any Brep attributes referenced by global index to this SmBrep Object
    add_attributes_to(env, b.key(), &data.attributes)?;

    // locals and inits
    let mut shell_map: Vec<u64> = Vec::new();
    b.set_editing_enabled(false);

    zone_tolerance(b, data, options.force_same_tol);

    let mut built = Built {
        shell1: vec![None; data.shells.len()],
        vertex: Vec::new(),
        face: vec![None; data.faces.len()],
        loop_: vec![None; data.loops.len()],
        edge: Vec::new(),
        edgeuse1: vec![None; data.edgeuses.len()],
        edgeuse2: vec![None; data.edgeuses.len()],
        surface_owner: vec![None; data.surfaces.len()],
        curve_owner: vec![None; data.curves_3d.len()],
    };

    build_regions(b, env, data, &mut built, &mut shell_map)?;
    build_vertices(k, b, env, data, &mut built)?;
    build_faces(k, b, data, env, &mut built, add_analytics)?;
    build_edges(k, b, data, env, &mut built, add_analytics)?;
    build_edgeuses(k, b, data, env, &mut built, add_analytics)?;
    if data.manifold == 0 {
        relink_radial_lists(b, data, &built)?;
    }
    fill_shells(b, env, data, &built, &shell_map)?;
    link_loops(b, data, &built)?;
    set_primary_edgeuses(b, data, &built)?;

    // Set isManifold after all objects are created and connected to one another
    if calculate_is_manifold {
        // pBrepData->m_bManifold = IsManifoldSolid()
        data.manifold = i32::from(crate::topology::brep::p2::is_manifold_solid(b)?);
    }

    // When requested and needed - find and heal common AssertValid problems
    let heal_this_brep = data.healer_version() < HEALER_VERSION;
    if options.healer_enabled && (heal_this_brep || options.always_heal) {
        // Run the Assert/Heal sequences as needed (status ignored upstream)
        p1::heal_brep(
            k,
            b,
            HealerOpType::All,
            None,
            None,
            None,
            options.make_uv_trim_curves,
        )?;
        // UV trim curves created inside the healer (Make_UVTrimCurves and
        // ClassifyLoops' snapshot write-back) have no caller owner yet; bind them to
        // their edge uses as upstream's SetUVTrimCurve(this) does.
        bind_uv_trim_owners(b, env.key_of)?;
    }
    let _ = options.check_uv_trim_curves; // SM_REF1: not used
    Ok(())
}

/// Takes the data's zone tolerance or computes one relative to the vertex box.
fn zone_tolerance(b: &mut Brep, data: &BrepData, force_same_tol: bool) {
    let mut size = 1.0;

    // Avoiding computation of the size estimate if it is not required
    if data.zone_tol_3d > 0.0 && force_same_tol {
        b.set_tolerance(data.zone_tol_3d);
        return;
    }
    // Typically 5 decimal places of accuracy is as much as we can expect from an
    // external object.
    if !data.vertices.is_empty() {
        let mut bbox = Extent3d::new();
        for v in &data.vertices {
            bbox.add_point(v.point);
        }
        let d1 = bbox.min().max_dimension();
        let d2 = bbox.max().max_dimension();
        size = max2(d1, d2);
    }

    // SM_USE_OLDTOL
    let tol = if data.zone_tol_3d > 0.0 {
        if data.zone_tol_3d > size * 1.0e-4 {
            size * 1.0e-4
        } else if data.zone_tol_3d < size * 1.0e-8 {
            size * 1.0e-8
        } else {
            data.zone_tol_3d
        }
    } else {
        size * 1.0e-5
    };
    b.set_tolerance(tol);
}

/// The region and shell sections: the implied infinite and void regions of an IGES
/// solid, or the regions as listed.
fn build_regions(
    b: &mut Brep,
    env: &mut TopologyEnv<'_>,
    data: &BrepData,
    built: &mut Built,
    shell_map: &mut Vec<u64>,
) -> Result<()> {
    let infinite = b.infinite_region().ok_or(SolidError::invalid_input(
        OP_MAKE,
        "B-rep without infinite region",
    ))?;
    if data.manifold == 1 && data.regions.len() == 1 {
        // smiges_ReadIges: a single solid region; the infinite region and the void
        // regions are implied.
        let rr = data.regions.first().ok_or(SolidError::sm_err(OP_MAKE))?;
        let solid = new_region(b, env, Region::default())?;
        add_attributes(env, EntityRef::Region(solid), &rr.topology.attributes)?;
        set_topology(env, EntityRef::Region(solid), &rr.topology);
        // Add region to Brep
        b.region_mut(solid)?.in_brep = true;

        // for every BrepData Shell - build 2 Brep SmShells
        for (ii, rs) in data.shells.iter().enumerate() {
            let s1 = new_shell(b, env, Shell::default())?;
            add_attributes(env, EntityRef::Shell(s1), &rs.topology.attributes)?;
            set_topology(env, EntityRef::Shell(s1), &rs.topology);
            let s2 = new_shell(b, env, Shell::default())?;
            let i = ii as u64;
            if ii == 0 {
                shell_map.push(i);
                shell_map.push(i + INSIDE_SHELL);
            } else {
                // InsertAt(1 + ii): the map holds 2 * ii entries
                let pos = ii + 1;
                if pos > shell_map.len() {
                    return Err(SolidError::invalid_input(OP_MAKE, "shell map position"));
                }
                shell_map.insert(pos, i + INSIDE_SHELL);
                shell_map.push(i);
            }
            if let Some(slot) = built.shell1.get_mut(ii) {
                *slot = Some(s1);
            }
            b.shell_mut(s1)?.shell_type = ShellType::Faceuse;
            b.shell_mut(s2)?.shell_type = ShellType::Faceuse;

            if ii == 0 {
                post_insert_shell(b, infinite, s1)?;
                post_insert_shell(b, solid, s2)?;
                b.region_mut(infinite)?.is_void = 1;
                b.region_mut(solid)?.is_void = 0;
            } else {
                // second shell is always inner shell and belongs to the void
                let void = new_region(
                    b,
                    env,
                    Region {
                        in_brep: true,
                        shells: Vec::new(),
                        is_void: 1,
                    },
                )?;
                post_insert_shell(b, solid, s2)?;
                post_insert_shell(b, void, s1)?;
            }
        }
        return Ok(());
    }

    // Regions just as they are specified: the first region is the infinite region.
    for (ii, rr) in data.regions.iter().enumerate() {
        let region = if ii == 0 {
            infinite
        } else {
            new_region(
                b,
                env,
                Region {
                    in_brep: true,
                    ..Region::default()
                },
            )?
        };

        add_attributes(env, EntityRef::Region(region), &rr.topology.attributes)?;
        set_topology(env, EntityRef::Region(region), &rr.topology);
        b.region_mut(region)?.is_void = rr.is_void;

        let mut jj = 0u64;
        while jj < rr.num_shells {
            let si = rr
                .start_shell
                .checked_add(jj)
                .ok_or(SolidError::invalid_input(OP_MAKE, "shell index overflow"))?;
            let si_usize = index(si, data.shells.len(), "shell index")?;
            let rs = data
                .shells
                .get(si_usize)
                .ok_or(SolidError::invalid_input(OP_MAKE, "shell index"))?;
            let s = new_shell(b, env, Shell::default())?;
            add_attributes(env, EntityRef::Shell(s), &rs.topology.attributes)?;
            set_topology(env, EntityRef::Shell(s), &rs.topology);

            shell_map.push(si);
            post_insert_shell(b, region, s)?;
            if let Some(slot) = built.shell1.get_mut(si_usize) {
                *slot = Some(s);
            }
            if rs.shell_type > 2 {
                return Err(SolidError::sm_err(OP_MAKE));
            }
            b.shell_mut(s)?.shell_type = match rs.shell_type {
                0 => ShellType::Faceuse,
                1 => ShellType::Wire,
                _ => ShellType::Vertex,
            };
            jj += 1;
        }
    }
    Ok(())
}

fn build_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut TopologyEnv<'_>,
    data: &BrepData,
    built: &mut Built,
) -> Result<()> {
    for rv in &data.vertices {
        let v = new_vertex(b, env, Vertex::default())?;
        add_attributes(env, EntityRef::Vertex(v), &rv.topology.attributes)?;
        set_topology(env, EntityRef::Vertex(v), &rv.topology);
        // add SmVertex to Brep - set its values
        let this = (env.key_of)(EntityRef::Vertex(v));
        let key_of = env.key_of;
        vertex::set_point(
            b,
            v,
            rv.point,
            env.records,
            env.attributes,
            Some(&mut k.context),
            this,
            &|vu| key_of(EntityRef::Vertexuse(vu)),
        )?;
        let tol = b.tolerance();
        b.vertex_mut(v)?.tolerance = max2(rv.zone_tol_3d, tol);
        built.vertex.push(Some(v));
    }
    Ok(())
}

fn build_faces(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    built: &mut Built,
    add_analytics: bool,
) -> Result<()> {
    for ii in 0..data.faces.len() {
        let rf = data
            .faces
            .get(ii)
            .cloned()
            .ok_or(SolidError::invalid_input(OP_MAKE, "face index"))?;

        // Allocate new face and faceuses (the shell is set when a face use is put
        // on one)
        let f = new_face(b, env, Face::new(ShellId::from_index(0)))?;
        add_attributes(env, EntityRef::Face(f), &rf.topology.attributes)?;
        set_topology(env, EntityRef::Face(f), &rf.topology);
        let fu1 = new_faceuse(b, env, Faceuse::default())?;
        let fu2 = new_faceuse(b, env, Faceuse::default())?;
        if let Some(slot) = built.face.get_mut(ii) {
            *slot = Some(f);
        }
        let face_ref = (env.key_of)(EntityRef::Face(f));

        // Set Face data
        let si = index(rf.surface, data.surfaces.len(), "surface index")?;
        let surface = face_surface(k, b, data, env, built, si, f, face_ref, add_analytics)?;

        let tol = b.tolerance();
        let rec = b.face_mut(f)?;
        let natural = surface.natural_uv_domain_with_nurb(k)?;
        rec.surface = Some(surface);
        rec.rectangular_trim = rf.rectangular_trim;
        rec.uv_domain = rf.uv_domain;
        // guarantee Face->Surface->UVDomain contains Face->UVDomain (Intersect, whose
        // status is ignored, Init()s the domain when they are disjoint)
        rec.uv_domain = if rec.uv_domain.are_disjoint(&natural, 0.0) {
            Extent2d::new()
        } else {
            rec.uv_domain.intersect(&natural)?
        };
        rec.tolerance = max2(rf.zone_tol_3d, tol);

        // Connect up face and faceuses and set FU fields
        rec.faceuse = Some(fu1); // FU1 is primary faceuse
        b.set_geometry_owner_key(EntityRef::Face(f), Some(face_ref));

        // the surface reads the face's tolerance through GetFace()
        b.sync_face_surface_tolerance(f)?;
        *b.faceuse_mut(fu1)? = Faceuse {
            face: Some(f),
            mate: Some(fu2),
            orientation: OrientType::Same,
            ..Faceuse::default()
        };
        *b.faceuse_mut(fu2)? = Faceuse {
            face: Some(f),
            mate: Some(fu1),
            orientation: OrientType::Opposite,
            ..Faceuse::default()
        };

        // loops
        let mut jj = 0u64;
        while jj < rf.num_loops {
            let li = rf
                .start_loop
                .checked_add(jj)
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop index overflow"))?;
            let li = index(li, data.loops.len(), "loop index")?;
            let rl = data
                .loops
                .get(li)
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop index"))?;
            build_loop(b, env, data, built, rl, li, f, fu1, fu2, jj == 0)?;
            jj += 1;
        }
    }
    Ok(())
}

/// The face's surface: a copy of the data surface (with analytic information when
/// asked) the first time, else a copy of the face surface it was first copied to.
fn face_surface(
    k: &mut Kernel,
    b: &Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    built: &mut Built,
    si: usize,
    f: FaceId,
    face_ref: ObjectRef,
    add_analytics: bool,
) -> Result<Surface> {
    let src = data
        .surfaces
        .get_mut(si)
        .ok_or(SolidError::invalid_input(OP_MAKE, "surface index"))?;
    if src.object.owner().is_none() {
        let attrs = take_attributes(env, src.key)?;

        // when asked and geometrically possible, copy pFaceSurf as an Analytic
        let mut copy = if add_analytics {
            src.object.copy_and_add_analytics(k)?
        } else {
            src.object.copy(k)?
        };
        let key = env.new_key()?;
        give_attributes(env, key, &attrs)?;
        let copy_ref = ObjectRef {
            type_code: copy.type_code(),
            key,
        };
        let old = copy.set_owner(Some(face_ref));
        notify_owner_change(k, env, copy_ref, face_ref, old)?;
        // points to its copy
        src.object.set_owner(Some(copy_ref));
        if let Some(slot) = built.surface_owner.get_mut(si) {
            *slot = Some(f);
        }
        return Ok(copy);
    }

    // If it has an owner, that owner is the common surface of an old composite face:
    // make a copy of it for this face.
    let owner_face =
        built
            .surface_owner
            .get(si)
            .copied()
            .flatten()
            .ok_or(SolidError::invalid_input(
                OP_MAKE,
                "data surface owned outside this B-rep",
            ))?;
    let owner = b
        .face(owner_face)?
        .surface
        .as_ref()
        .ok_or(SolidError::null_pointer(OP_MAKE))?;
    let mut copy = owner.copy(k)?;
    let key = env.new_key()?;
    let copy_ref = ObjectRef {
        type_code: copy.type_code(),
        key,
    };
    let old = copy.set_owner(Some(face_ref));
    notify_owner_change(k, env, copy_ref, face_ref, old)?;
    Ok(copy)
}

/// One loop of a face with its two loop uses (and vertex uses for a vertex loop).
fn build_loop(
    b: &mut Brep,
    env: &mut TopologyEnv<'_>,
    data: &BrepData,
    built: &mut Built,
    rl: &crate::topology::brep_data::LoopData,
    li: usize,
    f: FaceId,
    fu1: FaceuseId,
    fu2: FaceuseId,
    outer: bool,
) -> Result<()> {
    // Now allocate new loop and loopuses
    let l = new_loop(
        b,
        env,
        Loop {
            face: f,
            edges: Vec::new(),
            loopuse: None,
        },
    )?;
    let lu1 = new_loopuse(b, env, Loopuse::default())?;
    let lu2 = new_loopuse(b, env, Loopuse::default())?;
    if let Some(slot) = built.loop_.get_mut(li) {
        *slot = Some(l);
    }

    add_attributes(env, EntityRef::Loop(l), &rl.topology.attributes)?;
    set_topology(env, EntityRef::Loop(l), &rl.topology);
    add_attributes(env, EntityRef::Loopuse(lu1), &rl.lu_upper_attributes)?;
    add_attributes(env, EntityRef::Loopuse(lu2), &rl.lu_lower_attributes)?;

    // Connect up faceuses and loopuses and loops
    b.loop_mut(l)?.loopuse = Some(lu1);
    b.face_mut(f)?.loops.push(l);
    b.faceuse_mut(fu1)?.loopuses.push(lu1);
    b.faceuse_mut(fu2)?.loopuses.push(lu2);
    let orientation = if outer {
        OrientType::Same
    } else {
        OrientType::Opposite
    };

    let mut vertex_type = false;
    if rl.num_eu == 1 {
        let ei = index(rl.start_eu, data.edgeuses.len(), "loop edge use index")?;
        let reu = data
            .edgeuses
            .get(ei)
            .ok_or(SolidError::invalid_input(OP_MAKE, "loop edge use index"))?;
        if reu.eu_type == 1 {
            vertex_type = true; // Vertex type
        }
    }
    let start = |vertex_type: bool| {
        if vertex_type {
            LoopuseStart::Vertexuse(None)
        } else {
            LoopuseStart::Edgeuse(None)
        }
    };
    *b.loopuse_mut(lu1)? = Loopuse {
        faceuse: Some(fu1),
        loop_: Some(l),
        mate: Some(lu2),
        start: start(vertex_type),
        orientation,
    };
    *b.loopuse_mut(lu2)? = Loopuse {
        faceuse: Some(fu2),
        loop_: Some(l),
        mate: Some(lu1),
        start: start(vertex_type),
        orientation,
    };

    if rl.loop_type == 1 {
        let vi = index(rl.vertex, built.vertex.len(), "loop vertex index")?;
        let v = at(&built.vertex, vi, "loop vertex index")?;
        for lu in [lu1, lu2] {
            let vu = new_vertexuse(
                b,
                env,
                Vertexuse {
                    vertex: Some(v),
                    owner: VertexuseOwner::Loopuse(Some(lu)),
                    ..Vertexuse::default()
                },
            )?;
            b.loopuse_mut(lu)?.start = LoopuseStart::Vertexuse(Some(vu));
            b.vertex_mut(v)?.vertexuses.push(vu);
        }
    }
    Ok(())
}

/// The edge section: each edge with a copy of its 3D curve (a line when the data's
/// NURBS curve is one and analytics are asked for).
fn build_edges(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    built: &mut Built,
    add_analytics: bool,
) -> Result<()> {
    for ii in 0..data.edges.len() {
        let re = data
            .edges
            .get(ii)
            .cloned()
            .ok_or(SolidError::invalid_input(OP_MAKE, "edge index"))?;
        // the skeleton's vertex pair (upstream reaches the vertices through the uses)
        let vertex_of = |i: u64| {
            usize::try_from(i)
                .ok()
                .and_then(|i| built.vertex.get(i).copied().flatten())
                .unwrap_or(VertexId::from_index(0))
        };
        let e = new_edge(
            b,
            env,
            Edge::new([vertex_of(re.start_vertex), vertex_of(re.end_vertex)]),
        )?;
        add_attributes(env, EntityRef::Edge(e), &re.topology.attributes)?;
        set_topology(env, EntityRef::Edge(e), &re.topology);
        let edge_ref = (env.key_of)(EntityRef::Edge(e));

        let ci = index(re.curve, data.curves_3d.len(), "curve index")?;
        let curve = edge_curve(k, b, data, env, built, ci, e, edge_ref, &re, add_analytics)?;

        let tol = b.tolerance();
        let rec = b.edge_mut(e)?;
        rec.curve = Some(curve);
        rec.interval = re.interval;
        rec.tolerance = max2(re.zone_tol_3d, tol);
        b.set_geometry_owner_key(EntityRef::Edge(e), Some(edge_ref));
        built.edge.push(Some(e));
    }
    Ok(())
}

/// The edge's curve: the first time a line replacing the data's NURBS curve when
/// asked and it matches at the edge ends, else a copy; later a copy of the edge curve
/// it was first copied to.
fn edge_curve(
    k: &mut Kernel,
    b: &Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    built: &mut Built,
    ci: usize,
    e: EdgeId,
    edge_ref: ObjectRef,
    re: &crate::topology::brep_data::EdgeData,
    add_analytics: bool,
) -> Result<Curve> {
    let src = data
        .curves_3d
        .get_mut(ci)
        .ok_or(SolidError::invalid_input(OP_MAKE, "curve index"))?;
    // Prevent duplication by setting owner once curve has been copied one time
    if src.object.owner().is_none() {
        let src_key = src.key;
        let attrs = take_attributes(env, src_key)?;
        let src_ref = ObjectRef {
            type_code: src.object.type_code(),
            key: src_key,
        };

        let mut new: Option<(Curve, ObjectKey)> = None;
        if add_analytics && !src.object.is_analytic() {
            // Handle case of NURBS - see if we need to add analytic information
            let ivl = src.object.natural_interval();
            let created = src.object.create_analytic_curve(k, &ivl)?;
            // only keep lines
            if let Some(c) = created.filter(|c| c.is_kind_of(LINE_TYPE)) {
                let key = env.new_key()?;
                a_object::notify(
                    env.attributes,
                    Some(&mut k.context),
                    NotifyOperation::Copy,
                    src_ref,
                    Some(ObjectRef {
                        type_code: c.type_code(),
                        key,
                    }),
                    None,
                    None,
                )?;
                let ends_match = |t: f64| -> Result<bool> {
                    let or = src.object.evaluate_point(k, t)?;
                    let an = c.evaluate_point(k, t)?;
                    let too_far = or.distance(an) > EFF_ZERO * (1.0 + or.max_dimension());
                    Ok(!too_far)
                };
                if ends_match(re.interval.min())? && ends_match(re.interval.max())? {
                    new = Some((c, key));
                }
            }
        }

        // Just copy curve in its native form if it was not already created as a
        // analytic derived type
        let (mut curve, key) = match new {
            Some(n) => n,
            None => (src.object.copy(k)?, env.new_key()?),
        };
        give_attributes(env, key, &attrs)?;
        let curve_ref = ObjectRef {
            type_code: curve.type_code(),
            key,
        };
        let (_, change) = curve.set_owner(Some(edge_ref), None);
        if let Some(c) = change {
            notify_owner_change(k, env, curve_ref, edge_ref, c.old_owner)?;
        }
        // points to its copy
        let _ = src.object.set_owner(Some(curve_ref), None);
        if let Some(slot) = built.curve_owner.get_mut(ci) {
            *slot = Some(e);
        }
        return Ok(curve);
    }

    // If it has an owner, that owner is the common curve of an old composite edge:
    // make a copy of it for this edge.
    let owner_edge =
        built
            .curve_owner
            .get(ci)
            .copied()
            .flatten()
            .ok_or(SolidError::invalid_input(
                OP_MAKE,
                "data curve owned outside this B-rep",
            ))?;
    let owner = b
        .edge(owner_edge)?
        .curve
        .as_ref()
        .ok_or(SolidError::null_pointer(OP_MAKE))?;
    let mut curve = owner.copy(k)?;
    let key = env.new_key()?;
    let curve_ref = ObjectRef {
        type_code: curve.type_code(),
        key,
    };
    let (_, change) = curve.set_owner(Some(edge_ref), None);
    if let Some(c) = change {
        notify_owner_change(k, env, curve_ref, edge_ref, c.old_owner)?;
    }
    Ok(curve)
}

/// The edge use section: two edge uses and two vertex uses per loop or wire edge use
/// record, linked to the edge, its vertices and the loop uses or the wire shell.
fn build_edgeuses(
    k: &mut Kernel,
    b: &mut Brep,
    data: &mut BrepData,
    env: &mut TopologyEnv<'_>,
    built: &mut Built,
    add_analytics: bool,
) -> Result<()> {
    for ii in 0..data.edgeuses.len() {
        let reu = data
            .edgeuses
            .get(ii)
            .cloned()
            .ok_or(SolidError::invalid_input(OP_MAKE, "edge use index"))?;

        // done with Type = vertex at a pole in an edge loop
        if reu.eu_type == 3 {
            continue;
        }

        if reu.eu_type == 1 {
            // Corresponds to a Single Vertex Loop
            let vi = index(reu.vertex, built.vertex.len(), "edge use vertex index")?;
            let v = at(&built.vertex, vi, "edge use vertex index")?;
            let list = &b.vertex(v)?.vertexuses;
            // NER(pNewVU), NER(pNewVU->m_pNext): the next use of a non-empty
            // circular list is never null
            if list.is_empty() {
                return Err(SolidError::null_pointer(OP_MAKE));
            }
            continue;
        }

        // rEU.m_lEUType == 0 or 2, LoopEdge or WireEdge
        if reu.edge >= NO_OBJECT {
            continue;
        }
        let ei = index(reu.edge, data.edges.len(), "edge use edge index")?;
        let re = data
            .edges
            .get(ei)
            .ok_or(SolidError::invalid_input(OP_MAKE, "edge use edge index"))?;
        let e = at(&built.edge, ei, "edge use edge index")?;
        let start_vertex = at(
            &built.vertex,
            index(
                re.start_vertex,
                built.vertex.len(),
                "edge start vertex index",
            )?,
            "edge start vertex index",
        )?;
        let end_vertex = at(
            &built.vertex,
            index(re.end_vertex, built.vertex.len(), "edge end vertex index")?,
            "edge end vertex index",
        )?;

        // make 2 NewVUs and 2 NewEUs
        let vu1 = new_vertexuse(b, env, Vertexuse::default())?;
        let vu2 = new_vertexuse(b, env, Vertexuse::default())?;
        let eu1 = new_edgeuse(b, env, crate::topology::Edgeuse::default())?;
        let eu2 = new_edgeuse(b, env, crate::topology::Edgeuse::default())?;

        // Attach NewVU1 to NewEU1, NewVU2 to NewEU2
        b.edgeuse_mut(eu1)?.vertexuse = Some(vu1);
        b.vertexuse_mut(vu1)?.owner = VertexuseOwner::Edgeuse(Some(eu1));
        b.edgeuse_mut(eu2)?.vertexuse = Some(vu2);
        b.vertexuse_mut(vu2)?.owner = VertexuseOwner::Edgeuse(Some(eu2));

        add_attributes(env, EntityRef::Edgeuse(eu1), &reu.topology.attributes)?;
        set_topology(env, EntityRef::Edgeuse(eu1), &reu.topology);
        add_attributes(env, EntityRef::Edgeuse(eu2), &reu.mate_attributes)?;
        set_flags(
            env,
            EntityRef::Edgeuse(eu2),
            reu.mate_flags,
            reu.mate_user_index1,
            reu.mate_user_index2,
        );

        // connect NewStartVertex to 'same' oriented NewEU->NewVU pair
        let same = reu.orientation != 0;
        let (first, second) = if same { (vu1, vu2) } else { (vu2, vu1) };
        b.vertex_mut(start_vertex)?.vertexuses.push(first);
        b.vertexuse_mut(first)?.vertex = Some(start_vertex);
        b.vertex_mut(end_vertex)?.vertexuses.push(second);
        b.vertexuse_mut(second)?.vertex = Some(end_vertex);

        // Connect NewEUs to NewEdge making 'same' oriented NewEU the primary EU
        let (first, second) = if same { (eu1, eu2) } else { (eu2, eu1) };
        for eu in [first, second] {
            b.edge_mut(e)?.edgeuses.push(eu);
            b.edgeuse_mut(eu)?.edge = Some(e);
        }

        if reu.eu_type == 2 {
            // WireEdge Edgeuse
            let si = index(reu.shell, built.shell1.len(), "wire edge use shell index")?;
            let s = built.shell1.get(si).copied().flatten();
            b.edgeuse_mut(eu1)?.owner = EdgeuseOwner::Shell(s);
            b.edgeuse_mut(eu2)?.owner = EdgeuseOwner::Shell(s);
        } else {
            // Loop Edgeuse
            let li = index(reu.loop_, built.loop_.len(), "edge use loop index")?;
            let l = at(&built.loop_, li, "edge use of a loop no face built")?;
            let lu1 = b
                .loop_(l)?
                .loopuse
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop without loop use"))?;
            let lu2 = b.loopuse(lu1)?.mate;
            b.edgeuse_mut(eu1)?.owner = EdgeuseOwner::Loopuse(Some(lu1));
            b.edgeuse_mut(eu2)?.owner = EdgeuseOwner::Loopuse(lu2);

            // Put trim curve on EU1 if it is present
            if reu.uv_curve < NO_UV_CURVE {
                let ui = index(reu.uv_curve, data.uv_curves.len(), "UV curve index")?;
                // SM_CAST_PTR(SmBSplineCurve, pNewUV): NULL for a curve that is not an
                // SmBSplineCurve (the copy is owned by the edge use but not stored)
                let uv = uv_trim_curve(k, data, env, ui, eu1, add_analytics)?;
                b.edgeuse_mut(eu1)?.uv_trim_curve =
                    Some(uv).filter(|c| c.is_kind_of(BSPLINE_CURVE_TYPE));
            }
        }

        if let Some(slot) = built.edgeuse1.get_mut(ii) {
            *slot = Some(eu1);
        }
        if let Some(slot) = built.edgeuse2.get_mut(ii) {
            *slot = Some(eu2);
        }

        // set NewEU1.Orient == rEU.Orient
        let (o1, o2) = if same {
            (OrientType::Same, OrientType::Opposite)
        } else {
            (OrientType::Opposite, OrientType::Same)
        };
        b.edgeuse_mut(eu1)?.orientation = o1;
        b.edgeuse_mut(eu2)?.orientation = o2;
    }
    Ok(())
}

/// A copy of a data UV curve for an edge use (a line when it is one and analytics are
/// asked for), owned by the edge use. The caller stores it only when it is an
/// `SmBSplineCurve` (upstream `SM_CAST_PTR`).
fn uv_trim_curve(
    k: &mut Kernel,
    data: &BrepData,
    env: &mut TopologyEnv<'_>,
    ui: usize,
    eu: EdgeuseId,
    add_analytics: bool,
) -> Result<Curve> {
    let src = data
        .uv_curves
        .get(ui)
        .ok_or(SolidError::invalid_input(OP_MAKE, "UV curve index"))?;
    // save and remove attributes from UVCurve
    let attrs = take_attributes(env, src.key)?;

    let mut new: Option<Curve> = None;
    if add_analytics && !src.object.is_analytic() {
        let ivl = src.object.natural_interval();
        // only keep line curves
        new = src
            .object
            .create_analytic_curve(k, &ivl)?
            .filter(|c| c.is_kind_of(LINE_TYPE));
    }
    let mut curve = match new {
        Some(c) => c,
        None => src.object.copy(k)?,
    };
    let key = env.new_key()?;
    give_attributes(env, key, &attrs)?;
    let owner = (env.key_of)(EntityRef::Edgeuse(eu));
    let curve_ref = ObjectRef {
        type_code: curve.type_code(),
        key,
    };
    let (_, change) = curve.set_owner(Some(owner), None);
    if let Some(c) = change {
        // only a 3D curve owned by an edge notifies
        notify_owner_change(k, env, curve_ref, owner, c.old_owner)?;
    }
    Ok(curve)
}

/// The edge use built for a signed next index (`-i - 1` the mate edge use of record
/// `i`, `i` its edge use).
fn signed_edgeuse(built: &Built, index_value: i64) -> Result<EdgeuseId> {
    let (table, i) = if index_value < 0 {
        let i = index_value
            .checked_neg()
            .and_then(|i| i.checked_sub(1))
            .ok_or(SolidError::invalid_input(OP_MAKE, "edge use index"))?;
        (&built.edgeuse2, i)
    } else {
        (&built.edgeuse1, index_value)
    };
    let i = index(
        u64::try_from(i).unwrap_or(u64::MAX),
        table.len(),
        "edge use index",
    )?;
    at(table, i, "radial neighbour record built no edge use")
}

/// Reorders a non-manifold B-rep's radial edge use lists: each loop edge use's
/// `m_pNext` (and its successor's `m_pLast`) come from the record.
fn relink_radial_lists(b: &mut Brep, data: &BrepData, built: &Built) -> Result<()> {
    use std::collections::BTreeMap;
    let mut next: BTreeMap<EdgeuseId, EdgeuseId> = BTreeMap::new();
    let mut last: BTreeMap<EdgeuseId, EdgeuseId> = BTreeMap::new();
    let edges: Vec<EdgeId> = built.edge.iter().copied().flatten().collect();
    for &e in &edges {
        let list = &b.edge(e)?.edgeuses;
        for (i, &eu) in list.iter().enumerate() {
            let n = list.get((i + 1) % list.len()).copied().unwrap_or(eu);
            next.insert(eu, n);
            last.insert(n, eu);
        }
    }

    for (ii, reu) in data.edgeuses.iter().enumerate() {
        if matches!(reu.eu_type, 1..=3) {
            continue;
        }
        if reu.edge >= NO_OBJECT {
            continue;
        }
        let eu1 = at(&built.edgeuse1, ii, "edge use record built no edge use")?;
        let eu2 = at(&built.edgeuse2, ii, "edge use record built no edge use")?;
        let next_eu = signed_edgeuse(built, reu.next_eu)?;
        next.insert(eu1, next_eu);
        last.insert(next_eu, eu1);
        let mate_next = signed_edgeuse(built, reu.mate_next_eu)?;
        next.insert(eu2, mate_next);
        last.insert(mate_next, eu2);
    }

    // the lists from their (unchanged) heads
    let bound = b.edgeuses().len();
    for &e in &edges {
        let old = b.edge(e)?.edgeuses.clone();
        let Some(&head) = old.first() else {
            continue;
        };
        let mut list = Vec::new();
        let mut cur = head;
        loop {
            if b.edgeuse(cur)?.edge != Some(e) {
                return Err(SolidError::invalid_input(
                    OP_MAKE,
                    "radial list passes through another edge's edge use",
                ));
            }
            list.push(cur);
            let n = *next.get(&cur).ok_or(SolidError::invalid_input(
                OP_MAKE,
                "radial list member without successor",
            ))?;
            if last.get(&n) != Some(&cur) {
                return Err(SolidError::invalid_input(
                    OP_MAKE,
                    "radial list links disagree",
                ));
            }
            cur = n;
            if cur == head {
                break;
            }
            if list.len() > bound {
                return Err(SolidError::invalid_input(
                    OP_MAKE,
                    "radial list does not close at its head",
                ));
            }
        }
        if list.len() != old.len() {
            return Err(SolidError::invalid_input(
                OP_MAKE,
                "radial list leaves out an edge use of its edge",
            ));
        }
        b.edge_mut(e)?.edgeuses = list;
    }
    Ok(())
}

/// The regions block: each shell's vertex use, wire edge use or face uses.
fn fill_shells(
    b: &mut Brep,
    env: &mut TopologyEnv<'_>,
    data: &BrepData,
    built: &Built,
    shell_map: &[u64],
) -> Result<()> {
    let regions = p1::regions(b, None)?;
    let mut shell_count = 0usize;

    for region in regions {
        let shells = b.region(region)?.shells.clone();
        for s in shells {
            let mut inside = false;
            let mut si = *shell_map
                .get(shell_count)
                .ok_or(SolidError::invalid_input(OP_MAKE, "shell map index"))?;
            shell_count += 1;
            if si >= INSIDE_SHELL {
                inside = true;
                si -= INSIDE_SHELL;
            }
            let rs = data
                .shells
                .get(index(si, data.shells.len(), "shell index")?)
                .ok_or(SolidError::invalid_input(OP_MAKE, "shell index"))?;

            match b.shell(s)?.shell_type {
                ShellType::Vertex => {
                    // a single vertex shell: one vertex use connected to the shell
                    let vi = index(rs.vertex, built.vertex.len(), "shell vertex index")?;
                    let v = at(&built.vertex, vi, "shell vertex index")?;
                    let vu = new_vertexuse(
                        b,
                        env,
                        Vertexuse {
                            vertex: Some(v),
                            owner: VertexuseOwner::Shell(Some(s)),
                            ..Vertexuse::default()
                        },
                    )?;
                    b.shell_mut(s)?.members = ShellMembers::Vertexuse(vu);
                    b.vertex_mut(v)?.vertexuses.push(vu);
                }
                ShellType::Wire => {
                    let ei = index(rs.edge, built.edge.len(), "shell edge index")?;
                    let e = at(&built.edge, ei, "shell edge index")?;
                    let eu = eu1::primary_edgeuse(b, e)?;
                    b.shell_mut(s)?.members = eu.map_or(ShellMembers::Empty, ShellMembers::Edgeuse);
                }
                _ => {
                    let mut kk = 0u64;
                    while kk < rs.num_faceuses {
                        let fi = rs
                            .faceuse_start
                            .checked_add(kk)
                            .ok_or(SolidError::invalid_input(OP_MAKE, "face use index"))?;
                        let rfu = data
                            .faceuses
                            .get(index(fi, data.faceuses.len(), "face use index")?)
                            .ok_or(SolidError::invalid_input(OP_MAKE, "face use index"))?;
                        let fx = index(rfu.face, built.face.len(), "face use face index")?;
                        let face = at(&built.face, fx, "face use face index")?;
                        let mut fu1 = b
                            .face(face)?
                            .faceuse
                            .ok_or(SolidError::invalid_input(OP_MAKE, "face without face use"))?;
                        let mut fu2 = b
                            .faceuse(fu1)?
                            .mate
                            .ok_or(SolidError::invalid_input(OP_MAKE, "face use without mate"))?;
                        if inside {
                            // Take other side if not inside
                            core::mem::swap(&mut fu1, &mut fu2);
                        }
                        let fu = if rfu.orientation != 0 { fu1 } else { fu2 };
                        post_insert_faceuse(b, s, fu)?;
                        add_attributes(env, EntityRef::Faceuse(fu), &rfu.topology.attributes)?;
                        set_topology(env, EntityRef::Faceuse(fu), &rfu.topology);
                        kk += 1;
                    }
                }
            }
        }
    }
    Ok(())
}

/// `pShell->PostInsert(pFU)`, keeping the skeleton's face lists.
fn post_insert_faceuse(b: &mut Brep, s: ShellId, fu: FaceuseId) -> Result<()> {
    if b.faceuse(fu)?.shell.is_some() {
        return Err(SolidError::invalid_input(
            OP_MAKE,
            "face use put on a second shell",
        ));
    }
    let face = b.faceuse(fu)?.face;
    let mate_on_shell = match b.faceuse(fu)?.mate {
        Some(m) => b.faceuse(m)?.shell.is_some(),
        None => false,
    };
    b.faceuse_mut(fu)?.shell = Some(s);
    let shell = b.shell_mut(s)?;
    match &mut shell.members {
        ShellMembers::Faceuses(list) => list.push(fu),
        members => *members = ShellMembers::Faceuses(vec![fu]),
    }
    if let Some(f) = face {
        if !shell.faces.contains(&f) {
            shell.faces.push(f);
        }
        if !mate_on_shell {
            b.face_mut(f)?.shell = s;
        }
    }
    Ok(())
}

/// The loops block: the clockwise / counter-clockwise edge use links of each edge
/// loop and its loop uses' start edge uses.
fn link_loops(b: &mut Brep, data: &BrepData, built: &Built) -> Result<()> {
    for rl in &data.loops {
        if rl.loop_type == 1 {
            continue;
        }

        // Set up last Edgeuses in counter clockwise orientation (a ULONG converted to
        // long, as upstream)
        let mut prev = rl.start_eu.wrapping_add(rl.num_eu).wrapping_sub(1) as i64;
        let mut last: Option<(EdgeuseId, EdgeuseId)> = None;
        while prev > -1 {
            let pi = index(prev as u64, data.edgeuses.len(), "loop edge use index")?;
            let rlast = data
                .edgeuses
                .get(pi)
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop edge use index"))?;
            prev -= 1;
            if rlast.eu_type != 3 {
                if let Some(e1) = built.edgeuse1.get(pi).copied().flatten() {
                    let e2 = at(&built.edgeuse2, pi, "edge use without mate")?;
                    last = Some((e1, e2));
                    break;
                }
            }
        }
        let Some((mut last1, mut last2)) = last else {
            return Err(SolidError::sm_err(OP_MAKE)); // Big problems here
        };

        let mut first = true;
        let mut jj = 0u64;
        while jj < rl.num_eu {
            let ei = rl
                .start_eu
                .checked_add(jj)
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop edge use index"))?;
            jj += 1;
            let ei = index(ei, data.edgeuses.len(), "loop edge use index")?;
            let reu = data
                .edgeuses
                .get(ei)
                .ok_or(SolidError::invalid_input(OP_MAKE, "loop edge use index"))?;
            if reu.edge >= NO_OBJECT {
                continue;
            }
            if reu.eu_type == 3 {
                continue;
            }

            let cur1 = at(
                &built.edgeuse1,
                ei,
                "loop edge use record built no edge use",
            )?;
            let cur2 = at(
                &built.edgeuse2,
                ei,
                "loop edge use record built no edge use",
            )?;
            if first {
                first = false;
                for cur in [cur1, cur2] {
                    let EdgeuseOwner::Loopuse(Some(lu)) = b.edgeuse(cur)?.owner else {
                        return Err(SolidError::invalid_input(
                            OP_MAKE,
                            "first edge use of a loop is not a loop edge use",
                        ));
                    };
                    set_loopuse_start(b, lu, cur)?;
                }
            }

            b.edgeuse_mut(last1)?.ccw = Some(cur1);
            b.edgeuse_mut(cur1)?.cw = Some(last1);
            b.edgeuse_mut(last2)?.cw = Some(cur2);
            b.edgeuse_mut(cur2)?.ccw = Some(last2);
            last1 = cur1;
            last2 = cur2;
        }
    }
    Ok(())
}

/// `pLU->m_pEUorVU = pEU` (the loop use type stays as built).
fn set_loopuse_start(b: &mut Brep, lu: LoopuseId, eu: EdgeuseId) -> Result<()> {
    let rec = b.loopuse_mut(lu)?;
    match rec.start {
        LoopuseStart::Edgeuse(_) => {
            rec.start = LoopuseStart::Edgeuse(Some(eu));
            Ok(())
        }
        LoopuseStart::Vertexuse(_) | LoopuseStart::Unknown => Err(SolidError::invalid_input(
            OP_MAKE,
            "edge use set as the start of a vertex loop use",
        )),
    }
}

/// The edges block: each edge's primary edge use (`ResetPrimaryEdgeuse`, whose status
/// is ignored).
fn set_primary_edgeuses(b: &mut Brep, data: &BrepData, built: &Built) -> Result<()> {
    let n_eu = data.edgeuses.len();
    for (ii, re) in data.edges.iter().enumerate() {
        let e = at(&built.edge, ii, "edge index")?;
        let prim = if re.prim_eu < 0 {
            let i = re
                .prim_eu
                .checked_neg()
                .and_then(|i| i.checked_sub(1))
                .ok_or(SolidError::invalid_input(OP_MAKE, "primary edge use index"))?;
            let i = index(
                u64::try_from(i).unwrap_or(u64::MAX),
                n_eu,
                "primary edge use index",
            )?;
            built.edgeuse2.get(i).copied().flatten()
        } else if (re.prim_eu as u64) < n_eu as u64 {
            built.edgeuse1.get(re.prim_eu as usize).copied().flatten()
        } else {
            None
        };

        // This takes care of next/last pointers.
        match edge::reset_primary_edgeuse(b, e, prim) {
            Err(err) if err.is_unported() => return Err(err),
            Err(
                err @ (SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }
                | SolidError::NotFound { .. }),
            ) => return Err(err),
            // the status is ignored upstream
            Ok(()) | Err(_) => {}
        }
    }
    Ok(())
}

// ---- ProjectAndTrim ---------------------------------------------------------------------

/// `SmTrimType`: what [`project_and_trim`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimType {
    /// `SM_TT_KEEP_POINT`: keep the side of the trim curves the reference point is on.
    KeepPoint,
    /// `SM_TT_DELETE_POINT`: remove the side the reference point is on.
    DeletePoint,
    /// `SM_TT_SPLIT`: keep both sides.
    Split,
}

/// The optional arguments of [`project_and_trim`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectAndTrimOptions<'a> {
    /// `cpdOpt3DApproximationTol`.
    pub approximation_tol_3d: Option<f64>,
    /// `cpdOptAngleTolRadians`.
    pub angle_tol_radians: Option<f64>,
    /// `pOptSubsetFaces`: only search this subset of faces.
    pub subset_faces: Option<&'a [FaceId]>,
}

/// The identities [`project_and_trim`] needs for its topology solve.
pub struct ProjectEnv<'a> {
    /// This B-rep's id in the caller's shape store.
    pub brep_id: BrepId,
    /// Identity of each cache object of the B-rep.
    pub keys: &'a dyn Fn(CacheObject) -> ObjectRef,
    /// Owners of generated curves and edge uses, as for `Curve::edge`.
    pub owner_of: &'a dyn Fn(ObjectRef) -> Option<ObjectRef>,
}

/// The output of `CreateParallelProjectionCurves`.
struct ProjectionCurves {
    curves_3d: Vec<Curve>,
    uv_curves: Vec<Option<Curve>>,
    faces: Vec<FaceId>,
}

fn create_parallel_projection_curves(
    k: &Kernel,
    b: &mut Brep,
    curve: &Curve,
    direction: Vec3,
    tol: f64,
    angle_tol: Option<f64>,
    subset: Option<&[FaceId]>,
) -> Result<ProjectionCurves> {
    // CreateParallelProjectionCurves(ctx, curve, dir, NULL, &dTol, cpdOptAngleTolRadians,
    // &s3D, &sUV, &sFacesOfCurves, pOptSubsetFaces)
    let out = p1::create_parallel_projection_curves(
        k,
        b,
        curve,
        direction,
        &p1::ParallelProjectionOptions {
            projection_distance: None,
            approx_tol_3d: Some(tol),
            angle_tol_radians: angle_tol,
            subset_faces: subset,
            want_3d: true,
            want_uv: true,
            want_faces: true,
        },
    )?;
    Ok(ProjectionCurves {
        curves_3d: out.curves_3d.unwrap_or_default(),
        uv_curves: out.uv_curves.unwrap_or_default(),
        faces: out.faces_of_uv_curves.unwrap_or_default(),
    })
}

/// The classification objects of [`merge_curves_on_surface`] over the B-rep the curves
/// merge into: the handles are those of [`IntersectObjects`] over one B-rep (side
/// `false`), curve `i` being [`IntersectObject::Curve`]`(i)` and its UV curve
/// [`IntersectObject::UvCurve`]`(i, false)`. Upstream's `MergeIntervals` edits the B-rep
/// through the classification's `SmFace*`, `SmEdge*` and `SmVertex*` pointers, so the
/// adapter holds the B-rep mutably and hands the classification read-only views of it;
/// `CurveOnClassify` classifies on that B-rep in place ([`CurveOnClassifyBrep`]).
struct MergeCurvesWorld<'a, 'e> {
    b: &'a mut Brep,
    curves_3d: Vec<Curve>,
    uv: Vec<Option<Curve>>,
    env: &'a mut NotifyEnv<'e>,
    curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
    /// The surfaces behind [`IntersectObject::FreeSurface`] (the intersection surface).
    free_surfaces: Vec<Surface>,
}

impl MergeCurvesWorld<'_, '_> {
    /// The read-only objects over the same B-rep and curves.
    fn view(&self) -> IntersectObjects<'_> {
        IntersectObjects {
            b: &*self.b,
            other: &*self.b,
            side: false,
            curves_3d: &self.curves_3d,
            uv1: &self.uv,
            uv2: &[],
        }
    }
}

impl ClassGeometry<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn classification_vertex_point(&self, o: IntersectObject) -> Option<Point3> {
        self.view().classification_vertex_point(o)
    }
    fn topology_zone_tol_3d(&self, o: IntersectObject) -> Option<f64> {
        self.view().topology_zone_tol_3d(o)
    }
    fn curve(&self, o: IntersectObject) -> Option<&Curve> {
        self.view().object_curve(o)
    }
    fn surface(&self, o: IntersectObject) -> Option<&Surface> {
        match o {
            IntersectObject::FreeSurface(i) => self.free_surfaces.get(i),
            _ => self.view().object_surface(o),
        }
    }
    fn volume(&self, _o: IntersectObject) -> Option<&crate::surface::volume::Volume> {
        None
    }
    fn point(&self, _o: IntersectObject) -> Option<Point3> {
        None
    }
    fn topology_entity(&self, o: IntersectObject) -> Option<(&Brep, EntityRef)> {
        self.view().object_entity(o)
    }
    fn topology_handle(&self, e: EntityRef) -> Option<IntersectObject> {
        self.view().topology_handle(e)
    }
    fn topology_handle_in(&self, brep: &Brep, e: EntityRef) -> Option<IntersectObject> {
        self.view().topology_handle_in(brep, e)
    }
}

impl CurveOnClassifyObjects<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn face(&self, f: FaceId) -> IntersectObject {
        self.view().face(f)
    }
    fn vertex(&self, v: VertexId) -> IntersectObject {
        self.view().vertex(v)
    }
    fn edge(&self, e: EdgeId) -> IntersectObject {
        self.view().edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> IntersectObject {
        self.view().vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> IntersectObject {
        self.view().edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> IntersectObject {
        self.view().edge_curve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> IntersectObject {
        self.view().trim_curve(eu)
    }
    fn face_surface(&self, f: FaceId) -> Option<IntersectObject> {
        self.view().face_surface(f)
    }
    fn loop_id(&self, tgt_loop: IntersectObject) -> Option<LoopId> {
        self.view().loop_id(tgt_loop)
    }
}

impl ClassGeometryMut<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn curve_mut(&mut self, o: IntersectObject) -> Option<&mut Curve> {
        match o {
            IntersectObject::Curve(i) => self.curves_3d.get_mut(i),
            _ => None,
        }
    }
    fn topology_entity_mut(&mut self, o: IntersectObject) -> Option<(&mut Brep, EntityRef)> {
        match o {
            IntersectObject::Topology(e, _) => Some((&mut *self.b, e)),
            _ => None,
        }
    }
}

impl MergeIntervalBrep<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn merge_context(&mut self, o: IntersectObject) -> Option<MergeContext<'_, IntersectObject>> {
        let IntersectObject::Topology(target, side) = o else {
            return None;
        };
        if !matches!(
            target,
            EntityRef::Vertex(_) | EntityRef::Edge(_) | EntityRef::Face(_) | EntityRef::Region(_)
        ) {
            return None;
        }
        Some(MergeContext {
            brep: &mut *self.b,
            target,
            handle: Box::new(move |e| IntersectObject::Topology(e, side)),
            notifications: NotifyEnv {
                records: &mut *self.env.records,
                attributes: &mut *self.env.attributes,
                context: self.env.context.as_deref_mut(),
                key_of: self.env.key_of,
            },
            curve_ref: self.curve_ref,
        })
    }
}

impl FaceBrep for MergeCurvesWorld<'_, '_> {
    fn brep(&self) -> &Brep {
        self.b
    }
}

impl CurveOnClassifyBrep<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.b
    }
}

impl crate::topology::brep::p2::MergeCurvesObjects<IntersectObject> for MergeCurvesWorld<'_, '_> {
    fn edge_of(&self, object: IntersectObject) -> Option<EdgeId> {
        match object {
            IntersectObject::Topology(EntityRef::Edge(e), _) => Some(e),
            _ => None,
        }
    }
    fn face_of(&self, object: IntersectObject) -> Option<FaceId> {
        match object {
            IntersectObject::Topology(EntityRef::Face(f), _) => Some(f),
            _ => None,
        }
    }
}

/// `MergeCurvesOnSurface(*pSurf, dTol, s3DCurves, &sUVCurves, sNewFaces, sCurveEdges,
/// bUseProjection)` with the header defaults for the rest (no intersection surface,
/// `bUVSpaceOkay = TRUE`; `uv_curves` `None` for a null `pOptUVCurves`):
/// [`crate::topology::brep::p2::merge_curves_on_surface`] over `b` itself, its edits
/// notified through `env` (new edge curves named by `curve_ref`, from the edge and the
/// curve's native type code). Called by `ProjectAndTrim` (`SmBrep.cpp:25074`, default
/// `bUseProjection = FALSE`) and by `blend.hcr` (`SmHCR.cpp:1229`, `TRUE`). Returns
/// the new faces and the edges of the curves.
///
/// # Errors
///
/// Those of [`crate::topology::brep::p2::merge_curves_on_surface`].
#[allow(clippy::too_many_arguments)]
pub fn merge_curves_on_surface(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    face: FaceId,
    tol: f64,
    curves_3d: &[Curve],
    uv_curves: Option<&[Curve]>,
    use_projection: bool,
) -> Result<(Vec<FaceId>, Vec<EdgeId>)> {
    // curve i is IntersectObject::Curve(i), its UV curve IntersectObject::UvCurve(i, false)
    let curves: Vec<IntersectObject> = (0..curves_3d.len()).map(IntersectObject::Curve).collect();
    let uv: Option<Vec<Option<IntersectObject>>> = uv_curves.map(|uv| {
        (0..uv.len())
            .map(|i| Some(IntersectObject::UvCurve(i, false)))
            .collect()
    });
    let mut world = MergeCurvesWorld {
        b,
        curves_3d: curves_3d.to_vec(),
        uv: uv_curves
            .unwrap_or_default()
            .iter()
            .cloned()
            .map(Some)
            .collect(),
        env,
        curve_ref,
        free_surfaces: Vec::new(),
    };
    let merged = crate::topology::brep::p2::merge_curves_on_surface(
        k,
        &mut world,
        face,
        tol,
        &curves,
        uv.as_deref(),
        crate::topology::brep::p2::MergeCurvesOptions {
            use_projection,
            ..crate::topology::brep::p2::MergeCurvesOptions::default()
        },
    )?;
    Ok((merged.new_faces, merged.edges))
}

/// `MergeCurvesOnSurface(*pSurf, dTol, s3DCurves, pOptUVCurves, sNewFaces, sCurveEdges,
/// FALSE, pOptIntersectionSurface)` (`bUVSpaceOkay = TRUE`): as
/// [`merge_curves_on_surface`], with one optional UV curve per curve (`None` for a null
/// entry of upstream's array, which leaves that curve without one) and the optional
/// intersection surface the curves came
/// from (`SmPlaneCutter::MergeIntersection`'s cutting plane), which every curve
/// classification gets as its second surface
/// ([`IntersectObject::FreeSurface`]`(0)`) for `sm_RefineCCIToSurfaces`'s
/// surface/surface/surface refinement. The curves are edited in place, as upstream's
/// refinement edits the caller's curves (`sm_SetXSectCurveThroughPoint`). Returns the
/// new faces and the edges of the curves.
///
/// # Errors
///
/// Those of [`crate::topology::brep::p2::merge_curves_on_surface`].
#[allow(clippy::too_many_arguments)]
pub fn merge_curves_on_surface_through_surface(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    face: FaceId,
    tol: f64,
    curves_3d: &mut [Curve],
    uv_curves: Option<&[Option<Curve>]>,
    intersection_surface: Option<Surface>,
) -> Result<(Vec<FaceId>, Vec<EdgeId>)> {
    // curve i is IntersectObject::Curve(i), its UV curve IntersectObject::UvCurve(i, false)
    // (none for a null entry of upstream's array)
    let curves: Vec<IntersectObject> = (0..curves_3d.len()).map(IntersectObject::Curve).collect();
    let uv: Option<Vec<Option<IntersectObject>>> = uv_curves.map(|uv| {
        uv.iter()
            .enumerate()
            .map(|(i, c)| c.as_ref().map(|_| IntersectObject::UvCurve(i, false)))
            .collect()
    });
    let has_surface = intersection_surface.is_some();
    let mut world = MergeCurvesWorld {
        b,
        curves_3d: curves_3d.to_vec(),
        uv: uv_curves.unwrap_or_default().to_vec(),
        env,
        curve_ref,
        free_surfaces: intersection_surface.into_iter().collect(),
    };
    let merged = crate::topology::brep::p2::merge_curves_on_surface(
        k,
        &mut world,
        face,
        tol,
        &curves,
        uv.as_deref(),
        crate::topology::brep::p2::MergeCurvesOptions {
            intersection_surface: has_surface.then_some(IntersectObject::FreeSurface(0)),
            ..crate::topology::brep::p2::MergeCurvesOptions::default()
        },
    );
    // the refinement's edits of the curves reach the caller's
    for (c, edited) in curves_3d.iter_mut().zip(world.curves_3d) {
        *c = edited;
    }
    let merged = merged?;
    Ok((merged.new_faces, merged.edges))
}

/// `sm_FindSideOfCurve` (unit `topology.brep_p1`): the `SmBoolean` it sets on success.
fn find_side_of_curve(
    k: &Kernel,
    curve: &Curve,
    point: Point3,
    direction: Vec3,
    tol: f64,
) -> Result<i32> {
    p1::find_side_of_curve(k, curve, point, direction, tol).map(i32::from)
}

/// `GetFaces` (unit `topology.brep_p1`).
fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    p1::faces(b, None)
}

/// `SmFace::GetVertices(rVertices)` (unit `topology.face_p4`; default arguments).
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    crate::topology::face::p4::vertices(b, f, None, false, None)
}

/// `SmFace::GetEdges(rEdges)` (unit `topology.face_p4`; default arguments).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

/// `sm_GetPointFromSol(sSol, start)` ([`crate::topology::brep::p2::point_from_solution`]).
fn point_from_solution(
    k: &Kernel,
    b: &Brep,
    solution: &Solution<CacheObject>,
    start: usize,
) -> Result<Point3> {
    crate::topology::brep::p2::point_from_solution(k, b, solution, start)
}

fn remove_faces(
    k: &mut Kernel,
    b: &mut Brep,
    notify: &mut NotifyEnv<'_>,
    faces: &[FaceId],
) -> Result<()> {
    // SmBrep::RemoveFaces (topology.brep_p5) reads the B-rep's own m_bEditingEnabled
    let editing = b.editing_enabled();
    super::p5::remove_faces_notified(k, b, editing, faces, notify)
}

/// `SmBrep::ProjectAndTrim`: projects `curve` along `direction` onto the B-rep's faces
/// (or `subset_faces`), imprints the projections as edges, and unless
/// [`TrimType::Split`] removes the faces on the side of the projection plane the
/// reference point selects. Editing is enabled; the kernel context's "doing Boolean"
/// flag is set meanwhile. The merges' and face removals' B-rep edits are notified
/// through `notify`.
///
/// `curve` is one of the `SmBSplineCurve` classes (upstream `const SmBSplineCurve&`,
/// whose virtuals dispatch on the curve's class). Whole-face side classification
/// evaluates the caller's curve itself for the reference, vertex, edge-midpoint and
/// ray-fallback queries, not a knot-tuned copy.
///
/// # Errors
///
/// Those of the projection, merge, side classification and face removal;
/// [`SolidError::Unported`] for `CreateParallelProjectionCurves` and the other
/// unported steps.
pub fn project_and_trim(
    k: &mut Kernel,
    b: &mut Brep,
    env: &ProjectEnv<'_>,
    notify: &mut NotifyEnv<'_>,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
    trim_type: TrimType,
    options: &ProjectAndTrimOptions<'_>,
) -> Result<()> {
    let tol = b.tolerance() / 4.0;

    // Intersect this Brep with the plane defined by the curve and the direction.
    let projected = create_parallel_projection_curves(
        k,
        b,
        curve,
        direction,
        tol,
        options.angle_tol_radians,
        options.subset_faces,
    )?;

    b.set_editing_enabled(true);
    let previous = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let result = project_and_trim_body(
        k, b, env, notify, curve, direction, reference, trim_type, options, &projected, tol,
    );
    // the UV trim curves the projection's face queries and the merges' edge splits
    // made, owned by their edge uses as upstream's SetUVTrimCurve does
    let bound = bind_uv_trim_owners(b, notify.key_of);
    let result = result.and(bound);
    k.context.set_doing_boolean(previous);
    result
}

fn project_and_trim_body(
    k: &mut Kernel,
    b: &mut Brep,
    env: &ProjectEnv<'_>,
    notify: &mut NotifyEnv<'_>,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
    trim_type: TrimType,
    options: &ProjectAndTrimOptions<'_>,
    projected: &ProjectionCurves,
    mut tol: f64,
) -> Result<()> {
    // Process each surface
    for (ii, &face) in projected.faces.iter().enumerate() {
        let c3 = projected.curves_3d.get(ii).cloned();
        let uv = projected.uv_curves.get(ii).cloned().flatten();
        let c3: Vec<Curve> = c3.into_iter().collect();
        let uv: Vec<Curve> = uv.into_iter().collect();

        // Tolerance: max of dTol, Face tol, optional given tol.
        let face_tol = b.face(face)?.tolerance;
        if face_tol > tol {
            tol = face_tol;
        }
        if let Some(t) = options.approximation_tol_3d {
            if t > tol {
                tol = t;
            }
        }
        let curve_ref = |e: EdgeId, _type_code: i64| (env.keys)(CacheObject::Curve(e));
        let _ =
            merge_curves_on_surface(k, b, notify, &curve_ref, face, tol, &c3, Some(&uv), false)?;
    }

    // If we're just Splitting, then we're done.
    if trim_type == TrimType::Split {
        return Ok(());
    }

    // Turn off editing for classification.
    let editing = b.editing_enabled();
    b.set_editing_enabled(false);
    let to_delete = faces_to_delete(k, b, env, curve, direction, reference, trim_type, tol);
    b.set_editing_enabled(editing);
    let to_delete = to_delete?;

    // Remove the indicated Faces.
    remove_faces(k, b, notify, &to_delete)
}

/// Classifies each face against the plane of the projected curve: by the votes of
/// its vertices, else of its edges' mid-points, else by a ray along the projection
/// direction.
fn faces_to_delete(
    k: &mut Kernel,
    b: &mut Brep,
    env: &ProjectEnv<'_>,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
    trim_type: TrimType,
    tol: f64,
) -> Result<Vec<FaceId>> {
    let mut to_delete = Vec::new();
    let mut edges: Vec<EdgeId> = Vec::new();

    // Native classifies against crCurveToProject itself: every reference, vertex,
    // midpoint and ray-fallback vote uses the caller's curve, whose class's virtuals
    // the solver dispatches (a caller wrapping a BSplineCurve uses `clone_exact`, so
    // its knots are not tuned).

    // First find which side the reference point is on.
    let delete_right_side = find_side_of_curve(k, curve, reference, direction, tol)?;

    for face in brep_faces(b)? {
        let vertices = face_vertices(b, face)?;

        // voting with the vertices of the face
        let mut right_votes = 0u64;
        let mut left_votes = 0u64;
        for &v in &vertices {
            let rec = b.vertex(v)?;
            let tol1 = 5.0 * (rec.tolerance + tol);
            if let Some(side) = ignored(find_side_of_curve(k, curve, rec.point, direction, tol1))? {
                if side == 1 {
                    right_votes += 1;
                } else if side == 0 {
                    left_votes += 1;
                }
                // UNSURE if point is on the plane.
            }
        }

        if right_votes == 0 && left_votes == 0 {
            // Test mid-point of all edges
            edges = face_edges(b, face)?;
            for &e in &edges {
                let rec = b.edge(e)?;
                let c = rec
                    .curve
                    .as_ref()
                    .ok_or(SolidError::null_pointer(OP_PROJECT))?;
                let tol1 = 5.0 * (rec.tolerance + tol);
                let p = c.evaluate_point(k, rec.interval.evaluate(0.5))?;
                if let Some(side) = ignored(find_side_of_curve(k, curve, p, direction, tol1))? {
                    if side != 0 {
                        right_votes += 1;
                    } else {
                        left_votes += 1;
                    }
                }
            }
        }

        let mut face_right_side = i32::from(right_votes >= left_votes);

        if right_votes == left_votes {
            // Make a face shape with the face and its edges and vertices.
            let edge_keys: Vec<_> = edges.iter().map(|&e| (env.brep_id, e)).collect();
            let vertex_keys: Vec<_> = vertices.iter().map(|&v| (env.brep_id, v)).collect();
            let shape = Shape::new(
                k,
                &(env.brep_id, &*b),
                &[],
                &[(env.brep_id, face)],
                &edge_keys,
                &vertex_keys,
            )?;

            // Find a ray to determine which face needs to be kept or deleted
            let bbox = qbrep::calculate_bounding_box(k, &*b, BoundingBoxOptions::default())?;
            let sphere = bbox.sphere_bound();
            let param = line_closest_point(reference, direction, sphere.center);
            let projected_center = reference + direction * param;
            let ray_start = projected_center - direction * (2.0 * sphere.radius);
            let ray_end = projected_center + direction * (2.0 * sphere.radius);
            let segment = Curve::from(BSplineCurve::create_line_segment(3, ray_start, ray_end)?);

            let cache = make_brep_cache(k, CacheSource::Brep(&*b), env.keys)?;
            let identity = qbrep::BrepIdentity {
                brep_id: env.brep_id,
                keys: env.keys,
                owner_of: env.owner_of,
            };
            let brep_tol = b.tolerance();
            // the two output vectors of the solve (uninitialised upstream)
            let vectors = [Vec3::ZERO, Vec3::ZERO];
            let solutions = {
                let objects = qbrep::LiveBrepObjects::new(b, &identity, Some(&cache))?;
                TopologySolver::shape_curve_solve(
                    k,
                    &objects,
                    &shape,
                    CurveRef {
                        curve: &segment,
                        handle: CacheObject::Other(ObjectRef {
                            type_code: BSPLINE_CURVE_TYPE,
                            key: ObjectKey(0),
                        }),
                    },
                    &segment.natural_interval(),
                    TopologySolveOptions {
                        operation: SolverOperation::Minimize,
                        requested: SolutionRequested::Single,
                        tolerance_3d: brep_tol,
                        best_answer_so_far_sq: BIG_DOUBLE,
                        opt_vectors: &vectors,
                    },
                )?
            };

            let Some(first) = solutions.as_slice().first() else {
                continue;
            };
            let p = point_from_solution(k, b, first, 1)?;
            let tol1 = 5.0 * (b.face(face)?.tolerance + tol);
            match ignored(find_side_of_curve(k, curve, p, direction, tol1))? {
                Some(side) => face_right_side = side,
                // Surface is on opposite side as point: the closest point is under
                // the curve.
                None => face_right_side = i32::from(delete_right_side == 0),
            }
        }

        if (trim_type == TrimType::KeepPoint && delete_right_side != face_right_side)
            || (trim_type == TrimType::DeletePoint && delete_right_side == face_right_side)
        {
            to_delete.push(face);
        }
    }
    Ok(to_delete)
}

// ---- RemoveAllTopology --------------------------------------------------------------------

/// `GetWireEdges` (unit `topology.brep_p1`).
fn brep_wire_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    p1::wire_edges(b, None)
}

/// `SmBrep::DeleteEdge(edge)` (default `pOptSurfaceToKeep = NULL`), notifying through
/// `env`.
fn delete_edge(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>, e: EdgeId) -> Result<()> {
    let mut d = p4::DeleteEnv::from_notify(env);
    p4::delete_edge(k, b, &mut d, e, None)
}

/// `GetShellVertices` ([`crate::topology::brep::p2::shell_vertices`], no attribute filter).
fn brep_shell_vertices(b: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p2::shell_vertices(b, None)
}

/// `SmBrep::DeleteVertex(vertex)`, notifying through `env` (as [`delete_edge`]).
fn delete_vertex(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>, v: VertexId) -> Result<()> {
    let mut d = p4::DeleteEnv::from_notify(env);
    p4::delete_vertex(k, b, &mut d, v)
}

/// `SmBrep::RemoveAllTopology`: deletes every face, wire edge and shell vertex,
/// notifying through the kernel's callbacks (a caller without `SmTopology` records of
/// its own; see [`remove_all_topology_notified`]).
///
/// # Errors
///
/// `SM_ERR` when editing is not enabled; those of `RemoveFaces`;
/// [`SolidError::Unported`] for the unported face, edge and vertex removals.
pub fn remove_all_topology(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP_REMOVE));
    }
    crate::topology::brep::p3::with_kernel_callbacks(k, b, remove_all_topology_notified)
}

/// [`remove_all_topology`] notifying the caller's `env` (the objects' `SmTopology`
/// records and attributes), so merged and deleted objects propagate their flags and
/// user values as upstream's objects do.
///
/// # Errors
///
/// Those of [`remove_all_topology`].
pub fn remove_all_topology_notified(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
) -> Result<()> {
    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP_REMOVE));
    }

    let faces = brep_faces(b)?;
    let editing = b.editing_enabled();
    // increments unlocked mark value
    super::p5::remove_faces_notified(k, b, editing, &faces, env)?;

    for e in brep_wire_edges(b)? {
        // status ignored upstream
        ignored(delete_edge(k, b, env, e))?;
    }

    for v in brep_shell_vertices(b)? {
        // status ignored upstream
        ignored(delete_vertex(k, b, env, v))?;
    }
    Ok(())
}

// ---- TestClosureSplitRegion -----------------------------------------------------------

/// The new shells and regions of a region split by [`test_closure_split_region`]
/// (`rpNewShells`, `rpNewRegions`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClosureSplit {
    /// The new shells, one per new region.
    pub new_shells: Vec<Option<ShellId>>,
    /// The new regions.
    pub new_regions: Vec<RegionId>,
}

/// `SmBrep::TestClosureSplitRegion(SmBoolean, SmFaceuse*, ...)` (unit
/// `topology.brep_p6`) as the edge version calls it: `Some` outputs only for
/// `SM_SUCCESS` (`eStat == SM_SUCCESS`), `None` for a native error status; Rust-side
/// refusals and unported callees propagate. Each call claims a fresh mark (upstream
/// `SmNewMarkAndLock`), so a fresh [`Marks`] record per call is equivalent.
fn test_closure_split_region_at_faceuse(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    do_infinite_region_testing: bool,
    faceuse: Option<FaceuseId>,
) -> Result<Option<(Option<ShellId>, Option<RegionId>)>> {
    // pFUStart->GetMate() dereferences a NULL face use upstream
    let fu = faceuse.ok_or(SolidError::invalid_input(
        OP_CLOSURE,
        "edge use without face use",
    ))?;
    match p6::test_closure_split_region(
        k,
        b,
        &mut Marks::default(),
        env,
        do_infinite_region_testing,
        fu,
    ) {
        Ok(v) => Ok(Some(v)),
        Err(SolidError::Upstream { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

fn find_and_set_infinite_region(
    k: &mut Kernel,
    b: &mut Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    // the ray fires read the geometry under the caller's names, the ones its owners
    // carry (as MakeTopologyFromData sets them)
    let keys = |object| super::p2::caller_cache_key(key_of, object);
    let query = qbrep::BrepQuery::new(qbrep::BrepIdentity {
        brep_id: BrepId::from_index(0),
        keys: &keys,
        owner_of: &|_| None,
    });
    // SmBrep::FindAndSetInfiniteRegion() with its defaults (topology.brep_p5): no shell
    // to test, SM_VS_INFINITEREGION_SWAP, no box
    let status = super::p5::find_and_set_infinite_region(
        k,
        b,
        &query,
        None,
        p1::SetRegionVoidType::InfiniteRegionSwap,
        None,
    );
    // Lazy face queries attach new curves to typed edge-use slots. Complete the
    // native SetUVTrimCurve owner binding under the construction caller's names.
    bind_uv_trim_owners(b, key_of)?;
    status
}

/// Completes the native `SetUVTrimCurve` owner binding (`pUVTrimCurve->SetOwner(eu)`)
/// under the caller's names `key_of` for the UV trim curves the B-rep's own routines
/// left without owner (a B-rep under [`crate::topology::NamingScheme::Caller`]: lazy
/// face queries, edge splits). Existing owners are kept so a foreign or incorrect
/// owner remains detectable. An edge use the B-rep's routines added (edge splits) gets
/// its caller key recorded (`Brep::geometry_owner_key`) so trims made later are named
/// alike.
fn bind_uv_trim_owners(b: &mut Brep, key_of: &dyn Fn(EntityRef) -> ObjectRef) -> Result<()> {
    for edge in p1::edges(b, None)? {
        for id in b.edge(edge)?.edgeuses.clone() {
            let entity = EntityRef::Edgeuse(id);
            if b.geometry_owner_key(entity).is_none() {
                b.set_geometry_owner_key(entity, Some(key_of(entity)));
            }
            if let Some(curve) = b.edgeuse_mut(id)?.uv_trim_curve.as_mut() {
                if curve.owner().is_none() {
                    curve.set_owner(Some(key_of(EntityRef::Edgeuse(id))), None);
                }
            }
        }
    }
    Ok(())
}

/// `GetMate()->GetRadial()`: the next edge use pair around the edge in the same
/// direction.
fn next_pair(b: &Brep, eu: EdgeuseId) -> Result<EdgeuseId> {
    let mate = eu1::mate(b, eu)?.ok_or(SolidError::invalid_input(OP_CLOSURE, "no mate"))?;
    eu1::radial(b, mate)?.ok_or(SolidError::invalid_input(OP_CLOSURE, "no radial"))
}

/// `GetCWEdgeuse()`, dereferenced upstream.
fn cw(b: &Brep, eu: EdgeuseId) -> Result<EdgeuseId> {
    eu1::cw_edgeuse(b, eu)?.ok_or(SolidError::invalid_input(
        OP_CLOSURE,
        "edge use without clockwise neighbour",
    ))
}

/// `SmBrep::TestClosureSplitRegion` (edge version): when `test_edge` is a spine edge
/// (six or more edge uses) whose edge use loops pair up around the same edges, tests
/// each paired loop's face uses for a region they close off and splits it; with
/// `do_infinite_region_testing` the infinite region is found again. `env` is the
/// notification state the face use version's `SmBrep::Notify` calls go through.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without mate, radial or clockwise
/// neighbour (upstream dereferences null) or a walk that does not close;
/// [`SolidError::Unported`] for unported callees of the face use version (`topology.brep_p6`) and `FindAndSetInfiniteRegion`.
pub fn test_closure_split_region(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    do_infinite_region_testing: bool,
    test_edge: EdgeId,
) -> Result<ClosureSplit> {
    // init outputs
    let mut out = ClosureSplit::default();

    let test_eus = edge::edgeuses(b, test_edge, None, None)?;
    if test_eus.len() < 6 {
        return Ok(out); // not a spine edge.
    }

    let mut same_edge_eus1: Vec<EdgeuseId> = Vec::new();
    let mut same_edge_eus2: Vec<EdgeuseId> = Vec::new();

    let primary = eu1::primary_edgeuse(b, test_edge)?
        .ok_or(SolidError::invalid_input(OP_CLOSURE, "no primary edge use"))?;
    let bound = b.edgeuses().len() + 1;
    let too_long = || SolidError::invalid_input(OP_CLOSURE, "edge use walk does not close");

    // outer loop: start1 = all EU pairs of the given edge.
    let mut start1 = primary;
    let mut outer = 0usize;
    loop {
        let mut start2 = next_pair(b, start1)?;

        // inner loop: start2 = all other EU pairs of the given edge, past start1.
        let mut inner = 0usize;
        while start2 != primary {
            // Traverse around their loops until an edge doesn't match.
            let mut this1 = start1;
            let mut this2 = start2;
            let mut all_edges_same = true;
            let mut steps = 0usize;
            loop {
                // We know the first ones point to the same edge, so skip them.
                this1 = cw(b, this1)?;
                this2 = cw(b, this2)?;
                if eu1::edge(b, this1)? != eu1::edge(b, this2)? {
                    all_edges_same = false;
                    break;
                }
                if this1 == start1 {
                    break;
                }
                steps += 1;
                if steps > bound {
                    return Err(too_long());
                }
            }

            if all_edges_same {
                same_edge_eus1.push(this1);
                same_edge_eus2.push(this2);
            }

            // End of inner loop: move on to the next EU pair.
            start2 = next_pair(b, start2)?;
            inner += 1;
            if inner > bound {
                return Err(too_long());
            }
        }

        // End of outer loop: move on to the next EU pair.
        start1 = next_pair(b, start1)?;
        if start1 == primary {
            break;
        }
        outer += 1;
        if outer > bound {
            return Err(too_long());
        }
    }

    if same_edge_eus1.is_empty() {
        return Ok(out);
    }

    // For each, check its Faceuses to see whether it splits a region.
    for (&eu_a, &eu_b) in same_edge_eus1.iter().zip(&same_edge_eus2) {
        for eu in [eu_a, eu_b] {
            let fu = eu1::faceuse(b, eu)?;
            // Note: don't SER here; no region nesting in these calls.
            if let Some((shell, Some(region))) =
                test_closure_split_region_at_faceuse(k, b, env, false, fu)?
            {
                out.new_shells.push(shell);
                out.new_regions.push(region);
            }
        }
    }

    if do_infinite_region_testing {
        find_and_set_infinite_region(k, b, env.key_of)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::AttributeStore;
    use crate::topology::brep_data::{
        EdgeData, EuData, FaceData, FaceuseData, Keyed, LoopData, RegionData, ShellData, VertexData,
    };

    fn key_of(e: EntityRef) -> ObjectRef {
        let (t, i) = match e {
            EntityRef::Face(f) => (16002, u64::from(f.index())),
            EntityRef::Edge(x) => (16016, u64::from(x.index())),
            EntityRef::Edgeuse(x) => (16005, u64::from(x.index())),
            EntityRef::Vertex(x) => (16021, u64::from(x.index())),
            EntityRef::Vertexuse(x) => (16006, u64::from(x.index())),
            EntityRef::Loop(x) => (16003, u64::from(x.index())),
            EntityRef::Loopuse(x) => (16004, u64::from(x.index())),
            EntityRef::Faceuse(x) => (16022, u64::from(x.index())),
            EntityRef::Shell(x) => (16013, u64::from(x.index())),
            EntityRef::Region(x) => (16012, u64::from(x.index())),
            EntityRef::Brep(_) => (16011, 0),
        };
        ObjectRef {
            type_code: t,
            key: ObjectKey(1_000_000 + t as u64 * 100_000 + i),
        }
    }

    fn line(a: Point3, b: Point3) -> Curve {
        Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap())
    }

    fn plane_surface() -> Surface {
        use crate::nurbs::b_spline_surface::BSplineSurface;
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(1.0, 1.0, 0.0),
            )
            .unwrap(),
        )
    }

    /// A single square sheet face: 4 vertices, 4 edges, one loop, manifold flag `mf`.
    fn square(mf: i32) -> BrepData {
        let mut d = BrepData::new(mf, crate::topology::brep_data::DataSourceType::Unknown);
        d.healer_version = HEALER_VERSION;
        d.regions.push(RegionData {
            start_shell: 0,
            num_shells: 1,
            is_void: 1,
            ..RegionData::zeroed()
        });
        d.shells.push(ShellData {
            shell_type: 0,
            faceuse_start: 0,
            // an IGES solid lists only the solid side's face uses
            num_faceuses: if mf == 1 { 1 } else { 2 },
            ..ShellData::zeroed()
        });
        d.faceuses.push(FaceuseData {
            face: 0,
            orientation: 1,
            ..FaceuseData::zeroed()
        });
        d.faceuses.push(FaceuseData {
            face: 0,
            orientation: 0,
            ..FaceuseData::zeroed()
        });
        d.faces.push(FaceData {
            surface: 0,
            start_loop: 0,
            num_loops: 1,
            uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            rectangular_trim: 0,
            zone_tol_3d: 1e-6,
            ..FaceData::zeroed()
        });
        d.loops.push(LoopData {
            loop_type: 0,
            start_eu: 0,
            num_eu: 4,
            ..LoopData::zeroed()
        });
        let p = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        for (i, &q) in p.iter().enumerate() {
            d.vertices.push(VertexData {
                point: q,
                zone_tol_3d: 1e-7,
                ..VertexData::zeroed()
            });
            let n = (i + 1) % 4;
            d.curves_3d.push(Keyed {
                key: ObjectKey(i as u64 + 1),
                object: line(q, p[n]),
            });
            d.edges.push(EdgeData {
                curve: i as u64,
                start_vertex: i as u64,
                end_vertex: n as u64,
                prim_eu: i as i64,
                interval: crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0),
                zone_tol_3d: 1e-6,
                ..EdgeData::zeroed()
            });
            d.edgeuses.push(EuData {
                eu_type: 0,
                edge: i as u64,
                loop_: 0,
                orientation: 1,
                uv_curve: NO_UV_CURVE,
                next_eu: -(i as i64) - 1,
                mate_next_eu: i as i64,
                ..EuData::zeroed()
            });
        }
        d.surfaces.push(Keyed {
            key: ObjectKey(100),
            object: plane_surface(),
        });
        d
    }

    fn build(d: &mut BrepData, opts: MakeTopologyOptions) -> Result<Brep> {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region()?;
        let mut store = AttributeStore::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut env = TopologyEnv {
            all_attributes: &[],
            attributes: &mut store,
            key_of: &key_of,
            records: &mut records,
            next_key: 1,
        };
        make_topology_from_data(&mut k, &mut b, d, &mut env, opts)?;
        assert!(!k.context.doing_boolean());
        Ok(b)
    }

    #[test]
    fn square_sheet_builds_a_closed_loop() {
        {
            let mf = 0;
            let mut d = square(mf);
            let b = build(&mut d, MakeTopologyOptions::default()).unwrap();
            assert_eq!(b.regions().len(), 1);
            assert_eq!(b.faces().len(), 1);
            assert_eq!(b.edges().len(), 4);
            assert_eq!(b.edgeuses().len(), 8);
            assert_eq!(b.vertexuses().len(), 8);
            // the loop use walk closes after four edge uses
            let lu = b.loop_(LoopId::from_index(0)).unwrap().loopuse.unwrap();
            let LoopuseStart::Edgeuse(Some(start)) = b.loopuse(lu).unwrap().start else {
                panic!("no start");
            };
            let mut cur = start;
            for _ in 0..4 {
                cur = b.edgeuse(cur).unwrap().ccw.unwrap();
            }
            assert_eq!(cur, start);
            // one shell holding both face uses
            let s = b.shell(ShellId::from_index(0)).unwrap();
            assert_eq!(s.shell_type, ShellType::Faceuse);
            assert!(matches!(&s.members, ShellMembers::Faceuses(v) if v.len() == 2));
            // tolerances: clamped data tol, max with entity tols
            assert_eq!(b.tolerance(), 1e-5);
            assert_eq!(b.vertex(VertexId::from_index(0)).unwrap().tolerance, 1e-5);
            // analytic lines replace the NURBS lines
            let c = b
                .edge(EdgeId::from_index(0))
                .unwrap()
                .curve
                .as_ref()
                .unwrap();
            assert!(c.is_kind_of(LINE_TYPE));
            // data curves point at their copies
            assert!(d.curves_3d.iter().all(|c| c.object.owner().is_some()));
            assert!(d.surfaces[0].object.owner().is_some());
        }
    }

    #[test]
    fn healing_runs_and_manifold_is_computed() {
        // HealBrep (topology.brep_p1) runs; only the healer's own later stubs defer
        let mut d = square(1);
        d.healer_version = 0;
        match build(&mut d, MakeTopologyOptions::default()) {
            Ok(_) => {}
            Err(SolidError::Unported { unit, .. }) => assert_ne!(unit, "topology.brep_p1"),
            Err(e) => panic!("unexpected {e}"),
        }
        let mut d = square(1);
        d.healer_version = 0;
        let opts = MakeTopologyOptions {
            healer_enabled: false,
            ..MakeTopologyOptions::default()
        };
        assert!(build(&mut d, opts).is_ok());
        // an UNSURE manifold flag is computed (IsManifoldSolid): a sheet is not a
        // manifold solid
        let mut d = square(UNSURE);
        assert!(build(&mut d, MakeTopologyOptions::default()).is_ok());
        assert_eq!(d.manifold, 0);
        // sm_AddAttributes: an index past rAllAttributes (empty here)
        let mut d = square(1);
        d.attributes.push(0);
        assert!(matches!(
            build(&mut d, MakeTopologyOptions::default()),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn malformed_indices_are_rejected() {
        let mut d = square(1);
        d.faces[0].surface = 5;
        assert!(matches!(
            build(&mut d, MakeTopologyOptions::default()),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut d = square(0);
        d.shells[0].shell_type = 7;
        assert!(matches!(
            build(&mut d, MakeTopologyOptions::default()),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let mut d = square(0);
        d.edgeuses[0].next_eu = 99;
        assert!(matches!(
            build(&mut d, MakeTopologyOptions::default()),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut d = square(0);
        d.edgeuses[0].next_eu = i64::MIN;
        assert!(matches!(
            build(&mut d, MakeTopologyOptions::default()),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut d = square(0);
        d.regions[0].num_shells = u64::MAX;
        assert!(build(&mut d, MakeTopologyOptions::default()).is_err());
        let mut d = square(1);
        d.loops[0].start_eu = u64::MAX;
        assert!(build(&mut d, MakeTopologyOptions::default()).is_err());
        // a B-rep without infinite region
        let mut d = square(1);
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let mut store = AttributeStore::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut env = TopologyEnv {
            all_attributes: &[],
            attributes: &mut store,
            key_of: &key_of,
            records: &mut records,
            next_key: 1,
        };
        assert!(make_topology_from_data(
            &mut k,
            &mut b,
            &mut d,
            &mut env,
            MakeTopologyOptions::default()
        )
        .is_err());
    }

    #[test]
    fn iges_solid_gets_implied_regions() {
        let mut d = square(1);
        d.regions[0].is_void = 0;
        let b = build(&mut d, MakeTopologyOptions::default()).unwrap();
        // infinite + solid region, two shells (outer face uses swapped inside)
        assert_eq!(b.regions().len(), 2);
        assert_eq!(b.shells().len(), 2);
        assert_eq!(b.region(RegionId::from_index(0)).unwrap().is_void, 1);
        assert_eq!(b.region(RegionId::from_index(1)).unwrap().is_void, 0);
    }

    /// Triangles over shared edges (sheets: both face uses in one shell).
    fn triangles(pts: &[Point3], tris: &[[usize; 3]]) -> BrepData {
        let mut d = BrepData::new(1, crate::topology::brep_data::DataSourceType::Unknown);
        d.healer_version = HEALER_VERSION;
        d.regions.push(RegionData {
            start_shell: 0,
            num_shells: 1,
            is_void: 1,
            ..RegionData::zeroed()
        });
        d.regions.push(RegionData::zeroed());
        d.shells.push(ShellData {
            shell_type: 0,
            faceuse_start: 0,
            num_faceuses: 2 * tris.len() as u64,
            ..ShellData::zeroed()
        });
        for &q in pts {
            d.vertices.push(VertexData {
                point: q,
                ..VertexData::zeroed()
            });
        }
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (f, t) in tris.iter().enumerate() {
            for side in [1, 0] {
                d.faceuses.push(FaceuseData {
                    face: f as u64,
                    orientation: side,
                    ..FaceuseData::zeroed()
                });
            }
            d.surfaces.push(Keyed {
                key: ObjectKey(500 + f as u64),
                object: plane_surface(),
            });
            d.faces.push(FaceData {
                surface: f as u64,
                start_loop: f as u64,
                num_loops: 1,
                uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                ..FaceData::zeroed()
            });
            d.loops.push(LoopData {
                start_eu: d.edgeuses.len() as u64,
                num_eu: 3,
                ..LoopData::zeroed()
            });
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let key = (a.min(b), a.max(b));
                let e = edges.iter().position(|&x| x == key).unwrap_or_else(|| {
                    edges.push(key);
                    d.curves_3d.push(Keyed {
                        key: ObjectKey(900 + edges.len() as u64),
                        object: line(pts[key.0], pts[key.1]),
                    });
                    d.edges.push(EdgeData {
                        curve: edges.len() as u64 - 1,
                        start_vertex: key.0 as u64,
                        end_vertex: key.1 as u64,
                        prim_eu: d.edgeuses.len() as i64,
                        interval: crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0),
                        ..EdgeData::zeroed()
                    });
                    edges.len() - 1
                });
                d.edgeuses.push(EuData {
                    eu_type: 0,
                    edge: e as u64,
                    loop_: f as u64,
                    orientation: i32::from(a < b),
                    uv_curve: NO_UV_CURVE,
                    ..EuData::zeroed()
                });
            }
        }
        d
    }

    #[test]
    fn closure_walks_spine_edges() {
        let pts = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.5, 1.0, 0.0),
            Point3::new(0.5, 0.0, 1.0),
            Point3::new(0.5, -1.0, 0.0),
        ];
        let opts = MakeTopologyOptions {
            add_analytics: false,
            ..MakeTopologyOptions::default()
        };
        // three triangles around edge 0-1: a spine edge with six edge uses whose
        // loops do not pair up
        let mut d = triangles(&pts, &[[0, 1, 2], [0, 1, 3], [0, 1, 4]]);
        let mut b = build(&mut d, opts).unwrap();
        let mut k = Kernel::default();
        assert_eq!(b.edge(EdgeId::from_index(0)).unwrap().edgeuses.len(), 6);
        let mut state = crate::topology::brep::p2::RegionNotifyState::new(&b);
        for e in b.edges().ids().collect::<Vec<_>>() {
            let r = test_closure_split_region(&mut k, &mut b, &mut state.env(), true, e).unwrap();
            assert_eq!(r, ClosureSplit::default());
        }
        // a doubled triangle: its loops traverse the same edges, so the face use
        // version splits off the region between the two copies
        let mut d = triangles(&pts, &[[0, 1, 2], [0, 1, 2], [0, 1, 3]]);
        let mut b = build(&mut d, opts).unwrap();
        let mut state = crate::topology::brep::p2::RegionNotifyState::new(&b);
        let r = test_closure_split_region(
            &mut k,
            &mut b,
            &mut state.env(),
            false,
            EdgeId::from_index(0),
        );
        assert_eq!(
            r.unwrap(),
            ClosureSplit {
                new_shells: vec![Some(ShellId::from_index(1))],
                new_regions: vec![RegionId::from_index(2)],
            }
        );
    }

    #[test]
    fn closure_test_needs_a_spine_edge() {
        let mut d = square(1);
        let mut b = build(&mut d, MakeTopologyOptions::default()).unwrap();
        let mut k = Kernel::default();
        let mut state = crate::topology::brep::p2::RegionNotifyState::new(&b);
        let r = test_closure_split_region(
            &mut k,
            &mut b,
            &mut state.env(),
            true,
            EdgeId::from_index(0),
        )
        .unwrap();
        assert_eq!(r, ClosureSplit::default());
        assert!(remove_all_topology(&mut k, &mut b).is_err());
        b.set_editing_enabled(true);
        // topology.brep_p4 (SmBrep::DeleteEdge and friends) is ported
        assert!(remove_all_topology(&mut k, &mut b).is_ok());
    }
}

//! Offset executive settings, correspondence, synchronization and topology construction prefixes.
//!
//! Upstream: `source/SMLib/inc/SmOffsetExecutive.h` (201–222),
//! `source/SMLib/src/SmOffsetExecutive.cpp` (35–77, 1941–3450, 3517–4183).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`OffsetOperation`], [`OffsetExecutive::new`] | `SmOffsetOperationType`, `SmOffsetExecutive` |
//! | [`OffsetExecutive`] accessors | `GetInset`, `GetTangencyTolDegrees`, `GetExtendConvexEdges`, `GetOffsetBrep`, `GetShellFaces`, setters |
//! | `notify_edge_and_vertices` (private) | `sm_NotifyEdgeAndVertices` |
//! | [`OffsetExecutive::replace_implicit_offsets`] | `ReplaceImplicitOffsets` |
//! | [`OffsetExecutive::remove_excess_faces`] | `RemoveExcessFaces` |
//! | `forget_seconds`, `forget_firsts`, `removed_faces_edgeuses` (private) | `sm_ForgetSeconds`, `sm_ForgetFirsts`, `RemoveExcessFaces`' `sGone` |
//! | [`OffsetExecutive::get_create_extended_surface`] | `GetCreateExtendedSurface` |
//! | [`OffsetExecutive::insert_extended_surface`] | `m_vSurfExtSurf.Insert` (protected upstream; harness seeding) |
//! | [`OffsetExecutive::piecewise_merge`] | `PiecewiseMerge` |
//! | `sort_edgeuses`, `insert_breaks`, `remove_small_stuff`, `extract_gaps` (private) | `sm_SortEdgeuses`, `sm_InsertBreaks`, `sm_RemoveSmallStuff`, `sm_ExtractGaps` |
//! | [`OffsetExecutive::offset_edgeuse`] | `GetOffsetEdgeuse` |
//! | [`OffsetExecutive::offset_faces`] | `m_vFUToF.GetSeconds` (protected upstream; harness observation) |
//! | [`OffsetExecutive::synchronize_offset_edgeuses`] | `SyncronizeOffsetEdgeuses` |
//! | [`OffsetExecutive::build_side_wall_of_edgeuse`] | `BuildSideWallOfEdgeuse` |
//! | [`OffsetExecutive::build_offset_of_edgeuses`] | `BuildOffsetOfEdgeuses` |
//! | [`OffsetExecutive::fill_gap_edgeuses`] | `FillGapEdgeuses` |
//! | [`OffsetExecutive::build_offset_of_vertex`] | `BuildOffsetOfVertex` |
//! | [`OffsetExecutive::build_offset_of_faceuse`] | `BuildOffsetOfFaceuse` |
//! | `build_contained_offset_faces` (private) | `BuildOffsetOfFaceuse` contained-domain branch |
//! | `build_boolean_offset_faces` (private) | `BuildOffsetOfFaceuse` 2D-boolean branch |
//! | [`OffsetExecutive::map_edgeuses`] | `MapEdgeuses` |
//!
//! Deviations: RAII replaces the destructor. Constructor scalars are an immutable snapshot
//! of `SmOffsetGeometryCreation` parameters, not a retained creator reference or its implementation. `Relation` uses deterministic id order
//! instead of upstream hash/pointer order; bubble sorting remains stable, including unordered
//! floating-point comparisons. Extension identity is a caller-supplied key, never an address.
//! Every unavailable dependency is reported at its actual call, including ignored statuses.
//! Continuations after unconditional missing calls are documented there, not reported as success.
//! `RemoveExcessFaces` borrows the original B-rep immutably for queries
//! and keys its cache by each edge's and face's geometry owner (arena identity when
//! unowned), so geometry owned under a caller's notification keys is found; an owner
//! shared by two records or naming another record (an indirect owner chain) is
//! [`SolidError::Unsupported`] rather than silently skipped. Its relation forgetting
//! (`sm_ForgetSeconds` / `sm_ForgetFirsts` and the `sGone` / `sLive` walks) propagates
//! a dangling-id error from the topology queries where upstream's walks cannot fail;
//! only a corrupt B-rep reaches it.
//! Result-edge enumeration uses `SmBrep::GetEdges` list order (`topology::brep::p1::edges`);
//! the result B-rep is borrowed mutably because `NormalizedEvaluate` takes the edge use's
//! B-rep mutably (its 3D evaluation, used here, edits nothing). `BuildSideWallOfEdgeuse` takes
//! the original B-rep as an argument (upstream reaches it through the edge use pointer).
//! Edge-offset preparation and gap handling are shared with p2. Preparation can split
//! result edges and notify through caller-owned stores, using either an external
//! `NotifyEnv` or the actual kernel context. Geometry creators are ported separately;
//! vertex and edge caps use the shared creator through typed cross-arena adapters
//! (`EdgeuseOffset` edits both B-reps through the caller's stores and selected
//! context, and reads edge-use marks from the stores' records, or from the calling
//! loop's set inside p2). Cap faces are built with the caller's result identities.
//! The executive's creator snapshot keeps its angular tolerance.
//! `BuildOffsetOfFaceuse` moves the offset surface into `MakeFaceWithCurves` (upstream
//! hands over the pointer); a failed constructor drops it and, as upstream, relates no
//! face. Its `SmSSIData` UV curves are cloned into the merge input; a missing entry,
//! which upstream dereferences, is a null-pointer error. `GetInfiniteRegion` is the
//! `Brep` accessor. In the 2D-boolean branch the offset surface goes to the first
//! resulting face and each later face owns an exact copy (upstream they all share the
//! one pointer); the temporary planar and domain B-reps notify through arena-only
//! stores (`ArenaNotifyState`, faces named by `loopuse::notify_ref`), as merge p3's
//! planar boolean does, and `GetSurfaces` selects the surfaces owned under those
//! names. `FillGapEdgeuses` builds its corner-blend faces through `make_cap_face`'s
//! `CreateFaceFromSurface`, notifying through the caller's result-keyed stores; a
//! null closest edge, same-oriented edge use or end mate (dereferenced upstream) is a
//! null-pointer error, a null blended face is `RelatePair`'s invalid input, and any
//! failed `CreateCornerBlend` is `SM_ERR` (`SER(SM_ERR)`), unported callees aside.
//! No new input-driven recursion; existing topology walks are cycle-bounded.
//! The lamina clean-up's `DeleteFaces` runs through merge p4's arena intersector over
//! the offset B-rep alone (both upstream operands are that B-rep); it notifies under
//! `DELETE_FACES_NAMESPACE` rather than the caller's offset keys.

use std::collections::{BTreeMap, BTreeSet};

use crate::boolean::merge::p4::KernelManifoldIntersector;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::context::{Context, MarkType};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::{Solution, SolutionArray};
use crate::geom::tol::Relation;
use crate::heal::stitch::p1::{BorrowedStitchEdits, StitchEditContext};
use crate::heal::stitch::p2 as stitch;
use crate::intersect::topology_solver::p1::TopologySolver;
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{min2, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_surface_n_lib::p1::OffsetParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::offset::offset_geometry_creation::{OffsetGeometryCreation, SsiData};

use super::edgeuse::{CapNotifications, MarkSource};
use super::p2::OffsetNotifications;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::brep::p1::{self as bp1, SetRegionVoidType};
use crate::topology::brep::p4::shrink_geometry;
use crate::topology::brep::p7::{self as bp7, FaceCurvesInput, SelfIntersectionCurve};
use crate::topology::brep::p9::CurveIdentity;
use crate::topology::brep_cache::{
    arena_identity, make_brep_cache, BrepCache, BrepObjects, CacheObject, CacheSource,
};
use crate::topology::edge::{self, NotificationStores, NotifyEnv, NotifyState};
use crate::topology::edgeuse::{p1 as eu, p2 as eu2, p3 as eu3};
use crate::topology::face::{p2 as face, p3 as face3};
use crate::topology::topology;
use crate::topology::topology_traverser::face_edges;
use crate::topology::{loopuse, vertex, Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, VertexId};

const OP: &str = "offset::offset_executive::p1";

enum MapKernel<'a> {
    Read(&'a Kernel),
    Mutable(&'a mut Kernel),
}

impl MapKernel<'_> {
    fn kernel(&self) -> &Kernel {
        match self {
            Self::Read(kernel) => kernel,
            Self::Mutable(kernel) => kernel,
        }
    }

    fn with_env<T>(
        &mut self,
        state: &mut NotifyState<'_>,
        run: impl FnOnce(&mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Read(_) => state.with_context(None, run),
            Self::Mutable(kernel) => state.with_env(kernel, run),
        }
    }
}

/// Where an offset's results are placed (`SmOffsetOperationType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum OffsetOperation {
    /// Straight volumetric offset into a new B-rep.
    SolidOffset = 0,
    /// Non-manifold offset.
    NonManifoldOffset = 1,
    /// Offset into the original B-rep.
    Shell = 2,
    /// Local operation.
    LocalOperation = 3,
    /// Machining offset.
    MachineOffset = 4,
}

/// Synchronized offset uses, with null padding only in the upstream early-return branches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SynchronizedEdgeuses {
    /// Uses mapped from the original use.
    pub edgeuses: Vec<Option<EdgeuseId>>,
    /// Uses mapped from its radial partner.
    pub other_edgeuses: Vec<Option<EdgeuseId>>,
    /// First-side uses having no matching second-side interval.
    pub gaps: Vec<EdgeuseId>,
    /// Second-side uses having no matching surviving first-side interval.
    pub other_gaps: Vec<EdgeuseId>,
}

/// Geometry returned by `BuildOffsetOfVertex` (out-parameters upstream).
#[derive(Debug, Clone, Default)]
pub struct VertexOffset {
    /// New cap surface, absent when no cap is necessary.
    pub surface: Option<Surface>,
    /// Its 3D trimming curves.
    pub trim_curves: Vec<Curve>,
    /// Orientations of the trimming curves.
    pub curve_orientations: Vec<OrientType>,
}

/// Not upstream (WR-532; the fork's source fork@b283ea2, `sm_ExtendConeAsItself`):
/// cone (or cylinder) `surface` extended by `dist` along its rulings and around its axis
/// as itself, each direction's end spans carried on (`SM_CT_CINFINITY`), which stays on
/// the circle where a tangent extension (`SM_CT_G1R`) leaves it. Around the axis it goes
/// no farther than half the turn the cone leaves open on each side, so it never laps
/// itself; a closed cone is extended along its rulings only. Around the axis each
/// side is extended on its own and each old end made a knot of multiplicity equal to
/// the degree again ([`full_knot`]; source fork@3626d4b).
///
/// # Errors
///
/// Those of the extensions and knot insertions.
fn extend_cone_as_itself(
    kernel: &Kernel,
    surface: &Surface,
    cone: &crate::surface::cone::Cone,
    dist: f64,
) -> Result<Surface> {
    use crate::nurbs::b_spline_surface::SurfParamType;
    let (around, along) = if cone.rev().swap_uv() {
        (SurfParamType::V, SurfParamType::U)
    } else {
        (SurfParamType::U, SurfParamType::V)
    };
    let along_srf =
        surface.create_extended_surface_dir(kernel, along, dist, ContinuityType::CInfinity)?;

    // around: half the open turn on each side, in arc length on the smaller end circle
    let sweep_rad = cone.rev().step_uv_domain().u_interval().length() * PI / 180.0;
    let radius = min2(cone.bot_radius(), cone.top_radius());
    let open = 2.0 * PI - sweep_rad;
    let around_dist = min2(dist, 0.5 * 0.5 * open * radius);
    // (`SM_CAST_PTR(SmBSplineSurface, ..)`: the B-spline surface and its subclasses)
    let is_bspline = !matches!(
        along_srf.kind(),
        SurfaceKind::OffsetSurface(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::SrfInVolume(_)
    );
    if !is_bspline || open <= 0.0 || around_dist <= EFF_ZERO_SQRT * (1.0 + radius) {
        return Ok(along_srf);
    }
    // (source fork@3626d4b) each side on its own, each old end then a knot of
    // multiplicity equal to the degree again: SMLib's own form for a circle
    // (`SmCone::IsNurbSurfaceCone` takes only a triple knot at each end and double knots
    // inside); both sides in one call extend the first side's extended span again and
    // leave a single knot at the old end, and a face on that surface is no longer found
    // to be a cone
    let around_ivl = if around == SurfParamType::U {
        along_srf.natural_uv_domain_with_nurb(kernel)?.u_interval()
    } else {
        along_srf.natural_uv_domain_with_nurb(kernel)?.v_interval()
    };
    let (min_side, max_side) = if around == SurfParamType::U {
        (SurfParamType::UMin, SurfParamType::UMax)
    } else {
        (SurfParamType::VMin, SurfParamType::VMax)
    };
    let mut min_srf = along_srf.create_extended_surface_dir(
        kernel,
        min_side,
        around_dist,
        ContinuityType::CInfinity,
    )?;
    full_knot(&mut min_srf, around, around_ivl.min())?;
    let mut both = min_srf.create_extended_surface_dir(
        kernel,
        max_side,
        around_dist,
        ContinuityType::CInfinity,
    )?;
    full_knot(&mut both, around, around_ivl.max())?;
    Ok(both)
}

/// Not upstream (WR-532; source fork@3626d4b): makes the knot of B-spline
/// `surface` at `at` in direction `dir` of multiplicity equal to the degree
/// (`InsertOneKnot` of the missing count; a knot within `EFF_ZERO` of the interval's
/// length counts as the same).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a surface that is not a B-spline surface (upstream's
/// `NER` of the cast); those of the knot query and insertion.
fn full_knot(
    surface: &mut Surface,
    dir: crate::nurbs::b_spline_surface::SurfParamType,
    at: f64,
) -> Result<()> {
    use crate::nurbs::b_spline_surface::SurfParamType;
    const OP: &str = "offset::offset_executive::p1::extend_cone_as_itself";
    let SurfaceKind::BSpline(bs) = surface.kind_mut() else {
        return Err(SolidError::null_pointer(OP));
    };
    let (knots, mults) = bs.knots(dir, None)?;
    let domain = bs.natural_uv_domain();
    let ivl = if dir == SurfParamType::U {
        domain.u_interval()
    } else {
        domain.v_interval()
    };
    let same = EFF_ZERO * (1.0 + ivl.length());
    let mut have = 0;
    for (&knot, &mult) in knots.iter().zip(&mults) {
        if (knot - at).abs() <= same {
            have = mult;
        }
    }
    let degree = bs.degree(dir);
    if have < degree {
        bs.insert_one_knot(at, degree - have, dir)?;
    }
    Ok(())
}

/// The 27-bit object namespace under which the lamina clean-up's `DeleteFaces`
/// (merge p4's arena adapter) records its notifications; distinct from the Boolean
/// kernels' namespaces, which share no attribute store with an offset executive.
const DELETE_FACES_NAMESPACE: u32 = 0x00f5_e700;

/// Topological offset state. Original ids and result ids belong to separate B-reps.
/// Geometry-creator parameters are an immutable scalar snapshot; this executive does
/// not retain a creator reference or implement its offset-geometry algorithms.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // Four independent upstream settings, not exclusive states.
pub struct OffsetExecutive {
    operation: OffsetOperation,
    offset_distance: f64,
    approximation_tol_3d: f64,
    tangency_tol_radians: f64,
    inset: bool,
    extend_convex_edges: bool,
    merge_results: bool,
    create_offset_solid: bool,
    shell_faces: Vec<FaceId>,
    offset_brep: Option<Brep>,
    closing_faces: Vec<FaceId>,
    pub(super) faceuse_to_faces: Relation<FaceuseId, FaceId>,
    pub(super) edgeuse_to_edgeuses: Relation<EdgeuseId, EdgeuseId>,
    pub(super) edge_to_faces: Relation<EdgeId, FaceId>,
    pub(super) vertex_to_faces: Relation<VertexId, FaceId>,
    pub(super) blend_to_faces: Relation<FaceId, FaceId>,
    extended_surfaces: BTreeMap<u64, Surface>,
    /// Not upstream (WR-346): the original faces of an inset whose offset collapses (a
    /// convex round thinner than the wall), mated across instead of offset
    /// (`m_sCollapsedFaces`; only with `Kernel::fixes.offset_collapsed_rounds_mated`).
    pub(super) collapsed_faces: Vec<FaceId>,
    /// The faces of a local operation and the surfaces replacing theirs, in step (the
    /// part of `SmLocalOperationGeometryCreation` the executive reads: `m_vFaces`,
    /// `m_vSurfaces`).
    pub(super) local_faces: Vec<FaceId>,
    pub(super) local_surfaces: Vec<Surface>,
}

impl OffsetExecutive {
    /// Constructs a geometry-parameter snapshot with the creator's default angular
    /// tolerance, `2.0 * PI / 180.0` radians. Other executive settings use upstream defaults.
    pub fn new(
        operation: OffsetOperation,
        offset_distance: f64,
        approximation_tol_3d: f64,
    ) -> Self {
        Self::with_geometry_parameters(
            operation,
            offset_distance,
            approximation_tol_3d,
            2.0 * PI / 180.0,
        )
    }

    /// Constructs an immutable snapshot of the geometry creator's scalar parameters,
    /// including the angular tolerance that an upstream creator subclass may override.
    /// No creator reference is retained, and geometry creation remains a separate unit.
    pub fn with_geometry_parameters(
        operation: OffsetOperation,
        offset_distance: f64,
        approximation_tol_3d: f64,
        tangency_tol_radians: f64,
    ) -> Self {
        Self {
            operation,
            offset_distance,
            approximation_tol_3d,
            tangency_tol_radians,
            inset: false,
            extend_convex_edges: false,
            merge_results: false,
            create_offset_solid: false,
            shell_faces: Vec::new(),
            offset_brep: None,
            closing_faces: Vec::new(),
            faceuse_to_faces: Relation::default(),
            edgeuse_to_edgeuses: Relation::default(),
            edge_to_faces: Relation::default(),
            vertex_to_faces: Relation::default(),
            blend_to_faces: Relation::default(),
            extended_surfaces: BTreeMap::new(),
            collapsed_faces: Vec::new(),
            local_faces: Vec::new(),
            local_surfaces: Vec::new(),
        }
    }

    /// The requested operation (`GetOffsetOperation`).
    pub const fn operation(&self) -> OffsetOperation {
        self.operation
    }

    /// Whether the operation is an inset (`GetInset`).
    pub const fn inset(&self) -> bool {
        self.inset
    }

    /// Whether convex edges are extended rather than filleted (`GetExtendConvexEdges`).
    pub const fn extend_convex_edges(&self) -> bool {
        self.extend_convex_edges
    }

    /// Whether results are merged rather than stitched.
    pub const fn merge_results(&self) -> bool {
        self.merge_results
    }

    /// Whether lamina edges receive closing side walls.
    pub const fn create_offset_solid(&self) -> bool {
        self.create_offset_solid
    }

    /// The snapshotted geometry-creator angular tolerance, converted in upstream operation order.
    pub fn tangency_tol_degrees(&self) -> f64 {
        self.tangency_tol_radians * 180.0 / PI
    }

    /// Offset distance supplied by the geometry creator.
    pub const fn offset_distance(&self) -> f64 {
        self.offset_distance
    }

    /// Approximation tolerance supplied by the geometry creator.
    pub const fn approximation_tol_3d(&self) -> f64 {
        self.approximation_tol_3d
    }

    /// Faces copied rather than offset, preserving order and duplicates (`GetShellFaces`).
    pub fn shell_faces(&self) -> &[FaceId] {
        &self.shell_faces
    }

    /// The executive's temporary B-rep (`GetOffsetBrep`).
    pub const fn offset_brep(&self) -> Option<&Brep> {
        self.offset_brep.as_ref()
    }

    /// Faces created to close lamina boundaries.
    pub fn closing_faces(&self) -> &[FaceId] {
        &self.closing_faces
    }

    /// Sets `m_bInset`.
    pub fn set_inset(&mut self, value: bool) {
        self.inset = value;
    }

    /// Sets `m_bExtendConvexEdges`.
    pub fn set_extend_convex_edges(&mut self, value: bool) {
        self.extend_convex_edges = value;
    }

    /// Sets `m_bMergeResults`.
    pub fn set_merge_results(&mut self, value: bool) {
        self.merge_results = value;
    }

    /// Sets `m_bCreateOffsetSolid`.
    pub fn set_create_offset_solid(&mut self, value: bool) {
        self.create_offset_solid = value;
    }

    /// Replaces the shell-face list with a copy, without deduplicating (`SetShellFaces`).
    pub fn set_shell_faces(&mut self, faces: &[FaceId]) {
        self.shell_faces.clear();
        self.shell_faces.extend_from_slice(faces);
    }

    /// The geometry of a local operation (`SmLocalOperationGeometryCreation`'s face and
    /// surface lists): `surfaces[i]` replaces the surface of `faces[i]`.
    pub fn set_local_surfaces(&mut self, faces: &[FaceId], surfaces: Vec<Surface>) {
        self.local_faces = faces.to_vec();
        self.local_surfaces = surfaces;
    }

    /// Sets `m_eOffsetOperation`.
    pub fn set_operation(&mut self, operation: OffsetOperation) {
        self.operation = operation;
    }

    /// Transfers ownership of a temporary B-rep to this executive.
    pub fn set_offset_brep(&mut self, brep: Brep) {
        self.offset_brep = Some(brep);
    }

    /// Adds a many-to-many correspondence. Repeating a pair is a no-op (`SmRelation`).
    pub fn relate_edgeuses(&mut self, original: EdgeuseId, offset: EdgeuseId) {
        // ManyToMany has no cardinality error and typed ids cannot be null.
        let _ = self.edgeuse_to_edgeuses.relate_pair(original, offset);
    }

    /// Relates an original face use to a newly constructed offset face.
    pub fn relate_faceuse(&mut self, original: FaceuseId, offset: FaceId) {
        let _ = self.faceuse_to_faces.relate_pair(original, offset);
    }

    /// Relates an original edge to its offset cap face.
    pub fn relate_edge_face(&mut self, original: EdgeId, offset: FaceId) {
        let _ = self.edge_to_faces.relate_pair(original, offset);
    }

    /// Relates an original vertex to its offset cap face.
    pub fn relate_vertex_face(&mut self, original: VertexId, offset: FaceId) {
        let _ = self.vertex_to_faces.relate_pair(original, offset);
    }

    /// Relates a corner blend to one of the faces it blends.
    pub fn relate_blend_face(&mut self, blend: FaceId, face: FaceId) {
        let _ = self.blend_to_faces.relate_pair(blend, face);
    }

    /// The mapped offset use only when there is exactly one (`GetOffsetEdgeuse`).
    pub fn offset_edgeuse(&self, original: EdgeuseId) -> Option<EdgeuseId> {
        self.edgeuse_to_edgeuses.second(original)
    }

    /// The offset faces related to a face use (`m_vFUToF.GetSeconds`; protected
    /// upstream, for harness observation), in id order.
    pub fn offset_faces(&self, original: FaceuseId) -> Vec<FaceId> {
        self.faceuse_to_faces.seconds(original)
    }

    /// Not upstream (WR-346, `IsCollapsedFace`): whether original face `face` is an
    /// inset face whose offset collapsed.
    pub fn is_collapsed_face(&self, face: FaceId) -> bool {
        self.collapsed_faces.contains(&face)
    }

    /// Not upstream (WR-346, `GetEdgeuseAcrossCollapsedFace`): the original edge use
    /// beyond a collapsed face, seen from its edge use `edgeuse`. Across a rail (a
    /// tangent edge of a four-sided round) it is the radial of the opposite rail; across
    /// an end edge, seen from `from` (one of that edge's vertices), the radial of the
    /// rail at the edge's other vertex. `None` when the edge use's face did not
    /// collapse, the face across collapsed too, or the face has another shape.
    ///
    /// # Errors
    ///
    /// Those of the topology queries and the tangency test.
    pub fn edgeuse_across_collapsed_face(
        &self,
        kernel: &Kernel,
        original: &mut Brep,
        edgeuse: Option<EdgeuseId>,
        from: Option<VertexId>,
    ) -> Result<Option<EdgeuseId>> {
        let Some(edgeuse) = edgeuse else {
            return Ok(None);
        };
        match eu::face(original, edgeuse)? {
            Some(face) if self.is_collapsed_face(face) => {}
            _ => return Ok(None),
        }
        let tan_tol_deg = self.tangency_tol_degrees();
        let next = if eu2::is_tangent_sector(kernel, original, edgeuse, tan_tol_deg, false, true)? {
            let loopuse = required(eu::loopuse(original, edgeuse)?)?;
            if eu::loopuse_edgeuses(original, loopuse)?.len() != 4 {
                return Ok(None);
            }
            let ccw = required(eu::ccw_edgeuse(original, edgeuse)?)?;
            eu::ccw_edgeuse(original, ccw)?
        } else {
            let Some(from) = from else {
                return Ok(None);
            };
            // the end edge's vertex away from `from` (by position: `from` may be a
            // vertex of the rail beside it)
            let end_edge = edge_of(original, edgeuse)?;
            let v0 = required(edge::start_vertex(original, end_edge)?)?;
            let v1 = required(edge::end_vertex(original, end_edge)?)?;
            let from_point = original.vertex(from)?.point;
            let far = if original.vertex(v0)?.point.distance(from_point)
                > original.vertex(v1)?.point.distance(from_point)
            {
                v0
            } else {
                v1
            };
            let mut found = None;
            for side in [
                required(eu::cw_edgeuse(original, edgeuse)?)?,
                required(eu::ccw_edgeuse(original, edgeuse)?)?,
            ] {
                let side_edge = edge_of(original, side)?;
                if side != edgeuse
                    && (edge::start_vertex(original, side_edge)? == Some(far)
                        || edge::end_vertex(original, side_edge)? == Some(far))
                {
                    found = Some(side);
                    break;
                }
            }
            found
        };
        let Some(next) = next else {
            return Ok(None);
        };
        let Some(across) = eu::radial(original, next)? else {
            return Ok(None);
        };
        if across == next {
            return Ok(None);
        }
        match eu::face(original, across)? {
            Some(face) if self.is_collapsed_face(face) => Ok(None),
            _ => Ok(Some(across)),
        }
    }

    fn required_offset_brep(&self) -> Result<&Brep> {
        required(self.offset_brep.as_ref())
    }

    /// Temporarily lends ownership to another executive part, without copying topology.
    pub(super) fn take_offset_brep(&mut self) -> Option<Brep> {
        self.offset_brep.take()
    }

    /// Original mapped uses in the relation's deterministic first-id order.
    pub(super) fn mapped_original_edgeuses(&self) -> Vec<EdgeuseId> {
        self.edgeuse_to_edgeuses.all_firsts()
    }

    fn clear_relations(&mut self) {
        self.faceuse_to_faces.remove_all();
        self.edgeuse_to_edgeuses.remove_all();
        self.edge_to_faces.remove_all();
        self.vertex_to_faces.remove_all();
        self.blend_to_faces.remove_all();
    }

    /// Synchronizes mapped edge ranges, splitting offset edges at the other side's
    /// breaks with `MakeVertexSplitEdge`. `notifications` and `curve_ref` identify the
    /// offset B-rep's entities and edge curves for the split's notifications.
    pub fn synchronize_offset_edgeuses(
        &mut self,
        kernel: &mut Kernel,
        original: Option<EdgeuseId>,
        other: Option<EdgeuseId>,
        notifications: &mut NotifyEnv<'_>,
        curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<SynchronizedEdgeuses> {
        self.synchronize_offset_edgeuses_with_state(
            kernel,
            original,
            other,
            &mut NotifyState::from_env(notifications),
            curve_ref,
        )
    }

    /// Synchronizes through the kernel's actual context and caller-owned result stores.
    /// Like the external-environment entry point, this may split and notify result edges.
    pub fn synchronize_offset_edgeuses_with_kernel_context(
        &mut self,
        kernel: &mut Kernel,
        original: Option<EdgeuseId>,
        other: Option<EdgeuseId>,
        notifications: &mut NotificationStores<'_>,
        curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<SynchronizedEdgeuses> {
        self.synchronize_offset_edgeuses_with_state(
            kernel,
            original,
            other,
            &mut NotifyState::from_kernel_stores(notifications),
            curve_ref,
        )
    }

    fn synchronize_offset_edgeuses_with_state(
        &mut self,
        kernel: &mut Kernel,
        original: Option<EdgeuseId>,
        other: Option<EdgeuseId>,
        notifications: &mut NotifyState<'_>,
        curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<SynchronizedEdgeuses> {
        let original = required(original)?;
        let mut first = self.edgeuse_to_edgeuses.seconds(original);
        // An empty mapping does not dereference the B-rep upstream.
        if !first.is_empty() {
            remove_small_stuff(kernel, self.required_offset_brep()?, &mut first)?;
        }
        let mut second = other.map_or_else(Vec::new, |id| self.edgeuse_to_edgeuses.seconds(id));
        let mut out = SynchronizedEdgeuses::default();
        if first.is_empty() {
            // Deliberately do not filter the other side in this branch.
            if !second.is_empty() {
                sort_edgeuses(self.required_offset_brep()?, &mut second)?;
            }
            out.edgeuses.resize(second.len(), None);
            out.other_edgeuses = second.into_iter().map(Some).collect();
            return Ok(out);
        }

        let brep = required(self.offset_brep.as_mut())?;
        remove_small_stuff(kernel, brep, &mut second)?;
        if second.is_empty() {
            sort_edgeuses(brep, &mut first)?;
            out.other_edgeuses.resize(first.len(), None);
            out.edgeuses = first.into_iter().map(Some).collect();
            return Ok(out);
        }
        insert_breaks(kernel, brep, &first, &mut second, notifications, curve_ref)?;
        insert_breaks(kernel, brep, &second, &mut first, notifications, curve_ref)?;
        let brep = &*brep;
        sort_edgeuses(brep, &mut first)?;
        sort_edgeuses(brep, &mut second)?;
        extract_gaps(brep, &mut first, &second, &mut out.gaps)?;
        // The second extraction sees the already reduced first list, as upstream.
        extract_gaps(brep, &mut second, &first, &mut out.other_gaps)?;
        out.edgeuses = first.into_iter().map(Some).collect();
        out.other_edgeuses = second.into_iter().map(Some).collect();
        Ok(out)
    }

    /// Returns a cached extension, creating it with distance `offset_distance * 5.0`
    /// and G1-ruled continuity. A hit never re-extends or replaces the cached surface.
    ///
    /// `base_key` represents upstream pointer identity, including across Rust clones.
    /// Distinct source identities must use distinct keys. Failed extensions are not cached.
    pub fn get_create_extended_surface(
        &mut self,
        kernel: &Kernel,
        base_key: u64,
        base: &Surface,
    ) -> Result<&Surface> {
        match self.extended_surfaces.entry(base_key) {
            std::collections::btree_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::btree_map::Entry::Vacant(entry) => {
                // The obsolete offset-specific `else` at cpp:2158 is unreachable:
                // NER(pBase) already established pOrigSurf == pBase != NULL.
                let dist = self.offset_distance * 5.0;
                let extension = match base.kind() {
                    // Not upstream (WR-532), opt-in `offset_extends_cones_as_themselves`
                    SurfaceKind::Cone(cone) if kernel.fixes.offset_extends_cones_as_themselves => {
                        extend_cone_as_itself(kernel, base, cone, dist)?
                    }
                    _ => base.create_extended_surface(kernel, dist, ContinuityType::G1R)?,
                };
                Ok(entry.insert(extension))
            }
        }
    }

    /// Caches `surface` as the extension of `base_key` (`m_vSurfExtSurf.Insert`), replacing
    /// any earlier entry. Upstream's map is protected; the offset difftest shims reach it
    /// through a subclass to seed every base with itself, so that a lookup never extends,
    /// and their runners seed the port's cache through this method.
    pub fn insert_extended_surface(&mut self, base_key: u64, surface: Surface) {
        self.extended_surfaces.insert(base_key, surface);
    }

    /// Replaces implicit offset surfaces with approximations (`ReplaceImplicitOffsets`).
    /// The unconditional initial shrink is `topology.brep_p4`'s `ShrinkGeometry`.
    ///
    /// Every face with an `SmOffsetSurface` gets a cubic approximation of the offset of
    /// its extended (else its own) base, which must be a B-spline surface, within
    /// `approximation_tol_3d`. All approximations are built before any face is
    /// changed; each face is then notified (`PRE_EDIT`), given the new surface's
    /// natural domain and the surface (`SmBrep::ReplaceSurface`, old surface to the
    /// deleted list, UV trim curves rebuilt) and notified again (`POST_EDIT`), in
    /// face order. `notifications` must be keyed by `brep`'s identities.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a base that is not a B-spline surface; those of the approximation
    /// and of `ReplaceSurface` (`SER`). The notifications' statuses are not checked.
    pub fn replace_implicit_offsets(
        kernel: &mut Kernel,
        brep: &mut Brep,
        approximation_tol_3d: f64,
        notifications: &mut NotificationStores<'_>,
    ) -> Result<()> {
        // remove curve and surface portions not used by edges and faces (status
        // ignored upstream)
        ignore_status(shrink_geometry(kernel, brep))?;

        // for every face
        let mut fix = Vec::new();
        for f in bp1::faces(brep, None)? {
            // skip non SmOffsetSurfaces
            let Some(SurfaceKind::OffsetSurface(offset)) =
                brep.face(f)?.surface.as_ref().map(Surface::kind)
            else {
                continue;
            };
            // get SmOffsetSurface base (use extended surface if there is one)
            let base = offset
                .extended_base_surface()
                .unwrap_or_else(|| offset.base_surface());
            // make sure base surface is an SmBSplineSurface (`SM_CAST_PTR`, which every
            // subclass passes: plane, extrusion, revolution, cone, sphere, torus); its
            // B-spline part with the NURB `GetOrCreateGwNurbPointer` makes when missing
            let Some(nurb) = crate::topology::brep::p3::with_nurb(kernel, base)? else {
                return Err(SolidError::sm_err(OP));
            };
            // use NLIB function to build offset surface approximation
            let approximation = nurb.approximate_offset_surface(
                offset.offset_distance(),
                approximation_tol_3d,
                OffsetParamType::Inherited,
            )?;
            // remember face/surface pair
            fix.push((f, Surface::from(approximation)));
        }

        // for every face that had an approximate surface built for an exact offset
        // surface
        let mut state = NotifyState::from_kernel_stores(notifications);
        for (f, new_surface) in fix {
            state.with_kernel_env(kernel, |kernel, env| {
                let face_ref = (env.key_of)(EntityRef::Face(f));
                // SM_NO_GET_BREP is NULL in the pinned reference
                ignore_status(face3::notify(
                    brep,
                    f,
                    env,
                    None,
                    NotifyOperation::PreEdit,
                    [Some(face_ref), None, None],
                ))?;
                let domain = new_surface.natural_uv_domain_with_nurb(kernel)?;
                crate::topology::face::p2::set_uv_domain(brep, f, &domain)?;
                // swap the approx surface for the exact offset surface
                crate::topology::brep::p2::replace_surface(
                    kernel,
                    brep,
                    env,
                    f,
                    new_surface,
                    face_ref,
                    crate::topology::brep::p2::OldSurface::AddToDeletedList,
                    true,
                )?;
                ignore_status(face3::notify(
                    brep,
                    f,
                    env,
                    None,
                    NotifyOperation::PostEdit,
                    [Some(face_ref), None, None],
                ))
            })?;
        }
        Ok(())
    }

    /// Removes result faces too close to the original (`RemoveExcessFaces`).
    ///
    /// The source is borrowed immutably while solving, corresponding to the upstream
    /// editing-disabled query; the removal uses the result's actual editing flag and
    /// the kernel's traversal marks. An empty deletion list still calls the landed
    /// `RemoveFaces` implementation.
    /// Each original edge and face is identified by its geometry's owner key
    /// ([`arena_identity`] when unowned); an owner shared by two records or naming
    /// another record returns [`SolidError::Unsupported`] rather than omitting geometry.
    /// Cache and solver errors propagate before any result-face removal. Ordinary
    /// evaluation statuses are ignored, but an unported evaluator still defers.
    /// Result edges are visited in upstream edge list order (`SmBrep::GetEdges`).
    pub fn remove_excess_faces(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        result: &mut Brep,
    ) -> Result<()> {
        let Some(delete_faces) = self.excess_faces(kernel, original, result)? else {
            return Ok(());
        };

        // The faces about to be removed and their edgeuses, which RemoveFaces deletes
        let gone_faces: BTreeSet<FaceId> = delete_faces.iter().copied().collect();
        let gone = removed_faces_edgeuses(result, &delete_faces)?;

        // Clean up those faces we have collected.
        let editing_enabled = result.editing_enabled();
        // cpp:2099–2102 ignores ordinary RemoveFaces failures but still executes
        // its editing guard and traversal-mark effects. Missing work must defer.
        ignore_status(crate::topology::brep::p5::remove_faces(
            kernel,
            result,
            editing_enabled,
            &delete_faces,
        ))?;

        // Forget the deleted faces and edgeuses, so later lookups in the relations
        // (lamina side walls, PiecewiseMerge) cannot return them. For m_vEUToEU also
        // forget any edgeuse no result edge has.
        if !gone_faces.is_empty() {
            let mut live = BTreeSet::new();
            for e in bp1::edges(result, None)? {
                live.extend(edge::edgeuses(result, e, None, None)?);
            }
            forget_seconds(&mut self.edgeuse_to_edgeuses, &gone, Some(&live))?;
            self.forget_faces(&gone_faces)?;
        }
        Ok(())
    }

    /// `RemoveExcessFaces`' forgetting of the removed faces: `m_vFUToF`, `m_vEToF`,
    /// `m_vVToF` and `m_vBlendToFaces` forget the pairs whose second is gone, and
    /// `m_vBlendToFaces` also those whose first is gone.
    fn forget_faces(&mut self, gone: &BTreeSet<FaceId>) -> Result<()> {
        forget_seconds(&mut self.faceuse_to_faces, gone, None)?;
        forget_seconds(&mut self.edge_to_faces, gone, None)?;
        forget_seconds(&mut self.vertex_to_faces, gone, None)?;
        forget_seconds(&mut self.blend_to_faces, gone, None)?;
        forget_firsts(&mut self.blend_to_faces, gone)
    }

    fn excess_faces(
        &self,
        kernel: &Kernel,
        original: &mut Brep,
        result: &mut Brep,
    ) -> Result<Option<Vec<FaceId>>> {
        if !self.merge_results || !self.shell_faces.is_empty() {
            return Ok(None);
        }
        let min_distance = self.offset_distance * 0.95;
        let mut delete_faces = Vec::new();
        let mut original_cache = None;
        // SmBrep::GetEdges (unit topology.brep_p1): the m_pEdgeListHead list order
        for id in crate::topology::brep::p1::edges(result, None)? {
            let primary = required(eu::primary_edgeuse(result, id)?)?;
            // NormalizedEvaluate(0.5, FALSE, sPnt): 3d. Upstream ignores the evaluation
            // status and keeps the initialized point.
            let point = ignored(eu3::normalized_evaluate(
                kernel, result, primary, 0.5, false, false, true,
            ))?
            .map_or(Vec3::UNDEF, |eval| eval.point);
            // cpp:2069–2072: the solve runs with the original's editing disabled and
            // leaves it enabled afterwards (still disabled when the solve fails)
            original.set_editing_enabled(false);
            if original_cache.is_none() {
                original_cache = Some(make_original_brep_cache(kernel, original)?);
            }
            let (cache, keys) = required(original_cache.as_ref())?;
            let solutions = solve_original_brep_point(kernel, original, cache, keys, point)?;
            original.set_editing_enabled(true);
            collect_excess_faces(
                result,
                id,
                solutions.as_slice(),
                min_distance,
                &self.shell_faces,
                &mut delete_faces,
            )?;
        }
        // All candidates are gathered in first-seen order, without duplicates.
        Ok(Some(delete_faces))
    }

    /// Builds a closing ruled side wall, or succeeds without work for null/ambiguous mappings.
    /// The new face is named in an arena-only store (`topology::loopuse::notify_ref`).
    pub fn build_side_wall_of_edgeuse(
        &mut self,
        kernel: &mut Kernel,
        original_brep: &mut Brep,
        original: Option<EdgeuseId>,
    ) -> Result<()> {
        self.build_side_wall_of_edgeuse_with(kernel, original_brep, original, None)
    }

    /// [`Self::build_side_wall_of_edgeuse`] notifying the new face through
    /// `notifications`, keyed by result identities (`None`: an arena-only store).
    pub(super) fn build_side_wall_of_edgeuse_with(
        &mut self,
        kernel: &mut Kernel,
        original_brep: &mut Brep,
        original: Option<EdgeuseId>,
        notifications: Option<&mut NotifyState<'_>>,
    ) -> Result<()> {
        let Some(original) = original else {
            return Ok(());
        };
        let Some(offset) = self.offset_edgeuse(original) else {
            return Ok(());
        };
        let offset_brep = required(self.offset_brep.as_mut())?;
        // cpp:2812: ruled blend (0) between the original edge use and its offset edge use
        // (in the offset B-rep), curves in the same direction (1), no blend options
        let new_surface = crate::topology::brep::p1::create_edge_edge_blend(
            kernel,
            0,
            original_brep,
            Some(original),
            Some(offset_brep),
            Some(offset),
            1,
            None,
        )?;
        // cpp:2819: m_pOffsetBrep->CreateFaceFromSurface(pNewSurface, natural domain)
        let domain = new_surface.natural_uv_domain_with_nurb(kernel)?;
        let new_face = match notifications {
            Some(notifications) => {
                let key_of = notifications.key_of;
                let this_face = |face: FaceId| key_of(EntityRef::Face(face));
                notifications.with_kernel_env(kernel, |kernel, env| {
                    let mut marks = crate::topology::topology_traverser::Marks::default();
                    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
                        env,
                        marks: &mut marks,
                        this_face: &this_face,
                    };
                    crate::topology::brep::p8::create_face_from_surface(
                        kernel,
                        offset_brep,
                        Some(new_surface),
                        &domain,
                        &mut fenv,
                    )
                })?
            }
            None => create_face_from_surface(kernel, offset_brep, &new_surface, &domain)?,
        };
        self.closing_faces.push(new_face); // [B549]
        Ok(())
    }

    /// Builds edge cap offsets (`BuildOffsetOfEdgeuses`). Notifications use distinct
    /// original/result object keys; `offset_curve_ref` identifies offset edge curves
    /// split while synchronizing. Every synchronized pair reaches the shared
    /// `EdgeuseOffset` creator; a cap surface becomes a face of the offset B-rep that
    /// the original edge is related to. Edge-use marks are read from the stores'
    /// topology records under `mark_type`.
    #[allow(clippy::too_many_arguments)]
    pub fn build_offset_of_edgeuses(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        other: EdgeuseId,
        mark_type: MarkType,
        notifications: &mut NotifyEnv<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        self.build_offset_of_edgeuses_with_state(
            kernel,
            original,
            edgeuse,
            other,
            mark_type,
            MarkSource::Records,
            &mut NotifyState::from_env(notifications),
            offset_key_of,
            offset_curve_ref,
        )
    }

    /// Builds the edge cap offsets with real caller stores and the kernel context.
    /// `notifications.key_of` identifies source entities; `offset_key_of` identifies
    /// result entities, including any edges created by synchronization. `mark_type`
    /// is the existing traversal mark, not a request to acquire or advance one.
    #[allow(clippy::too_many_arguments)]
    pub fn build_offset_of_edgeuses_with_kernel_context(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        other: EdgeuseId,
        mark_type: MarkType,
        notifications: &mut NotificationStores<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        self.build_offset_of_edgeuses_with_state(
            kernel,
            original,
            edgeuse,
            other,
            mark_type,
            MarkSource::Records,
            &mut NotifyState::from_kernel_stores(notifications),
            offset_key_of,
            offset_curve_ref,
        )
    }

    /// As [`Self::build_offset_of_edgeuses_with_kernel_context`], with the original
    /// edge uses the calling loop has marked as the mark source.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_offset_of_edgeuses_with_marked(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        other: EdgeuseId,
        mark_type: MarkType,
        marked: &BTreeSet<EdgeuseId>,
        notifications: &mut NotificationStores<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        self.build_offset_of_edgeuses_with_state(
            kernel,
            original,
            edgeuse,
            other,
            mark_type,
            MarkSource::Set(marked),
            &mut NotifyState::from_kernel_stores(notifications),
            offset_key_of,
            offset_curve_ref,
        )
    }

    /// Not upstream (WR-346): `m_rGeometryCreation.MateEdges(ctx, this, pEU, pAcross,
    /// pOffEU, pOffAcross, eMarkType)`, mating the offsets of the faces on either side
    /// of a collapsed face (`edgeuse` and `across` are original edge uses beside it).
    ///
    /// # Errors
    ///
    /// Those of `MateEdges`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mate_edges_across_with_marked(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        across: EdgeuseId,
        offset_edgeuse: EdgeuseId,
        offset_across: EdgeuseId,
        mark_type: MarkType,
        marked: &BTreeSet<EdgeuseId>,
        notifications: &mut NotificationStores<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<()> {
        let creator = self.geometry_creator();
        NotifyState::from_kernel_stores(notifications).with_kernel_env(kernel, |kernel, env| {
            let mut result = required(self.offset_brep.take())?;
            let made = super::edgeuse::mate(
                kernel,
                &creator,
                self,
                original,
                &mut result,
                edgeuse,
                across,
                offset_edgeuse,
                offset_across,
                mark_type,
                MarkSource::Set(marked),
                CapNotifications {
                    records: &mut *env.records,
                    attributes: &mut *env.attributes,
                    context: env.context.as_deref_mut(),
                    original_key_of: env.key_of,
                    offset_key_of,
                },
            );
            self.offset_brep = Some(result);
            made
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_offset_of_edgeuses_with_state(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        other: EdgeuseId,
        mark_type: MarkType,
        marks: MarkSource<'_>,
        notifications: &mut NotifyState<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        let Some((edge, synchronized)) = self.prepare_offset_edgeuses(
            kernel,
            original,
            edgeuse,
            other,
            &mut notifications.rekey(offset_key_of),
            offset_curve_ref,
        )?
        else {
            return Ok(());
        };
        let creator = self.geometry_creator();
        let mut new_faces = Vec::new();
        // cpp:2918–3071: every mapped offset edge-use pair may need a cap face.
        for (index, offset) in synchronized.edgeuses.iter().copied().enumerate() {
            // sOtherEUS[ii]: the synchronized lists have one entry per pair.
            let mut other_offset = *required(synchronized.other_edgeuses.get(index))?;
            // clear lamina other offset edges
            if eu::edge_is_lamina(original, edge)? {
                other_offset = None;
            }
            if let Some(offset) = offset {
                let result = self.required_offset_brep()?;
                let result_edge = edge_of(result, offset)?;
                notifications.with_env(kernel, |env| {
                    notify_edge_and_vertices(
                        original,
                        edge,
                        result,
                        result_edge,
                        env,
                        offset_key_of,
                    )
                })?;
            }
            // SER(m_rGeometryCreation.EdgeuseOffset(..)): the creator edits both
            // B-reps through the caller's stores and the selected context.
            let cap = notifications.with_kernel_env(kernel, |kernel, env| {
                let mut result = required(self.offset_brep.take())?;
                let made = super::edgeuse::build(
                    kernel,
                    &creator,
                    self,
                    original,
                    &mut result,
                    edgeuse,
                    other,
                    offset,
                    other_offset,
                    mark_type,
                    marks,
                    CapNotifications {
                        records: &mut *env.records,
                        attributes: &mut *env.attributes,
                        context: env.context.as_deref_mut(),
                        original_key_of: env.key_of,
                        offset_key_of,
                    },
                );
                self.offset_brep = Some(result);
                made
            })?;
            let Some(surface) = cap.surface else {
                continue;
            };
            // Build a face in m_pOffsetBrep for the new cap surface; its own
            // `sLoopPoints` is empty.
            let new_face = self.make_cap_face(
                kernel,
                surface,
                cap.trim_curves,
                cap.trim_orientations,
                &[],
                &mut notifications.rekey(offset_key_of),
            )?;
            // Not upstream (opt-in `Kernel::fixes.local_operation_step_walls`, fork
            // source fork 5d428ae): a step wall spans the gap between two edges
            // exactly, as a lamina side wall does, and is kept out of the face extensions
            // as one (extended along its rulings it can pass through its own axis, a step
            // round a circle, and fold over itself).
            if cap.closing_wall {
                self.closing_faces.push(new_face);
            }
            // map origEdge to its offset capFaces
            new_faces.push(new_face);
            self.relate_edge_face(edge, new_face);
        }

        self.fill_offset_edgeuse_gaps(
            kernel,
            original,
            edge,
            &synchronized,
            &new_faces,
            &mut notifications.rekey(offset_key_of),
        )
    }

    /// The geometry creator this executive's scalars stand for
    /// (`m_rGeometryCreation`), with its angular tolerance.
    fn geometry_creator(&self) -> OffsetGeometryCreation {
        OffsetGeometryCreation::new(self.offset_distance, self.approximation_tol_3d)
            .with_tangency_tol_radians(self.tangency_tol_radians)
    }

    /// Shared prefix; synchronization mutates/notifies the result exactly once.
    /// `None` is the selected-shell-face no-op, not an empty synchronization.
    fn prepare_offset_edgeuses(
        &mut self,
        kernel: &mut Kernel,
        original: &Brep,
        edgeuse: EdgeuseId,
        other: EdgeuseId,
        notifications: &mut NotifyState<'_>,
        curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<Option<(EdgeId, SynchronizedEdgeuses)>> {
        let edge = edge_of(original, edgeuse)?;
        let first_face = eu::face(original, edgeuse)?;
        let other_face = eu::face(original, other)?;
        if first_face.is_some_and(|id| self.shell_faces.contains(&id))
            && other_face.is_some_and(|id| self.shell_faces.contains(&id))
            && !self.extend_convex_edges
        {
            return Ok(None);
        }
        let synchronized = self.synchronize_offset_edgeuses_with_state(
            kernel,
            Some(edgeuse),
            Some(other),
            notifications,
            curve_ref,
        )?;
        Ok(Some((edge, synchronized)))
    }

    /// Tail after the synchronized pair loop, shared with p2: the gap fills' blend
    /// faces notify through `notifications` (keyed by result identities).
    /// Both gap calls run before either status is inspected, as upstream.
    pub(super) fn fill_offset_edgeuse_gaps(
        &mut self,
        kernel: &mut Kernel,
        original: &Brep,
        edge: EdgeId,
        synchronized: &SynchronizedEdgeuses,
        new_faces: &[FaceId],
        notifications: &mut NotifyState<'_>,
    ) -> Result<()> {
        let first = self.fill_gap_edgeuses_with_state(
            kernel,
            edge,
            &synchronized.gaps,
            new_faces,
            notifications,
        );
        let second = self.fill_gap_edgeuses_with_state(
            kernel,
            edge,
            &synchronized.other_gaps,
            new_faces,
            notifications,
        );
        let mut problems = false;
        for status in [first, second] {
            match status {
                Ok(()) => {}
                Err(error) if error.is_unported() => return Err(error),
                Err(_) => problems = true,
            }
        }
        if problems {
            use crate::primitive::primitive_creation::model::PrimitiveModel;
            use crate::primitive::primitive_creation::p1::PrimitiveCreation;

            // cpp:3088-3094: a pipe of the offset distance along the edge's trimmed NURBS
            // curve, built into the offset B-rep (SmPrimitiveCreation on its infinite
            // region, no caps, not translational); the faces it makes are not used.
            let curve = edge::create_trimmed_nurbs_curve(kernel, original, edge)?;
            let Some(CurveKind::BSpline(path)) = curve.as_ref().map(Curve::kind) else {
                // a NULL curve handed to CreatePipeSweep upstream
                return Err(SolidError::null_pointer(OP));
            };
            let offset = required(self.offset_brep.take())?;
            let region = offset.infinite_region();
            let mut model = PrimitiveModel::new(kernel, offset);
            let (mut start, mut side, mut end) = (Vec::new(), Vec::new(), Vec::new());
            let swept = PrimitiveCreation::new(region, 0, false).create_pipe_sweep(
                &mut model,
                PrimitiveModel::MAIN,
                self.offset_distance,
                path,
                self.approximation_tol_3d,
                false,
                false,
                &mut start,
                &mut side,
                &mut end,
            );
            // the offset B-rep comes back whatever the sweep's status (SER)
            self.offset_brep = Some(model.into_brep()?);
            swept?;
        }
        Ok(())
    }

    /// Fills unmatched edge gaps (`FillGapEdgeuses`). Empty gaps succeed without
    /// dereferencing any topology. For each gap use (3D midpoint first), every
    /// candidate-face vertex within the summed vertex tolerances of either end
    /// contributes its incident edge whose far vertex is closest to the midpoint; the
    /// candidate list persists across gap uses. Unless there are exactly two
    /// candidates this returns success; otherwise the gap use and the candidates'
    /// same-oriented uses get a corner blend whose surfaces become offset faces,
    /// related to `edge` and to the faces they blend. `notifications` identifies the
    /// offset B-rep's entities for the new faces' notifications.
    pub fn fill_gap_edgeuses(
        &mut self,
        kernel: &mut Kernel,
        edge: EdgeId,
        gaps: &[EdgeuseId],
        candidate_faces: &[FaceId],
        notifications: &mut NotifyEnv<'_>,
    ) -> Result<()> {
        self.fill_gap_edgeuses_with_state(
            kernel,
            edge,
            gaps,
            candidate_faces,
            &mut NotifyState::from_env(notifications),
        )
    }

    fn fill_gap_edgeuses_with_state(
        &mut self,
        kernel: &mut Kernel,
        edge: EdgeId,
        gaps: &[EdgeuseId],
        candidate_faces: &[FaceId],
        notifications: &mut NotifyState<'_>,
    ) -> Result<()> {
        if gaps.is_empty() {
            return Ok(());
        }
        let mut all_vertices = Vec::new();
        {
            let brep = self.required_offset_brep()?;
            for &face in candidate_faces {
                all_vertices.extend(face_vertices(brep, face)?);
            }
        }
        let mut candidate_edges: Vec<Option<EdgeId>> = Vec::new();
        for &gap in gaps {
            let brep = required(self.offset_brep.as_mut())?;
            // NormalizedEvaluate(0.5, FALSE, sMidPoint): 3d
            let mid_point =
                eu3::normalized_evaluate(kernel, brep, gap, 0.5, false, false, true)?.point;

            // Look at each end of the Gap EU
            for jj in 0..2 {
                let end_use = if jj == 1 {
                    required(eu::mate(brep, gap)?)?
                } else {
                    gap
                };
                let v = start_vertex(brep, end_use)?;
                let v_pnt = brep.vertex(v)?.point;
                let v_tol = vertex::tolerance(brep, v)?;

                // Look at each vertex of the candidate faces
                for &test_v in &all_vertices {
                    let tol = vertex::tolerance(brep, test_v)? + v_tol;
                    let test_pnt = brep.vertex(test_v)?.point;

                    // See if a test vertex is on current end of Gap EU
                    if test_pnt.distance(v_pnt) < tol {
                        let mut closest = None;
                        let mut closest_dist = BIG_DOUBLE;
                        for e in vertex::edges(brep, test_v, None)? {
                            let other_v = required(edge::other_vertex(brep, e, Some(test_v))?)?;
                            let dist = mid_point.distance(brep.vertex(other_v)?.point);
                            if dist < closest_dist {
                                closest_dist = dist;
                                closest = Some(e);
                            }
                        }
                        candidate_edges.push(closest);
                    }
                }
            }

            let [first, second] = candidate_edges.as_slice() else {
                // MSG(_T("Possible Problem In Offset"));
                return Ok(());
            };

            // sBlendedFaces may hold a null face (a wire edge use); RelatePair rejects it.
            let mut blended_faces: Vec<Option<FaceId>> = Vec::new();
            let mut edgeuses = vec![gap];
            for candidate in [*first, *second] {
                // upstream dereferences a null closest edge or same-oriented edge use
                let e = required(candidate)?;
                let same = required(edge::same_oriented_edgeuse(brep, e)?)?;
                edgeuses.push(same);
                let face = eu::face(brep, same)?;
                if !blended_faces.contains(&face) {
                    blended_faces.push(face);
                }
            }

            // We should now have enough information to make a blend.
            let blend_surfaces = match bp1::create_corner_blend(
                kernel,
                brep,
                &edgeuses,
                None,
                self.approximation_tol_3d,
                self.tangency_tol_radians,
            ) {
                Ok(surfaces) => surfaces,
                Err(error) if error.is_unported() => return Err(error),
                Err(_) => return Err(SolidError::sm_err(OP)),
            };

            for surface in blend_surfaces {
                let new_face = self.make_cap_face(
                    kernel,
                    Surface::from(surface),
                    Vec::new(),
                    Vec::new(),
                    &[],
                    notifications,
                )?;
                self.relate_edge_face(edge, new_face);
                // SER(m_vBlendToFaces.RelateToMany(pNewFace, sBlendedFaces)): pairs
                // before a null face stay related.
                for &face in &blended_faces {
                    let face = face.ok_or_else(|| {
                        SolidError::invalid_input(OP, "RelatePair: null blended face")
                    })?;
                    self.blend_to_faces.relate_pair(new_face, face)?;
                }
            }
        } // end iter every GapEdgeuse
        Ok(())
    }

    /// Builds a vertex cap from its circular/short-line offset boundary edges
    /// (`BuildOffsetOfVertex`), using the original vertex's geometry and tolerance.
    pub fn build_offset_of_vertex(
        &mut self,
        k: &Kernel,
        original: &Brep,
        vertex: Option<VertexId>,
    ) -> Result<VertexOffset> {
        let vertex = required(vertex)?;
        let point = original.vertex(vertex)?.point;
        let result = self.required_offset_brep()?;
        let tol = result.tolerance();
        let radius = self.offset_distance;
        let mut edges = vertex::edges(original, vertex, None)?;
        let mut edges_to_blend = Vec::new();

        if let [only] = edges.as_slice() {
            let mut edgeuse = required(eu::primary_edgeuse(original, *only)?)?;
            if start_vertex(original, edgeuse)? != vertex {
                edgeuse = required(eu::radial(original, edgeuse)?)?;
            }
            if eu::is_strut(original, edgeuse)? {
                let faceuse = required(eu::faceuse(original, edgeuse)?)?;
                let mut faces = self.faceuse_to_faces.seconds(faceuse);
                if faces.is_empty() {
                    // GetSeconds accepts a null key and returns an empty list upstream.
                    faces = original
                        .faceuse(faceuse)?
                        .mate
                        .map_or_else(Vec::new, |mate| self.faceuse_to_faces.seconds(mate));
                }
                for face in faces {
                    // Upstream reuses sEdges; on this strut branch it returns below.
                    edges = face_edges(result, face)?;
                    for &edge in &edges {
                        let curve = bspline_edge_curve(result, edge)?;
                        if let Some((frame, arc_radius, _, _)) = curve.is_arc(k, 10, tol)? {
                            if arc_radius < self.offset_distance
                                && frame.origin().distance(point) < self.offset_distance + tol
                            {
                                edges_to_blend.push(edge);
                            }
                        }
                    }
                }
                if edges_to_blend.len() != 1 {
                    return Ok(VertexOffset::default());
                }
                return self.vertex_offset_geometry(k, original, vertex, &edges_to_blend);
            }
        }

        let mut offset_faces = Vec::new();
        for &edge in &edges {
            let faces = self.edge_to_faces.seconds(edge);
            if faces.is_empty() {
                return Ok(VertexOffset::default());
            }
            offset_faces.extend(faces);
        }
        for face in offset_faces {
            for edge in face_edges(result, face)? {
                let curve = bspline_edge_curve(result, edge)?;
                if let Some((frame, _, _, _)) = curve.is_arc(k, 10, tol)? {
                    if frame.origin().distance(point) < tol {
                        edges_to_blend.push(edge);
                    }
                } else if let Some((line_point, line_vector)) = curve.is_line(k, 10, tol)? {
                    if (line_vector.length() - radius).abs() < tol {
                        let (distance, _) = crate::geom::geom_utility::p1::segment_point_distance(
                            line_point,
                            line_point + line_vector,
                            point,
                        );
                        if distance < tol {
                            edges_to_blend.push(edge);
                        }
                    }
                }
            }
        }
        // This call is unconditional, even when the vertex has no incident edges.
        self.vertex_offset_geometry(k, original, vertex, &edges_to_blend)
    }

    fn vertex_offset_geometry(
        &mut self,
        kernel: &Kernel,
        original: &Brep,
        vertex: VertexId,
        bounding_edges: &[EdgeId],
    ) -> Result<VertexOffset> {
        // VertexOffset uses only these creator parameters, not the angular or
        // analytic-conversion settings used by the edge/face creator methods.
        let creator = OffsetGeometryCreation::new(self.offset_distance, self.approximation_tol_3d);
        super::vertex::build(
            kernel,
            &creator,
            original,
            vertex,
            required(self.offset_brep.as_mut())?,
            bounding_edges,
        )
    }

    /// Builds a cap face for a new edge or vertex cap surface in the offset B-rep:
    /// `CreateFaceFromSurface` over the surface's natural domain without trimming
    /// curves, else `MakeFaceWithCurves` in the infinite region with one loop of all
    /// curves, no UV curves, `SM_OT_SAME` and the caller's loop points.
    /// `notifications` must be keyed by result identities.
    pub(super) fn make_cap_face(
        &mut self,
        kernel: &mut Kernel,
        surface: Surface,
        curves: Vec<Curve>,
        orientations: Vec<OrientType>,
        loop_points: &[Vec3],
        notifications: &mut NotifyState<'_>,
    ) -> Result<FaceId> {
        let domain = surface.natural_uv_domain_with_nurb(kernel)?;
        let key_of = notifications.key_of;
        let this_face = |face: FaceId| key_of(EntityRef::Face(face));
        let brep = required(self.offset_brep.as_mut())?;
        if curves.is_empty() {
            return notifications.with_kernel_env(kernel, |kernel, env| {
                let mut marks = crate::topology::topology_traverser::Marks::default();
                let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
                    env,
                    marks: &mut marks,
                    this_face: &this_face,
                };
                crate::topology::brep::p8::create_face_from_surface(
                    kernel,
                    brep,
                    Some(surface),
                    &domain,
                    &mut fenv,
                )
            });
        }
        let region = brep.infinite_region();
        let input = FaceCurvesInput {
            loops: vec![curves.len()],
            curves_3d: Some(curves.into_iter().map(Some).collect()),
            uv_curves: None,
            orientations,
            loop_points: loop_points.to_vec(),
        };
        let made = bp7::make_face_with_curves_with_state(
            kernel,
            brep,
            region,
            input,
            surface,
            &domain,
            OrientType::Same,
            None,
            &this_face,
            notifications,
        )?;
        required(made.face)
    }
}

impl OffsetExecutive {
    /// Stitches or merges the assembled offset (`PiecewiseMerge`).
    ///
    /// On the non-merge path the B-rep is taken and relations are cleared *before*
    /// stitching, as upstream. A deferred stitch therefore leaves `offset_brep()` empty.
    /// The merge path builds a new merge B-rep and runs `SmMerge::PiecewiseMerge` over
    /// the offset temporary in the caller's `notifications` stores (`None` refuses it);
    /// the temporary stays in `offset_brep()`, as `m_pOffsetBrep` upstream.
    ///
    /// # Errors
    ///
    /// Those of the stitch and of the merge; [`SolidError::InvalidInput`] for a merge
    /// without `notifications` or without their merge identities.
    pub fn piecewise_merge(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        notifications: Option<&mut OffsetNotifications<'_>>,
    ) -> Result<Brep> {
        if !self.merge_results {
            self.clear_relations();
            let mut result = required(self.offset_brep.take())?;
            // the stitch statuses are ignored upstream. Its edits notify the offset's
            // objects (its glue merges and splits carry their attributes, as upstream's
            // objects' own do) through the caller's stores under the result identities;
            // without them, on call-local records.
            let create_offset_solid = self.create_offset_solid;
            let run = |kernel: &mut Kernel,
                       result: &mut Brep,
                       edits: Option<&mut dyn StitchEditContext>|
             -> Result<()> {
                if create_offset_solid {
                    // Tol: StitchIntoShell() adds in the tols of the topology objects,
                    // so the Brep's tolerance should be plenty.
                    let stitch_tol_3d = result.tolerance();
                    ignore_status(stitch::stitch_into_shell_with(
                        kernel,
                        result,
                        true,
                        stitch_tol_3d,
                        edits,
                    ))
                } else {
                    ignore_status(stitch::stitch_into_solid_with(kernel, result, edits))
                }
            };
            match notifications {
                Some(n) => {
                    let key_of = n.result_key_of;
                    let curve_ref = n.result_curve_ref;
                    let (records, attributes) = (&mut *n.records, &mut *n.attributes);
                    Context::with_notification_context(kernel, |kernel, context| {
                        let mut env = NotifyEnv {
                            records,
                            attributes,
                            context: Some(context),
                            key_of,
                        };
                        let mut edits = BorrowedStitchEdits {
                            env: &mut env,
                            curve_identity: CurveIdentity::Legacy(curve_ref),
                        };
                        run(kernel, &mut result, Some(&mut edits))
                    })??;
                }
                None => run(kernel, &mut result, None)?,
            }

            if bp1::regions(&result, None)?.len() > 1 {
                let old = result.editing_enabled();
                result.set_editing_enabled(false);
                let r = stitch::find_and_set_infinite_region(
                    kernel,
                    &mut result,
                    None,
                    SetRegionVoidType::InfiniteRegionSwap,
                    None,
                );
                result.set_editing_enabled(old);
                r?;
            }
            return Ok(result);
        }

        // Surfaces are face-owned concrete values. Face ids identify the two sides
        // of each surface pair; no geometry is copied to build this heuristic.
        let result = self.required_offset_brep()?;
        let mut no_ssi: Relation<FaceId, FaceId> = Relation::default();
        for faceuse in self.faceuse_to_faces.all_firsts() {
            let face = required(original.faceuse(faceuse)?.face)?;
            for offset_face in self.faceuse_to_faces.seconds(faceuse) {
                for edge_id in face_edges(original, face)? {
                    let edge_faces = self.edge_to_faces.seconds(edge_id);
                    for &edge_face in &edge_faces {
                        for blended in self.blend_to_faces.seconds(edge_face) {
                            relate_surface_pair(result, &mut no_ssi, blended, edge_face)?;
                        }
                        relate_surface_pair(result, &mut no_ssi, offset_face, edge_face)?;
                        let start = edge::vertex(original, edge_id)?;
                        let end = edge::other_vertex(original, edge_id, start)?;
                        for vertex in [start, end].into_iter().flatten() {
                            if let Some(vertex_face) = self.vertex_to_faces.second(vertex) {
                                relate_surface_pair(result, &mut no_ssi, edge_face, vertex_face)?;
                            }
                        }
                    }
                    if !edge_faces.is_empty() {
                        continue;
                    }
                    if edge::is_manifold(original, edge_id)? {
                        let primary = required(eu::primary_edgeuse(original, edge_id)?)?;
                        // Not upstream (WR-344, source issue #469), opt-in
                        // `Kernel::fixes.offset_extended_edges_mated`: with extended convex edges,
                        // every edge without an offset cap face had its two faces' offsets
                        // mated along it (EdgeuseOffset), so they share their intersection
                        // edge already, as a tangent sector's do (the tangency test is then
                        // skipped, as the fork's `||` does).
                        let mated =
                            kernel.fixes.offset_extended_edges_mated && self.extend_convex_edges();
                        if mated
                            || eu2::is_tangent_sector(
                                kernel,
                                original,
                                primary,
                                self.tangency_tol_degrees(),
                                false,
                                true,
                            )?
                        {
                            for edgeuse in edge::edgeuses(original, edge_id, None, None)? {
                                if eu::faceuse(original, edgeuse)? == Some(faceuse) {
                                    let radial = required(eu::radial(original, edgeuse)?)?;
                                    let radial_faceuse = required(eu::faceuse(original, radial)?)?;
                                    for radial_face in self.faceuse_to_faces.seconds(radial_faceuse)
                                    {
                                        relate_surface_pair(
                                            result,
                                            &mut no_ssi,
                                            offset_face,
                                            radial_face,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
                for vertex in face_vertices(original, face)? {
                    if let Some(vertex_face) = self.vertex_to_faces.second(vertex) {
                        relate_surface_pair(result, &mut no_ssi, offset_face, vertex_face)?;
                    }
                }
            }
        }
        self.clear_relations();
        // Shrink the geometry - this helps some for self intersections
        shrink_geometry(kernel, required(self.offset_brep.as_mut())?)?;
        // Now Get things ready for piecewise merge (cpp:2453–2474)
        let notifications = notifications.ok_or_else(|| {
            SolidError::invalid_input(OP, "a merging PiecewiseMerge needs notification stores")
        })?;
        let offset = required(self.offset_brep.take())?;
        let (result, offset) = super::merge::merge_offset(
            kernel,
            offset,
            original.tolerance(),
            &no_ssi,
            notifications,
        );
        self.offset_brep = offset;
        result
    }

    /// Builds a face from the supplied offset surface (`BuildOffsetOfFaceuse`).
    /// A null surface is a successful no-op. When the face domain lies inside the
    /// offset surface's natural domain (or for a local operation) the offset face is
    /// made from the face's UV curve copies and takes ownership of the surface
    /// (`offset_surface` becomes `None`); `notifications` keys original-side
    /// entities and `offset_key_of` offset-side ones, and `offset_curve_ref` the curves
    /// of offset edges split by the self-intersection merge. Otherwise the face's UV
    /// domain is intersected with the offset surface's in 2D and the surface (taken
    /// from `offset_surface` once that boolean has run) trims one offset face per
    /// resulting face.
    #[allow(clippy::too_many_arguments)]
    pub fn build_offset_of_faceuse(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        faceuse: Option<FaceuseId>,
        offset_surface: &mut Option<Surface>,
        self_intersections: &SsiData<Curve>,
        notifications: &mut NotifyEnv<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        let faceuse = required(faceuse)?;
        let Some(surface) = offset_surface.as_mut() else {
            return Ok(());
        };
        self.required_offset_brep()?;
        // SM_ENSURE_SURFACE_MPNURB: its return value is not checked.
        match surface.kind_mut() {
            SurfaceKind::BSpline(surface) => {
                ignore_status(surface.get_or_create_gw_nurb())?;
            }
            SurfaceKind::Plane(surface) if surface.base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb())?;
            }
            SurfaceKind::SurfOfRevolution(surface) if surface.base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb(kernel))?;
            }
            SurfaceKind::SurfOfExtrusion(surface) if surface.base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb(kernel))?;
            }
            // (SmCone/SmCylinder, SmSphere and SmTorus are SmBSplineSurfaces too)
            SurfaceKind::Cone(surface) if surface.rev().base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb(kernel))?;
            }
            SurfaceKind::Sphere(surface) if surface.base().base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb(kernel))?;
            }
            SurfaceKind::Torus(surface) if surface.base().base().gw_nurb().is_none() => {
                ignore_status(surface.make_nurb(kernel))?;
            }
            _ => {}
        }
        let original_faceuse = faceuse;
        let faceuse = original.faceuse(faceuse)?;
        let face_id = required(faceuse.face)?;
        let orientation = faceuse.orientation;
        // Not upstream (WR-342, source issue #974), opt-in
        // `Kernel::fixes.offset_clip_right_side_out`: a face that does not fit the offset
        // surface's domain is clipped to it by a 2D boolean of planar faces made from its
        // UV loops, which need them counterclockwise (the SM_OT_SAME faceuse's sense); an
        // inset's SM_OT_OPPOSITE loops made that face inside out and the clip empty.
        let right_side_out = kernel.fixes.offset_clip_right_side_out;
        let loop_orientation = if right_side_out {
            let fits = original
                .face(face_id)?
                .uv_domain
                .is_contained_by(&surface.natural_uv_domain_with_nurb(kernel)?, EFF_ZERO)
                || self.operation == OffsetOperation::LocalOperation;
            if fits {
                orientation
            } else {
                OrientType::Same
            }
        } else {
            orientation
        };
        // The translated-3D-curve plane case (cpp:3560–3600) is `#if 0` upstream.
        let mut curves = face::create_curves_from_face(
            kernel,
            original,
            face_id,
            loop_orientation,
            false,
            true,
        )?;
        let offset_domain = surface.natural_uv_domain_with_nurb(kernel)?;
        let face_domain = original.face(face_id)?.uv_domain;
        if face_domain.is_contained_by(&offset_domain, EFF_ZERO)
            || self.operation == OffsetOperation::LocalOperation
        {
            let surface = required(offset_surface.take())?;
            let new_faces = self.build_contained_offset_faces(
                kernel,
                original,
                face_id,
                orientation,
                curves,
                surface,
                &offset_domain,
                self_intersections,
                notifications,
                offset_key_of,
                offset_curve_ref,
            )?;
            for new_face in new_faces {
                self.relate_faceuse(original_faceuse, new_face);
                self.map_edgeuses(
                    kernel,
                    original,
                    original_faceuse,
                    new_face,
                    notifications,
                    offset_key_of,
                )?;
            }
            return Ok(());
        }

        // cpp:3765–3928: the 2D domain boolean.
        let new_faces = self.build_boolean_offset_faces(
            kernel,
            original,
            face_id,
            orientation,
            &mut curves,
            offset_surface,
            &offset_domain,
            notifications,
            offset_key_of,
        )?;
        for new_face in new_faces {
            self.relate_faceuse(original_faceuse, new_face);
            self.map_edgeuses(
                kernel,
                original,
                original_faceuse,
                new_face,
                notifications,
                offset_key_of,
            )?;
        }
        Ok(())
    }

    /// The 2D-boolean branch of `BuildOffsetOfFaceuse` (cpp:3765–3928), when the face
    /// domain is not inside the offset surface's: the face's UV trim curves (as 3D
    /// curves at `z = 0`) bound a face on a plane, which is intersected
    /// (`SmPrimitiveCreation::Boolean2D`) with the offset surface's UV domain box;
    /// each resulting face's curves, back in 2D, trim an offset face on the surface.
    /// Returns the offset faces in the result's face order. The surface is taken from
    /// `offset_surface` once the domain boolean has run.
    #[allow(clippy::too_many_arguments)]
    fn build_boolean_offset_faces(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        face_id: FaceId,
        orientation: OrientType,
        curves: &mut face::FaceCurves,
        offset_surface: &mut Option<Surface>,
        offset_domain: &Extent2d,
        notifications: &mut NotifyEnv<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<Vec<FaceId>> {
        use crate::geom::tol::USE_DEFAULT;
        use crate::primitive::primitive_creation::p1::{Boolean2d, PrimitiveCreation};
        use crate::topology::brep::p2::ArenaNotifyState;
        use crate::topology::brep::p3::new_brep;
        use crate::topology::brep::p8::{create_planar_face_with_3d_curves, FaceBuildEnv};
        use crate::topology::topology_traverser::Marks;

        let arena_face = |face: FaceId| loopuse::notify_ref(EntityRef::Face(face));
        // The domain and planar B-reps and their 2D Boolean are made through the
        // kernel context, whose callbacks a kernel-context caller has lent to
        // `notifications.context` (upstream: the one context).
        let lent = notifications.context.as_deref_mut();
        let result =
            Context::with_lease_returned(kernel, lent, |kernel| -> Result<Option<Brep>> {
                let mut planar_brep = new_brep(kernel, USE_DEFAULT, USE_DEFAULT)?;
                planar_brep.set_editing_enabled(true);
                planar_brep.set_tolerance(1.0e-6);

                // get bounding box containing all pFU->Face->UVTrimCurves
                let mut total_bbox = Extent3d::new();
                if let Some(uv_curves) = curves.uv_curves.as_mut() {
                    for curve in uv_curves {
                        let bbox = uv_curve_bounding_box(kernel, curve)?;
                        total_bbox = total_bbox.union(&bbox);
                    }
                }

                // create a 3d line between every offsetSurface domain corner point
                let min = offset_domain.min();
                let max = offset_domain.max();
                let mut box_curves = Vec::new();
                for (start, end, ivl) in [
                    (
                        min,
                        offset_domain.evaluate(1.0, 0.0),
                        Extent1d::from_raw(min.x, max.x),
                    ),
                    (
                        offset_domain.evaluate(0.0, 1.0),
                        max,
                        Extent1d::from_raw(min.x, max.x),
                    ),
                    (
                        min,
                        offset_domain.evaluate(0.0, 1.0),
                        Extent1d::from_raw(min.y, max.y),
                    ),
                    (
                        offset_domain.evaluate(1.0, 0.0),
                        max,
                        Extent1d::from_raw(min.y, max.y),
                    ),
                ] {
                    let mut line = crate::nurbs::b_spline_curve::BSplineCurve::create_line_segment(
                        3,
                        Vec3::new(start.x, start.y, 0.0),
                        Vec3::new(end.x, end.y, 0.0),
                    )?;
                    ignore_status(line.edit_parameterization(&ivl))?;
                    box_curves.push(Curve::from(line));
                }

                // make a new Brep containing 1 planar face bounded by the 3d Curves just made
                let mut domain_brep = new_brep(kernel, USE_DEFAULT, USE_DEFAULT)?;
                domain_brep.set_tolerance(planar_brep.tolerance());
                domain_brep.set_editing_enabled(true);
                {
                    let mut state = ArenaNotifyState::new(&domain_brep);
                    let mut env = state.env(None);
                    let mut marks = Marks::default();
                    let mut fenv = FaceBuildEnv {
                        env: &mut env,
                        marks: &mut marks,
                        this_face: &arena_face,
                    };
                    let region = domain_brep.infinite_region();
                    let tolerance = domain_brep.tolerance();
                    create_planar_face_with_3d_curves(
                        kernel,
                        &mut domain_brep,
                        region,
                        &box_curves,
                        tolerance,
                        &mut fenv,
                    )?;
                }

                // copy the plane just made and get its natural domain
                let mut copy_plane = {
                    // GetSurfaces: the surfaces their faces own, under the faces' identities.
                    let owners = bp1::surface_owners(&domain_brep, None, &arena_face)?;
                    let first_face = required(owners.first().copied())?;
                    let first = required(domain_brep.face(first_face)?.surface.as_ref())?;
                    if !matches!(first.kind(), SurfaceKind::Plane(_)) {
                        // SM_CAST_PTR yields NULL, dereferenced next upstream.
                        return Err(SolidError::null_pointer(OP));
                    }
                    first.copy(kernel)?
                };
                let mut copy_domain = copy_plane.natural_uv_domain_with_nurb(kernel)?;
                let SurfaceKind::Plane(plane) = copy_plane.kind_mut() else {
                    return Err(SolidError::null_pointer(OP));
                };

                // increase the copy domain to include the projection of every UV trim curve
                // bounding box corner onto this plane.
                for corner in [
                    total_bbox.min(),
                    total_bbox.max(),
                    total_bbox.evaluate(1.0, 0.0, 0.0),
                    total_bbox.evaluate(0.0, 1.0, 0.0),
                ] {
                    copy_domain.add_point(plane.project_point_to_uv_domain(corner));
                }
                ignore_status(plane.adjust_step_uv_domain(&copy_domain))?;
                let copy_plane_domain = copy_plane.natural_uv_domain_with_nurb(kernel)?;

                // make a planar face using the offset UV trim curves (as 3D curves)
                let mut loops = Vec::new();
                for &count in &curves.curve_loops {
                    loops.push(
                        usize::try_from(count).map_err(|_| {
                            SolidError::invalid_input(OP, "loop count exceeds usize")
                        })?,
                    );
                }
                let input = FaceCurvesInput {
                    loops,
                    curves_3d: curves
                        .uv_curves
                        .take()
                        .map(|list| list.into_iter().map(Some).collect()),
                    uv_curves: None,
                    orientations: std::mem::take(&mut curves.curve_orients),
                    loop_points: std::mem::take(&mut curves.loop_points),
                };
                {
                    let mut state = ArenaNotifyState::new(&planar_brep);
                    let mut env = state.env(None);
                    let region = planar_brep.infinite_region();
                    bp7::make_face_with_curves(
                        kernel,
                        &mut planar_brep,
                        region,
                        input,
                        copy_plane,
                        &copy_plane_domain,
                        OrientType::Same,
                        None,
                        &arena_face,
                        &mut env,
                    )?;
                }

                // Intersect in 2d the domain B-rep with the planar B-rep.
                let mut result = None;
                {
                    let mut primitive = PrimitiveCreation::new(None, 0, false);
                    let mut domain_marks = Marks::default();
                    let mut planar_marks = Marks::default();
                    let mut state = ArenaNotifyState::new(&domain_brep);
                    state.seed(&planar_brep);
                    primitive.boolean_2d(
                        kernel,
                        &mut domain_brep,
                        &mut planar_brep,
                        Boolean2d::Intersection,
                        &mut result,
                        (&mut domain_marks, &mut planar_marks),
                        &mut state.env(None),
                    )?;
                }
                Ok(result)
            })?;
        // Upstream dereferences the result next, even when Boolean2D left it null.
        let mut result = required(result)?;

        // The result's faces represent the UV domain of the part of the original face
        // that corresponds to the offset face. Their edge curves are UV trim curves,
        // mapped into the offset B-rep with the offset surface.
        let surface = required(offset_surface.take())?;
        let result_faces = bp1::faces(&result, None)?;
        let mut new_faces = Vec::new();
        for (index, &result_face) in result_faces.iter().enumerate() {
            // (WR-342 opt-in: the loops in the sense of the faceuse's orientation, which
            // the offset face is made with)
            let result_orientation = if kernel.fixes.offset_clip_right_side_out {
                orientation
            } else {
                OrientType::Same
            };
            let made = face::create_curves_from_face(
                kernel,
                &mut result,
                result_face,
                result_orientation,
                true,
                false,
            )?;
            let mut uv_curves = made.curves_3d.unwrap_or_default();
            // Convert face 3dCurves into 2D curves (all their Z components are zero).
            for curve in &mut uv_curves {
                ignore_status(curve.convert_to_2d(kernel))?;
            }
            let mut loops = Vec::new();
            for &count in &made.curve_loops {
                loops.push(
                    usize::try_from(count)
                        .map_err(|_| SolidError::invalid_input(OP, "loop count exceeds usize"))?,
                );
            }
            let input = FaceCurvesInput {
                loops,
                curves_3d: None,
                uv_curves: Some(uv_curves),
                orientations: made.curve_orients,
                loop_points: made.loop_points,
            };
            // Upstream every face shares the one offset surface; here each face but
            // the first owns an exact copy of it.
            let face_surface = if index == 0 {
                surface.clone()
            } else {
                surface.copy(kernel)?
            };
            let this_face = |face| offset_key_of(EntityRef::Face(face));
            let brep = required(self.offset_brep.as_mut())?;
            let region = brep.infinite_region();
            let made = bp7::make_face_with_curves(
                kernel,
                brep,
                region,
                input,
                face_surface,
                offset_domain,
                orientation,
                None,
                &this_face,
                &mut offset_env(notifications, offset_key_of),
            )?;
            let offset_face = required(made.face)?;
            new_faces.push(offset_face);
            // SM_NO_GET_BREP is NULL in the pinned reference; the status is not checked.
            ignore_status(face3::notify(
                original,
                face_id,
                notifications,
                None,
                NotifyOperation::Copy,
                [
                    Some(offset_key_of(EntityRef::Face(offset_face))),
                    None,
                    None,
                ],
            ))?;
        }
        Ok(new_faces)
    }

    /// The contained-domain branch of `BuildOffsetOfFaceuse` (cpp:3613–3762): the
    /// offset faces to relate to the face use, empty when the face constructor fails
    /// (upstream ignores that status).
    #[allow(clippy::too_many_arguments)]
    fn build_contained_offset_faces(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        face_id: FaceId,
        orientation: OrientType,
        curves: face::FaceCurves,
        surface: Surface,
        offset_domain: &Extent2d,
        self_intersections: &SsiData<Curve>,
        notifications: &mut NotifyEnv<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<Vec<FaceId>> {
        let mut loops = Vec::new();
        for &count in &curves.curve_loops {
            loops.push(
                usize::try_from(count)
                    .map_err(|_| SolidError::invalid_input(OP, "loop count exceeds usize"))?,
            );
        }
        let input = FaceCurvesInput {
            loops,
            curves_3d: None,
            uv_curves: curves.uv_curves,
            orientations: curves.curve_orients,
            loop_points: curves.loop_points,
        };
        let this_face = |face| offset_key_of(EntityRef::Face(face));
        let brep = required(self.offset_brep.as_mut())?;
        let region = brep.infinite_region();
        // Only success continues; a failed constructor leaves no face to relate.
        // The surface moves into the face constructor and is dropped on failure.
        let made = match bp7::make_face_with_curves(
            kernel,
            brep,
            region,
            input,
            surface,
            offset_domain,
            orientation,
            None,
            &this_face,
            &mut offset_env(notifications, offset_key_of),
        ) {
            Ok(made) => made,
            Err(error) if error.is_unported() => return Err(error),
            Err(_) => return Ok(Vec::new()),
        };
        let new_face = required(made.face)?;
        // SM_NO_GET_BREP is NULL in the pinned reference; the status is not checked.
        ignore_status(face3::notify(
            original,
            face_id,
            notifications,
            None,
            NotifyOperation::Copy,
            [Some(offset_key_of(EntityRef::Face(new_face))), None, None],
        ))?;

        let mut new_edges = Vec::new();
        if self.merge_results && !self_intersections.curves_3d.is_empty() {
            // Upstream dereferences each UV curve unchecked.
            let mut merged = Vec::new();
            for (index, curve_3d) in self_intersections.curves_3d.iter().enumerate() {
                let uv_curve = |list: &[Option<Curve>]| -> Result<Curve> {
                    required(list.get(index).and_then(Option::as_ref)).cloned()
                };
                merged.push(SelfIntersectionCurve {
                    curve_3d: curve_3d.clone(),
                    uv_curve_1: uv_curve(&self_intersections.uv_curves1)?,
                    uv_curve_2: uv_curve(&self_intersections.uv_curves2)?,
                });
            }
            let tolerance = brep.tolerance();
            new_edges = bp7::merge_self_intersection_curves(
                kernel,
                brep,
                Some(new_face),
                tolerance,
                &merged,
                &mut offset_env(notifications, offset_key_of),
                offset_curve_ref,
            )?;
        }
        let mut new_faces = vec![new_face];
        for &edge in &new_edges {
            for face in edge::faces(brep, edge, None)? {
                if !new_faces.contains(&face) {
                    new_faces.push(face);
                }
            }
        }

        let mut delete_faces = Vec::new();
        for &face in &new_faces {
            if self.extend_convex_edges {
                continue;
            }
            for edge in face_edges(brep, face)? {
                if !eu::edge_is_lamina(brep, edge)? {
                    continue;
                }
                let edgeuse = required(edge::same_oriented_edgeuse(brep, edge)?)?;
                let point =
                    eu3::normalized_evaluate(kernel, brep, edgeuse, 0.4567, false, false, true)?
                        .point;
                let offset_distance = self.offset_distance;
                let test_distance = offset_distance * 1.1;
                let original_face = original.face(face_id)?;
                let uv_domain = original_face.uv_domain;
                let original_surface = required(original_face.surface.as_ref())?;
                // Upstream turns the surface cache's point test off here so the solve
                // keeps all solutions without classifying them against the trim
                // boundaries (`SmOffsetExecutive.cpp:3730-3734`).
                let solutions = original_surface.global_point_solve_point_test_off(
                    kernel,
                    Some(NoObject),
                    &uv_domain,
                    SolverOperation::Minimize,
                    point,
                    face::tolerance(brep, face)?,
                    Some(test_distance),
                    SolutionRequested::Single,
                )?;
                let offset_tol = self.approximation_tol_3d;
                let Some(solution) = solutions.get(0) else {
                    continue;
                };
                if solution.start.solution_value < offset_distance - 2.0 * offset_tol {
                    if !delete_faces.contains(&face) {
                        delete_faces.push(face);
                    }
                    break;
                }
            }
        }
        if !delete_faces.is_empty() {
            // cpp:3749-3759: SmTopologyIntersector sTI(m_crContext, m_pOffsetBrep,
            // m_pOffsetBrep, SM_EFF_ZERO, SM_EFF_ZERO); sTI.DeleteFaces(sDelFaces)
            // (pOptKeepAsWires = NULL, status ignored); then FindElement / RemoveAt
            // on sNewFaces. The intersector over the one offset B-rep is merge p4's
            // arena adapter; its notifications are keyed under
            // `DELETE_FACES_NAMESPACE`, not `offset_key_of`.
            ignore_status(KernelManifoldIntersector::delete_faces_in(
                kernel,
                brep,
                &delete_faces,
                &mut *notifications.records,
                &mut *notifications.attributes,
                DELETE_FACES_NAMESPACE,
            ))?;
            new_faces.retain(|face| !delete_faces.contains(face));
        }
        Ok(new_faces)
    }

    /// Matches original and offset edge uses by their UV midpoints (`MapEdgeuses`):
    /// the offset face side whose normal agrees with the original face use is
    /// searched at tolerances scaled 1, 10, 100 and 1000, falling back to an
    /// intersecting point drop above 10. Each match is related, its edges and vertices
    /// are notified (`notifications` keys original-side entities, `offset_key_of`
    /// offset-side ones) and the original use is consumed.
    pub fn map_edgeuses(
        &mut self,
        kernel: &Kernel,
        original: &mut Brep,
        original_faceuse: FaceuseId,
        offset_face: FaceId,
        notifications: &mut NotifyEnv<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<()> {
        let mut state = NotifyState::from_env(notifications);
        self.map_edgeuses_with_state(
            MapKernel::Read(kernel),
            original,
            original_faceuse,
            offset_face,
            &mut state,
            offset_key_of,
        )
    }

    /// Map corresponding edge uses and notify through the live kernel context.
    /// Geometry is evaluated with the kernel borrowed immutably; its context is
    /// borrowed mutably only for each notification callback.
    pub fn map_edgeuses_with_kernel_context(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        original_faceuse: FaceuseId,
        offset_face: FaceId,
        notifications: &mut NotificationStores<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<()> {
        let mut state = NotifyState::from_kernel_stores(notifications);
        self.map_edgeuses_with_state(
            MapKernel::Mutable(kernel),
            original,
            original_faceuse,
            offset_face,
            &mut state,
            offset_key_of,
        )
    }

    fn map_edgeuses_with_state(
        &mut self,
        mut kernel: MapKernel<'_>,
        original: &mut Brep,
        original_faceuse: FaceuseId,
        offset_face: FaceId,
        notifications: &mut NotifyState<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    ) -> Result<()> {
        let original_use = original.faceuse(original_faceuse)?;
        let original_orientation = original_use.orientation;
        let original_face = original.face(required(original_use.face)?)?;
        let brep = self.required_offset_brep()?;
        let offset_face = brep.face(offset_face)?;
        // Intersect's status is ignored; upstream initializes its output on failure.
        let domain = ignored(original_face.uv_domain.intersect(&offset_face.uv_domain))?
            .unwrap_or_else(Extent2d::new);
        let mid_uv = domain.evaluate(0.5, 0.5);
        // (`EvaluateNormal(sMidUV, TRUE, TRUE, ..)`: from the left)
        let normal = ignored(required(original_face.surface.as_ref())?.evaluate_normal(
            kernel.kernel(),
            mid_uv,
            Side::Left,
            Side::Left,
        ))?
        .unwrap_or(Vec3::UNDEF);
        let offset_normal = ignored(required(offset_face.surface.as_ref())?.evaluate_normal(
            kernel.kernel(),
            mid_uv,
            Side::Left,
            Side::Left,
        ))?
        .unwrap_or(Vec3::UNDEF);
        let same_normals = normal.dot(offset_normal) > 0.0;
        let mut original_uses: Vec<Option<EdgeuseId>> =
            eu::faceuse_edgeuses(original, original_faceuse, false, None)?
                .into_iter()
                .map(Some)
                .collect();
        let upward = required(offset_face.faceuse)?;
        let offset_use = if (same_normals && original_orientation == OrientType::Same)
            || (!same_normals && original_orientation == OrientType::Opposite)
        {
            upward
        } else {
            required(brep.faceuse(upward)?.mate)?
        };
        for offset in eu::faceuse_edgeuses(brep, offset_use, false, None)? {
            // Only 0.5 is used: matched uses may run in opposite directions.
            let brep = required(self.offset_brep.as_mut())?;
            let offset_point =
                eu3::normalized_evaluate(kernel.kernel(), brep, offset, 0.5, true, false, true)?
                    .point;
            let mut found = false;
            let mut scale = 1.0;
            while !found && scale < 10000.0 {
                for slot in 0..original_uses.len() {
                    let Some(original_use) = original_uses.get(slot).copied().flatten() else {
                        continue;
                    };
                    let original_point = eu3::normalized_evaluate(
                        kernel.kernel(),
                        original,
                        original_use,
                        0.5,
                        true,
                        false,
                        true,
                    )?
                    .point;
                    let distance = original_point.distance(offset_point);
                    let uv_tol = EFF_ZERO_SQRT * (1.0 + offset_point.max_dimension());
                    found = distance < uv_tol * scale;
                    // Curves may have been split: drop onto the original UV curve.
                    if !found && scale > 10.0 {
                        let uv_curve = required(eu::uv_trim_curve(original, original_use)?.0)?;
                        found = uv_curve
                            .drop_point(
                                kernel.kernel(),
                                &uv_curve.natural_interval(),
                                offset_point,
                                None,
                                uv_tol * scale,
                                None,
                                SolverOperation::Intersect,
                            )?
                            .success;
                    }
                    if found {
                        self.relate_edgeuses(original_use, offset);
                        let result = self.required_offset_brep()?;
                        let original_edge = edge_of(original, original_use)?;
                        let offset_edge = edge_of(result, offset)?;
                        kernel.with_env(notifications, |env| {
                            notify_edge_and_vertices(
                                original,
                                original_edge,
                                result,
                                offset_edge,
                                env,
                                offset_key_of,
                            )
                        })?;
                        if let Some(slot) = original_uses.get_mut(slot) {
                            *slot = None;
                        }
                        break;
                    }
                }
                scale *= 10.0;
            }
        }
        Ok(())
    }

    /// Builds a face while binding source notifications to the supplied kernel context.
    /// Source and newly-created result entities retain their separate key namespaces.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_offset_of_faceuse_with_kernel_context(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        faceuse: FaceuseId,
        offset_surface: &mut Option<Surface>,
        self_intersections: &SsiData<Curve>,
        notifications: &mut NotificationStores<'_>,
        offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
        offset_curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
    ) -> Result<()> {
        Context::with_notification_context(kernel, |kernel, context| {
            self.build_offset_of_faceuse(
                kernel,
                original,
                Some(faceuse),
                offset_surface,
                self_intersections,
                &mut NotifyEnv {
                    records: &mut *notifications.records,
                    attributes: &mut *notifications.attributes,
                    context: Some(context),
                    key_of: notifications.key_of,
                },
                offset_key_of,
                offset_curve_ref,
            )
        })?
    }
}

/// The notification environment of offset-side entities: the caller's records,
/// attributes and context under the offset keys.
fn offset_env<'a>(
    notifications: &'a mut NotifyEnv<'_>,
    offset_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
) -> NotifyEnv<'a> {
    NotifyEnv {
        records: &mut *notifications.records,
        attributes: &mut *notifications.attributes,
        context: notifications.context.as_deref_mut(),
        key_of: offset_key_of,
    }
}

fn relate_surface_pair(
    result: &Brep,
    relation: &mut Relation<FaceId, FaceId>,
    a: FaceId,
    b: FaceId,
) -> Result<()> {
    // Upstream `SmOffsetExecutive::PiecewiseMerge` (cpp:2313–2412) does not null-check
    // `GetSurface()`, and `SmRelation::RelatePair` (SmRelation.h:203–206, unchecked) is
    // a no-op when either pointer is NULL — including faces already destroyed by
    // `RemoveExcessFaces` whose `~SmFace` cleared `m_pSurface` (UB-256).
    let has_surface = |id: FaceId| result.faces().get(id).is_some_and(|f| f.surface.is_some());
    if !has_surface(a) || !has_surface(b) {
        return Ok(());
    }
    relation.relate_pair(a, b)?;
    relation.relate_pair(b, a)
}

fn uv_curve_bounding_box(kernel: &Kernel, curve: &mut Curve) -> Result<Extent3d> {
    // CreateCurvesFromFace supplied B-spline slicing copies or lines.
    match curve.kind_mut() {
        CurveKind::BSpline(curve) => ignore_status(curve.convert_to_3d())?,
        CurveKind::Line(curve) => ignore_status(curve.convert_to_3d())?,
        // UV curves from CreateCurvesFromFace are slicing copies, never analytic
        // non-line kinds. Reject malformed values rather than silently changing them.
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "UV copy is not a B-spline or line",
            ));
        }
    }
    // cpp:3777–3780 ignores both statuses. An ordinary bounding-box failure leaves
    // the initialized extent, while a genuinely missing dependency must still defer.
    Ok(ignored(curve.calculate_bounding_box(
        kernel,
        &curve.natural_interval(),
        crate::nurbs::b_spline_curve::BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        },
        true,
    ))?
    .and_then(|boxes| boxes.normal)
    .unwrap_or_else(Extent3d::new))
}

/// `SmBrep::CreateFaceFromSurface(pSurface, sDomain, rpNewFace)` (unit
/// `topology.brep_p8`): the new face.
///
/// Notifications go to an arena-only store (`ArenaNotifyState`: default records, no
/// attributes, no context), the new face owning its surface under its arena identity;
/// the closing face's notifications have no observer here (upstream: the offset B-rep's
/// context, which the executive does not hold).
fn create_face_from_surface(
    kernel: &mut Kernel,
    b: &mut Brep,
    surface: &Surface,
    domain: &Extent2d,
) -> Result<FaceId> {
    let mut state = crate::topology::brep::p2::ArenaNotifyState::new(b);
    let mut env = state.env(None);
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let this_face = |f: FaceId| loopuse::notify_ref(EntityRef::Face(f));
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env: &mut env,
        marks: &mut marks,
        this_face: &this_face,
    };
    crate::topology::brep::p8::create_face_from_surface(
        kernel,
        b,
        Some(surface.clone()),
        domain,
        &mut fenv,
    )
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}

fn edge_of(brep: &Brep, edgeuse: EdgeuseId) -> Result<EdgeId> {
    required(brep.edgeuse(edgeuse)?.edge)
}

fn interval(brep: &Brep, edgeuse: EdgeuseId) -> Result<Extent1d> {
    Ok(brep.edge(edge_of(brep, edgeuse)?)?.interval)
}

fn start_vertex(brep: &Brep, edgeuse: EdgeuseId) -> Result<VertexId> {
    let vertexuse = required(brep.edgeuse(edgeuse)?.vertexuse)?;
    required(brep.vertexuse(vertexuse)?.vertex)
}

fn bspline_edge_curve(brep: &Brep, edge: EdgeId) -> Result<&Curve> {
    let curve = brep
        .edge(edge)?
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::sm_err(OP))?;
    if matches!(
        curve.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    ) {
        Ok(curve)
    } else {
        Err(SolidError::sm_err(OP))
    }
}

fn face_vertices(brep: &Brep, face: FaceId) -> Result<Vec<VertexId>> {
    let mut out = Vec::new();
    if let Some(faceuse) = brep.face(face)?.faceuse {
        for loopuse in eu::faceuse_loopuses(brep, faceuse, false, None)? {
            for vertex in loopuse::vertices(brep, loopuse, None)? {
                out.push(required(vertex)?);
            }
        }
    }
    Ok(out)
}

fn sort_edgeuses(brep: &Brep, edgeuses: &mut [EdgeuseId]) -> Result<()> {
    // Keep the upstream bubble sort: `>` does not exchange ties or NaNs.
    loop {
        let mut done = true;
        for i in 1..edgeuses.len() {
            let last = required(edgeuses.get(i - 1).copied())?;
            let current = required(edgeuses.get(i).copied())?;
            if interval(brep, last)?.min() > interval(brep, current)?.min() {
                edgeuses.swap(i - 1, i);
                done = false;
            }
        }
        if done {
            return Ok(());
        }
    }
}

fn insert_breaks(
    kernel: &mut Kernel,
    brep: &mut Brep,
    edgeuses: &[EdgeuseId],
    to_split: &mut Vec<EdgeuseId>,
    notifications: &mut NotifyState<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
) -> Result<()> {
    for max_end in [false, true] {
        for &edgeuse in edgeuses {
            let source = interval(brep, edgeuse)?;
            let param = if max_end { source.max() } else { source.min() };
            // Uses appended by a split are scanned too: upstream re-reads GetSize.
            let mut index = 0;
            while let Some(&split) = to_split.get(index) {
                index += 1;
                let target = interval(brep, split)?;
                if !target.contains_value(param, 0.0)
                    || target.is_value_on_boundary(param, EFF_ZERO).0
                {
                    continue;
                }
                // The split reads the offset B-rep's own editing state (the executive's
                // constructors enable it, cpp:437, 1073). Dependency errors propagate;
                // only SM_ERR continues.
                let edge_id = edge_of(brep, split)?;
                let output = crate::topology::brep::p9::make_vertex_split_edge_with_state(
                    kernel,
                    brep,
                    edge_id,
                    param,
                    notifications,
                    curve_ref,
                )?;
                if !output.split {
                    continue; // Can't split if too close to end point
                }
                let new_edges = required(output.edges)?;
                let ccw = required(eu::ccw_edgeuse(brep, split)?)?;
                let cw = required(eu::cw_edgeuse(brep, split)?)?;
                if new_edges.contains(&edge_of(brep, ccw)?) {
                    to_split.push(ccw);
                }
                if new_edges.contains(&edge_of(brep, cw)?) {
                    to_split.push(cw);
                }
                break;
            }
        }
    }
    Ok(())
}

fn remove_small_stuff(k: &Kernel, brep: &Brep, edgeuses: &mut Vec<EdgeuseId>) -> Result<()> {
    let mut lengths = Vec::new();
    let mut total_length = 0.0;
    for &edgeuse in edgeuses.iter() {
        let edge = brep.edge(edge_of(brep, edgeuse)?)?;
        let curve = required(edge.curve.as_ref())?;
        let length = curve.approximate_length(k, &edge.interval, 10, false)?.0;
        lengths.push(length);
        total_length += length;
    }
    let mut kept = Vec::new();
    for (&edgeuse, length) in edgeuses.iter().zip(lengths) {
        if length > total_length / 10.0 {
            kept.push(edgeuse);
        }
    }
    *edgeuses = kept;
    Ok(())
}

fn extract_gaps(
    brep: &Brep,
    edgeuses: &mut Vec<EdgeuseId>,
    radial: &[EdgeuseId],
    gaps: &mut Vec<EdgeuseId>,
) -> Result<()> {
    let mut found = Vec::new();
    for &edgeuse in edgeuses.iter() {
        let mid = interval(brep, edgeuse)?.evaluate(0.5);
        let mut matched = false;
        for &other in radial {
            if interval(brep, other)?.contains_value(mid, 0.0) {
                found.push(edgeuse);
                matched = true;
                break;
            }
        }
        if !matched {
            gaps.push(edgeuse);
        }
    }
    *edgeuses = found;
    Ok(())
}

/// The identities `RemoveExcessFaces`' point solve keys the original B-rep by: each
/// edge (and its curve) and face (and its surface) is identified by the key its
/// geometry is owned under (upstream compares pointers, so geometry belongs to the
/// record that holds it), and by [`arena_identity`] when it has no owner; every other
/// object by [`arena_identity`].
struct OriginalKeys {
    edges: BTreeMap<EdgeId, ObjectRef>,
    faces: BTreeMap<FaceId, ObjectRef>,
}

impl OriginalKeys {
    fn new(original: &Brep) -> Result<Self> {
        let mut edges = BTreeMap::new();
        for (id, edge) in original.edges().iter() {
            if let Some(owner) = edge.curve.as_ref().and_then(Curve::owner) {
                edges.insert(id, owner);
            }
        }
        let mut faces = BTreeMap::new();
        for (id, face) in original.faces().iter() {
            if let Some(owner) = face.surface.as_ref().and_then(Surface::owner) {
                faces.insert(id, owner);
            }
        }
        let keys = Self { edges, faces };
        // An owner shared by two records, or naming another record's identity, is an
        // indirect owner chain: reject it before cache construction can skip geometry.
        let mut seen = BTreeSet::new();
        let objects = original
            .edges()
            .ids()
            .map(CacheObject::Edge)
            .chain(original.faces().ids().map(CacheObject::Face))
            .chain(original.vertices().ids().map(CacheObject::Vertex))
            .chain([CacheObject::Brep]);
        for object in objects {
            let key = keys.key(object);
            if !seen.insert(key) {
                return Err(SolidError::Unsupported {
                    operation: OP,
                    reason: "point-solve geometry owners must identify one edge or face each",
                });
            }
        }
        Ok(keys)
    }

    fn key(&self, object: CacheObject) -> ObjectRef {
        let owner = match object {
            CacheObject::Edge(e) | CacheObject::Curve(e) => self.edges.get(&e),
            CacheObject::Face(f) | CacheObject::Surface(f) => self.faces.get(&f),
            _ => None,
        };
        owner.copied().unwrap_or_else(|| arena_identity(object))
    }
}

fn make_original_brep_cache(kernel: &Kernel, original: &Brep) -> Result<(BrepCache, OriginalKeys)> {
    let keys = OriginalKeys::new(original)?;
    let cache = make_brep_cache(kernel, CacheSource::Brep(original), &|object| {
        keys.key(object)
    })?;
    Ok((cache, keys))
}

/// `pOriginal->BrepPointSolve(..)`. The face surfaces reach their faces' caches
/// (`smsurf_GetSurfaceCache`, which may create UV trim curves on `original`): the
/// read-only solve requests each missing cache or trim, which is prepared on
/// `original` before the solve runs again from its start.
fn solve_original_brep_point(
    kernel: &Kernel,
    original: &mut Brep,
    cache: &BrepCache,
    keys: &OriginalKeys,
    point: Vec3,
) -> Result<SolutionArray<CacheObject>> {
    let mut preparation = crate::query::brep::RayPreparation::default();
    loop {
        let objects = BrepObjects {
            preparation: Some(&preparation),
            brep: original,
            // There is exactly one B-rep in this query adapter's local namespace.
            brep_id: crate::topology::BrepId::from_index(0),
            cache: Some(cache),
            keys: &|object| keys.key(object),
            // OriginalKeys identifies every edge and face by its geometry's own owner.
            owner_of: &|_| None,
        };
        let result = TopologySolver::brep_point_solve(
            kernel,
            &objects,
            Some(CacheObject::Brep),
            point,
            TopologySolveOptions {
                operation: SolverOperation::Minimize,
                requested: SolutionRequested::Single,
                tolerance_3d: original.tolerance(),
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            },
        );
        match (result, preparation.take_request()) {
            (Err(SolidError::PreparationRequired { .. }), Some(request)) => {
                preparation.prepare(kernel, original, request)?;
            }
            (result, _) => return result,
        }
    }
}

fn collect_excess_faces(
    result: &Brep,
    edge: EdgeId,
    solutions: &[Solution<CacheObject>],
    min_distance: f64,
    shell_faces: &[FaceId],
    delete_faces: &mut Vec<FaceId>,
) -> Result<()> {
    if let [solution] = solutions {
        if solution.start.solution_value < min_distance {
            let edge_faces = edge::faces(result, edge, None)?;
            if edge_faces.iter().any(|face| shell_faces.contains(face)) {
                return Ok(());
            }
            for face in edge_faces {
                if !delete_faces.contains(&face) {
                    delete_faces.push(face);
                }
            }
        }
    }
    Ok(())
}

fn notify_edge_and_vertices(
    original: &Brep,
    old_edge: EdgeId,
    result: &Brep,
    new_edge: EdgeId,
    env: &mut NotifyEnv<'_>,
    offset_key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    // For Copy, the Edge and Vertex overrides directly delegate to SmTopology;
    // neither traverses uses or invalidates geometry. SM_NO_GET_BREP is NULL in
    // the pinned reference. Distinct key callbacks disambiguate the two arenas.
    let mut notify = |old, new| {
        ignore_status(topology::notify(
            env.records,
            env.attributes,
            env.context.as_deref_mut(),
            NotifyOperation::Copy,
            (env.key_of)(old),
            Some(offset_key_of(new)),
            None,
            None,
        ))
    };
    notify(EntityRef::Edge(old_edge), EntityRef::Edge(new_edge))?;
    let old_start = required(edge::start_vertex(original, old_edge)?)?;
    let new_start = required(edge::start_vertex(result, new_edge)?)?;
    notify(EntityRef::Vertex(old_start), EntityRef::Vertex(new_start))?;
    let old_end = required(edge::other_vertex(original, old_edge, Some(old_start))?)?;
    if old_end != old_start {
        let new_end = required(edge::other_vertex(result, new_edge, Some(new_start))?)?;
        notify(EntityRef::Vertex(old_end), EntityRef::Vertex(new_end))?;
    }
    Ok(())
}

/// `sm_ForgetSeconds`: forgets the pairs of `relation` whose second is in `gone` or,
/// with `live`, is not in `live`.
fn forget_seconds<A: Ord + Copy, B: Ord + Copy>(
    relation: &mut Relation<A, B>,
    gone: &BTreeSet<B>,
    live: Option<&BTreeSet<B>>,
) -> Result<()> {
    for second in relation.all_seconds() {
        if !gone.contains(&second) && live.is_none_or(|live| live.contains(&second)) {
            continue;
        }
        for first in relation.firsts(second) {
            relation.remove_pair(first, second)?;
        }
    }
    Ok(())
}

/// `sm_ForgetFirsts`: forgets the pairs of `relation` whose first is in `gone`.
fn forget_firsts<A: Ord + Copy, B: Ord + Copy>(
    relation: &mut Relation<A, B>,
    gone: &BTreeSet<A>,
) -> Result<()> {
    for first in relation.all_firsts() {
        if !gone.contains(&first) {
            continue;
        }
        for second in relation.seconds(first) {
            relation.remove_pair(first, second)?;
        }
    }
    Ok(())
}

/// The edge uses `RemoveFaces` deletes with `faces` (`RemoveExcessFaces`' `sGone`,
/// collected before the removal): for each face, the uses of its edges on that face.
fn removed_faces_edgeuses(result: &Brep, faces: &[FaceId]) -> Result<BTreeSet<EdgeuseId>> {
    let mut gone = BTreeSet::new();
    for &f in faces {
        for e in face_edges(result, f)? {
            for u in edge::edgeuses(result, e, None, None)? {
                if eu::face(result, u)? == Some(f) {
                    gone.insert(u);
                }
            }
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::geom::vector2d::Vec2;
    use crate::nurbs::b_spline_curve::{BSplineCurve, NurbCircleParam};
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::plane::Plane;
    use crate::topology::{Edge, Edgeuse, Face, Faceuse, ShellId, Vertex, Vertexuse};

    /// Runs `run` with empty notification stores keyed by arena identity.
    fn with_state<R>(run: impl FnOnce(&mut NotifyState<'_>) -> R) -> R {
        let mut records = topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let key = |entity| loopuse::notify_ref(entity);
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key,
        };
        run(&mut NotifyState::from_env(&mut env))
    }

    fn executive() -> OffsetExecutive {
        OffsetExecutive::new(OffsetOperation::SolidOffset, 0.125, 1.0e-6)
    }

    fn add_use(brep: &mut Brep, min: f64, max: f64, length: f64) -> EdgeuseId {
        let mut curve = Line::from_points(Vec3::ZERO, Vec3::new(length, 0.0, 0.0), 3);
        let interval = Extent1d::from_raw(min, max);
        if length != 0.0 {
            curve.edit_parameterization(&interval).unwrap();
        }
        let edge = brep
            .add_edge_record(Edge {
                vertices: [VertexId::from_index(0); 2],
                edgeuses: Vec::new(),
                interval,
                tolerance: 0.0,
                curve: Some(Curve::from(curve)),
            })
            .unwrap();
        let edgeuse = brep
            .add_edgeuse_record(Edgeuse {
                edge: Some(edge),
                ..Edgeuse::default()
            })
            .unwrap();
        brep.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
        edgeuse
    }

    /// An edge use on a unit-length B-spline line at height `y`.
    fn add_bspline_use(brep: &mut Brep, y: f64) -> EdgeuseId {
        let line = Line::from_points(Vec3::new(0.0, y, 0.0), Vec3::new(1.0, y, 0.0), 3);
        let curve = Curve::from(line.base_with_nurb().unwrap().into_owned());
        let edge = brep
            .add_edge_record(Edge {
                vertices: [VertexId::from_index(0); 2],
                edgeuses: Vec::new(),
                interval: Extent1d::from_raw(0.0, 1.0),
                tolerance: 0.0,
                curve: Some(curve),
            })
            .unwrap();
        let edgeuse = brep
            .add_edgeuse_record(Edgeuse {
                edge: Some(edge),
                ..Edgeuse::default()
            })
            .unwrap();
        brep.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
        edgeuse
    }

    fn plane(z: f64) -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, z),
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    /// A wire edge with real vertices and a primary/mate pair, as in the p9 split tests.
    fn wire_use(brep: &mut Brep, min: f64, max: f64) -> EdgeuseId {
        let mut curve = Line::from_points(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 3);
        let interval = Extent1d::from_raw(min, max);
        curve.edit_parameterization(&interval).unwrap();
        let v0 = brep.add_vertex(Vec3::ZERO).unwrap();
        let v1 = brep.add_vertex(Vec3::new(1.0, 0.0, 0.0)).unwrap();
        let edge = brep.add_edge(v0, v1).unwrap();
        let record = brep.edge_mut(edge).unwrap();
        record.interval = interval;
        record.curve = Some(Curve::from(curve));
        let mut primary = None;
        for (vertex, orientation) in [(v0, OrientType::Same), (v1, OrientType::Opposite)] {
            let vertexuse = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(vertex),
                    ..Vertexuse::default()
                })
                .unwrap();
            let edgeuse = brep
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    owner: crate::topology::EdgeuseOwner::Shell(None),
                    vertexuse: Some(vertexuse),
                    orientation,
                    ..Edgeuse::default()
                })
                .unwrap();
            brep.vertexuse_mut(vertexuse).unwrap().owner =
                crate::topology::VertexuseOwner::Edgeuse(Some(edgeuse));
            brep.vertex_mut(vertex).unwrap().vertexuses.push(vertexuse);
            brep.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
            primary.get_or_insert(edgeuse);
        }
        primary.unwrap()
    }

    fn split_entities(brep: &Brep) -> Vec<EntityRef> {
        brep.edges()
            .iter()
            .map(|(id, _)| EntityRef::Edge(id))
            .chain(brep.edgeuses().iter().map(|(id, _)| EntityRef::Edgeuse(id)))
            .chain(brep.vertices().iter().map(|(id, _)| EntityRef::Vertex(id)))
            .chain(
                brep.vertexuses()
                    .iter()
                    .map(|(id, _)| EntityRef::Vertexuse(id)),
            )
            .collect()
    }

    fn split_env<R>(
        brep_key: ObjectKey,
        entities: &[EntityRef],
        run: impl FnOnce(&mut NotifyEnv<'_>, &dyn Fn(EdgeId, i64) -> ObjectRef) -> R,
    ) -> R {
        let key = |entity| {
            let (index, type_code) = match entity {
                EntityRef::Edge(id) => (id.index(), edge::TYPE),
                EntityRef::Edgeuse(id) => (id.index(), eu::TYPE),
                EntityRef::Vertex(id) => (id.index(), vertex::TYPE),
                EntityRef::Vertexuse(id) => (id.index(), crate::topology::vertexuse::TYPE),
                _ => panic!("unexpected split fixture entity"),
            };
            ObjectRef {
                key: ObjectKey(
                    1_000_000 + u64::try_from(type_code).unwrap() * 1000 + u64::from(index),
                ),
                type_code,
            }
        };
        let mut records =
            topology::NotifyRecords::from(BTreeMap::from([(brep_key, topology::Topology::new())]));
        for &entity in entities {
            records.insert(key(entity).key, topology::Topology::new());
        }
        let mut attributes = AttributeStore::default();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key,
        };
        run(&mut env, &|edge, kind| ObjectRef {
            key: ObjectKey(1000 + u64::from(edge.index())),
            type_code: kind,
        })
    }

    fn sync(
        executive: &mut OffsetExecutive,
        original: Option<EdgeuseId>,
        other: Option<EdgeuseId>,
    ) -> Result<SynchronizedEdgeuses> {
        let mut kernel = Kernel::default();
        let entities = executive
            .offset_brep()
            .map(split_entities)
            .unwrap_or_default();
        let brep_key = executive.offset_brep().map_or(ObjectKey(0), Brep::key);
        split_env(brep_key, &entities, |env, curve_ref| {
            executive.synchronize_offset_edgeuses(&mut kernel, original, other, env, curve_ref)
        })
    }

    #[test]
    fn settings_defaults_and_scalar_view() {
        for (code, operation) in [
            OffsetOperation::SolidOffset,
            OffsetOperation::NonManifoldOffset,
            OffsetOperation::Shell,
            OffsetOperation::LocalOperation,
            OffsetOperation::MachineOffset,
        ]
        .into_iter()
        .enumerate()
        {
            let executive = OffsetExecutive::new(operation, -3.25, 1.0e-12);
            assert_eq!(operation as usize, code);
            assert_eq!(executive.operation(), operation);
            assert!(!executive.inset());
            assert!(!executive.extend_convex_edges());
            assert!(!executive.merge_results());
            assert!(!executive.create_offset_solid());
            assert!(executive.shell_faces().is_empty());
            assert!(executive.offset_brep().is_none());
            let radians = 2.0 * PI / 180.0;
            assert_eq!(
                executive.tangency_tol_degrees().to_bits(),
                (radians * 180.0 / PI).to_bits()
            );
            assert_eq!(executive.offset_distance(), -3.25);
            assert_eq!(executive.approximation_tol_3d(), 1.0e-12);
        }
    }

    #[test]
    fn geometry_parameter_snapshot_preserves_custom_tangency_conversion_order() {
        for radians in [
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            1.0e-300,
            1.0e-12,
            PI,
            PI / 7.0,
            0.31,
        ] {
            let executive = OffsetExecutive::with_geometry_parameters(
                OffsetOperation::LocalOperation,
                -4.25,
                3.0e-8,
                radians,
            );
            assert_eq!(executive.operation(), OffsetOperation::LocalOperation);
            assert_eq!(executive.offset_distance(), -4.25);
            assert_eq!(executive.approximation_tol_3d(), 3.0e-8);
            assert_eq!(
                executive.tangency_tol_degrees().to_bits(),
                (radians * 180.0 / PI).to_bits()
            );
            assert!(!executive.inset());
            assert!(!executive.extend_convex_edges());
            assert!(!executive.merge_results());
            assert!(!executive.create_offset_solid());
        }
        // This value distinguishes upstream's order from precomputing 180 / PI.
        let radians = 0.31;
        assert_ne!(
            (radians * 180.0 / PI).to_bits(),
            (radians * (180.0 / PI)).to_bits()
        );
    }

    #[test]
    fn setters_are_independent_and_copy_shell_faces_without_deduplication() {
        let mut executive = executive();
        executive.set_inset(true);
        executive.set_extend_convex_edges(true);
        executive.set_merge_results(true);
        executive.set_create_offset_solid(true);
        executive.set_operation(OffsetOperation::MachineOffset);
        let f = FaceId::from_index(9);
        let mut faces = vec![f, FaceId::from_index(2), f];
        executive.set_shell_faces(&faces);
        faces.clear();
        assert_eq!(executive.shell_faces(), &[f, FaceId::from_index(2), f]);
        assert!(executive.inset() && executive.extend_convex_edges());
        assert!(executive.merge_results() && executive.create_offset_solid());
        assert_eq!(executive.operation(), OffsetOperation::MachineOffset);
        executive.set_shell_faces(&[]);
        assert!(executive.shell_faces().is_empty());
        executive.set_offset_brep(Brep::new());
        assert!(executive.offset_brep().unwrap().faces().is_empty());
        executive.set_inset(false);
        assert!(!executive.inset());
        assert!(executive.extend_convex_edges());
    }

    #[test]
    fn relations_deduplicate_but_do_not_choose_among_multiple_results() {
        let mut executive = executive();
        let a = EdgeuseId::from_index(2);
        let b = EdgeuseId::from_index(7);
        let c = EdgeuseId::from_index(8);
        assert_eq!(executive.offset_edgeuse(a), None);
        executive.relate_edgeuses(a, b);
        executive.relate_edgeuses(a, b);
        assert_eq!(executive.offset_edgeuse(a), Some(b));
        executive.relate_edgeuses(a, c);
        assert_eq!(executive.offset_edgeuse(a), None);
        executive.relate_edgeuses(c, b);
        assert_eq!(executive.offset_edgeuse(c), Some(b));
    }

    #[test]
    fn synchronization_null_original_and_empty_maps_do_not_need_a_brep() {
        let mut executive = executive();
        assert!(matches!(
            sync(&mut executive, None, None),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        let a = EdgeuseId::from_index(100);
        assert_eq!(
            sync(&mut executive, Some(a), None).unwrap(),
            SynchronizedEdgeuses::default()
        );
    }

    #[test]
    fn unmapped_first_side_sorts_but_does_not_filter_the_other_side() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let high = add_use(&mut brep, 5.0, 6.0, 100.0);
        let low = add_use(&mut brep, 0.0, 1.0, 0.01);
        // Early return must not even ask for curves on the other side.
        let high_edge = edge_of(&brep, high).unwrap();
        let low_edge = edge_of(&brep, low).unwrap();
        brep.edge_mut(high_edge).unwrap().curve = None;
        brep.edge_mut(low_edge).unwrap().curve = None;
        let a = EdgeuseId::from_index(50);
        let b = EdgeuseId::from_index(51);
        executive.relate_edgeuses(b, high);
        executive.relate_edgeuses(b, low);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert_eq!(out.edgeuses, vec![None, None]);
        assert_eq!(out.other_edgeuses, vec![Some(low), Some(high)]);
        assert!(out.gaps.is_empty() && out.other_gaps.is_empty());
    }

    #[test]
    fn strict_ten_percent_removal_then_null_padding() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let small = add_use(&mut brep, 0.0, 1.0, 1.0);
        let large = add_use(&mut brep, 1.0, 2.0, 9.0);
        let original = EdgeuseId::from_index(90);
        executive.relate_edgeuses(original, small);
        executive.relate_edgeuses(original, large);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(original), None).unwrap();
        assert_eq!(out.edgeuses, vec![Some(large)]);
        assert_eq!(out.other_edgeuses, vec![None]);
        // Filtering is local; correspondence cardinality is not rewritten.
        assert_eq!(executive.offset_edgeuse(original), None);
    }

    #[test]
    fn zero_length_and_ten_equal_pieces_are_all_removed() {
        let k = &Kernel::default();
        let mut brep = Brep::new();
        let zero = add_use(&mut brep, 0.0, 0.0, 0.0);
        let mut uses = vec![zero];
        remove_small_stuff(k, &brep, &mut uses).unwrap();
        assert!(uses.is_empty());
        for i in 0..10 {
            uses.push(add_use(&mut brep, f64::from(i), f64::from(i + 1), 1.0));
        }
        remove_small_stuff(k, &brep, &mut uses).unwrap();
        assert!(uses.is_empty());
    }

    #[test]
    fn synchronization_second_side_is_filtered_before_its_empty_branch() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let keep = add_use(&mut brep, 0.0, 2.0, 2.0);
        let zero = add_use(&mut brep, 0.0, 0.0, 0.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        executive.relate_edgeuses(a, keep);
        executive.relate_edgeuses(b, zero);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert_eq!(out.edgeuses, vec![Some(keep)]);
        assert_eq!(out.other_edgeuses, vec![None]);
    }

    #[test]
    fn synchronization_filtered_first_side_still_leaves_second_unfiltered() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let zero = add_use(&mut brep, 0.0, 0.0, 0.0);
        let small = add_use(&mut brep, 0.0, 1.0, 1.0);
        let large = add_use(&mut brep, 1.0, 2.0, 9.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        executive.relate_edgeuses(a, zero);
        executive.relate_edgeuses(b, small);
        executive.relate_edgeuses(b, large);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert_eq!(out.edgeuses, vec![None, None]);
        assert_eq!(out.other_edgeuses, vec![Some(small), Some(large)]);
    }

    #[test]
    fn synchronization_matches_intervals_without_requiring_splits() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let high_a = add_use(&mut brep, 1.0, 2.0, 1.0);
        let low_a = add_use(&mut brep, 0.0, 1.0, 1.0);
        let low_b = add_use(&mut brep, 0.0, 1.0, 1.0);
        let high_b = add_use(&mut brep, 1.0, 2.0, 1.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        for offset in [high_a, low_a] {
            executive.relate_edgeuses(a, offset);
        }
        for offset in [low_b, high_b] {
            executive.relate_edgeuses(b, offset);
        }
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert_eq!(out.edgeuses, vec![Some(low_a), Some(high_a)]);
        assert_eq!(out.other_edgeuses, vec![Some(low_b), Some(high_b)]);
        assert!(out.gaps.is_empty() && out.other_gaps.is_empty());
    }

    #[test]
    fn synchronization_extracts_gaps_in_sorted_order() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let high = add_use(&mut brep, 4.0, 5.0, 1.0);
        let low = add_use(&mut brep, 0.0, 1.0, 1.0);
        let other = add_use(&mut brep, 2.0, 3.0, 1.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        executive.relate_edgeuses(a, high);
        executive.relate_edgeuses(a, low);
        executive.relate_edgeuses(b, other);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert!(out.edgeuses.is_empty() && out.other_edgeuses.is_empty());
        assert_eq!(out.gaps, vec![low, high]);
        assert_eq!(out.other_gaps, vec![other]);
    }

    #[test]
    fn bubble_sort_preserves_ties_and_unordered_comparisons() {
        let mut brep = Brep::new();
        let a = add_use(&mut brep, 1.0, 2.0, 1.0);
        let b = add_use(&mut brep, 1.0, 3.0, 1.0);
        let c = add_use(&mut brep, 0.0, 1.0, 1.0);
        let mut uses = vec![b, a, c];
        sort_edgeuses(&brep, &mut uses).unwrap();
        assert_eq!(uses, vec![c, b, a]);
        let a_edge = edge_of(&brep, a).unwrap();
        brep.edge_mut(a_edge).unwrap().interval = Extent1d::from_raw(f64::NAN, 1.0);
        let mut uses = vec![b, a, c];
        sort_edgeuses(&brep, &mut uses).unwrap();
        assert_eq!(uses, vec![b, a, c]);
    }

    #[test]
    fn break_insertion_skips_boundaries_and_continues_past_refused_splits() {
        let mut kernel = Kernel::default();
        let mut brep = Brep::new();
        let whole = wire_use(&mut brep, 0.0, 1.0);
        let equal = add_use(&mut brep, 0.0, 1.0, 1.0);
        let near = add_use(&mut brep, EFF_ZERO / 2.0, 1.0, 1.0);
        let max_break = add_use(&mut brep, 0.0, 0.5, 1.0);
        let edges = brep.edges().len();
        let insert = |brep: &mut Brep, kernel: &mut Kernel, uses: &[EdgeuseId]| {
            let mut to_split = vec![whole];
            let entities = split_entities(brep);
            let result = split_env(brep.key(), &entities, |env, curve_ref| {
                insert_breaks(
                    kernel,
                    brep,
                    uses,
                    &mut to_split,
                    &mut NotifyState::from_env(env),
                    curve_ref,
                )
            });
            (result, to_split)
        };
        // Editing disabled (the B-rep constructor's default): every split is
        // refused with SM_ERR, so the scan continues and nothing is split.
        let (result, to_split) = insert(&mut brep, &mut kernel, &[equal, near, max_break]);
        result.unwrap();
        assert_eq!(to_split, vec![whole]);
        assert_eq!(brep.edges().len(), edges);
        // Interior by the boundary test but within MakeVertexSplitEdge's scaled zero:
        // refused even with editing enabled.
        brep.set_editing_enabled(true);
        let exact = add_use(&mut brep, EFF_ZERO, 1.0, 1.0);
        let edges = brep.edges().len();
        let (result, to_split) = insert(&mut brep, &mut kernel, &[equal, near, exact]);
        result.unwrap();
        assert_eq!(to_split, vec![whole]);
        assert_eq!(brep.edges().len(), edges);
        assert!(!kernel.context.doing_boolean());
        // A strict interior break splits; the scan then reads the split use's CCW edge
        // use, which this hand-built wire use leaves unlinked (upstream would dereference
        // NULL), so it stops there with SM_ERR_NULL_POINTER.
        let (result, _) = insert(&mut brep, &mut kernel, &[max_break]);
        assert!(matches!(
            result,
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        assert_eq!(brep.edges().len(), edges + 1);
    }

    #[test]
    fn synchronization_without_editing_refuses_splits() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let part = add_use(&mut brep, 0.25, 1.0, 1.0);
        let whole = wire_use(&mut brep, 0.0, 1.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        executive.relate_edgeuses(a, part);
        executive.relate_edgeuses(b, whole);
        executive.set_offset_brep(brep);
        let out = sync(&mut executive, Some(a), Some(b)).unwrap();
        assert_eq!(out.edgeuses, vec![Some(part)]);
        assert_eq!(out.other_edgeuses, vec![Some(whole)]);
        assert_eq!(executive.offset_brep().unwrap().edges().len(), 2);
    }

    #[test]
    fn synchronization_reports_split_dependency_without_changing_relations() {
        let mut executive = executive();
        let mut brep = Brep::new();
        let part = add_use(&mut brep, 0.25, 1.0, 1.0);
        let whole = wire_use(&mut brep, 0.0, 1.0);
        let a = EdgeuseId::from_index(80);
        let b = EdgeuseId::from_index(81);
        executive.relate_edgeuses(a, part);
        executive.relate_edgeuses(b, whole);
        brep.set_editing_enabled(true);
        executive.set_offset_brep(brep);
        // The split completes; the unlinked CCW edge use of the hand-built wire use then
        // stops the scan (see break_insertion_skips_boundaries_and_continues_past_refused_splits).
        assert!(matches!(
            sync(&mut executive, Some(a), Some(b)),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
        assert_eq!(executive.offset_brep().unwrap().edges().len(), 3);
        assert_eq!(executive.offset_edgeuse(a), Some(part));
        assert_eq!(executive.offset_edgeuse(b), Some(whole));
    }

    #[test]
    fn extraction_uses_midpoint_inclusive_containment_and_preserves_existing_gaps() {
        let mut brep = Brep::new();
        let a = add_use(&mut brep, 0.0, 2.0, 1.0);
        let b = add_use(&mut brep, 5.0, 6.0, 1.0);
        let radial = add_use(&mut brep, 1.0, 3.0, 1.0);
        let mut uses = vec![b, a];
        let mut gaps = vec![radial];
        extract_gaps(&brep, &mut uses, &[radial], &mut gaps).unwrap();
        assert_eq!(uses, vec![a]);
        assert_eq!(gaps, vec![radial, b]);
    }

    #[test]
    fn invalid_mapped_ids_return_errors_instead_of_panicking() {
        let mut executive = executive();
        executive.relate_edgeuses(EdgeuseId::from_index(80), EdgeuseId::from_index(u32::MAX));
        executive.set_offset_brep(Brep::new());
        assert!(matches!(
            sync(&mut executive, Some(EdgeuseId::from_index(80)), None),
            Err(SolidError::NotFound { .. })
        ));
    }

    #[test]
    fn extension_cache_hits_by_caller_identity_and_keeps_keys_distinct() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let base = plane(0.0);
        let expected = base
            .create_extended_surface(&kernel, 0.125 * 5.0, ContinuityType::G1R)
            .unwrap();
        let first = executive
            .get_create_extended_surface(&kernel, 41, &base)
            .unwrap();
        assert_eq!(
            first.natural_uv_domain_if_built(),
            expected.natural_uv_domain_if_built()
        );
        let first_point = first
            .evaluate_point(
                &kernel,
                first.natural_uv_domain_if_built().evaluate(0.5, 0.5),
            )
            .unwrap();
        // A cached identity does not evaluate even a different/malformed supplied base.
        let broken = Surface::from(BSplineSurface::new());
        let cached = executive
            .get_create_extended_surface(&kernel, 41, &broken)
            .unwrap();
        assert_eq!(
            cached
                .evaluate_point(
                    &kernel,
                    cached.natural_uv_domain_if_built().evaluate(0.5, 0.5)
                )
                .unwrap(),
            first_point
        );
        let other = executive
            .get_create_extended_surface(&kernel, 42, &plane(7.0))
            .unwrap();
        assert_eq!(
            other
                .evaluate_point(
                    &kernel,
                    other.natural_uv_domain_if_built().evaluate(0.5, 0.5)
                )
                .unwrap()
                .z,
            7.0
        );
        assert_eq!(executive.extended_surfaces.len(), 2);
    }

    #[test]
    fn failed_extensions_are_not_cached_and_can_be_retried() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let broken = Surface::from(BSplineSurface::new());
        assert!(executive
            .get_create_extended_surface(&kernel, 1, &broken)
            .is_err());
        assert!(executive.extended_surfaces.is_empty());
        assert!(executive
            .get_create_extended_surface(&kernel, 1, &plane(0.0))
            .is_ok());
    }

    #[test]
    fn sidewalls_handle_null_missing_ambiguous_and_single_mappings() {
        let mut kernel = Kernel::default();
        let mut executive = executive();
        // B-spline edge curves (a Line whose parameterisation was edited carries no NURB)
        let mut original_brep = Brep::new();
        let original = add_bspline_use(&mut original_brep, 0.0);
        let mut offset_brep = Brep::new();
        let offset = add_bspline_use(&mut offset_brep, 1.0);
        executive.set_offset_brep(offset_brep);
        executive
            .build_side_wall_of_edgeuse(&mut kernel, &mut original_brep, None)
            .unwrap();
        executive
            .build_side_wall_of_edgeuse(&mut kernel, &mut original_brep, Some(original))
            .unwrap();
        // the ruled blend across the two B-reps runs; the closing face is made by
        // topology.brep_p8 (CreateFaceFromSurface) on the offset B-rep, a bare
        // `Brep::new()` here (no infinite region), and fails there
        executive.relate_edgeuses(original, offset);
        let r =
            executive.build_side_wall_of_edgeuse(&mut kernel, &mut original_brep, Some(original));
        assert!(r.is_err());
        assert!(!matches!(
            r,
            Err(SolidError::Unported {
                unit: "topology.brep_p8",
                ..
            })
        ));
        executive.relate_edgeuses(original, EdgeuseId::from_index(2));
        executive
            .build_side_wall_of_edgeuse(&mut kernel, &mut original_brep, Some(original))
            .unwrap();
    }

    #[test]
    fn excess_face_early_returns_and_actual_dependencies() {
        let mut kernel = Kernel::default();
        let mut executive = executive();
        let mut original = Brep::new();
        let mut result = Brep::new();
        executive
            .remove_excess_faces(&mut kernel, &mut original, &mut result)
            .unwrap();
        executive.set_merge_results(true);
        executive.set_shell_faces(&[FaceId::from_index(1)]);
        executive
            .remove_excess_faces(&mut kernel, &mut original, &mut result)
            .unwrap();
        executive.set_shell_faces(&[]);
        // No result edge: the original's editing flag is not touched; the empty
        // deletion list still reaches RemoveFaces, whose editing-disabled refusal is
        // ignored (cpp:2099-2102).
        executive
            .remove_excess_faces(&mut kernel, &mut original, &mut result)
            .unwrap();
        assert!(!original.editing_enabled());
        add_use(&mut result, 0.0, 1.0, 1.0);
        add_use(&mut result, 0.0, 1.0, 2.0);
        // The edges evaluate and solve against the empty original; no result face is
        // close to it, so nothing is removed.
        executive
            .remove_excess_faces(&mut kernel, &mut original, &mut result)
            .unwrap();
        assert_eq!(result.edges().len(), 2);
        assert_eq!(result.edgeuses().len(), 2);
        assert!(result.faces().is_empty());
        // Each edge's solve leaves the original's editing enabled (cpp:2069–2072).
        assert!(original.editing_enabled());
        // ShrinkGeometry trims the two edge curves; without offset surfaces the
        // continuation's loops are empty (SM_SUCCESS)
        let (mut records, mut attributes) =
            (topology::NotifyRecords::new(), AttributeStore::default());
        OffsetExecutive::replace_implicit_offsets(
            &mut Kernel::default(),
            &mut result,
            1.0e-6,
            &mut NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &loopuse::notify_ref,
            },
        )
        .unwrap();
        assert_eq!(result.edges().len(), 2);
    }

    #[test]
    fn removed_offset_faces_leave_every_correspondence_in_both_directions() {
        let mut exec = executive();
        let gone = FaceId::from_index(0);
        let kept = FaceId::from_index(1);
        let blend = FaceId::from_index(2);
        let faceuses = [FaceuseId::from_index(0), FaceuseId::from_index(1)];
        let edges = [EdgeId::from_index(0), EdgeId::from_index(1)];
        let vertices = [VertexId::from_index(0), VertexId::from_index(1)];
        for face in [gone, kept] {
            for source in faceuses {
                exec.relate_faceuse(source, face);
            }
            for source in edges {
                exec.relate_edge_face(source, face);
            }
            for source in vertices {
                exec.relate_vertex_face(source, face);
            }
            exec.relate_blend_face(blend, face);
        }
        exec.relate_blend_face(gone, kept);
        exec.relate_blend_face(gone, gone);
        exec.relate_blend_face(kept, gone);

        // Duplicate candidates and repeated cleanup are harmless. Original ids can
        // have the same numeric indices as result ids, but must remain related to kept.
        exec.forget_faces(&BTreeSet::from([gone])).unwrap();
        exec.forget_faces(&BTreeSet::from([gone])).unwrap();
        for source in faceuses {
            assert_eq!(exec.offset_faces(source), vec![kept]);
        }
        for source in edges {
            assert_eq!(exec.edge_to_faces.seconds(source), vec![kept]);
        }
        for source in vertices {
            assert_eq!(exec.vertex_to_faces.seconds(source), vec![kept]);
        }
        assert_eq!(exec.blend_to_faces.seconds(blend), vec![kept]);
        assert!(exec.blend_to_faces.seconds(gone).is_empty());
        assert!(exec.blend_to_faces.seconds(kept).is_empty());
        assert!(exec.faceuse_to_faces.firsts(gone).is_empty());
        assert!(exec.edge_to_faces.firsts(gone).is_empty());
        assert!(exec.vertex_to_faces.firsts(gone).is_empty());
        assert!(exec.blend_to_faces.firsts(gone).is_empty());
        assert_eq!(exec.blend_to_faces.firsts(kept), vec![blend]);

        exec.forget_faces(&BTreeSet::from([kept])).unwrap();
        assert!(exec.faceuse_to_faces.all_firsts().is_empty());
        assert!(exec.edge_to_faces.all_firsts().is_empty());
        assert!(exec.vertex_to_faces.all_firsts().is_empty());
        assert!(exec.blend_to_faces.all_firsts().is_empty());
        assert!(exec.blend_to_faces.all_seconds().is_empty());
    }

    /// `RemoveExcessFaces`: `m_vEUToEU` forgets the pairs whose result edge use belongs
    /// to a removed face or to no result edge, and keeps the rest.
    #[test]
    fn forget_seconds_drops_removed_and_dead_edgeuses() {
        let mut k = Kernel::default();
        let result = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::new(0.0, 0.0, 0.0),
            1.0,
            2.0,
            3.0,
        )
        .unwrap();
        let faces = bp1::faces(&result, None).unwrap();
        let removed = faces[0];
        let mut on_removed = None;
        let mut elsewhere = None;
        let mut live = BTreeSet::new();
        for e in bp1::edges(&result, None).unwrap() {
            for u in edge::edgeuses(&result, e, None, None).unwrap() {
                live.insert(u);
                if eu::face(&result, u).unwrap() == Some(removed) {
                    on_removed.get_or_insert(u);
                } else {
                    elsewhere.get_or_insert(u);
                }
            }
        }
        let (on_removed, elsewhere) = (on_removed.unwrap(), elsewhere.unwrap());
        let dead = EdgeuseId::from_index(100_000);
        let originals = [1, 2, 3].map(EdgeuseId::from_index);
        let mut x = executive();
        for (o, r) in originals.into_iter().zip([on_removed, elsewhere, dead]) {
            x.edgeuse_to_edgeuses.relate_pair(o, r).unwrap();
        }
        let gone = removed_faces_edgeuses(&result, &[removed]).unwrap();
        assert!(gone.contains(&on_removed) && !gone.contains(&elsewhere));
        forget_seconds(&mut x.edgeuse_to_edgeuses, &gone, Some(&live)).unwrap();
        assert_eq!(x.edgeuse_to_edgeuses.all_seconds(), vec![elsewhere]);
        assert_eq!(x.edgeuse_to_edgeuses.seconds(originals[1]), vec![elsewhere]);
    }

    #[test]
    fn implicit_offsets_are_replaced_by_approximations_of_their_base() {
        use crate::surface::offset_surface::OffsetSurface;
        let patch = || {
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
            )
            .unwrap()
        };
        let kernel = Kernel::default();
        let offset = OffsetSurface::new(&kernel, 0.25, patch().into(), true, 0).unwrap();
        let mut brep = Brep::new();
        let (plain, _) = add_face(&mut brep, plane(0.0));
        let (offset_face, _) = add_face(&mut brep, Surface::from(offset));
        let (mut records, mut attributes) =
            (topology::NotifyRecords::new(), AttributeStore::default());
        let mut kernel = kernel;
        OffsetExecutive::replace_implicit_offsets(
            &mut kernel,
            &mut brep,
            1.0e-6,
            &mut NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &loopuse::notify_ref,
            },
        )
        .unwrap();
        // the plane is untouched; the offset face gets the approximation of its base
        // offset (the bilinear patch at z = 0.25) and that surface's natural domain
        assert!(matches!(
            brep.face(plain).unwrap().surface.as_ref().unwrap().kind(),
            SurfaceKind::Plane(_)
        ));
        let face = brep.face(offset_face).unwrap();
        let surface = face.surface.as_ref().unwrap();
        assert!(matches!(surface.kind(), SurfaceKind::BSpline(_)));
        assert_eq!(face.uv_domain, surface.natural_uv_domain_if_built());
        let p = surface
            .evaluate_point(&kernel, Vec2::new(0.5, 0.5))
            .unwrap();
        assert!((p.z - 0.25).abs() < 1.0e-6, "{p:?}");

        // a plane is an SmBSplineSurface too (issue #1476): its offset is approximated
        let offset = OffsetSurface::new(&kernel, 0.25, plane(0.0), true, 0).unwrap();
        let mut brep = Brep::new();
        let (offset_face, _) = add_face(&mut brep, Surface::from(offset));
        OffsetExecutive::replace_implicit_offsets(
            &mut kernel,
            &mut brep,
            1.0e-6,
            &mut NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &loopuse::notify_ref,
            },
        )
        .unwrap();
        let surface = brep.face(offset_face).unwrap().surface.clone().unwrap();
        assert!(matches!(surface.kind(), SurfaceKind::BSpline(_)));
        let mid = surface.natural_uv_domain_if_built().mid();
        let p = surface.evaluate_point(&kernel, mid).unwrap();
        assert!((p.z - 0.25).abs() < 1.0e-6, "{p:?}");

        // a base that is not a B-spline surface (here an offset surface) is SM_ERR
        let inner = OffsetSurface::new(&kernel, 0.25, plane(0.0), true, 0).unwrap();
        let offset = OffsetSurface::new(&kernel, 0.25, Surface::from(inner), true, 0).unwrap();
        let mut brep = Brep::new();
        add_face(&mut brep, Surface::from(offset));
        let status = OffsetExecutive::replace_implicit_offsets(
            &mut kernel,
            &mut brep,
            1.0e-6,
            &mut NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &loopuse::notify_ref,
            },
        );
        assert!(
            matches!(status, Err(SolidError::Upstream { code: 1001, .. })),
            "{status:?}"
        );
    }

    #[test]
    fn kernel_context_removal_uses_actual_editing_flag_and_releases_nested_mark() {
        let mut kernel = Kernel::default();
        let mut original = Brep::new();
        let mut result = Brep::new_with_infinite_region().unwrap();
        let mut exec = executive();
        exec.set_merge_results(true);
        // Native RemoveExcessFaces ignores the editing-disabled refusal from
        // RemoveFaces. No traversal mark is acquired by that refused inner call.
        exec.remove_excess_faces(&mut kernel, &mut original, &mut result)
            .unwrap();
        assert!(!result.editing_enabled());
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
        result.set_editing_enabled(true);
        crate::geom::context::with_mark_lock(
            &mut kernel,
            crate::geom::context::ALL_MARKS,
            |kernel, mark| {
                assert_eq!(mark, MarkType::Mark);
                exec.remove_excess_faces(kernel, &mut original, &mut result)
                    .unwrap();
                assert!(kernel.context.is_mark_locked(mark));
                assert!(!kernel.context.is_mark_locked(MarkType::Mark2));
                assert_eq!(kernel.context.current_mark(MarkType::Mark2), 3);
            },
        )
        .unwrap();
        assert!(!kernel.context.is_mark_locked(MarkType::Mark));
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
    }

    fn excess_face_fixture() -> (Brep, EdgeId, FaceId, FaceId) {
        let mut result = Brep::new();
        let edgeuse = add_use(&mut result, 0.0, 1.0, 1.0);
        let edge_id = edge_of(&result, edgeuse).unwrap();
        let (first_face, first_use) = add_face(&mut result, plane(0.0));
        let (second_face, second_use) = add_face(&mut result, plane(1.0));
        let other = result
            .add_edgeuse_record(Edgeuse {
                edge: Some(edge_id),
                ..Edgeuse::default()
            })
            .unwrap();
        result.edge_mut(edge_id).unwrap().edgeuses.push(other);
        for (edgeuse, faceuse) in [(edgeuse, first_use), (other, second_use)] {
            let loopuse = result
                .add_loopuse_record(crate::topology::Loopuse {
                    faceuse: Some(faceuse),
                    start: crate::topology::LoopuseStart::Edgeuse(Some(edgeuse)),
                    ..crate::topology::Loopuse::default()
                })
                .unwrap();
            result.faceuse_mut(faceuse).unwrap().loopuses.push(loopuse);
            let record = result.edgeuse_mut(edgeuse).unwrap();
            record.owner = crate::topology::EdgeuseOwner::Loopuse(Some(loopuse));
            record.ccw = Some(edgeuse);
            record.cw = Some(edgeuse);
        }
        (result, edge_id, first_face, second_face)
    }

    fn prepare(
        executive: &mut OffsetExecutive,
        original: &Brep,
        first: EdgeuseId,
        other: EdgeuseId,
    ) -> Result<Option<(EdgeId, SynchronizedEdgeuses)>> {
        let entities = executive
            .offset_brep()
            .map(split_entities)
            .unwrap_or_default();
        let brep_key = executive.offset_brep().map_or(ObjectKey(0), Brep::key);
        split_env(brep_key, &entities, |env, curve_ref| {
            executive.prepare_offset_edgeuses(
                &mut Kernel::default(),
                original,
                first,
                other,
                &mut NotifyState::from_env(env),
                curve_ref,
            )
        })
    }

    #[test]
    fn shared_edge_preparation_preserves_shell_skip_and_validation_order() {
        let (mut original, edge, first_face, other_face) = excess_face_fixture();
        let first = original.edge(edge).unwrap().edgeuses[0];
        let other = original.edge(edge).unwrap().edgeuses[1];
        let mut exec = executive();
        assert_eq!(
            prepare(&mut exec, &original, first, other),
            Ok(Some((edge, SynchronizedEdgeuses::default())))
        );
        exec.set_shell_faces(&[first_face, other_face]);
        let missing = EdgeuseId::from_index(99);
        exec.relate_edgeuses(first, missing);
        exec.set_offset_brep(Brep::new());
        // The selected-face no-op precedes synchronization of the stale mapping.
        assert_eq!(prepare(&mut exec, &original, first, other), Ok(None));
        exec.set_extend_convex_edges(true);
        assert!(matches!(
            prepare(&mut exec, &original, first, other),
            Err(SolidError::NotFound { entity: EntityRef::Edgeuse(id), .. }) if id == missing
        ));
        // A missing first edge is checked before even reading the other face.
        original.edgeuse_mut(first).unwrap().edge = None;
        assert!(matches!(
            prepare(&mut exec, &original, first, missing),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn shared_gap_tail_keeps_empty_success_deferred_errors_and_pipe_fallback() {
        let mut kernel = Kernel::default();
        let mut original = Brep::new();
        let original_use = add_use(&mut original, 0.0, 1.0, 1.0);
        let edge = edge_of(&original, original_use).unwrap();
        let mut exec = executive();
        assert_eq!(
            with_state(|state| exec.fill_offset_edgeuse_gaps(
                &mut kernel,
                &original,
                edge,
                &SynchronizedEdgeuses::default(),
                &[],
                state,
            )),
            Ok(())
        );
        assert!(exec.offset_brep().is_none());
        let mut result = Brep::new();
        // evaluates, but has no vertex use and no mate (upstream dereferences them)
        let live = add_use(&mut result, 0.0, 1.0, 1.0);
        let missing = EdgeuseId::from_index(99);
        exec.set_offset_brep(result);
        // Either failing gap fill is a problem; both run, then the pipe fallback.
        for (first, second) in [(missing, live), (live, missing), (missing, missing)] {
            let synchronized = SynchronizedEdgeuses {
                gaps: vec![first],
                other_gaps: vec![second],
                ..SynchronizedEdgeuses::default()
            };
            // the pipe fallback runs (CreatePipeSweep): this fixture's offset B-rep has no
            // region to sweep into, a failure upstream returns too, not a missing call;
            // the offset B-rep is handed back either way
            let r = with_state(|state| {
                exec.fill_offset_edgeuse_gaps(
                    &mut kernel,
                    &original,
                    edge,
                    &synchronized,
                    &[],
                    state,
                )
            });
            assert!(matches!(&r, Err(e) if !e.is_unported()), "{r:?}");
            assert!(exec.offset_brep().is_some());
        }
    }

    #[test]
    fn excess_face_collection_requires_one_solution_and_strict_distance() {
        let (result, edge_id, first_face, second_face) = excess_face_fixture();
        let min_distance = 2.0 * 0.95;
        let mut solution = Solution::<CacheObject>::new();
        solution.start.solution_value = min_distance;
        let mut delete_faces = Vec::new();
        for solutions in [vec![], vec![solution], vec![solution, solution]] {
            collect_excess_faces(
                &result,
                edge_id,
                &solutions,
                min_distance,
                &[],
                &mut delete_faces,
            )
            .unwrap();
            assert!(delete_faces.is_empty());
        }
        for distance in [min_distance + 1.0, f64::NAN] {
            solution.start.solution_value = distance;
            collect_excess_faces(
                &result,
                edge_id,
                &[solution],
                min_distance,
                &[],
                &mut delete_faces,
            )
            .unwrap();
            assert!(delete_faces.is_empty());
        }
        solution.start.solution_value = min_distance - 0.01;
        collect_excess_faces(
            &result,
            edge_id,
            &[solution],
            min_distance,
            &[],
            &mut delete_faces,
        )
        .unwrap();
        assert_eq!(delete_faces, vec![first_face, second_face]);
    }

    #[test]
    fn excess_face_collection_deduplicates_stably_and_skips_shell_incident_edges() {
        let (result, edge_id, first_face, second_face) = excess_face_fixture();
        let mut solution = Solution::<CacheObject>::new();
        solution.start.solution_value = 0.0;
        let mut delete_faces = vec![second_face];
        for _ in 0..2 {
            collect_excess_faces(&result, edge_id, &[solution], 1.0, &[], &mut delete_faces)
                .unwrap();
        }
        assert_eq!(delete_faces, vec![second_face, first_face]);
        delete_faces.clear();
        collect_excess_faces(
            &result,
            edge_id,
            &[solution],
            1.0,
            &[second_face],
            &mut delete_faces,
        )
        .unwrap();
        assert!(delete_faces.is_empty());
        // An inset's negative minimum distance must not be replaced by its magnitude.
        collect_excess_faces(&result, edge_id, &[solution], -0.95, &[], &mut delete_faces).unwrap();
        assert!(delete_faces.is_empty());
    }

    #[test]
    fn original_brep_cache_builds_through_the_brep_box() {
        let kernel = Kernel::default();
        // The real cache constructor computes the B-rep box (query.brep) first.
        assert!(make_original_brep_cache(&kernel, &Brep::new()).is_ok());
    }

    #[test]
    fn original_brep_query_calls_live_solver_and_propagates_its_status() {
        let kernel = Kernel::default();
        let mut original = Brep::new();
        let cache = BrepCache::new(crate::geom::a_object::BREP_TYPE).unwrap();
        let keys = OriginalKeys::new(&original).unwrap();
        assert!(matches!(
            solve_original_brep_point(&kernel, &mut original, &cache, &keys, Vec3::ZERO),
            Err(SolidError::Upstream {
                operation: "intersect::topology_solver::p1::TopologyObjects::brep_trees",
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn original_brep_query_keys_records_by_their_geometry_owners() {
        let kernel = Kernel::default();
        let mut original = Brep::new();
        let first_use = add_use(&mut original, 0.0, 1.0, 1.0);
        let second_use = add_use(&mut original, 0.0, 1.0, 1.0);
        let first = edge_of(&original, first_use).unwrap();
        let second = edge_of(&original, second_use).unwrap();
        let custom = ObjectRef {
            key: ObjectKey(900),
            type_code: edge::TYPE,
        };
        let set_owner = |original: &mut Brep, edge_id: EdgeId, owner: ObjectRef| {
            original
                .edge_mut(edge_id)
                .unwrap()
                .curve
                .as_mut()
                .unwrap()
                .set_owner(Some(owner), None);
        };
        // A custom owner is the edge's identity; an unowned edge keeps its arena one.
        set_owner(&mut original, first, custom);
        let (_, keys) = make_original_brep_cache(&kernel, &original).unwrap();
        assert_eq!(keys.key(CacheObject::Edge(first)), custom);
        assert_eq!(keys.key(CacheObject::Curve(first)), custom);
        assert_eq!(
            keys.key(CacheObject::Edge(second)),
            arena_identity(CacheObject::Edge(second))
        );
        // One owner for two edges, or an owner naming another record, is rejected.
        set_owner(&mut original, second, custom);
        assert!(matches!(
            make_original_brep_cache(&kernel, &original),
            Err(SolidError::Unsupported { .. })
        ));
        set_owner(
            &mut original,
            second,
            arena_identity(CacheObject::Edge(first)),
        );
        set_owner(
            &mut original,
            first,
            arena_identity(CacheObject::Edge(second)),
        );
        assert!(make_original_brep_cache(&kernel, &original).is_ok());
        set_owner(&mut original, first, arena_identity(CacheObject::Brep));
        assert!(matches!(
            make_original_brep_cache(&kernel, &original),
            Err(SolidError::Unsupported { .. })
        ));
    }

    #[test]
    fn merge_clears_all_relations_and_preserves_upstream_transfer_order() {
        let mut kernel = Kernel::default();
        let mut original = Brep::new();
        for solid in [false, true] {
            let mut executive = executive();
            executive.set_offset_brep(Brep::new());
            executive.set_create_offset_solid(solid);
            executive.relate_edgeuses(EdgeuseId::from_index(1), EdgeuseId::from_index(2));
            // StitchIntoShell and StitchIntoSolid of the empty offset B-rep succeed (the
            // B-rep box is ported, query.brep); the merged B-rep is handed back
            let merged = executive
                .piecewise_merge(&mut kernel, &mut original, None)
                .unwrap();
            assert!(merged.faces().is_empty());
            assert!(executive.offset_brep().is_none());
            assert_eq!(executive.offset_edgeuse(EdgeuseId::from_index(1)), None);
        }
        let mut executive = executive();
        executive.set_merge_results(true);
        executive.set_offset_brep(Brep::new());
        executive.relate_edgeuses(EdgeuseId::from_index(1), EdgeuseId::from_index(2));
        // ShrinkGeometry of the (empty) offset B-rep succeeds; the merge itself needs
        // the caller's notification stores, and the temporary stays
        assert!(matches!(
            executive.piecewise_merge(&mut kernel, &mut original, None),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(executive.offset_brep().is_some());
        assert_eq!(executive.offset_edgeuse(EdgeuseId::from_index(1)), None);
    }

    #[test]
    fn relate_surface_pair_skips_destroyed_faces_without_surfaces() {
        let mut brep = Brep::new();
        let (live_a, _) = add_face(
            &mut brep,
            Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)),
        );
        let (live_b, _) = add_face(
            &mut brep,
            Surface::from(Plane::from_normal(Vec3::Z, Vec3::Z)),
        );
        let (destroyed, _) = add_face(
            &mut brep,
            Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Y)),
        );
        brep.face_mut(destroyed).unwrap().surface = None;

        let mut no_ssi = Relation::default();
        relate_surface_pair(&brep, &mut no_ssi, destroyed, live_a).unwrap();
        relate_surface_pair(&brep, &mut no_ssi, live_a, FaceId::from_index(999)).unwrap();
        assert!(no_ssi.all_firsts().is_empty());

        relate_surface_pair(&brep, &mut no_ssi, live_a, live_b).unwrap();
        assert_eq!(no_ssi.seconds(live_a), vec![live_b]);
        assert_eq!(no_ssi.seconds(live_b), vec![live_a]);
    }

    fn add_face(brep: &mut Brep, surface: Surface) -> (FaceId, FaceuseId) {
        let face = brep
            .add_face_record(Face {
                surface: Some(surface),
                uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                ..Face::new(ShellId::from_index(0))
            })
            .unwrap();
        let faceuse = brep
            .add_faceuse_record(Faceuse {
                face: Some(face),
                orientation: OrientType::Same,
                ..Faceuse::default()
            })
            .unwrap();
        brep.face_mut(face).unwrap().faceuse = Some(faceuse);
        (face, faceuse)
    }

    #[test]
    fn uv_boolean_bounds_ignore_ordinary_failures_and_keep_conversion_side_effects() {
        let kernel = Kernel::default();
        // Rust-only error-path input: upstream ConvertTo3D dereferences the missing NURB.
        let mut curve = Curve::from(BSplineCurve::new(2));
        let bbox = uv_curve_bounding_box(&kernel, &mut curve).unwrap();
        assert_eq!(bbox.min(), Extent3d::new().min());
        assert_eq!(bbox.max(), Extent3d::new().max());
        let CurveKind::BSpline(curve) = curve.kind() else {
            panic!("expected the original B-spline kind");
        };
        assert_eq!(curve.dim(), 3);
    }

    #[test]
    fn uv_boolean_bounds_compute_converted_bspline_and_line_copies() {
        let kernel = Kernel::default();
        let spline = BSplineCurve::create_line_segment(
            3,
            Vec3::new(-2.0, 1.0, 8.0),
            Vec3::new(6.0, 1.0, 9.0),
        )
        .unwrap();
        let line = Line::from_points(Vec3::new(-2.0, 1.0, 0.0), Vec3::new(6.0, 1.0, 0.0), 2);
        for mut curve in [Curve::from(spline), Curve::from(line)] {
            let bbox = uv_curve_bounding_box(&kernel, &mut curve).unwrap();
            assert_eq!(bbox.min(), Vec3::new(-2.0, 1.0, 0.0));
            assert_eq!(bbox.max(), Vec3::new(6.0, 1.0, 0.0));
        }
    }

    fn face_env<R>(
        run: impl FnOnce(&mut NotifyEnv<'_>, &dyn Fn(EntityRef) -> ObjectRef) -> R,
    ) -> R {
        let mut records = topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let key = |_: EntityRef| ObjectRef {
            key: ObjectKey(1),
            type_code: edge::TYPE,
        };
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key,
        };
        run(&mut env, &|_| ObjectRef {
            key: ObjectKey(2),
            type_code: edge::TYPE,
        })
    }

    fn build_face(
        executive: &mut OffsetExecutive,
        original: &mut Brep,
        faceuse: Option<FaceuseId>,
        offset: &mut Option<Surface>,
    ) -> Result<()> {
        let mut kernel = Kernel::default();
        face_env(|env, offset_key_of| {
            executive.build_offset_of_faceuse(
                &mut kernel,
                original,
                faceuse,
                offset,
                &SsiData::default(),
                env,
                offset_key_of,
                &|_, type_code| ObjectRef {
                    key: ObjectKey(3),
                    type_code,
                },
            )
        })
    }

    #[test]
    fn face_prefix_handles_null_surface_and_selects_domain_constructor() {
        let mut executive = executive();
        let mut original = Brep::new();
        let (_, faceuse) = add_face(&mut original, plane(0.0));
        // NER(pFU) precedes the null-surface no-op.
        assert!(build_face(&mut executive, &mut original, None, &mut None).is_err());
        build_face(&mut executive, &mut original, Some(faceuse), &mut None).unwrap();
        executive.set_offset_brep(Brep::new());
        // A loopless face fails MakeFaceWithCurves (invalid input), whose status
        // upstream ignores: nothing is related, and the face took the surface.
        let mut offset = Some(plane(1.0));
        build_face(&mut executive, &mut original, Some(faceuse), &mut offset).unwrap();
        assert!(offset.is_none());
        assert!(executive.faceuse_to_faces.second(faceuse).is_none());
        let smaller = Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.2, 0.2, 0.8, 0.8),
        ));
        let mut offset = Some(smaller.clone());
        // The 2D-boolean branch: the loopless face gives the planar face no curves,
        // and that MakeFaceWithCurves status is returned (SER) before the boolean,
        // with the offset surface still the caller's.
        match build_face(&mut executive, &mut original, Some(faceuse), &mut offset) {
            Err(error) => assert!(!error.is_unported(), "{error:?}"),
            Ok(()) => panic!("a loopless planar face must fail"),
        }
        assert!(offset.is_some());
        executive.set_operation(OffsetOperation::LocalOperation);
        let mut offset = Some(smaller);
        build_face(&mut executive, &mut original, Some(faceuse), &mut offset).unwrap();
        assert!(offset.is_none());
    }

    #[test]
    fn face_prefix_drops_uv_trim_curve_before_domain_boolean() {
        let mut executive = executive();
        let (mut original, _, face, _) = excess_face_fixture();
        let faceuse = original.face(face).unwrap().faceuse;
        original.face_mut(face).unwrap().uv_domain = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        executive.set_offset_brep(Brep::new());
        let mut offset = Some(plane(1.0));
        // Since topology.edgeuse_p3 (#117) builds the UV trim curve, the face prefix
        // runs the plane's DropCurve (wired by intersect.surface_drop_lib_p3, #136)
        // before the domain boolean; the fixture's edge use then has no vertex use to
        // set the curve on.
        match build_face(&mut executive, &mut original, faceuse, &mut offset).unwrap_err() {
            SolidError::InvalidInput { operation, reason } => {
                assert_eq!(operation, "topology::edgeuse::p1");
                assert!(reason.starts_with("SetUVTrimCurve"), "{reason}");
            }
            other => panic!("expected SetUVTrimCurve to fail, got {other:?}"),
        }
    }

    #[test]
    fn mapping_empty_faces_completes_orientation_selection() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let mut original = Brep::new();
        let (_, original_use) = add_face(&mut original, plane(0.0));
        let mut result = Brep::new();
        let (face, _) = add_face(&mut result, plane(1.0));
        executive.set_offset_brep(result);
        face_env(|env, offset_key_of| {
            executive.map_edgeuses(
                &kernel,
                &mut original,
                original_use,
                face,
                env,
                offset_key_of,
            )
        })
        .unwrap();
    }

    #[test]
    fn mapping_kernel_context_is_bound_only_at_notification_boundary() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::geom::context::Context;
        use crate::geom::object::NotifyOperation;
        use crate::topology::edge::notification_test_support as callbacks;
        use crate::topology::edge::NotificationStores;
        use crate::topology::topology::Topology;

        let caller = ObjectRef {
            key: ObjectKey(3101),
            type_code: 71,
        };
        let payload = ObjectRef {
            key: ObjectKey(3102),
            type_code: 72,
        };
        let result_key = ObjectRef {
            key: ObjectKey(3103),
            type_code: 73,
        };
        let mut kernel = Kernel::default();
        let kernel_events = callbacks::install(&mut kernel.context);
        let mut external_context = Context::default();
        let external_events = callbacks::install(&mut external_context);
        let mut records = topology::NotifyRecords::new();
        records.insert(caller.key, Topology::default());
        let mut attributes = AttributeStore::new();
        let key_of = |_entity| result_key;
        let notify = |env: &mut NotifyEnv<'_>| {
            if let Some(context) = env.context.as_deref_mut() {
                context.set_doing_boolean(true);
            }
            env.records
                .get_mut(&caller.key)
                .ok_or_else(|| SolidError::invalid_input(OP, "missing test topology"))?
                .set_user_index1(918);
            topology::notify(
                env.records,
                env.attributes,
                env.context.as_deref_mut(),
                NotifyOperation::PreEdit,
                caller,
                Some(payload),
                Some(result_key),
                None,
            )
        };
        // Kernel-bound state routes both callbacks to the actual kernel context.
        {
            let mut stores = NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &key_of,
            };
            let mut state = NotifyState::from_kernel_stores(&mut stores);
            let mut mapping_kernel = MapKernel::Mutable(&mut kernel);
            mapping_kernel.with_env(&mut state, notify).unwrap();
        }
        assert_eq!(records.get(&caller.key).unwrap().user_index1(), 918);
        let event_matches = |events: &std::sync::Arc<std::sync::Mutex<Vec<callbacks::Event>>>| {
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            for (name, event) in ["user", "system"].into_iter().zip(events.iter()) {
                assert_eq!(event.callback, name);
                assert_eq!(event.operation, NotifyOperation::PreEdit);
                assert_eq!(event.caller, caller);
                assert_eq!(event.data, [Some(payload), Some(result_key), None]);
            }
        };
        event_matches(&kernel_events);
        assert!(external_events.lock().unwrap().is_empty());
        assert!(kernel.context.doing_boolean());

        // Legacy None remains context-free and cannot fall through to Kernel.
        kernel.context.set_doing_boolean(false);
        records.get_mut(&caller.key).unwrap().set_user_index1(0);
        {
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &key_of,
            };
            let mut state = NotifyState::from_env(&mut env);
            let mut mapping_kernel = MapKernel::Read(&kernel);
            mapping_kernel.with_env(&mut state, notify).unwrap();
        }
        assert_eq!(records.get(&caller.key).unwrap().user_index1(), 918);
        assert!(!kernel.context.doing_boolean());
        assert_eq!(kernel_events.lock().unwrap().len(), 2);

        // Legacy Some uses its selected external context, not the supplied Kernel.
        records.get_mut(&caller.key).unwrap().set_user_index1(0);
        {
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: Some(&mut external_context),
                key_of: &key_of,
            };
            let mut state = NotifyState::from_env(&mut env);
            let mut mapping_kernel = MapKernel::Read(&kernel);
            mapping_kernel.with_env(&mut state, notify).unwrap();
        }
        assert_eq!(records.get(&caller.key).unwrap().user_index1(), 918);
        assert!(!kernel.context.doing_boolean());
        event_matches(&external_events);
    }

    fn strut_vertex_fixture() -> (Brep, VertexId, FaceuseId) {
        let mut original = Brep::new();
        let vertex = original.add_vertex_record(Vertex::default()).unwrap();
        let (_, faceuse) = add_face(&mut original, plane(0.0));
        let edgeuse = add_use(&mut original, 0.0, 1.0, 1.0);
        let vertexuse = original
            .add_vertexuse_record(Vertexuse {
                vertex: Some(vertex),
                owner: crate::topology::VertexuseOwner::Edgeuse(Some(edgeuse)),
                ..Vertexuse::default()
            })
            .unwrap();
        original
            .vertex_mut(vertex)
            .unwrap()
            .vertexuses
            .push(vertexuse);
        let loopuse = original
            .add_loopuse_record(crate::topology::Loopuse {
                faceuse: Some(faceuse),
                start: crate::topology::LoopuseStart::Edgeuse(Some(edgeuse)),
                ..crate::topology::Loopuse::default()
            })
            .unwrap();
        original
            .faceuse_mut(faceuse)
            .unwrap()
            .loopuses
            .push(loopuse);
        let record = original.edgeuse_mut(edgeuse).unwrap();
        record.owner = crate::topology::EdgeuseOwner::Loopuse(Some(loopuse));
        record.vertexuse = Some(vertexuse);
        record.ccw = Some(edgeuse);
        record.cw = Some(edgeuse);
        record.orientation = OrientType::Same;
        assert!(eu::is_strut(&original, edgeuse).unwrap());
        (original, vertex, faceuse)
    }

    #[test]
    fn vertex_strut_missing_or_unmapped_mate_returns_empty_geometry() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let (mut original, vertex, faceuse) = strut_vertex_fixture();
        let (_, mate) = add_face(&mut original, plane(0.0));
        executive.set_offset_brep(Brep::new());
        for mate in [None, Some(mate)] {
            original.faceuse_mut(faceuse).unwrap().mate = mate;
            let offset = executive
                .build_offset_of_vertex(&kernel, &original, Some(vertex))
                .unwrap();
            assert!(offset.surface.is_none());
            assert!(offset.trim_curves.is_empty());
            assert!(offset.curve_orientations.is_empty());
        }
    }

    #[test]
    fn vertex_strut_mate_mapping_builds_geometry_and_preserves_primary_precedence() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let (mut original, vertex, faceuse) = strut_vertex_fixture();
        let (_, mate) = add_face(&mut original, plane(0.0));
        original.faceuse_mut(faceuse).unwrap().mate = Some(mate);
        let (mut result, edge, cap_face, _) = excess_face_fixture();
        result.set_tolerance(1.0e-6);
        let curve = Curve::from(
            BSplineCurve::create_circle_segment(
                3,
                &Axis2Placement::default(),
                executive.offset_distance() * 0.5,
                0.0,
                90.0,
                NurbCircleParam::Quadratic,
            )
            .unwrap(),
        );
        assert!(curve
            .is_arc(&kernel, 10, result.tolerance())
            .unwrap()
            .is_some());
        result.edge_mut(edge).unwrap().curve = Some(curve);
        let (empty_face, _) = add_face(&mut result, plane(0.0));
        executive.set_offset_brep(result);
        executive.relate_faceuse(mate, cap_face);
        let cap = executive
            .build_offset_of_vertex(&kernel, &original, Some(vertex))
            .unwrap();
        assert!(matches!(
            cap.surface.unwrap().kind(),
            SurfaceKind::BSpline(_)
        ));
        assert!(cap.trim_curves.is_empty());
        assert!(cap.curve_orientations.is_empty());
        executive.relate_faceuse(faceuse, empty_face);
        let offset = executive
            .build_offset_of_vertex(&kernel, &original, Some(vertex))
            .unwrap();
        assert!(offset.surface.is_none());
        assert!(offset.trim_curves.is_empty());
        assert!(offset.curve_orientations.is_empty());
    }

    #[test]
    fn vertex_null_is_rejected_and_isolated_vertex_uses_the_source_arena() {
        let kernel = Kernel::default();
        let mut executive = executive();
        let mut original = Brep::new();
        let point = Vec3::new(1.0, -2.0, 3.0);
        original.set_tolerance(1.0e-6);
        let vertex = original
            .add_vertex_record(Vertex {
                point,
                ..Vertex::default()
            })
            .unwrap();
        let mut result = Brep::new();
        result.set_tolerance(1.0e-3);
        let result_vertex = result
            .add_vertex_record(Vertex {
                point: Vec3::new(20.0, 30.0, 40.0),
                ..Vertex::default()
            })
            .unwrap();
        assert_eq!(vertex, result_vertex);
        executive.set_offset_brep(result);
        assert!(executive
            .build_offset_of_vertex(&kernel, &original, None)
            .is_err());
        let cap = executive
            .build_offset_of_vertex(&kernel, &original, Some(vertex))
            .unwrap();
        let surface = cap.surface.unwrap();
        assert!(matches!(surface.kind(), SurfaceKind::BSpline(_)));
        assert!(cap.trim_curves.is_empty());
        assert!(cap.curve_orientations.is_empty());
        let domain = surface.natural_uv_domain_if_built();
        for (u, v) in [(0.17, 0.29), (0.43, 0.61), (0.83, 0.79)] {
            let uv = crate::math::Point2::new(
                (1.0 - u) * domain.min().x + u * domain.max().x,
                (1.0 - v) * domain.min().y + v * domain.max().y,
            );
            let sample = surface.evaluate_point(&kernel, uv).unwrap();
            assert!((sample.distance(point) - executive.offset_distance()).abs() < 1.0e-12);
        }
        assert_eq!(original.tolerance(), 1.0e-6);
        assert_eq!(executive.offset_brep().unwrap().tolerance(), 1.0e-3);
        with_state(|state| {
            executive.fill_gap_edgeuses_with_state(
                &mut Kernel::default(),
                EdgeId::from_index(u32::MAX),
                &[],
                &[],
                state,
            )
        })
        .unwrap();
    }

    #[test]
    fn notify_copies_edge_and_distinct_vertex_records_without_cross_arena_aliases() {
        let mut original = Brep::new();
        let mut result = Brep::new();
        let old = add_use(&mut original, 0.0, 1.0, 1.0);
        let new = add_use(&mut result, 0.0, 1.0, 1.0);
        // Different primary-use orientations must not swap endpoint metadata.
        for (brep, edgeuse, orientation) in [
            (&mut original, old, OrientType::Opposite),
            (&mut result, new, OrientType::Same),
        ] {
            let v1 = brep.add_vertex_record(Vertex::default()).unwrap();
            let v2 = brep.add_vertex_record(Vertex::default()).unwrap();
            let vu1 = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v1),
                    ..Vertexuse::default()
                })
                .unwrap();
            let vu2 = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v2),
                    ..Vertexuse::default()
                })
                .unwrap();
            let edge = edge_of(brep, edgeuse).unwrap();
            brep.edgeuse_mut(edgeuse).unwrap().vertexuse = Some(vu1);
            brep.edgeuse_mut(edgeuse).unwrap().orientation = orientation;
            let mate = brep
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    vertexuse: Some(vu2),
                    orientation: if orientation == OrientType::Same {
                        OrientType::Opposite
                    } else {
                        OrientType::Same
                    },
                    ..Edgeuse::default()
                })
                .unwrap();
            brep.edge_mut(edge).unwrap().edgeuses.push(mate);
            brep.edge_mut(edge).unwrap().vertices = if orientation == OrientType::Same {
                [v1, v2]
            } else {
                [v2, v1]
            };
        }
        let key = |entity| match entity {
            EntityRef::Edge(id) => ObjectRef {
                key: ObjectKey(10 + u64::from(id.index())),
                type_code: edge::TYPE,
            },
            EntityRef::Vertex(id) => ObjectRef {
                key: ObjectKey(20 + u64::from(id.index())),
                type_code: vertex::TYPE,
            },
            _ => panic!("unexpected notified entity"),
        };
        let offset_key = |entity| {
            let mut value = key(entity);
            value.key.0 += 100;
            value
        };
        let mut records = topology::NotifyRecords::new();
        for (entity, metadata) in [
            (EntityRef::Edge(EdgeId::from_index(0)), 37),
            (EntityRef::Vertex(VertexId::from_index(0)), 11),
            (EntityRef::Vertex(VertexId::from_index(1)), 53),
        ] {
            let mut record = topology::Topology::default();
            record.set_user_index1(metadata);
            records.insert(key(entity).key, record);
            records.insert(offset_key(entity).key, topology::Topology::default());
        }
        let mut attributes = AttributeStore::default();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key,
        };
        notify_edge_and_vertices(
            &original,
            EdgeId::from_index(0),
            &result,
            EdgeId::from_index(0),
            &mut env,
            &offset_key,
        )
        .unwrap();
        // The public helper must still use these actual records and distinct
        // arena keys before EdgeuseOffset dereferences the wire use's null face.
        for entity in [
            EntityRef::Edge(EdgeId::from_index(0)),
            EntityRef::Vertex(VertexId::from_index(0)),
            EntityRef::Vertex(VertexId::from_index(1)),
        ] {
            env.records
                .insert(offset_key(entity).key, topology::Topology::default());
        }
        let mut exec = executive();
        exec.set_offset_brep(result);
        exec.relate_edgeuses(old, new);
        let status = exec.build_offset_of_edgeuses(
            &mut Kernel::default(),
            &mut original,
            old,
            old,
            MarkType::Mark,
            &mut env,
            &offset_key,
            &|edge, kind| ObjectRef {
                key: ObjectKey(1000 + u64::from(edge.index())),
                type_code: kind,
            },
        );
        assert!(
            matches!(&status, Err(SolidError::InvalidInput { reason, .. }) if *reason == "edge use without face"),
            "{status:?}"
        );
        for (entity, expected) in [
            (EntityRef::Edge(EdgeId::from_index(0)), 37),
            (EntityRef::Vertex(VertexId::from_index(0)), 53),
            (EntityRef::Vertex(VertexId::from_index(1)), 11),
        ] {
            assert_eq!(
                records.get(&offset_key(entity).key).unwrap().user_index1(),
                expected
            );
        }
        assert_eq!(
            edge::vertex(&original, EdgeId::from_index(0)).unwrap(),
            Some(VertexId::from_index(0))
        );
        assert_eq!(
            edge::start_vertex(&original, EdgeId::from_index(0)).unwrap(),
            Some(VertexId::from_index(1))
        );
    }
}

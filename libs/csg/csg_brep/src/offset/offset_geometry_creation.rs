//! Offset geometry creation (`SmOffsetGeometryCreation`): the offset surfaces of
//! faces, the cap surfaces of edges and vertices of an offset, and the mating,
//! extension and trimming of the offset edges, used by the offset executive.
//!
//! Upstream: `source/SMLib/src/SmOffsetGeometryCreation.cpp`,
//! `source/SMLib/inc/SmOffsetGeometryCreation.h`.
//!
//! | Rust                                                        | Upstream                                              |
//! |-------------------------------------------------------------|-------------------------------------------------------|
//! | [`OffsetGeometryCreation`], [`OffsetGeometryCreation::new`] | `SmOffsetGeometryCreation`, its constructor           |
//! | `Drop` (implicit)                                           | `~SmOffsetGeometryCreation`                           |
//! | [`OffsetGeometryCreation::this_approx_tol_3d`]              | `GetThisApproxTol3d`                                  |
//! | [`OffsetGeometryCreation::tangency_tol_radians`]            | `GetTangencyTolRadians`                               |
//! | [`OffsetGeometryCreation::tangency_tol_degrees`]            | `GetTangencyTolDegrees`                               |
//! | [`OffsetGeometryCreation::offset_distance`]                 | `GetOffsetDistance`                                   |
//! | [`OffsetGeometryCreation::set_make_analytics`]              | `SetMakeAnalytics`                                    |
//! | [`OffsetGeometryCreation::faceuse_offset`]                  | `SmOffsetGeometryCreation::FaceuseOffset`             |
//! | [`LocalOperationGeometryCreation`], [`LocalOperationGeometryCreation::faceuse_offset`] | `SmLocalOperationGeometryCreation`, its `FaceuseOffset` |
//! | [`OffsetGeometryCreation::vertex_offset`]                   | `VertexOffset`                                                |
//! | [`OffsetGeometryCreation::update_face_surface_domain`]      | `UpdateFaceSurfaceDomain`                             |
//! | [`OffsetGeometryCreation::replace_edge_curves`]             | `ReplaceEdgeCurves`                                   |
//! | [`OffsetGeometryCreation::trim_extended_edge`]              | `TrimExtendedEdge`                                    |
//! | [`OffsetGeometryCreation::extend_trim_edges`]               | `ExtendTrimEdges`                                     |
//! | [`OffsetGeometryCreation::mate_edges`]                      | `MateEdges`                                           |
//! | [`OffsetGeometryCreation::have_tangent_gaps`]               | `HaveTangentGaps`                                     |
//! | [`OffsetGeometryCreation::edgeuse_offset`]                  | `EdgeuseOffset`                                       |
//! | [`OffsetGeometryCreation::is_step_across_edge`]             | the fork's `IsStepAcrossEdge` (source fork 5d428ae; opt-in) |
//! | [`OffsetGeometryCreation::create_step_wall`]                | the fork's `CreateStepWall` (source fork 5d428ae, 90aabff; opt-in) |
//! | `make_non_rational` (private)                               | the fork's uniform-weight rail made polynomial (90aabff) |
//! | [`SsiData`]                                                 | `SmSSIData` (`SmSSIData.h`, CO-PORTED: no unit owns it) |
//! | [`OffsetExecutive`]                                         | what this unit uses of `SmOffsetExecutive` (ported in `offset.offset_executive_p1`), over handles |
//! | [`OffsetBrep`]                                              | what the unit reads from and writes to the two B-reps |
//! | `VertexOffsetBrep` (private)                                | the vertex creator's topology/owned-geometry subset |
//! | `FaceOffsetBrep` (private)                                   | the face creator's topology/owned-geometry subset |
//! | `create_analytic_extension` (private)                       | the source fork's `sm_CreateAnalyticExtension` (source fork@fddc366, @98ab1b0; opt-in) |
//! | `exact_piece` (private)                                     | the fork's `sm_ExactPiece` (source fork@fddc366; opt-in) |
//! | [`make_edges_on_analytics_exact`]                          | the fork's `MakeEdgesOnAnalyticsExact` (source fork@cd85d34; opt-in) |
//!
//! Deviations:
//! - **Object handles.** Every `SmObject*` (topology, curves, surfaces) is a caller
//!   handle `O` ([`ClassObject`] + [`SolutionObject`]), as in
//!   [`proto_topology`](crate::topology::proto_topology) and the topology intersector.
//!   The B-reps are reached through [`OffsetBrep`] (each method names the ported
//!   function it stands for), geometry through [`ClassGeometry`] /
//!   [`ProtoGeometry`]; new curves and surfaces go to the owner's stores
//!   (`insert_curve` / `insert_surface`) and their handles are returned where upstream
//!   returns owned pointers. The offset executive is reached through
//!   [`OffsetExecutive`] (its stored flags, shell faces, edge-use map and
//!   extended-surface cache), which its owner implements over the ported
//!   [`crate::offset::offset_executive::p1::OffsetExecutive`]. The executive's face and
//!   vertex adapters use narrow internal backends; public full-backend methods delegate
//!   to the same algorithms, preserving lookup/storage error translation.
//! - **Context.** `crContext` is the [`Kernel`] passed to each method.
//! - **Out-params** become return values ([`VertexOffsetGeometry`], [`EdgeuseOffset`],
//!   [`ReplaceEdgeCurves`], the trim interval of `TrimExtendedEdge` as an `Option`,
//!   the surface and self-intersections of `FaceuseOffset`). The `NotUsed`
//!   parameters are kept.
//! - **Deleted objects.** Geometry retained by handle stays in the owner's store
//!   when upstream deletes it ([`ReplaceEdgeCurves`] reports the `rbDelete*` flags).
//!   Trimmed UV and approximated offset-rail temporaries instead stay locally owned.
//! - **Callees not ported yet** are private stubs returning [`SolidError::Unported`],
//!   each tagged `UNPORTED(<unit>)`:
//!   `SmCircularCrossSectionFSG::CreateSurfaceFromCurves` is
//!   `blend.fillet_standard_solver_p1`'s, on the centre line's `SmBSplineCurve` part.
//!   The code after a stub is ported in full;
//!   `SmEdgeuse::NormalizedEvaluate` / `CreateUVTrimCurve` (`topology.edgeuse_p3`,
//!   through [`OffsetBrep`]), `SmBrep::MateLaminaEdgeToSurface` / `ReplaceSurface`
//!   (`topology.brep_p2`, through [`OffsetBrep`]), `SmSurface::GlobalCurveIntersect` (`surface.surface_p8`) and
//!   `SmSurface::CreateOffsetSurface` / the `SmOffsetSurface` constructor
//!   (`surface.offset_surface`) are real calls. `EdgeuseOffset` approximates its
//!   missing rails through [`crate::curve::crv_on_surf::CrvOnSurf`] on the actual
//!   offset surface, retaining its concrete evaluation and supplied knot sequence.
//! - `SM_CAST_PTR(SmOffsetSurface, ..)` on an analytic copy: an offset surface gets
//!   `SetAllowExtension(FALSE)`, as upstream.
//! - **Undefined behaviour upstream** (null objects dereferenced, a curve that is
//!   not an `SmBSplineCurve` cast as one and used, a null curve extension used) is
//!   [`SolidError::InvalidInput`] with [`UNDEFINED_UPSTREAM`] as operation, which tells
//!   it apart from a callee's real `SM_ERR_INVALID_INPUT`; the statuses upstream
//!   ignores are ignored whatever they are, except these and [`SolidError::Unported`].
//! - **Opt-in (WR-530, #402).** With `Kernel::fixes.local_operation_refuses_loose_tangent_edges`,
//!   in a local operation ([`OffsetExecutive::is_local_operation`]) `ExtendTrimEdges`
//!   fails when the lifted edge drops onto the other extended surface farther than ten
//!   times `max(edge tolerance, approximation tolerance)`, and `EdgeuseOffset` passes
//!   that failure on (the fork's source fork@23c5b0a).
//! - **Opt-in (WR-530 addendum).** With `Kernel::fixes.local_operation_step_walls`, a
//!   local operation's tangent edge whose two faces stay parallel but apart (one moved
//!   along its normal) gets the ruled step wall between the two offset edges instead of
//!   the extension (the fork's 5d428ae, 90aabff); `EdgeuseOffset::closing_wall` marks it.
//!   `IsStepAcrossEdge` samples 11 points along the edge (a fixed count).
//! - **`ExtendTrimEdges`' empty seam drop.** When a candidate's `GlobalPointSolve`
//!   (`SM_SR_SINGLE`, minimize) finds no solution, upstream reads `sSolutions2[0]`
//!   anyway (cpp:1610): defined behaviour, the array's backing `sData2[0]` (a default
//!   `SmSolution`, value `0.0`, or an earlier candidate's result). A single minimizing
//!   solve always has a solution, so this is unreachable; the port refuses it with the
//!   same [`UNDEFINED_UPSTREAM`] error instead of reproducing the stale value.
//! - **Virtual calls.** `pSur->CreateOffsetSurface` dispatches on the surface kind
//!   (the plane, surface-of-revolution and B-spline overrides, else the base); the
//!   curves cast `SM_CAST_PTR(SmBSplineCurve, ..)` keep their kind (a line or a circle
//!   is not sliced to its B-spline part), so `IsArc`, `IsLine`, `IsClosed`,
//!   `CalculateBoundingBox`, `Trim`, `Copy`, `DropPoint` and the evaluations take the
//!   kind's override; only `new SmBSplineCurve(*p)` and the static B-spline routines
//!   (`CreateTrimmedPlaneFromRuled`'s reading of an arc, `MakeCurvesCompatible`, the
//!   knots and extension of a UV curve) see the B-spline part.
//! - **Code behind stubs** is ported as upstream runs it, unchecked statuses included:
//!   `HaveTangentGaps` ignores `NormalizedEvaluate`'s and `ClosestPoint`'s statuses (a
//!   failed evaluation leaves the point at `SmPoint3d`'s `SM_UNDEF_DOUBLE` default, a
//!   failed closest point counts as `bSuccess == FALSE`), `TrimExtendedEdge` ignores the
//!   `Evaluate` / `EvaluateNormal` of a solution the same way, and `ExtendTrimEdges` keeps
//!   `pOtherUVCurve` as the edge use's or radial's own UV trim curve when there is no
//!   other extended surface (`cpp:1452`, `1458`), passes it to `TrimExtendedEdge` and trims
//!   it in place (`cpp:1694`). The unchecked `Copy` calls of `ExtendTrimEdges` and
//!   `MateEdges` copy B-spline or line curves, which cannot fail in the port (upstream's
//!   only failure is an allocation), so their `?` never fires.
//!   `SmBSplineSurface::CreateTrimmedPlaneFromRuled` fills its trimming arrays in place
//!   before its last step (the bilinear plane from the plane's own corners) and keeps
//!   them when that step fails; the port returns them only on success, which differs
//!   only if that bilinear surface fails.
//! - **Trim intervals.** A `Trim(SmExtent1d &)` may snap its interval to knots; where
//!   upstream trims several curves to one interval, the snapped interval reaches the
//!   next trim as it does there.
//! - `MSG` / `SM_ASSERT` only report; the `SM_DEBUG_CODE` and `#if 0` blocks and the
//!   empty `bShelling` branch are not ported. `SM_VALIDATE_INTERSECTORS` is not
//!   defined in the reference build.
//! - **Centre line of the circular FSG.** `CreateSurfaceFromCurves` receives the centre
//!   line as the [`Curve`] itself, so `GetDegree` / `IsLine` / `IsArc` / `Evaluate`
//!   dispatch on an `SmLine` (or other analytic) centre line as upstream does
//!   (`SmFilletStandardSolver.cpp:3353-3384`).
//! - **Opt-in fork fix.** With `Kernel::fixes.offset_analytic_mate_edges` (WR-495),
//!   `mate_edges` runs the source fork's exact-piece swap (source fork@fddc366
//!   with @98ab1b0) between the marched intersection and its trimming; off, it is
//!   upstream's. `exact_piece` counts the whole turns it moves a conic angle by and
//!   returns [`SolidError::NotConverged`] past `MAX_EXACT_PIECE_TURNS` (65536), where
//!   the fork's loop would not end for a non-finite or huge angle.
//! - **Opt-in fork fix.** With `Kernel::fixes.shell_edges_on_analytics_exact` (WR-545),
//!   the shell's result runs [`make_edges_on_analytics_exact`]. Upstream's
//!   `CreateTrimmedNURBSCurve` gives `NULL` where the port returns an error; any error
//!   but [`SolidError::Unported`] skips the edge as `NULL` does, as does a failing
//!   `GlobalSurfaceIntersect`. `SmBrep::ReplaceCurve` is written out in place
//!   (`brep_replace_curve`, private), with `SmEdge::ClearUVCaches`.
//! - **Recursion.** None.

use crate::blend::fillet_standard_solver::p1::CircularCrossSectionFsg;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::{Curve, CurveKind, ELLIPSE_TYPE, LINE_TYPE};
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::assert_array::ObjectKey;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::MarkType;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::point_project_to_plane;
use crate::geom::geom_utility::p2::{circle_plane_intersect, ContinuityType, CurveIntersection};
use crate::geom::global_solver::{ObjectZoneTol, SolutionRequested, SolverOperation};
use crate::geom::object::NotifyOperation;
use crate::geom::solution_array::{SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{are_same, zone_tol_3d_of_curve};
use crate::heal::gap::{EdgeEdgeGap, VertexEdgeGap};
use crate::intersect::point_class::{ClassGeometry, ClassObject};
use crate::kernel::Kernel;
use crate::math::{
    max2, min2, Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQRT, PI,
    UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
use crate::nurbs::b_spline_curve::p2::{ArcData, BoundingBoxRequest, ControlPointForm};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{NOW, NOZ};
use crate::query::edge::ClosestPoint;
use crate::surface::offset_surface::{OffsetSurface, SS_UNKNOWN as OFFSET_SS_UNKNOWN};
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p2::{
    SurfaceKind, TsectCurveType, CONE_TYPE, CYLINDER_TYPE, PLANE_TYPE, SPHERE_TYPE, TORUS_TYPE,
};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::SurfaceCache;
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p2 as brep_p2;
use crate::topology::edge::{self as topo_edge, NotificationStores, NotifyState, SetCurveOptions};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p3::create_uv_trim_curves;
use crate::topology::proto_topology::ProtoGeometry;
use crate::topology::{Brep, EdgeId, FaceId};

const OP_LOCAL: &str =
    "offset::offset_geometry_creation::LocalOperationGeometryCreation::faceuse_offset";
const OP_FACEUSE: &str = "offset::offset_geometry_creation::faceuse_offset";
const OP_VERTEX: &str = "offset::offset_geometry_creation::vertex_offset";
const OP_UPDATE: &str = "offset::offset_geometry_creation::update_face_surface_domain";
const OP_EXTEND: &str = "offset::offset_geometry_creation::extend_trim_edges";
const OP_MATE: &str = "offset::offset_geometry_creation::mate_edges";
const OP_EDGEUSE: &str = "offset::offset_geometry_creation::edgeuse_offset";
const OP_EXACT_PIECE: &str = "offset::offset_geometry_creation::exact_piece";
const OP_ANALYTICS_EXACT: &str = "offset::offset_geometry_creation::make_edges_on_analytics_exact";

/// The most whole turns `exact_piece` moves a conic's angle by (a guard: the angles
/// are within a few turns of each other).
const MAX_EXACT_PIECE_TURNS: u32 = 1 << 16;

/// The `operation` of the [`SolidError::InvalidInput`] this module raises where upstream
/// dereferences a null pointer or a failed cast (undefined behaviour), so that a runner
/// tells it apart from a real `SM_ERR_INVALID_INPUT` status of a callee.
pub const UNDEFINED_UPSTREAM: &str =
    "offset::offset_geometry_creation (undefined behaviour upstream)";

/// A dereference of a null pointer or a cast that fails upstream (undefined
/// behaviour): `InvalidInput` with [`UNDEFINED_UPSTREAM`] as operation.
const fn dangling(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: UNDEFINED_UPSTREAM,
        reason,
    }
}

/// `SM_ERR` of an operation.
const fn sm_err(operation: &'static str) -> SolidError {
    SolidError::sm_err(operation)
}

/// A status upstream ignores (whatever it is, `SM_ERR_INVALID_INPUT` included), except
/// the errors that cannot be ignored here: unported callees and undefined behaviour
/// upstream ([`dangling`]).
fn ignore_soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput {
                operation: UNDEFINED_UPSTREAM,
                ..
            }),
        ) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `SmTArray::FindElement`.
fn contains<O: PartialEq>(list: &[O], item: &O) -> bool {
    list.iter().any(|x| x == item)
}

// ---- the data types -----------------------------------------------------------------------

/// The surf/surf intersection curves of a surface (`SmSSIData`): parallel arrays
/// of 3D curves, their UV curves on each surface, kinds and deviations.
#[derive(Debug, Clone, PartialEq)]
pub struct SsiData<O> {
    /// `m_v3DCurves`.
    pub curves_3d: Vec<O>,
    /// `m_vUVCurves1`.
    pub uv_curves1: Vec<Option<O>>,
    /// `m_vUVCurves2`.
    pub uv_curves2: Vec<Option<O>>,
    /// `m_vCurveTypes`.
    pub curve_types: Vec<TsectCurveType>,
    /// `m_vDeviations`.
    pub deviations: Vec<f64>,
}

impl<O> Default for SsiData<O> {
    fn default() -> Self {
        Self {
            curves_3d: Vec::new(),
            uv_curves1: Vec::new(),
            uv_curves2: Vec::new(),
            curve_types: Vec::new(),
            deviations: Vec::new(),
        }
    }
}

impl<O> SsiData<O> {
    /// Empties every array (the `ReSet` calls at the start of the offsets).
    pub fn reset(&mut self) {
        self.curves_3d.clear();
        self.uv_curves1.clear();
        self.uv_curves2.clear();
        self.curve_types.clear();
        self.deviations.clear();
    }
}

/// What [`OffsetGeometryCreation::vertex_offset`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexOffsetGeometry<O> {
    /// `r3DTrimmingCurves`.
    pub trimming_curves: Vec<O>,
    /// `rOrients`.
    pub orients: Vec<OrientType>,
    /// `rpNewSurface`.
    pub surface: Option<O>,
}

/// What [`OffsetGeometryCreation::edgeuse_offset`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeuseOffset<O> {
    /// `rpNewSurface`.
    pub surface: Option<O>,
    /// `r3DTrimmingCurves`.
    pub trimming_curves: Vec<O>,
    /// `rTrimOrientations`.
    pub trim_orientations: Vec<OrientType>,
    /// `rSelfIntersections` (never filled: the self-intersection block is `#if 0`).
    pub self_intersections: SsiData<O>,
    /// Not upstream (`*pbOptClosingWall`, opt-in `Kernel::fixes.local_operation_step_walls`,
    /// fork source fork 5d428ae): `surface` is a step wall, which spans the gap
    /// between two edges and is never extended (as a lamina side wall).
    pub closing_wall: bool,
}

/// What [`OffsetGeometryCreation::replace_edge_curves`] returns: whether the caller
/// should delete the candidate 3D curve and UV curve (`rbDelete3DCurve`,
/// `rbDeleteUVCurve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaceEdgeCurves {
    /// `rbDelete3DCurve`.
    pub delete_3d_curve: bool,
    /// `rbDeleteUVCurve`.
    pub delete_uv_curve: bool,
}

// ---- what the unit needs from the executive and the B-reps --------------------------------

/// What this unit uses of the offset executive (`SmOffsetExecutive`, ported as
/// [`crate::offset::offset_executive::p1::OffsetExecutive`]): its stored flags, the
/// faces to shell, the relation of original edge uses to offset edge uses and its
/// extended-surface cache, over this unit's object handles `O`. The executive's owner
/// implements it on top of the ported struct, mapping handles to its typed ids. The
/// tangency tolerance is not here: upstream's executive calls back into the geometry
/// creation (`m_rGeometryCreation.GetTangencyTolDegrees()`), so this module reads its own.
///
/// # Implementing this trait
///
/// As for [`OffsetBrep`], the only implementation today is the difftest runner's (`Exec`,
/// wrapping the ported struct). An adapter keeps upstream's identities: the extension
/// cache is keyed by the base surface's identity (upstream's pointer; one key per
/// surface object, never per copy), a lookup that hits returns the cached surface's
/// handle (the runner seeds the cache with each surface mapped to itself, as the C++
/// shim seeds `m_vSurfExtSurf`, so a hit returns the base handle), and an extension made
/// on a miss is stored once and its handle returned again on later hits.
/// [`OffsetExecutive::offset_edgeuse`] follows `GetOffsetEdgeuse`'s exactly-one rule.
pub trait OffsetExecutive<O> {
    /// `GetInset`.
    fn inset(&self) -> bool;
    /// `GetExtendConvexEdges`.
    fn extend_convex_edges(&self) -> bool;
    /// `GetShellFaces`: the faces to shell rather than offset (empty without a list).
    fn shell_faces(&self) -> Vec<O>;
    /// `GetOffsetOperation() == SM_OO_LOCAL_OPERATION`: whether the executive runs a
    /// local operation (`SmBrep::LocalOperation`); `false` unless the adapter says so.
    fn is_local_operation(&self) -> bool {
        false
    }
    /// Not upstream (WR-346): whether any inset face's offset collapsed (a convex round
    /// thinner than the wall); `false` unless `Kernel::fixes.offset_collapsed_rounds_mated`.
    fn has_collapsed_faces(&self) -> bool {
        false
    }
    /// Not upstream (WR-346, `IsCollapsedFace`): whether original face `face` is an
    /// inset face whose offset collapsed.
    fn is_collapsed_face(&self, face: O) -> bool {
        let _ = face;
        false
    }
    /// `GetOffsetEdgeuse`: the offset edge use related to `original`, only when exactly
    /// one is (`m_vEUToEU` is a many-to-many `SmRelation`; none or several give `None`).
    fn offset_edgeuse(&self, original: O) -> Option<O>;
    /// `GetCreateExtendedSurface(pBase, rUVDomainOfOffset, rpExtSurface)`: the extension
    /// of `base` cached by the executive (`m_vSurfExtSurf`), created on a miss by
    /// extending `base` by five offset distances
    /// (`m_rGeometryCreation.GetOffsetDistance() * 5.0`, G1-ruled) with
    /// [`crate::offset::offset_executive::p1::OffsetExecutive::get_create_extended_surface`],
    /// and its natural UV domain. A newly made extension is stored where `brep` resolves
    /// the returned handle; a hit returns the cached handle again.
    ///
    /// # Errors
    ///
    /// The extension's errors, and [`SolidError::InvalidInput`] for a `base` that does not
    /// resolve to a surface (upstream `NER(pBase)`).
    fn get_create_extended_surface<B: OffsetBrep<O>>(
        &mut self,
        k: &Kernel,
        brep: &mut B,
        base: O,
    ) -> Result<(O, Extent2d)>;
}

/// What the unit reads from and writes to the original and offset B-reps,
/// implemented by their owner on top of [`ClassGeometry`] / [`ProtoGeometry`] with
/// the ported topology functions each method names. A method returns `Ok(None)` or
/// [`SolidError::InvalidInput`] where upstream would dereference null.
///
/// # Implementing this trait
///
/// The only implementation today is the `offset.offset_geometry_creation` difftest
/// runner's (`Two` in `crates/difftest/src/units/offset_offset_geometry_creation.rs`),
/// which is compared against upstream; a library-side adapter (for
/// [`crate::offset::offset_executive::p1::OffsetExecutive`]'s `EdgeuseOffset` /
/// `VertexOffset` calls) must make the same choices where a method's upstream call has
/// side effects:
/// - For [`OffsetBrep::edge_set_curve`], a copy of the curve through
///   `topology::edge::set_curve` with `SetCurveOptions { delete_old_curve: true,
///   dbg_warn_leaks: true, call_notify: true }` and the B-rep's notification environment
///   (`topology::edge::NotifyEnv`: its topology records, attribute store and object
///   keys), then `Curve::set_owner` on the edge's new curve with the edge's key.
/// - For [`OffsetBrep::set_edge_tolerance`], `topology::edge::set_tolerance(.., true, true)`
///   (`bUpdateOnlyIfLarger`, `bCascadeToBndries`: upstream's defaults).
/// - For [`OffsetBrep::edgeuse_set_uv_trim_curve`], `topology::edgeuse::p1::set_uv_trim_curve`
///   with a new curve (or null), the given distance and `true` (delete the existing UV
///   trim curve), no drop-curve record.
/// - For [`OffsetBrep::set_vertex_point`], `topology::vertex::set_point` with the same
///   notification environment (vertex uses keyed as the B-rep's topology records).
/// - For [`OffsetBrep::insert_curve`] / [`OffsetBrep::insert_surface`], new objects get
///   handles of their own; they are never attached to topology by the store.
/// - The surfaces of faces keep their face as owner (`Surface::set_owner`), so that a
///   trimmed surface cache built on them reaches the face as upstream's does.
#[allow(clippy::too_many_arguments)]
pub trait OffsetBrep<O>: ProtoGeometry<O> {
    /// Prepares the surface cache when a numerical solve actually requests it.
    /// Backends with owning topology override this to build the real trimmed
    /// cache; the default retains the existing opaque-owner deferral.
    fn prepare_surface_solve_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>>
    where
        O: Copy + PartialEq,
    {
        let mut cache = crate::surface::surface::p1::get_surface_cache(k, surface, handle)?;
        // the solve's GetTree: a reused cache whose tree is not built builds it now
        if cache.tree().is_none() {
            cache.get_tree(k)?;
        }
        Ok(cache)
    }

    /// The surface cache as `smsurf_GetSurfaceCache` hands it to a reader of its state
    /// bits only, without the solve's `GetTree` (`GlobalPointSolve`'s fast-drop face
    /// test). Backends that keep face caches override this; the default is the
    /// solve's cache.
    fn prepare_surface_state_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>>
    where
        O: Copy + PartialEq,
    {
        self.prepare_surface_solve_cache(k, surface, handle)
    }

    /// Read-only face queries after cache preparation. One service resolves all
    /// owning B-reps in this operation's object space.
    fn prepared_face_queries(&self) -> Option<&dyn FaceQueries<O>> {
        None
    }

    /// Stores a new curve and returns its handle; `None` when the store is full.
    fn insert_curve(&mut self, curve: Curve) -> Option<O>;
    /// Stores a new surface and returns its handle; `None` when the store is full.
    fn insert_surface(&mut self, surface: Surface) -> Option<O>;
    /// Mutable access to the surface behind a handle.
    fn surface_mut(&mut self, o: O) -> Option<&mut Surface>;
    /// `GetBrep()->GetTolerance()` of a topology object (`Brep::tolerance`).
    fn brep_tolerance(&self, topo: O) -> Result<f64>;
    /// `SmFaceuse::GetFace` (`topology::edgeuse::p1::faceuse_face`).
    fn faceuse_face(&self, fu: O) -> Result<Option<O>>;
    /// `SmFaceuse::GetOrientation` (`topology::edgeuse::p1::faceuse_orientation`).
    fn faceuse_orientation(&self, fu: O) -> Result<OrientType>;
    /// `SmFace::GetSurface` (`topology::face::p2::surface`).
    fn face_surface(&self, face: O) -> Result<Option<O>>;
    /// `SmFace::GetUVDomain` (`topology::face::p2::uv_domain`).
    fn face_uv_domain(&self, face: O) -> Result<Extent2d>;
    /// `SmFace::SetUVDomain` (`topology::face::p2::set_uv_domain`).
    fn set_face_uv_domain(&mut self, face: O, domain: Extent2d) -> Result<()>;
    /// `SmFace::CalculateUVDomainFromUVTrimCurves(rDomain)` with upstream's defaults
    /// (`bLimitDomain = TRUE`, no loop gaps;
    /// `topology::face::p1::calculate_uv_domain_from_uv_trim_curves`). It makes the face's
    /// UV trim curves.
    fn face_uv_domain_from_uv_trim_curves(&mut self, k: &Kernel, face: O) -> Result<Extent2d>;
    /// `SmEdge::GetCurve` (`topology::edge::curve`).
    fn edge_curve(&self, edge: O) -> Result<Option<O>>;
    /// `SmEdge::GetInterval` (`topology::edge::interval`).
    fn edge_interval(&self, edge: O) -> Result<Extent1d>;
    /// `SmEdge::SetInterval` (`topology::edge::set_interval`).
    fn set_edge_interval(&mut self, edge: O, interval: Extent1d) -> Result<()>;
    /// `SmEdge::GetTolerance` (`topology::edge::tolerance`).
    fn edge_tolerance(&self, edge: O) -> Result<f64>;
    /// `SmEdge::SetTolerance(tol)` with upstream's defaults (`topology::edge::set_tolerance`).
    fn set_edge_tolerance(&mut self, k: &Kernel, edge: O, tolerance: f64) -> Result<()>;
    /// `SmEdge::SetCurve(curve, TRUE)`, `curve->SetOwner(edge)` (`topology::edge::set_curve`,
    /// `Curve::set_owner`): the edge takes the curve (its old curve and the edge
    /// uses' UV trim curves are dropped).
    fn edge_set_curve(&mut self, k: &Kernel, edge: O, curve: O) -> Result<()>;
    /// `SmBrep::MateLaminaEdgeToSurface(pEdge, crSurface, dSurfaceExtensionFactor,
    /// dUVCurveExtensionFactor)` of the edge's B-rep (`topology::brep::p2`, with the
    /// owner's arenas and notification state).
    fn brep_mate_lamina_edge_to_surface(
        &mut self,
        k: &Kernel,
        edge: O,
        surface: O,
        surface_extension_factor: f64,
        uv_curve_extension_factor: f64,
    ) -> Result<()>;
    /// `SmBrep::ReplaceSurface(pFace, pNewSurface, bAddToDeletedList, NULL,
    /// bCreateUVTrimCurves)` of the face's B-rep (`topology::brep::p2`): the face takes
    /// the surface behind `surface`.
    fn brep_replace_surface(
        &mut self,
        k: &Kernel,
        face: O,
        surface: O,
        add_to_deleted_list: bool,
        create_uv_trim_curves: bool,
    ) -> Result<()>;
    /// `SmEdge::GetFaces` (`topology::edge::faces`).
    fn edge_faces(&self, edge: O) -> Result<Vec<O>>;
    /// `SmEdge::GetEdgeuses` (`topology::edge::edgeuses`).
    fn edge_edgeuses(&self, edge: O) -> Result<Vec<O>>;
    /// `SmEdge::IsLamina` (`topology::edge::is_lamina`).
    fn edge_is_lamina(&self, edge: O) -> Result<bool>;
    /// `SmEdge::IsSeam(*face->GetSurface())` (`topology::edge::is_seam_of_surface`).
    fn edge_is_seam_of_face_surface(&mut self, k: &Kernel, edge: O, face: O) -> Result<bool>;
    /// `SmEdge::GetVertex` (`topology::edge::vertex`).
    fn edge_vertex(&self, edge: O) -> Result<Option<O>>;
    /// `SmEdge::GetOtherVertex` (`topology::edge::other_vertex`).
    fn edge_other_vertex(&self, edge: O, vertex: O) -> Result<Option<O>>;
    /// `SmEdge::ClosestPoint` (`query::edge::closest_point`): `None` without a
    /// solution (`rbSuccess == FALSE`).
    fn edge_closest_point(
        &self,
        k: &Kernel,
        edge: O,
        point: Point3,
    ) -> Result<Option<ClosestPoint>>;
    /// `SmEdge::CreateTrimmedNURBSCurve` (`topology::edge::create_trimmed_nurbs_curve`):
    /// the handle of the new curve, `None` when the edge's curve is not a B-spline.
    fn edge_create_trimmed_nurbs_curve(&mut self, k: &Kernel, edge: O) -> Result<Option<O>>;
    /// `SmVertex::GetPoint` (`topology::vertex::point`).
    fn vertex_point(&self, vertex: O) -> Result<Point3>;
    /// `SmVertex::SetPoint` (`topology::vertex::set_point`).
    fn set_vertex_point(&mut self, k: &Kernel, vertex: O, point: Point3) -> Result<()>;
    /// `SmVertex::IsLaminaVertex` (`topology::vertex::is_lamina_vertex`).
    fn vertex_is_lamina(&self, vertex: O) -> Result<bool>;
    /// `SmVertex::AreEdgesTangent(e1, e2, angTolDeg)` (`topology::vertex::are_edges_tangent`).
    fn vertex_are_edges_tangent(&self, vertex: O, e1: O, e2: O, ang_tol_deg: f64) -> Result<bool>;
    /// `SmEdgeuse::GetEdge` (`topology::edgeuse::p1::edge`).
    fn edgeuse_edge(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetMate` (`topology::edgeuse::p1::mate`).
    fn edgeuse_mate(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetRadial` (`topology::edgeuse::p1::radial`).
    fn edgeuse_radial(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetCWEdgeuse` (`topology::edgeuse::p1::cw_edgeuse`).
    fn edgeuse_cw(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetCCWEdgeuse` (`topology::edgeuse::p1::ccw_edgeuse`).
    fn edgeuse_ccw(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetVertexuse()->GetVertex()` (`topology::edgeuse::p1::vertexuse`,
    /// `Vertexuse::vertex`).
    fn edgeuse_vertex(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetFace` (`topology::edgeuse::p1::face`).
    fn edgeuse_face(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetFaceuse` (`topology::edgeuse::p1::faceuse`).
    fn edgeuse_faceuse(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetOrientation` (`topology::edgeuse::p1::orientation`), read only by
    /// the opt-in step walls (`Kernel::fixes.local_operation_step_walls`); an
    /// implementation that does not provide it fails there.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] unless implemented.
    fn edgeuse_orientation(&self, eu: O) -> Result<OrientType> {
        let _ = eu;
        Err(dangling("edge use orientation not provided"))
    }
    /// `SmEdgeuse::GetUVTrimCurve` (`topology::edgeuse::p1::uv_trim_curve`): the handle
    /// of the UV trim curve, if any.
    fn edgeuse_uv_trim_curve(&self, eu: O) -> Result<Option<O>>;
    /// `SmEdgeuse::SetUVTrimCurve(curve, maxDist, TRUE)`
    /// (`topology::edgeuse::p1::set_uv_trim_curve`, deleting the old curve).
    fn edgeuse_set_uv_trim_curve(
        &mut self,
        k: &Kernel,
        eu: O,
        curve: Option<O>,
        max_dist: f64,
    ) -> Result<()>;
    /// `SmEdgeuse::IsTangentSector(tolDeg)` with upstream's other defaults
    /// (`topology::edgeuse::p2::is_tangent_sector`).
    fn edgeuse_is_tangent_sector(&mut self, k: &Kernel, eu: O, tol_deg: f64) -> Result<bool>;
    /// `SmEdgeuse::IsConvexRadialSector(samples)` with upstream's other defaults
    /// (`topology::edgeuse::p2::is_convex_radial_sector`).
    fn edgeuse_is_convex_radial_sector(
        &mut self,
        k: &Kernel,
        eu: O,
        num_samples: u64,
    ) -> Result<bool>;
    /// `SmEdgeuse::IsMarked(eMarkType)` (`topology::topology::Topology::is_marked`).
    fn edgeuse_is_marked(&self, k: &Kernel, eu: O, mark: MarkType) -> Result<bool>;
    /// `SmEdgeuse::NormalizedEvaluate(t, bParameterSpaceEval, rPoint, pOptFirstDeriv,
    /// bOKToMakeUVTrimCurve)` (`topology::edgeuse::p3::normalized_evaluate`): the point
    /// (`want_first_deriv` is `pOptFirstDeriv != NULL`; the derivative is not returned).
    fn edgeuse_normalized_evaluate(
        &mut self,
        k: &Kernel,
        eu: O,
        t: f64,
        parameter_space_eval: bool,
        want_first_deriv: bool,
        ok_to_make_uv_trim_curve: bool,
    ) -> Result<Point3>;
    /// `SmEdgeuse::CreateUVTrimCurve(rdMaxDist, rpUVCurve, pdMaxDeviation,
    /// pOptDropCurveFail)` (`topology::edgeuse::p3::create_uv_trim_curve`): `rdMaxDist`
    /// and the new, unattached UV curve (`want_max_deviation` / `want_drop_curve_fail`:
    /// the optional out-parameters are non-null; their values are not returned).
    fn edgeuse_create_uv_trim_curve(
        &mut self,
        k: &Kernel,
        eu: O,
        want_max_deviation: bool,
        want_drop_curve_fail: bool,
    ) -> Result<(f64, Curve)>;
}

/// The topology and owned-geometry subset used by `VertexOffset`. Keeping this
/// interface separate lets the executive borrow the real source and result arenas
/// without implementing unrelated SSI, classification or editing operations.
pub(crate) trait VertexOffsetBrep<O> {
    /// The source vertex's B-rep tolerance, not the result B-rep's tolerance.
    fn brep_tolerance(&self, vertex: O) -> Result<f64>;
    /// `SmVertex::GetPoint` in the source arena.
    fn vertex_point(&self, vertex: O) -> Result<Point3>;
    /// `SmVertex::IsLaminaVertex` in the source arena.
    fn vertex_is_lamina(&self, vertex: O) -> Result<bool>;
    /// The result edge's incident faces in native traversal order.
    fn edge_faces(&self, edge: O) -> Result<Vec<O>>;
    /// `SmEdge::IsSeam(*face->GetSurface())` in the result arena.
    fn edge_is_seam_of_face_surface(&mut self, k: &Kernel, edge: O, face: O) -> Result<bool>;
    /// The result edge's curve handle.
    fn edge_curve(&self, edge: O) -> Result<Option<O>>;
    /// The result edge's parameter interval.
    fn edge_interval(&self, edge: O) -> Result<Extent1d>;
    /// Resolves a curve without replacing its concrete kind by its B-spline part.
    fn curve(&self, curve: O) -> Result<&Curve>;
    /// Stores an owned trimming curve.
    fn insert_curve(&mut self, curve: Curve) -> Result<O>;
    /// Stores the owned cap surface.
    fn insert_surface(&mut self, surface: Surface) -> Result<O>;
}

/// Adapts the existing full backend so public callers keep using the same path.
struct OffsetVertexBrep<'a, B>(&'a mut B);

impl<O: ClassObject, B: OffsetBrep<O>> VertexOffsetBrep<O> for OffsetVertexBrep<'_, B> {
    fn brep_tolerance(&self, vertex: O) -> Result<f64> {
        self.0.brep_tolerance(vertex)
    }

    fn vertex_point(&self, vertex: O) -> Result<Point3> {
        self.0.vertex_point(vertex)
    }

    fn vertex_is_lamina(&self, vertex: O) -> Result<bool> {
        self.0.vertex_is_lamina(vertex)
    }

    fn edge_faces(&self, edge: O) -> Result<Vec<O>> {
        self.0.edge_faces(edge)
    }

    fn edge_is_seam_of_face_surface(&mut self, k: &Kernel, edge: O, face: O) -> Result<bool> {
        self.0.edge_is_seam_of_face_surface(k, edge, face)
    }

    fn edge_curve(&self, edge: O) -> Result<Option<O>> {
        self.0.edge_curve(edge)
    }

    fn edge_interval(&self, edge: O) -> Result<Extent1d> {
        self.0.edge_interval(edge)
    }

    fn curve(&self, curve: O) -> Result<&Curve> {
        curve_of(self.0, curve)
    }

    fn insert_curve(&mut self, curve: Curve) -> Result<O> {
        store_curve(self.0, curve)
    }

    fn insert_surface(&mut self, surface: Surface) -> Result<O> {
        store_surface(self.0, surface)
    }
}

/// The narrow source/owned-geometry interface needed by `FaceuseOffset`.
pub(crate) trait FaceOffsetBrep<O> {
    /// `SmFaceuse::GetOrientation` in the source arena.
    fn faceuse_orientation(&self, faceuse: O) -> Result<OrientType>;
    /// `SmFaceuse::GetFace` in the source arena.
    fn faceuse_face(&self, faceuse: O) -> Result<Option<O>>;
    /// The source face's surface, preserving its concrete kind and owner.
    fn face_surface(&self, face: O) -> Result<Option<O>>;
    /// Resolves either source geometry or newly owned offset geometry.
    fn surface(&self, surface: O) -> Result<&Surface>;
    /// Stores a newly created surface without attaching it to topology.
    fn insert_surface(&mut self, surface: Surface) -> Result<O>;
    /// Stores a newly created intersection curve without changing its kind.
    fn insert_curve(&mut self, curve: Curve) -> Result<O>;
}

struct OffsetFaceBrep<'a, B>(&'a mut B);

impl<O: ClassObject, B: OffsetBrep<O>> FaceOffsetBrep<O> for OffsetFaceBrep<'_, B> {
    fn faceuse_orientation(&self, faceuse: O) -> Result<OrientType> {
        self.0.faceuse_orientation(faceuse)
    }

    fn faceuse_face(&self, faceuse: O) -> Result<Option<O>> {
        self.0.faceuse_face(faceuse)
    }

    fn face_surface(&self, face: O) -> Result<Option<O>> {
        self.0.face_surface(face)
    }

    fn surface(&self, surface: O) -> Result<&Surface> {
        surface_of(self.0, surface)
    }

    fn insert_surface(&mut self, surface: Surface) -> Result<O> {
        store_surface(self.0, surface)
    }

    fn insert_curve(&mut self, curve: Curve) -> Result<O> {
        store_curve(self.0, curve)
    }
}

// ---- callees through the B-rep, and unported callees ----------------------------------------
//
// Each helper carries every argument of the upstream call (literals and header defaults
// included, as named parameters) plus the kernel and the B-rep / executive it is called
// on, so wiring a stub changes no call site.

/// `pEdgeuse->NormalizedEvaluate(t, bParameterSpaceEval, rPoint, pOptFirstDeriv = NULL,
/// bOKToMakeUVTrimCurve = TRUE)`: the point at the normalised parameter (every call
/// here passes `FALSE`, a 3D evaluation, and no derivative).
fn edgeuse_normalized_evaluate<O>(
    k: &Kernel,
    brep: &mut impl OffsetBrep<O>,
    eu: O,
    t: f64,
    parameter_space_eval: bool,
    want_first_deriv: bool,
    ok_to_make_uv_trim_curve: bool,
) -> Result<Point3> {
    brep.edgeuse_normalized_evaluate(
        k,
        eu,
        t,
        parameter_space_eval,
        want_first_deriv,
        ok_to_make_uv_trim_curve,
    )
}

/// `pEdgeuse->CreateUVTrimCurve(rdMaxDist, rpUVCurve, pdMaxDeviation = NULL,
/// pOptDropCurveFail = NULL)`: the largest distance and the new UV curve (stored, not
/// attached to the edge use: upstream's caller owns it).
fn edgeuse_create_uv_trim_curve<O: ClassObject>(
    k: &Kernel,
    brep: &mut impl OffsetBrep<O>,
    eu: O,
    want_max_deviation: bool,
    want_drop_curve_fail: bool,
) -> Result<(f64, O)> {
    let (max_dist, curve) =
        brep.edgeuse_create_uv_trim_curve(k, eu, want_max_deviation, want_drop_curve_fail)?;
    Ok((max_dist, store_curve(brep, curve)?))
}

/// `pEdge->GetBrep()->MateLaminaEdgeToSurface(pEdge, crSurface, dTolMult = 1.5,
/// dMaxTolMult = 10.0)` (both values passed explicitly upstream).
fn brep_mate_lamina_edge_to_surface<O>(
    k: &Kernel,
    brep: &mut impl OffsetBrep<O>,
    edge: O,
    surface: O,
    tol_mult: f64,
    max_tol_mult: f64,
) -> Result<()> {
    brep.brep_mate_lamina_edge_to_surface(k, edge, surface, tol_mult, max_tol_mult)
}

/// `pFace->GetBrep()->ReplaceSurface(pFace, pNewSurface, bAddToDeleteSurfaceList = FALSE,
/// rOptModifiedFaces = NULL, bOptCreateUVTrimCurves = TRUE)`.
fn brep_replace_surface<O>(
    k: &Kernel,
    brep: &mut impl OffsetBrep<O>,
    face: O,
    surface: O,
    add_to_delete_surface_list: bool,
    want_modified_faces: bool,
    create_uv_trim_curves: bool,
) -> Result<()> {
    // rOptModifiedFaces = NULL: the modified faces are not wanted
    let _ = want_modified_faces;
    brep.brep_replace_surface(
        k,
        face,
        surface,
        add_to_delete_surface_list,
        create_uv_trim_curves,
    )
}

/// `SmSurface::GlobalCurveIntersect(domain, curve, interval, tol, solutions)` of the
/// surface and curve behind the handles (unit `surface.surface_p8`).
struct OffsetSurfaceSolveContext<'a, B>(&'a mut B);

impl<O: ClassObject, B: OffsetBrep<O>> SurfaceSolveContext<O> for OffsetSurfaceSolveContext<'_, B> {
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>> {
        self.0.prepare_surface_solve_cache(k, surface, handle)
    }

    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>> {
        self.0.prepare_surface_state_cache(k, surface, handle)
    }

    fn face_queries(&self) -> Option<&dyn FaceQueries<O>> {
        self.0.prepared_face_queries()
    }
}

fn surface_global_curve_intersect<O: ClassObject + SolutionObject>(
    k: &Kernel,
    geometry: &mut impl OffsetBrep<O>,
    surface: O,
    domain: &Extent2d,
    curve: O,
    interval: &Extent1d,
    tol: f64,
) -> Result<SolutionArray<O>> {
    let c = curve_of(geometry, curve)?.clone_for_evaluation()?;
    let s = surface_of(geometry, surface)?.clone_for_evaluation()?;
    // `GlobalCurveIntersect` reaches the surface's cache through
    // `smsurf_GetSurfaceCache` where it needs it: its degenerate-curve point solve and
    // coincidence check read the cache before its `GetTree` (the fast-drop face test
    // sees an unbuilt cache's state bits), so each step asks the context
    s.global_curve_intersect_with_context(
        k,
        [Some(curve), Some(surface)],
        None,
        &mut OffsetSurfaceSolveContext(geometry),
        domain,
        &c,
        interval,
        tol,
    )
}

/// Approximates a trimmed UV curve on a temporary offset surface, as
/// `SmCrvOnSurf::ApproximateCurve` in `EdgeuseOffset`. The opposite rail supplies
/// the breaks when present; otherwise the UV curve supplies them. Both temporary
/// curves stay owned here rather than being inserted into the caller's store.
fn offset_surface_rail<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    surface: O,
    uv_curve: Curve,
    signed_distance: f64,
    owns_surface: bool,
    knots_source: Option<&BSplineCurve>,
    tol: f64,
    approximation: ApproximateCurveFlags,
) -> Result<BSplineCurve> {
    let base = surface_of(geometry, surface)?.clone_for_evaluation()?;
    let offset = OffsetSurface::new(k, signed_distance, base, owns_surface, OFFSET_SS_UNKNOWN)?;
    let mut rail = crate::curve::crv_on_surf::CrvOnSurf::new(
        k,
        uv_curve,
        Surface::new(SurfaceKind::OffsetSurface(offset)),
        None,
        0,
    )?;
    let knots = match knots_source {
        Some(curve) => curve.knots(None)?.0,
        None => rail.uv_curve().knots(k, None)?.0,
    };
    // SmCrvOnSurf::ApproximateCurve explicitly ignores these two flags upstream.
    let _ = (
        approximation.create_analytics,
        approximation.just_copy_b_splines,
    );
    rail.approximate_curve(k, &knots, tol, approximation.match_parameterization)
        .map(|(curve, _)| curve)
}

/// The three trailing flags of `SmCurve::ApproximateCurve(.., bOptCreateAnalytics,
/// bOptMatchParameterization, bJustCopyBSplines)`.
#[derive(Debug, Clone, Copy)]
struct ApproximateCurveFlags {
    /// `bOptCreateAnalytics`.
    create_analytics: bool,
    /// `bOptMatchParameterization`.
    match_parameterization: bool,
    /// `bJustCopyBSplines`.
    just_copy_b_splines: bool,
}

/// `SmCircularCrossSectionFSG sFSG(bApproximateArcs = FALSE, dTolPercentRadius = 0.05,
/// bMirror = FALSE, bComplement = FALSE)` then `sFSG.CreateSurfaceFromCurves(ctx,
/// *pCenterLine, *pRail1, *pRail2, tol, bNeedG1Continuity = FALSE, NULL, NULL, NULL, NULL,
/// pNewSurface)`; the four NULLs are the optional surfaces and UV curves of the rails
/// (`blend.fillet_standard_solver_p1`'s generator takes none). The curves are non-const
/// upstream, so they are `&mut`; the centre line is passed as the curve itself, so its
/// virtuals dispatch on its kind (`rCenterLine` binds the `SmBSplineCurve` kinds).
fn circular_cross_section_surface(
    k: &Kernel,
    fsg: CircularCrossSectionFsg,
    center_line: &mut Curve,
    rail1: &mut BSplineCurve,
    rail2: &mut BSplineCurve,
    tol: f64,
    need_g1_continuity: bool,
) -> Result<BSplineSurface> {
    fsg.create_surface_from_curves(k, center_line, rail1, rail2, tol, need_g1_continuity)
}

// ---- helpers over handles -------------------------------------------------------------------

/// `SM_CAST_PTR(SmBSplineCurve, curve)` on the curve behind `h`: `None` when the cast
/// fails (the curve is not an `SmBSplineCurve` kind); otherwise the curve itself, whose
/// methods dispatch on its real kind (a line, a conic or a B-spline), as the virtual
/// calls through the `SmBSplineCurve *` do upstream.
fn b_spline_kind_of<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    h: O,
) -> Result<Option<&Curve>> {
    let c = curve_of(geometry, h)?;
    Ok(is_b_spline_kind(c).then_some(c))
}

/// The same cast through the vertex-only backend; concrete curve kinds stay intact.
fn vertex_b_spline_kind_of<O>(geometry: &impl VertexOffsetBrep<O>, h: O) -> Result<Option<&Curve>> {
    let c = geometry.curve(h)?;
    Ok(is_b_spline_kind(c).then_some(c))
}

/// Whether `SM_CAST_PTR(SmBSplineCurve, c)` succeeds: `c` is an `SmBSplineCurve` or one
/// of its derived kinds (line, ellipse, hyperbola, parabola).
fn is_b_spline_kind(c: &Curve) -> bool {
    matches!(
        c.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

/// The B-spline data of the curve behind `h` as a plain B-spline curve
/// ([`b_spline_part`]): `None` when it is not an `SmBSplineCurve` kind.
fn b_spline_of<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    h: O,
) -> Result<Option<BSplineCurve>> {
    b_spline_part(curve_of(geometry, h)?)
}

/// `SmBSplineCurve::Copy(crContext, SmBSplineCurve*&)`, the non-virtual overload a
/// call on an `SmBSplineCurve*` binds: `new SmBSplineCurve(*this)`, a plain B-spline
/// copy of the curve's B-spline part whatever its kind (an `SmLine` or conic loses
/// its analytic data; a NURB that is not made yet stays unmade, as the copy
/// constructor copies only an existing one). A curve that is not an `SmBSplineCurve`
/// cannot be behind such a pointer.
fn b_spline_overload_copy(c: &Curve) -> Result<Curve> {
    Ok(Curve::from(match c.kind() {
        CurveKind::BSpline(b) => b.clone(),
        CurveKind::Line(l) => l.base.clone(),
        CurveKind::Ellipse(e) => e.base().clone(),
        CurveKind::Hyperbola(h) => h.base().clone(),
        CurveKind::Parabola(p) => p.base().clone(),
        _ => return Err(dangling("UV trim curve is not a B-spline curve")),
    }))
}

/// `new SmBSplineCurve(*pCurve)`, or the `SmBSplineCurve` data a static B-spline
/// routine reads: a plain B-spline copy of the NURB part of an `SmBSplineCurve` kind
/// (a line's NURB is made when it has none); `None` for the other kinds. Only for the
/// sites where upstream copies or reads the base object, never for the virtual calls.
fn b_spline_part(c: &Curve) -> Result<Option<BSplineCurve>> {
    Ok(match c.kind() {
        CurveKind::BSpline(b) => Some(b.clone()),
        CurveKind::Line(l) => Some(l.base_with_nurb()?.into_owned()),
        CurveKind::Ellipse(e) => Some(e.base().clone()),
        CurveKind::Hyperbola(h) => Some(h.base().clone()),
        CurveKind::Parabola(p) => Some(p.base().clone()),
        _ => None,
    })
}

/// The NURB part of an `SmBSplineCurve` kind, in place: the static
/// `SmBSplineCurve::MakeCurvesCompatible` edits the NURB of whatever kind it is given
/// (a line's NURB is made first when it has none); `None` for the other kinds.
fn b_spline_part_mut(c: &mut Curve) -> Result<Option<&mut BSplineCurve>> {
    Ok(match c.kind_mut() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => {
            let made = match l.base_with_nurb()? {
                std::borrow::Cow::Owned(b) => Some(b),
                std::borrow::Cow::Borrowed(_) => None,
            };
            if let Some(b) = made {
                *l.base_mut() = b;
            }
            Some(l.base_mut())
        }
        CurveKind::Ellipse(e) => Some(e.base_mut()),
        CurveKind::Hyperbola(h) => Some(h.base_mut()),
        CurveKind::Parabola(p) => Some(p.base_mut()),
        _ => None,
    })
}

/// The curve behind `h` (a dereference upstream).
fn curve_of<O: ClassObject>(geometry: &impl ClassGeometry<O>, h: O) -> Result<&Curve> {
    geometry
        .curve(h)
        .ok_or(dangling("curve handle cannot be resolved"))
}

/// The surface behind `h` (a dereference upstream).
fn surface_of<O: ClassObject>(geometry: &impl ClassGeometry<O>, h: O) -> Result<&Surface> {
    geometry
        .surface(h)
        .ok_or(dangling("surface handle cannot be resolved"))
}

/// `SmBSplineCurve::MakeNonRational` (fork use, `CreateStepWall`): every weighted control
/// point divided through by its weight and made non-rational.
fn make_non_rational(curve: &mut BSplineCurve) -> Result<()> {
    let nurb = curve.nurb.as_mut().ok_or(dangling("curve without NURB"))?;
    for cp in nurb.cpts_mut() {
        if cp.w != NOW {
            let w = cp.w;
            cp.x /= w;
            cp.y /= w;
            if cp.z != NOZ {
                cp.z /= w;
            }
            cp.w = NOW;
        }
    }
    Ok(())
}

/// Stores `curve` and returns its handle.
fn store_curve<O: ClassObject>(brep: &mut impl OffsetBrep<O>, curve: Curve) -> Result<O> {
    brep.insert_curve(curve)
        .ok_or(dangling("curve store is full"))
}

/// Stores `surface` and returns its handle.
fn store_surface<O: ClassObject>(brep: &mut impl OffsetBrep<O>, surface: Surface) -> Result<O> {
    brep.insert_surface(surface)
        .ok_or(dangling("surface store is full"))
}

/// `SmPoint3d::DistanceBetween`.
fn distance(a: Point3, b: Point3) -> f64 {
    a.distance(b)
}

/// `smos_Max`: `a > b ? a : b`.
fn smos_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min`: `a < b ? a : b`.
fn smos_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// The fork's `sm_CreateAnalyticExtension` (source fork@fddc366 with
/// @98ab1b0, behind [`crate::kernel::UpstreamFixes::offset_analytic_mate_edges`]): a
/// plane, cylinder, cone, sphere or torus copied and extended as itself where the
/// offset's extended surfaces (`GetCreateExtendedSurface`, tangent (G1R) B-spline
/// extensions) still are that surface: a plane all round, a cylinder or cone along its
/// rulings (a cone up to its apex), each side by `dist` (3D) unless it is a
/// singularity. Not round a turn, where the tangent extension leaves the surface.
/// `None` for any other surface.
fn create_analytic_extension(k: &Kernel, surface: &Surface, dist: f64) -> Result<Option<Surface>> {
    let ty = surface.type_code();
    let plane = ty == PLANE_TYPE;
    if !plane && ty != CYLINDER_TYPE && ty != CONE_TYPE && ty != SPHERE_TYPE && ty != TORUS_TYPE {
        return Ok(None);
    }

    let domain = surface.step_uv_domain();
    let max = if plane {
        domain
    } else {
        surface.max_analytic_domain(k)?
    };
    let (mut new_min, mut new_max) = (domain.min(), domain.max());
    for dir in 0..2 {
        // 0 = u, 1 = v
        if !plane && (dir == 0 || ty == SPHERE_TYPE || ty == TORUS_TYPE) {
            continue; // a turn: the faces' extensions are not the surface there
        }
        let lo = if dir == 0 {
            domain.min().x
        } else {
            domain.min().y
        };
        let hi = if dir == 0 {
            domain.max().x
        } else {
            domain.max().y
        };
        let mid = if dir == 0 {
            0.5 * (domain.min().y + domain.max().y)
        } else {
            0.5 * (domain.min().x + domain.max().x)
        };
        let mut grow = [0.0_f64, 0.0];
        for (side, g) in grow.iter_mut().enumerate() {
            let at = if side == 0 { lo } else { hi };
            let uv = if dir == 0 {
                Point2::new(at, mid)
            } else {
                Point2::new(mid, at)
            };
            let from = if side == 0 { Side::Left } else { Side::Right };
            let ders = surface.evaluate_step(
                k,
                uv,
                1,
                1,
                EvaluateFlags {
                    u_side: from,
                    v_side: from,
                    only_upper_half: false,
                    non_zero_tangents: true,
                },
            )?;
            // upstream's [D, Dv, Du, Duv]: Du is ders[1][0], Dv is ders[0][1]
            let der = if dir == 0 {
                ders.get(1).and_then(|r| r.first())
            } else {
                ders.first().and_then(|r| r.get(1))
            };
            let speed = der.ok_or(dangling("surface derivative"))?.length();
            if speed > EFF_ZERO {
                // no extension through a singularity
                *g = dist / speed;
            }
        }
        let [grow_lo, grow_hi] = grow;
        let mut new_lo = lo - grow_lo;
        let mut new_hi = hi + grow_hi;
        if !plane {
            // a cylinder's or cone's rulings, as far as the cone reaches (its apex)
            let (max_lo, max_hi) = (max.min().y, max.max().y);
            new_lo = smos_max(new_lo, smos_min(lo, max_lo));
            new_hi = smos_min(new_hi, smos_max(hi, max_hi));
        }
        if dir == 0 {
            new_min.x = new_lo;
            new_max.x = new_hi;
        } else {
            new_min.y = new_lo;
            new_max.y = new_hi;
        }
    }

    let mut copy = surface.copy(k)?;
    copy.adjust_step_uv_domain(k, &Extent2d::from_points(new_min, new_max))?;
    Ok(Some(copy))
}

/// The fork's `sm_ExactPiece` (source fork@fddc366, behind
/// [`crate::kernel::UpstreamFixes::offset_analytic_mate_edges`]): the exact line or
/// conic of `exact` that `marched` (a marched intersection piece) approximates, as a
/// copy over the same span: from `marched`'s start point to its end point, the way
/// round through its middle (all of a closed conic for a closed piece). `None` when no
/// line or conic of `exact` runs through all three points within `tol`.
fn exact_piece(k: &Kernel, marched: &Curve, exact: &[Curve], tol: f64) -> Result<Option<Curve>> {
    let ivl = marched.natural_interval();
    // start, middle, end
    let p = [
        marched.evaluate_point(k, ivl.min())?,
        marched.evaluate_point(k, ivl.evaluate(0.5))?,
        marched.evaluate_point(k, ivl.max())?,
    ];
    for candidate in exact {
        let line = candidate.is_kind_of(LINE_TYPE);
        if !line && !candidate.is_kind_of(ELLIPSE_TYPE) {
            continue;
        }
        let mut t = [0.0_f64; 3];
        let mut on = true;
        for (tk, pk) in t.iter_mut().zip(p.iter()) {
            if !on {
                break;
            }
            *tk = candidate.step_inversion(k, *pk)?;
            let on_curve = candidate.evaluate_step_point(k, *tk)?;
            on = distance(on_curve, *pk) <= tol;
        }
        if !on {
            continue;
        }
        let [t0, t1, t2] = t;
        let [p0, _, p2] = p;
        let span = if line {
            Extent1d::from_min_max(smos_min(t0, t2), smos_max(t0, t2))
        } else {
            // a conic: its STEP parameter is its angle in degrees
            let turn = 360.0;
            // `angle` moved into (from, from + turn]; the turns are counted so that a
            // parameter far outside a turn cannot loop without bound (the angles are
            // a conic's STEP parameters, within a few turns)
            let after = |mut angle: f64, from: f64| -> Result<f64> {
                let mut turns = 0_u32;
                while angle <= from {
                    angle += turn;
                    turns += 1;
                    if turns > MAX_EXACT_PIECE_TURNS {
                        return Err(SolidError::NotConverged {
                            operation: OP_EXACT_PIECE,
                            iterations: turns,
                        });
                    }
                }
                while angle > from + turn {
                    angle -= turn;
                    turns += 1;
                    if turns > MAX_EXACT_PIECE_TURNS {
                        return Err(SolidError::NotConverged {
                            operation: OP_EXACT_PIECE,
                            iterations: turns,
                        });
                    }
                }
                Ok(angle)
            };
            if distance(p0, p2) <= tol {
                Extent1d::from_min_max(t0, t0 + turn)
            } else if after(t1, t0)? < after(t2, t0)? {
                Extent1d::from_min_max(t0, after(t2, t0)?)
            } else {
                Extent1d::from_min_max(t2, after(t0, t2)?)
            }
        };
        let mut copy = candidate.copy(k)?;
        match copy.kind_mut() {
            CurveKind::Line(l) => l.adjust_step_interval(&span)?,
            CurveKind::Ellipse(e) => e.adjust_step_interval(k, &span, None)?,
            // (only lines and conics reach here)
            _ => return Err(sm_err(OP_EXACT_PIECE)),
        }
        return Ok(Some(copy));
    }
    Ok(None)
}

/// The fork's `SmOffsetGeometryCreation::MakeEdgesOnAnalyticsExact`
/// (source fork@cd85d34, behind
/// [`crate::kernel::UpstreamFixes::shell_edges_on_analytics_exact`]; WR-545): each edge
/// of `b` whose curve is not analytic (an intersection marched and approximated)
/// between two faces on analytic surfaces takes the exact line or conic the surfaces
/// meet on through its ends and middle (`exact_piece`), running as the edge did,
/// unless that curve fails its own validity check. The faces of the changed edges get
/// their UV trim curves made again. Notifications go through `stores`.
///
/// # Errors
///
/// [`SolidError::Unported`] from the intersections; those of the evaluations, of the
/// curve replacement and of `CreateUVTrimCurves`.
pub fn make_edges_on_analytics_exact(
    k: &mut Kernel,
    b: &mut Brep,
    stores: &mut NotificationStores<'_>,
) -> Result<()> {
    let tol = b.tolerance();
    let ssi_tol = tol / 4.0;
    let deg20_rad = 20.0 * PI / 180.0;
    let use_surface_edges = [false, false];
    let mut changed: Vec<FaceId> = Vec::new();
    for e in bp1::edges(b, None)? {
        let Some(curve) = topo_edge::curve(b, e)? else {
            continue;
        };
        if curve.is_analytic() {
            continue;
        }
        let faces = topo_edge::faces(b, e, None)?;
        let &[f0, f1] = faces.as_slice() else {
            continue;
        };
        if f0 == f1 {
            continue;
        }
        let (Some(s1), Some(s2)) = (b.face(f0)?.surface.as_ref(), b.face(f1)?.surface.as_ref())
        else {
            continue;
        };
        if !s1.is_analytic()? || !s2.is_analytic()? {
            continue;
        }
        // The surfaces' own intersection: none found (tangent surfaces) leaves the edge.
        let exact = match s1.global_surface_intersect(
            k,
            &s1.natural_uv_domain_with_nurb(k)?,
            s2,
            &s2.natural_uv_domain_with_nurb(k)?,
            use_surface_edges,
            Some(ssi_tol),
            Some(deg20_rad),
        ) {
            Ok(exact) => exact,
            Err(err @ SolidError::Unported { .. }) => return Err(err),
            Err(_) => continue,
        };
        let trimmed = match topo_edge::create_trimmed_nurbs_curve(k, b, e) {
            Ok(Some(trimmed)) => trimmed,
            Err(err @ SolidError::Unported { .. }) => return Err(err),
            Ok(None) | Err(_) => continue,
        };
        let Some(mut piece) = exact_piece(k, &trimmed, &exact.curves_3d, tol)? else {
            continue;
        };
        // The piece runs as the edge does (exact_piece may give it end to start).
        let old = trimmed.evaluate(k, trimmed.natural_interval().evaluate(0.5), 1, true, true)?;
        let new = piece.evaluate(k, piece.natural_interval().evaluate(0.5), 1, true, true)?;
        let (Some(old_tangent), Some(new_tangent)) = (old.get(1), new.get(1)) else {
            return Err(sm_err(OP_ANALYTICS_EXACT));
        };
        if old_tangent.dot(*new_tangent) < 0.0 {
            let ivl = piece.natural_interval();
            piece.reverse_parameterization(k, &ivl)?;
        }
        // A conic whose own parameters SMLib cannot hold consistently (its polar
        // conversion off by more than its own zero, far from the origin) fails its
        // validity check: the edge keeps its marched curve rather than take it.
        // (AssertValid(NULL, SM_LEVEL_2, SM_NO_WALK) of the line or conic)
        let valid = match piece.kind_mut() {
            CurveKind::Line(l) => l.assert_valid(k, None, false, ObjectKey(0))?,
            CurveKind::Ellipse(c) => c.assert_valid(k, None, false, ObjectKey(0), ObjectKey(0))?,
            // (exact_piece gives only lines and conics)
            _ => return Err(sm_err(OP_ANALYTICS_EXACT)),
        };
        if !valid {
            continue;
        }
        brep_replace_curve(k, b, e, piece, stores)?;
        for f in [f0, f1] {
            if !changed.contains(&f) {
                changed.push(f);
            }
        }
    }
    for f in changed {
        create_uv_trim_curves(k, b, f, true, None)?;
    }
    Ok(())
}

/// `SmBrep::ReplaceCurve(pEdge, pNewCurve)`: the B-rep's pre-edit notification,
/// `SmEdge::Replace3DCurve` (`SetCurve(curve, TRUE)`, the curve owned by the edge and
/// the edge's interval the curve's natural interval) and `SmEdge::RemoveUVTrimCurves`
/// (`ClearUVCaches`: the edge's pre-edit notification, every edge use's UV trim curve
/// deleted and its gap to the next edge use reset, every vertex use's vertex/edge gap
/// reset).
fn brep_replace_curve(
    k: &mut Kernel,
    b: &mut Brep,
    e: EdgeId,
    mut curve: Curve,
    stores: &mut NotificationStores<'_>,
) -> Result<()> {
    let interval = curve.natural_interval();
    NotifyState::from_kernel_stores(stores).with_kernel_env(k, |_k, env| {
        let this = Some(brep_p2::NotifyArg::Object(brep_p2::brep_ref(b)));
        brep_p2::notify(b, env, None, NotifyOperation::PreEdit, [this, None, None])?;

        // Replace3DCurve
        let owner = (env.key_of)(EntityRef::Edge(e));
        let _ = curve.set_owner(Some(owner), Some(brep_p2::brep_ref(b)));
        let options = SetCurveOptions {
            delete_old_curve: true,
            ..SetCurveOptions::default()
        };
        let _ = topo_edge::set_curve(b, e, Some(curve), options, env, [None, None])?;
        topo_edge::set_interval(b, e, &interval)?;

        // RemoveUVTrimCurves (ClearUVCaches)
        let edge_ref = (env.key_of)(EntityRef::Edge(e));
        let brep_ref = brep_p2::brep_ref(b);
        topo_edge::notify(
            b,
            e,
            env,
            NotifyOperation::PreEdit,
            [Some(edge_ref), Some(brep_ref), None],
        )?;
        for eu in topo_edge::edgeuses(b, e, None, None)? {
            let _ = eu1::set_uv_trim_curve(b, eu, eu1::UvTrimCurveArg::Null, 0.0, true, None)?;
            b.edgeuse_mut(eu)?.edge_ccw_edge_gap = EdgeEdgeGap::new();
        }
        for vu in topo_edge::vertexuses(b, e, None)? {
            let vu = vu.ok_or(dangling("edge use without vertex use"))?;
            b.vertexuse_mut(vu)?.vertex_edge_gap = VertexEdgeGap::new();
        }
        Ok(())
    })
}

/// `pSur->CreateOffsetSurface(ctx, signed_distance, approx_tol_3d, rNewSurface)`: the
/// virtual call ([`Surface::create_offset_surface`] dispatches on the kind: the plane,
/// surface of revolution and offset surface overrides, else `SmSurface`'s, which
/// `SmBSplineSurface`'s calls: an `SmOffsetSurface` owning a copy). `None` is
/// upstream's success with `rNewSurface == NULL` (the `SmTorus` override, whose minor
/// radius vanishes, not reached: no [`SurfaceKind`] is a torus yet; the `SmCone`
/// override when both offset radii collapse).
fn create_offset_surface(
    k: &Kernel,
    s: &Surface,
    signed_distance: f64,
    approx_tol_3d: f64,
) -> Result<Option<Surface>> {
    s.create_offset_surface(k, signed_distance, approx_tol_3d)
}

/// `IsArc(samples, tol, ..)` of a plain B-spline curve (a rail or an arc this module
/// built).
fn is_arc(k: &Kernel, c: &BSplineCurve, num_samples: u64, tol: f64) -> Result<Option<ArcData>> {
    Curve::from(c.clone()).is_arc(k, num_samples, tol)
}

// ---- the geometry creation --------------------------------------------------------------------

/// The offset geometry creator (`SmOffsetGeometryCreation`).
#[derive(Debug, Clone, PartialEq)]
pub struct OffsetGeometryCreation {
    /// `m_dOffsetDistance`.
    offset_distance: f64,
    /// `m_dThisApproxTol3d`.
    this_approx_tol_3d: f64,
    /// `m_dTangencyTolRadians`.
    tangency_tol_radians: f64,
    /// `m_bMakeAnalytics`.
    make_analytics: bool,
}

impl OffsetGeometryCreation {
    /// `SmOffsetGeometryCreation(dOffsetDistance, dThisApproxTol3d)`: tangency
    /// tolerance 2 degrees, analytics made.
    #[must_use]
    pub fn new(offset_distance: f64, this_approx_tol_3d: f64) -> Self {
        Self {
            offset_distance,
            this_approx_tol_3d,
            tangency_tol_radians: 2.0 * PI / 180.0,
            make_analytics: true,
        }
    }

    /// The same creator with the angular tolerance `tangency_tol_radians`, which an
    /// upstream creator subclass may override (`m_dTangencyTolRadians`).
    #[must_use]
    pub const fn with_tangency_tol_radians(mut self, tangency_tol_radians: f64) -> Self {
        self.tangency_tol_radians = tangency_tol_radians;
        self
    }

    /// `GetThisApproxTol3d` (`SM_ASSERT_TOL` only reports).
    #[must_use]
    pub const fn this_approx_tol_3d(&self) -> f64 {
        self.this_approx_tol_3d
    }

    /// `GetTangencyTolRadians`.
    #[must_use]
    pub const fn tangency_tol_radians(&self) -> f64 {
        self.tangency_tol_radians
    }

    /// `GetTangencyTolDegrees`.
    #[must_use]
    pub fn tangency_tol_degrees(&self) -> f64 {
        self.tangency_tol_radians * 180.0 / PI
    }

    /// `GetOffsetDistance`.
    #[must_use]
    pub const fn offset_distance(&self) -> f64 {
        self.offset_distance
    }

    /// `m_bMakeAnalytics`.
    #[must_use]
    pub const fn make_analytics(&self) -> bool {
        self.make_analytics
    }

    /// `SetMakeAnalytics`.
    pub fn set_make_analytics(&mut self, make_analytics: bool) {
        self.make_analytics = make_analytics;
    }

    /// `SmOffsetGeometryCreation::FaceuseOffset`: the offset surface of the face use's
    /// face (offset against the surface normal for an opposite face use), made
    /// analytic when asked and possible, and, unless `skip_self_intersection`, its
    /// self-intersections. `None` when no offset surface was made.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the offset surface (`surface.offset_surface`);
    /// `SER` of the analytic copy and the self-intersection;
    /// [`SolidError::InvalidInput`] for a face use without face or surface.
    pub fn faceuse_offset<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        original_faceuse: O,
        skip_self_intersection: bool,
    ) -> Result<(Option<O>, SsiData<O>)> {
        self.faceuse_offset_with_backend(
            k,
            &mut OffsetFaceBrep(brep),
            original_faceuse,
            skip_self_intersection,
        )
    }

    /// Shared face-use algorithm for the full creator and owned executive adapter.
    pub(crate) fn faceuse_offset_with_backend<O: Copy>(
        &self,
        k: &Kernel,
        brep: &mut impl FaceOffsetBrep<O>,
        original_faceuse: O,
        skip_self_intersection: bool,
    ) -> Result<(Option<O>, SsiData<O>)> {
        // init output
        let mut self_intersections = SsiData::default();

        // locals
        let sign = if brep.faceuse_orientation(original_faceuse)? == OrientType::Opposite {
            -1.0
        } else {
            1.0
        };
        let face = brep
            .faceuse_face(original_faceuse)?
            .ok_or(dangling("face use without face"))?;
        let sur = brep
            .face_surface(face)?
            .ok_or(dangling("face without surface"))?;

        // construct an offset surface from this surface
        // (SER_DELETE: the surface is deleted on failure; here its handle is simply not returned)
        let Some(offset) = create_offset_surface(
            k,
            brep.surface(sur)?,
            sign * self.offset_distance,
            self.this_approx_tol_3d / 10.0,
        )?
        else {
            // `Kernel::fixes.offset_missing_surface_fails` (WR-464, #1072): only a collapse
            // is no surface (`Surface::offset_collapses`); any other miss has failed.
            if k.fixes.offset_missing_surface_fails
                && !brep.surface(sur)?.offset_collapses(
                    k,
                    sign * self.offset_distance,
                    self.this_approx_tol_3d / 10.0,
                )?
            {
                return Err(sm_err(OP_FACEUSE));
            }
            return Ok((None, self_intersections));
        };
        let offset_h = brep.insert_surface(offset)?;
        let mut new_surface: Option<O> = Some(offset_h);

        // when working with analytics - replace all current appropriate BSplineSurface
        // with analytic copies when the surface is properly represented
        if self.make_analytics {
            // skip surfaces which are already analytics
            if !brep.surface(offset_h)?.is_analytic()? {
                // see if this surface is secretly an analytic - Copy pS, when possible as an analytic surface
                let mut analytic = brep.surface(offset_h)?.copy_and_add_analytics(k)?;
                if let SurfaceKind::OffsetSurface(offset) = analytic.kind_mut() {
                    // SM_CAST_PTR(SmOffsetSurface, pAnalyticS)
                    offset.set_allow_extension(false);
                }
                // store the copy
                new_surface = Some(brep.insert_surface(analytic)?);
            }
        } // end m_bMakeAnalytics check

        // when only one offset surface was made and checking for self-intersections
        if let (Some(off), false) = (new_surface, skip_self_intersection) {
            // self-intersection locals
            let angle_tol = 20.0 * PI / 180.0;
            let use_surface_edges = [false, false];
            let uv_domain = brep.surface(off)?.natural_uv_domain_with_nurb(k)?;

            // find self intersections
            let ssi = {
                let s = brep.surface(off)?;
                s.global_surface_intersect(
                    k,
                    &uv_domain,
                    s,
                    &uv_domain,
                    use_surface_edges,
                    Some(self.this_approx_tol_3d),
                    Some(angle_tol),
                )?
            };
            for c in ssi.curves_3d {
                self_intersections.curves_3d.push(brep.insert_curve(c)?);
            }
            for c in ssi.surface1_uv_curves {
                let h = match c {
                    Some(c) => Some(brep.insert_curve(c)?),
                    None => None,
                };
                self_intersections.uv_curves1.push(h);
            }
            for c in ssi.surface2_uv_curves {
                let h = match c {
                    Some(c) => Some(brep.insert_curve(c)?),
                    None => None,
                };
                self_intersections.uv_curves2.push(h);
            }
            self_intersections.curve_types = ssi.curve_types;
            self_intersections.deviations = ssi.deviations;
        } // end self-intersection check

        // all done
        Ok((new_surface, self_intersections))
    }

    /// `VertexOffset(ctx, pOriginalVertex, rBoundingEdges, ..)`: the cap surface of a
    /// vertex of the offset from the offset edges bounding it. One bounding edge that
    /// is a seam, or two coincident bounding edges, need no cap (`None`); a vertex
    /// without exactly three bounding edges, or a lamina vertex, gets a sphere (of
    /// revolution of a half circle, or of the arc through a closed bounding arc);
    /// three bounding arcs get the sphere through them (`CreateSphereFromArcs`, with
    /// arcs that miss their neighbours' planes re-made between the circle/plane
    /// intersections), or, when shelling (an arc and two lines), the trimmed plane
    /// ruled between the arc and its centre.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a null vertex; `SM_ERR` when a bounding curve of a
    /// three-edge vertex is not an arc, when an arc/plane intersection is degenerate
    /// or too far, or when the sphere cannot be made; `SER` of the geometry calls;
    /// [`SolidError::InvalidInput`] for a bounding edge without curve or a non-B-spline
    /// curve cast as one.
    #[allow(clippy::too_many_lines)]
    pub fn vertex_offset<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        original_vertex: Option<O>,
        bounding_edges: &[O],
    ) -> Result<VertexOffsetGeometry<O>> {
        self.vertex_offset_with_backend(
            k,
            &mut OffsetVertexBrep(brep),
            original_vertex,
            bounding_edges,
        )
    }

    /// The shared vertex algorithm, also used by the executive's cross-arena adapter.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn vertex_offset_with_backend<O: Copy>(
        &self,
        k: &Kernel,
        brep: &mut impl VertexOffsetBrep<O>,
        original_vertex: Option<O>,
        bounding_edges: &[O],
    ) -> Result<VertexOffsetGeometry<O>> {
        // init output
        let original_vertex = original_vertex.ok_or(SolidError::null_pointer(OP_VERTEX))?;
        let mut out = VertexOffsetGeometry {
            trimming_curves: Vec::new(),
            orients: Vec::new(),
            surface: None,
        };

        // locals
        let tolerance = brep.brep_tolerance(original_vertex)?;

        // If we have no bounding edges or don't have 3, just create a spherical surface
        // and hope that the merge will take care of the rest.
        if bounding_edges.len() == 1 {
            // Check for closed case
            let e = bounding_edges
                .first()
                .copied()
                .ok_or(dangling("edge list"))?;
            let faces = brep.edge_faces(e)?;
            let f = faces
                .first()
                .copied()
                .ok_or(dangling("edge without faces"))?;
            if brep.edge_is_seam_of_face_surface(k, e, f)? {
                // no need for a vertex cap Surface
                return Ok(out);
            }
        } // end just 1 BoundingEdge check

        // Check for case where bounding edges are coincident.
        if bounding_edges.len() == 2 {
            let (e0, e1) = (
                bounding_edges
                    .first()
                    .copied()
                    .ok_or(dangling("edge list"))?,
                bounding_edges
                    .get(1)
                    .copied()
                    .ok_or(dangling("edge list"))?,
            );
            let c1 = brep.edge_curve(e0)?.ok_or(dangling("edge without curve"))?;
            let c2 = brep.edge_curve(e1)?.ok_or(dangling("edge without curve"))?;
            let ivl0 = brep.edge_interval(e0)?;
            let ivl1 = brep.edge_interval(e1)?;
            let param2_start = ivl1.min();
            let param2_end = ivl1.max();
            let max_dist = brep
                .curve(c1)?
                .curve_max_distance_between(
                    k,
                    &ivl0,
                    brep.curve(c2)?,
                    param2_start,
                    param2_end,
                    10,
                    Some(tolerance),
                )?
                .distance;
            if max_dist < tolerance {
                // Don't need to do anything here just quit because they are the same geometric edges.
                return Ok(out);
            }

            // check to see if the curves have opposite parameterization
            let max_dist = brep
                .curve(c1)?
                .curve_max_distance_between(
                    k,
                    &ivl0,
                    brep.curve(c2)?,
                    param2_end,
                    param2_start,
                    10,
                    Some(tolerance),
                )?
                .distance;
            if max_dist < tolerance {
                // Don't need to do anything here just quit because they are the same geometric edges.
                return Ok(out);
            }
        } // end 2 coincident bounding curve checks

        // when vertex does not have 3 bounding edges or is part of a lamina edge
        if bounding_edges.len() != 3 || brep.vertex_is_lamina(original_vertex)? {
            let mut arc: Option<BSplineCurve> = None;
            let mut placement = Axis2Placement::IDENTITY;
            let vertex_point = brep.vertex_point(original_vertex)?;
            placement.translate(vertex_point);

            // when there is just 1 bounding edge
            if bounding_edges.len() == 1 {
                let e = bounding_edges
                    .first()
                    .copied()
                    .ok_or(dangling("edge list"))?;
                let c = brep.edge_curve(e)?.ok_or(dangling("edge without curve"))?;
                // SM_CAST_PTR(SmBSplineCurve, curve): the failed cast is dereferenced
                let blend = vertex_b_spline_kind_of(brep, c)?
                    .ok_or(dangling("bounding curve is not a B-spline curve"))?;
                let ivl = brep.edge_interval(e)?;
                if blend.is_closed(k, &ivl, 0.0)? {
                    let mut ref_frame = Axis2Placement::IDENTITY;
                    if let Some((frame, _arc_rad, _start_ang, _end_ang)) =
                        blend.is_arc(k, 10, tolerance)?
                    {
                        ref_frame = frame;
                        let mut z_axis = ref_frame.z_axis();
                        let center = vertex_point;
                        let vec_to_edge = ref_frame.origin() - center;
                        if vec_to_edge.dot(z_axis) < 0.0 {
                            z_axis = -z_axis;
                        }
                        let point1 = blend.evaluate_point(k, blend.natural_interval().min())?;
                        let point2 = center + self.offset_distance * z_axis;
                        arc = Some(BSplineCurve::create_arc_from_points(
                            center,
                            point1,
                            point2,
                            NurbCircleParam::Quadratic,
                        )?);
                    }
                    // SetCanonical's status is ignored upstream
                    let _ = placement.set_canonical(
                        placement.origin(),
                        ref_frame.z_axis(),
                        ref_frame.x_axis(),
                    );
                }
            } // end 1 bounding edge check

            // create the vertex endCap surface
            let sphere = match arc {
                None => {
                    let arc = BSplineCurve::create_circle_segment(
                        3,
                        &placement,
                        self.offset_distance,
                        0.0,
                        180.0,
                        NurbCircleParam::Quadratic,
                    )?;
                    BSplineSurface::create_surf_of_revolution(
                        k,
                        &arc,
                        placement.origin(),
                        placement.x_axis(),
                        180.0,
                        false,
                        None,
                    )?
                }
                Some(arc) => BSplineSurface::create_surf_of_revolution(
                    k,
                    &arc,
                    placement.origin(),
                    placement.x_axis(),
                    360.0,
                    false,
                    None,
                )?,
            };
            out.surface = Some(brep.insert_surface(Surface::from(sphere))?);
        } else {
            // 3 boundary curves none of which are lamina
            // In this simple case we will assume that we are doing a constant radius
            // offset and that all of the edges are arcs of the same radius
            // (SM_CAST_PTR(SmBSplineCurve, curve), NER: a missing curve or a failed cast is
            // SM_ERR_NULL_POINTER; the curves keep their kinds for the virtual calls below)
            let mut curves: Vec<Curve> = Vec::new();
            for &e in bounding_edges {
                let c = brep
                    .edge_curve(e)?
                    .ok_or(SolidError::null_pointer(OP_VERTEX))?;
                let bsc =
                    vertex_b_spline_kind_of(brep, c)?.ok_or(SolidError::null_pointer(OP_VERTEX))?;
                curves.push(bsc.clone());
            }

            // Please note that some of the arcs may not be complete - Check them
            // against the corresponding 3 planes and see if one of them needs to be extended.
            if curves.len() == 3 {
                // Check for shelling case
                {
                    let mut shell_arc: Option<usize> = None;
                    let mut line_count = 0usize;
                    let mut frame = Axis2Placement::IDENTITY;
                    for (jj, line) in curves.iter().enumerate() {
                        if let Some((f, _radius, _start, _end)) = line.is_arc(k, 10, tolerance)? {
                            frame = f;
                            shell_arc = Some(jj);
                        }
                        if line.is_line(k, 10, tolerance)?.is_some() {
                            line_count += 1;
                        }
                    }
                    if let (Some(arc_idx), 2) = (shell_arc, line_count) {
                        let point_curve = BSplineCurve::create_degenerate_curve(3, frame.origin())?;
                        // Now that we have the curves create the geometry for the surface.
                        // (`*pShellArc` as a `const SmBSplineCurve &`: the callee reads the
                        // ends and the NURB of an arc, which no kind overrides)
                        let shell_arc = curves.get(arc_idx).ok_or(dangling("arc index"))?;
                        let shell_arc = b_spline_part(shell_arc)?.ok_or(dangling("arc kind"))?;
                        let trimmed = BSplineSurface::create_trimmed_plane_from_ruled(
                            k,
                            &shell_arc,
                            &point_curve,
                        )?;
                        for c in trimmed.trimming_curves {
                            out.trimming_curves.push(brep.insert_curve(c)?);
                        }
                        out.orients = trimmed.trim_orientations;
                        out.surface = Some(brep.insert_surface(Surface::from(trimmed.plane))?);
                        return Ok(out);
                    }
                }

                // Create planes
                let tol = self.this_approx_tol_3d();
                let mut plane_points = [Point3::new(0.0, 0.0, 0.0); 3];
                let mut plane_normals = [Vec3::new(0.0, 0.0, 0.0); 3];
                for j in 0..curves.len() {
                    let arc = curves.get(j).ok_or(dangling("curve index"))?;
                    // If we have a line then we probably are shelling
                    let Some((frame, _radius, _start, _end)) = arc.is_arc(k, 10, tol)? else {
                        return Err(sm_err(OP_VERTEX));
                    };
                    let next = (j + 1) % curves.len();
                    let other = curves.get(next).ok_or(dangling("curve index"))?;
                    let mid = other.evaluate_point(k, other.natural_interval().evaluate(0.5))?;
                    let pp = frame.origin();
                    let mut pn = frame.z_axis();
                    let vec = pp - mid;
                    if vec.dot(pn) < 0.0 {
                        pn = -pn;
                    }
                    if let (Some(p), Some(n)) = (plane_points.get_mut(j), plane_normals.get_mut(j))
                    {
                        *p = pp;
                        *n = pn;
                    }
                }

                // (bShelling is never set: its branch is empty upstream)

                // See if all arc points lie on 2 planes
                for kidx in 0..curves.len() {
                    // (upstream keeps pArc and its ends from before the kk loop: a replacement
                    // of sCurves[k] inside the loop does not change what the next kk reads)
                    let orig_arc = curves.get(kidx).ok_or(dangling("curve index"))?.clone();
                    let (start, end) = {
                        let ends = orig_arc.ends(k)?;
                        (ends.start, ends.end)
                    };
                    for kk in 0..curves.len() {
                        if kk == kidx {
                            continue;
                        }
                        let (pp, pn) = (
                            *plane_points.get(kk).ok_or(dangling("plane index"))?,
                            *plane_normals.get(kk).ok_or(dangling("plane index"))?,
                        );
                        let proj_point = point_project_to_plane(start, pp, pn);
                        let dist1 = distance(proj_point, start);
                        let proj_point2 = point_project_to_plane(end, pp, pn);
                        let dist2 = distance(proj_point2, end);
                        if dist1 > tol && dist2 > tol {
                            // Have a bad arc because at least one point needs to be on the plane.
                            let mut other = (kk + 1) % 3;
                            if other == kidx {
                                other = (other + 1) % 3;
                            }

                            let arc = &orig_arc;
                            // the status of IsArc is not checked here; a non-arc leaves the
                            // frame and radius uninitialised upstream
                            let Some((frame, radius, _, _)) = arc.is_arc(k, 10, tol)? else {
                                return Err(dangling("arc data of a non-arc read"));
                            };
                            let mid =
                                arc.evaluate_point(k, arc.natural_interval().evaluate(0.5))?;

                            let closest_pt1 =
                                Self::closest_circle_plane_point(tol, radius, &frame, pp, pn, mid)?;
                            let (op, on) = (
                                *plane_points.get(other).ok_or(dangling("plane index"))?,
                                *plane_normals.get(other).ok_or(dangling("plane index"))?,
                            );
                            let closest_pt2 =
                                Self::closest_circle_plane_point(tol, radius, &frame, op, on, mid)?;

                            // Make a new arc using the intersection points.
                            let p0 = *plane_points.first().ok_or(dangling("plane index"))?;
                            let new_arc = BSplineCurve::create_arc_from_points(
                                p0,
                                closest_pt1,
                                closest_pt2,
                                NurbCircleParam::Quadratic,
                            )?;
                            if let Some(slot) = curves.get_mut(kidx) {
                                *slot = Curve::from(new_arc);
                            }
                        }
                    }
                }
            } // end exactly 3 curves check

            // (any failure is SM_ERR; an unported callee stays unported)
            let sphere = match BSplineSurface::create_sphere_from_arcs(k, &curves, tolerance) {
                Ok(sphere) => sphere,
                Err(e @ SolidError::Unported { .. }) => return Err(e),
                Err(_) => return Err(sm_err(OP_VERTEX)),
            };
            let trim_curves: Vec<Curve> = sphere
                .ccw_arcs
                .iter()
                .filter_map(|&i| curves.get(i).cloned())
                .collect();
            out.orients = sphere.orients;
            out.surface = Some(brep.insert_surface(Surface::from(sphere.sphere))?);
            for orig in trim_curves {
                // SM_CAST_PTR(SmBSplineCurve, curve), new SmBSplineCurve(*pOrig): a plain
                // B-spline copy of the curve's NURB part
                let copy = b_spline_part(&orig)?.ok_or(dangling("trim curve kind"))?;
                out.trimming_curves
                    .push(brep.insert_curve(Curve::from(copy))?);
            }
        } // end given 3 boundary curves branch

        // all done
        Ok(out)
    }

    /// The "find the closest point to the plane" blocks of `VertexOffset`: of the
    /// intersections of the arc's circle with a plane, the one closest to the arc's
    /// midpoint; `SM_ERR` for three intersections (impossible) or when the closest is
    /// farther than ten radii.
    fn closest_circle_plane_point(
        tol: f64,
        radius: f64,
        frame: &Axis2Placement,
        plane_point: Point3,
        plane_normal: Vec3,
        mid: Point3,
    ) -> Result<Point3> {
        let mut closest_dist = BIG_DOUBLE;
        let mut closest = Point3::new(0.0, 0.0, 0.0);
        let points = match circle_plane_intersect(
            tol,
            radius,
            frame.origin(),
            frame.z_axis(),
            plane_point,
            plane_normal,
        )? {
            CurveIntersection::Points(p) => p,
            CurveIntersection::Coincident => return Err(sm_err(OP_VERTEX)),
        };
        for &p in points.iter().take(2) {
            let dist = distance(p, mid);
            if dist < closest_dist {
                closest_dist = dist;
                closest = p;
            }
        }
        if closest_dist > radius * 10.0 {
            return Err(sm_err(OP_VERTEX));
        }
        Ok(closest)
    }

    /// `UpdateFaceSurfaceDomain(pFace, pExtendedSurface)`: sets the face's UV domain to
    /// that of its UV trim curves (snapped to the surface domain's edges within
    /// `SM_EFF_ZERO`), and replaces the face's surface by a copy of the extended one
    /// when the new domain is not inside the surface's.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the trim curve domain cannot be computed;
    /// [`SolidError::Unported`] from `SmFace::CalculateUVDomainFromUVTrimCurves`' callees
    /// (`CreateUVTrimCurves`, `topology.face_p3`); those of `SmBrep::ReplaceSurface`
    /// ([`OffsetBrep::brep_replace_surface`]).
    pub fn update_face_surface_domain<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        face: O,
        extended_surface: O,
    ) -> Result<()> {
        let face_domain = match brep.face_uv_domain_from_uv_trim_curves(k, face) {
            Ok(d) => d,
            // `SmExtent2d::Intersect`'s `SM_ERR_INVALID_INPUT` (disjoint domains) is a real
            // status upstream, which `!= SM_SUCCESS` turns into `SM_ERR`; every other
            // `InvalidInput` is a null dereference upstream.
            Err(SolidError::InvalidInput {
                operation: "geom::extent2d::Extent2d::intersect",
                ..
            }) => return Err(sm_err(OP_UPDATE)),
            Err(
                e @ (SolidError::Unported { .. }
                | SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }),
            ) => return Err(e),
            Err(_) => return Err(sm_err(OP_UPDATE)),
        };

        let sur = brep
            .face_surface(face)?
            .ok_or(dangling("face without surface"))?;
        let surf_domain = surface_of(brep, sur)?.natural_uv_domain_with_nurb(k)?;
        let mut min = face_domain.min();
        let mut max = face_domain.max();
        if (min.x - surf_domain.min().x).abs() < EFF_ZERO {
            min.x = surf_domain.min().x;
        }
        if (min.y - surf_domain.min().y).abs() < EFF_ZERO {
            min.y = surf_domain.min().y;
        }
        if (max.x - surf_domain.max().x).abs() < EFF_ZERO {
            max.x = surf_domain.max().x;
        }
        if (max.y - surf_domain.max().y).abs() < EFF_ZERO {
            max.y = surf_domain.max().y;
        }
        let new_domain = Extent2d::from_points(min, max);
        brep.set_face_uv_domain(face, new_domain)?;

        if new_domain.is_contained_by(&surf_domain, EFF_ZERO) {
            return Ok(()); // no extension surface needed
        }

        // Need to replace it with the extended surface
        let ext = surface_of(brep, extended_surface)?.copy(k)?;
        let ext_h = store_surface(brep, ext)?;
        brep_replace_surface(k, brep, face, ext_h, false, false, true)?;

        Ok(())
    }

    /// `ReplaceEdgeCurves(pEdgeuse, rp3DCurve, rpUVCurve, cpOptOrigEdgeuse, ..)`: when
    /// `curve_3d` is given and its ends differ from the edge curve's (in either order,
    /// beyond the edge tolerance), the curve (reversed when it runs against the edge
    /// use's, or the original edge use's, curve) becomes the edge's curve and interval;
    /// the UV curve becomes the edge use's trim curve in every case; with a new 3D curve
    /// the start and end vertices move to its ends. Returns whether the caller should
    /// delete the candidate curves (both, when the ends matched).
    ///
    /// # Errors
    ///
    /// `SER` of the evaluations (`SmEdgeuse::NormalizedEvaluate` included) and the
    /// vertex moves;
    /// [`SolidError::InvalidInput`] for an edge use without edge, an edge without
    /// curve, or a null UV curve where upstream dereferences it.
    pub fn replace_edge_curves<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        edgeuse: O,
        curve_3d: Option<O>,
        uv_curve: Option<O>,
        opt_orig_edgeuse: Option<O>,
    ) -> Result<ReplaceEdgeCurves> {
        // locals
        let edge = brep
            .edgeuse_edge(edgeuse)?
            .ok_or(dangling("edge use without edge"))?;
        let eu_curve = brep.edge_curve(edge)?; // dereferenced only with a 3D curve

        let mut out = ReplaceEdgeCurves {
            delete_3d_curve: false,
            delete_uv_curve: false,
        };

        // when given a 3dCurve with a new shape - Update pEdge->Curve = rp3DCurve
        if let Some(c3) = curve_3d {
            // SM_ASSERT_MSG(rp3DCurve != pEUCurve) only reports
            let eu_curve = eu_curve.ok_or(dangling("edge without curve"))?;
            let edge_ivl = brep.edge_interval(edge)?;
            let edge_tol = brep.edge_tolerance(edge)?;

            // Edgesuse->Curve Start and End pts
            let edgeuse_start = curve_of(brep, eu_curve)?.evaluate_point(k, edge_ivl.min())?;
            let edgeuse_end = curve_of(brep, eu_curve)?.evaluate_point(k, edge_ivl.max())?;

            // input 3DCurve- Start and End pts
            let c3_ivl = curve_of(brep, c3)?.natural_interval();
            let curve_start = curve_of(brep, c3)?.evaluate_point(k, c3_ivl.min())?;
            let curve_end = curve_of(brep, c3)?.evaluate_point(k, c3_ivl.max())?;

            // low work - Edgesuse->Curve and 3DCurve start and end Points are within tol
            if distance(edgeuse_start, curve_start) < edge_tol
                && distance(edgeuse_end, curve_end) < edge_tol
            {
                // No change in ends of curve - don't bother using the inputs - delete them and get out.
                out.delete_3d_curve = true;
                out.delete_uv_curve = true;
                return Ok(out);
            }

            // low work - Edgeuse->Curve and 3DCurve start and end Points are within tol with reverse parameterization.
            if distance(edgeuse_start, curve_end) < edge_tol
                && distance(edgeuse_end, curve_start) < edge_tol
            {
                // No change in ends of curve - just delete the curves and get out
                out.delete_3d_curve = true;
                out.delete_uv_curve = true;
                return Ok(out);
            }

            // Edgeuse MidPoint (use cpOptOrigEdgeuse if given)
            let pv = match opt_orig_edgeuse {
                None => {
                    let param = edge_ivl.evaluate(0.5);
                    curve_of(brep, eu_curve)?.evaluate(k, param, 1, true, true)?
                }
                Some(orig) => {
                    let orig_edge = brep
                        .edgeuse_edge(orig)?
                        .ok_or(dangling("edge use without edge"))?;
                    let orig_curve = brep
                        .edge_curve(orig_edge)?
                        .ok_or(dangling("edge without curve"))?;
                    let param = brep.edge_interval(orig_edge)?.evaluate(0.5);
                    curve_of(brep, orig_curve)?.evaluate(k, param, 1, true, true)?
                }
            };

            // input 3DCurve midPoint
            let pv2 = curve_of(brep, c3)?.evaluate(k, c3_ivl.evaluate(0.5), 1, true, true)?;

            // when input and existing edge curves run in opposite directions - reverse 3DCurve and UVCurve
            let t1 = pv.get(1).copied().ok_or(dangling("tangent missing"))?;
            let t2 = pv2.get(1).copied().ok_or(dangling("tangent missing"))?;
            if t1.dot(t2) < 0.0 {
                // Opposite orientation - reverse 3DCurve and UVCurve (statuses ignored)
                let ivl = curve_of(brep, c3)?.natural_interval();
                let c = brep
                    .curve_mut(c3)
                    .ok_or(dangling("curve handle cannot be resolved"))?;
                let _ = ignore_soft(c.reverse_parameterization(k, &ivl))?;
                let uv = uv_curve.ok_or(dangling("null UV curve reversed"))?;
                let ivl = curve_of(brep, uv)?.natural_interval();
                let c = brep
                    .curve_mut(uv)
                    .ok_or(dangling("curve handle cannot be resolved"))?;
                let _ = ignore_soft(c.reverse_parameterization(k, &ivl))?;
            } // end Edge and Input curve running in opposite directions check

            // set input curves as Edge geometry (deleting the pre-existing curve and the
            // UV trim curves) and the edge interval
            brep.edge_set_curve(k, edge, c3)?;
            let natural = curve_of(brep, c3)?.natural_interval();
            brep.set_edge_interval(edge, natural)?;
        } // end rp3DCurve existence check

        // Update Edgeuse UVTrimCurve = rpUVCurve (TRUE = delete existing UVTrimCurve)
        let edge_tol = brep.edge_tolerance(edge)?;
        brep.edgeuse_set_uv_trim_curve(k, edgeuse, uv_curve, edge_tol)?;

        // when given a 3DCurve - Update pEdgeuse Start and End Vertex->Positions = rp3DCurve->Evals
        if curve_3d.is_some() {
            // Update the geometry of the start vertex.
            let v = brep
                .edgeuse_vertex(edgeuse)?
                .ok_or(dangling("edge use without vertex"))?;
            let pnt = edgeuse_normalized_evaluate(k, brep, edgeuse, 0.0, false, false, true)?; // FALSE = 3d Eval
            brep.set_vertex_point(k, v, pnt)?;

            // Update the geometry of the other vertex
            let mate = brep
                .edgeuse_mate(edgeuse)?
                .ok_or(dangling("edge use without mate"))?;
            let v = brep
                .edgeuse_vertex(mate)?
                .ok_or(dangling("edge use without vertex"))?;
            let pnt = edgeuse_normalized_evaluate(k, brep, edgeuse, 1.0, false, false, true)?;
            brep.set_vertex_point(k, v, pnt)?;
        } // end given 3DCurve check

        // all done
        Ok(out)
    }

    /// `TrimExtendedEdge(ctx, exec, pBaseEdgeuse, pOtherBaseEdgeuse, pOffsetEdgeuse,
    /// pOtherOffsetEdgeuse, p3DCurve, pUVCurveBase, pUVCurveOtherBase, ..)`: the trim
    /// interval of the extended intersection curve, found end by end (the start vertex
    /// of the base edge use first, then its end vertex) from candidate trim points: the
    /// offset vertices of a neighbouring tangent or lamina edge use that was already
    /// mated, else the intersection of the base UV curve with the neighbour's UV trim
    /// curve extended ten lengths each way; the intersections of the 3D curve with the
    /// extended surfaces of the offsets of non-tangent neighbours (non-grazing ones
    /// only); or, without candidates, the drop of the base vertex onto the curve. The
    /// candidate closest to the base vertex (and not the start already chosen) wins.
    /// `None` (`bGoodEdge == FALSE`) when an end has no candidate or the interval is
    /// zero length. `offset_edgeuse` and `other_offset_edgeuse` are `NotUsed`.
    ///
    /// # Errors
    ///
    /// `SER` of the point solves, intersections and evaluations;
    /// [`SolidError::InvalidInput`] for null objects upstream dereferences. `SER` of
    /// `SmEdgeuse::CreateUVTrimCurve` is included.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn trim_extended_edge<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &mut impl OffsetExecutive<O>,
        base_edgeuse: O,
        other_base_edgeuse: Option<O>,
        offset_edgeuse: Option<O>,
        other_offset_edgeuse: Option<O>,
        curve_3d: O,
        uv_curve_base: Option<O>,
        uv_curve_other_base: Option<O>,
        mark: MarkType,
    ) -> Result<Option<Extent1d>> {
        let _ = (offset_edgeuse, other_offset_edgeuse);

        // locals
        let mut trim_ivl = Extent1d::new();
        let tan_tol_deg = self.tangency_tol_degrees();
        let base_edge = brep
            .edgeuse_edge(base_edgeuse)?
            .ok_or(dangling("edge use without edge"))?;

        // for two iterations - find a one 3DCurve trim point
        for i in 0..2 {
            let mut solutions: SolutionArray<O> = SolutionArray::with_capacity(16);

            // Let's first try to take care of start vertex of pBaseEdgeuse
            let mut off_adj_eus: Vec<O> = Vec::new(); // list of
            let mut tan_eus: Vec<O> = Vec::new(); // list of NextEdgeuses between tangent faces
            let mut tan_uvs: Vec<Option<O>> = Vec::new(); // list of BaseCurve UVTrimCurves when NextEdgeuse is a tangent sector

            // the base vertex and the neighbours of each iteration
            let base_vertex: O;
            if i == 0 {
                // start at BaseEdgeuse->StartVertex
                base_vertex = brep
                    .edgeuse_vertex(base_edgeuse)?
                    .ok_or(dangling("edge use without vertex"))?;
                let bcw = brep
                    .edgeuse_cw(base_edgeuse)?
                    .ok_or(dangling("edge use without CW"))?;

                // See if base edgeuse is tangent and save it along with the curve on the
                // correct surface to be used during intersection.
                if bcw != base_edgeuse {
                    self.collect_neighbour(
                        k,
                        brep,
                        executive,
                        base_vertex,
                        base_vertex,
                        base_edge,
                        bcw,
                        uv_curve_base,
                        tan_tol_deg,
                        &mut tan_eus,
                        &mut tan_uvs,
                        &mut off_adj_eus,
                    )?;
                }

                // See if otherBase edgeuse is tangent and save it along with the curve on the
                // correct surface to be used during intersection.
                if let Some(other_base) = other_base_edgeuse {
                    let obccw = brep
                        .edgeuse_ccw(other_base)?
                        .ok_or(dangling("edge use without CCW"))?;
                    if obccw != other_base {
                        let other_base_vertex = brep
                            .edge_other_vertex(base_edge, base_vertex)?
                            .ok_or(dangling("edge without other vertex"))?;
                        let other_base_edge = brep
                            .edgeuse_edge(other_base)?
                            .ok_or(dangling("edge use without edge"))?;
                        self.collect_neighbour(
                            k,
                            brep,
                            executive,
                            base_vertex,
                            other_base_vertex,
                            other_base_edge,
                            obccw,
                            uv_curve_other_base,
                            tan_tol_deg,
                            &mut tan_eus,
                            &mut tan_uvs,
                            &mut off_adj_eus,
                        )?;
                    }
                }
            } else {
                // 2nd iteration: start at BaseEdgeuse->EndVertex
                let mate = brep
                    .edgeuse_mate(base_edgeuse)?
                    .ok_or(dangling("edge use without mate"))?;
                base_vertex = brep
                    .edgeuse_vertex(mate)?
                    .ok_or(dangling("edge use without vertex"))?;
                let bccw = brep
                    .edgeuse_ccw(base_edgeuse)?
                    .ok_or(dangling("edge use without CCW"))?;

                // See if base edgeuse is tangent and save it along with the curve on the
                // correct surface to be used during intersection.
                if bccw != base_edgeuse {
                    self.collect_neighbour(
                        k,
                        brep,
                        executive,
                        base_vertex,
                        base_vertex,
                        base_edge,
                        bccw,
                        uv_curve_base,
                        tan_tol_deg,
                        &mut tan_eus,
                        &mut tan_uvs,
                        &mut off_adj_eus,
                    )?;
                }

                // See if otherBase edgeuse is tangent and save it along with the curve on the
                // correct surface to be used during intersection.
                if let Some(other_base) = other_base_edgeuse {
                    let obcw = brep
                        .edgeuse_cw(other_base)?
                        .ok_or(dangling("edge use without CW"))?;
                    if obcw != other_base {
                        let other_base_vertex = brep
                            .edge_other_vertex(base_edge, base_vertex)?
                            .ok_or(dangling("edge without other vertex"))?;
                        let other_base_edge = brep
                            .edgeuse_edge(other_base)?
                            .ok_or(dangling("edge use without edge"))?;
                        self.collect_neighbour(
                            k,
                            brep,
                            executive,
                            base_vertex,
                            other_base_vertex,
                            other_base_edge,
                            obcw,
                            uv_curve_other_base,
                            tan_tol_deg,
                            &mut tan_eus,
                            &mut tan_uvs,
                            &mut off_adj_eus,
                        )?;
                    }
                }
            } // end 2nd iteration branch

            // Tangent sector handling - extend parameter space and intersect it for one or two curves.
            for (itry, &tan_eu) in tan_eus.iter().enumerate() {
                let tan_uv = tan_uvs.get(itry).copied().flatten();
                let mut uv_curve = brep.edgeuse_uv_trim_curve(tan_eu)?;

                // If this has already been mated then we should just grab the point of the vertex
                let mut found_3d_vertex = false;
                // (the mate is dereferenced only when the edge use is marked)
                if brep.edgeuse_is_marked(k, tan_eu, mark)? && {
                    let tan_mate = brep
                        .edgeuse_mate(tan_eu)?
                        .ok_or(dangling("edge use without mate"))?;
                    brep.edgeuse_is_marked(k, tan_mate, mark)?
                } {
                    let tan_edge = brep
                        .edgeuse_edge(tan_eu)?
                        .ok_or(dangling("edge use without edge"))?;
                    let all_eus = brep.edge_edgeuses(tan_edge)?;
                    for eu in all_eus {
                        // skip
                        let Some(off_eu) = executive.offset_edgeuse(eu) else {
                            continue;
                        };
                        let mut v = brep
                            .edgeuse_vertex(off_eu)?
                            .ok_or(dangling("edge use without vertex"))?;

                        // get point on 3DCurve closest to pOffEu->StartVertex
                        let tol_3d = self.this_approx_tol_3d * 10.0;
                        let point = brep.vertex_point(v)?;
                        let sols2 = Self::point_solve(
                            k,
                            brep,
                            curve_3d,
                            SolverOperation::Minimize,
                            point,
                            tol_3d,
                            Some(tol_3d),
                            SolutionRequested::All,
                        )?;

                        // save solutions
                        if !sols2.is_empty() {
                            solutions.append(&sols2);
                            found_3d_vertex = true;
                        } // end found a solution check

                        // not get point on 3DCurve closest to pOffEd0->EndVertex
                        let off_edge = brep
                            .edgeuse_edge(off_eu)?
                            .ok_or(dangling("edge use without edge"))?;
                        let Some(other_v) = brep.edge_other_vertex(off_edge, v)? else {
                            continue; // Closed Curve Case
                        };
                        v = other_v;
                        let point = brep.vertex_point(v)?;
                        let sols2 = Self::point_solve(
                            k,
                            brep,
                            curve_3d,
                            SolverOperation::Minimize,
                            point,
                            tol_3d,
                            Some(tol_3d),
                            SolutionRequested::All,
                        )?;

                        // save solutions
                        if !sols2.is_empty() {
                            solutions.append(&sols2);
                            found_3d_vertex = true;
                        }
                    }
                } // end pTanEu and its mate are marked checks

                // If we didn't find a 3D vertex from the offset close enough then we need to do
                // UV curve extension and intersections to solve for this tangency case.
                if let (false, Some(tan_uv)) = (found_3d_vertex, tan_uv) {
                    // get a temporary copy of the pTanEU->UVTrimCurve
                    let uv_copy: Curve = match uv_curve.take() {
                        None => {
                            let (_max_dist, h) =
                                edgeuse_create_uv_trim_curve(k, brep, tan_eu, false, false)?;
                            curve_of(brep, h)?.clone()
                        }
                        // (`pUVCurve->Copy(crContext, pUVCurve)` on an `SmBSplineCurve*`
                        // binds the non-virtual `SmBSplineCurve*&` overload: a plain
                        // B-spline copy, a line's analytic data is dropped)
                        Some(h) => b_spline_overload_copy(curve_of(brep, h)?)?,
                    };
                    // (SmBSplineCurve *pUVCurve = pTanEU->GetUVTrimCurve(): the NURB data of
                    // a B-spline kind, which CreateExtendedCurve reads)
                    let uv_bs = b_spline_part(&uv_copy)?
                        .ok_or(dangling("UV trim curve is not a B-spline curve"))?;

                    // set pUVCurve extension amounts
                    let curve_extension_factor = 10.0; // Curve Extension Factor
                    let ivl = uv_bs.natural_interval();
                    let extension_parameter = ivl.min() - curve_extension_factor * ivl.length();
                    let extension_parameter2 = ivl.max() + curve_extension_factor * ivl.length();

                    // extend both ends of pUVCurve (a null extension, of a zero-length
                    // interval, is dereferenced upstream)
                    let precise_extension = true; // Otherwise it adds another length of sIvl.
                    let curve_extended = uv_bs
                        .create_extended_curve(
                            extension_parameter,
                            ContinuityType::G1G2,
                            precise_extension,
                        )?
                        .ok_or(dangling("null extended curve"))?;
                    let mut curve_extended2 = curve_extended
                        .create_extended_curve(
                            extension_parameter2,
                            ContinuityType::G1G2,
                            precise_extension,
                        )?
                        .ok_or(dangling("null extended curve"))?;
                    curve_extended2.remove_extra_knots(EFF_ZERO_SQRT / 100.0)?;

                    // intersect pTanUV/Extended pTanEU->UVTrimCurve (the extended curve
                    // goes to the store: the solutions name it as their object, which
                    // the solver's comparisons dereference for its parameter interval)
                    let ext_h = store_curve(brep, Curve::from(curve_extended2))?;
                    let ext_curve = curve_of(brep, ext_h)?;
                    let ext_ivl = ext_curve.natural_interval();
                    let tan_curve = curve_of(brep, tan_uv)?;
                    let sols2: SolutionArray<O> = tan_curve.global_curve_intersect(
                        k,
                        &tan_curve.natural_interval(),
                        ext_curve,
                        &ext_ivl,
                        EFF_ZERO_SQRT,
                        &CurveIntersectOptions {
                            skip_global_coincidence_check: false,
                            handles: [Some(tan_uv), Some(ext_h)],
                            analytic_handles: [None, None],
                        },
                    )?;

                    // save solutions as possible trim Points
                    solutions.append(&sols2);
                }
            } // end iter every sTanEU/sTanUV pair of members

            // Now handle intersecting faces if there are any

            // for every sOffAdjEU - try letting trimPoint = OffAdjEu->ExtendedSurface/3DCurve intersection
            let mut last_face: Option<O> = None;
            for &off_adj_eu in &off_adj_eus {
                let f = brep
                    .edgeuse_face(off_adj_eu)?
                    .ok_or(dangling("edge use without face"))?;

                // skip LastFace
                if Some(f) == last_face {
                    continue;
                }

                // Extend pF->Surface
                last_face = Some(f);
                let tol_3d = brep.brep_tolerance(f)?;
                let f_surface = brep
                    .face_surface(f)?
                    .ok_or(dangling("face without surface"))?;
                let (ext_surface, ext_domain) =
                    executive.get_create_extended_surface(k, brep, f_surface)?;

                // intersect pF->ExtendedSurface with 3DCurve
                let crv_ivl = curve_of(brep, curve_3d)?.natural_interval();
                let Some(sols2) = ignore_soft(surface_global_curve_intersect(
                    k,
                    brep,
                    ext_surface,
                    &ext_domain,
                    curve_3d,
                    &crv_ivl,
                    tol_3d,
                ))?
                else {
                    continue;
                };

                // Note that the adjacent surface intersection overrides tangent intersections
                // Test the solutions to see if they are not nearly tangent.
                for sol in sols2.as_slice() {
                    // (neither status is checked upstream: a failed evaluation leaves its
                    // output at SmPoint3d's SM_UNDEF_DOUBLE default)
                    let undef = Vec3::new(UNDEF_DOUBLE, UNDEF_DOUBLE, UNDEF_DOUBLE);
                    let pv = ignore_soft(curve_of(brep, curve_3d)?.evaluate(
                        k,
                        sol.start.param(0)?,
                        1,
                        true,
                        true,
                    ))?;
                    let uv = Point2::new(sol.start.param(1)?, sol.start.param(2)?);
                    let normal = ignore_soft(surface_of(brep, ext_surface)?.evaluate_normal(
                        k,
                        uv,
                        Side::Left,
                        Side::Left,
                    ))?
                    .unwrap_or(undef);
                    let tangent = pv
                        .and_then(|pv| pv.get(1).copied())
                        .unwrap_or(undef)
                        .unitize_or_z();
                    let angle_rad = tangent.angle_between(normal)?;
                    let angle_deg = angle_rad * 180.0 / PI;
                    if angle_deg < 85.0 || angle_deg > 95.0 {
                        // save solution as possible trimPoint
                        solutions.add(sol);
                    }
                }
            } // end iter every sOffAdjEU

            let mut found = false;
            let mut found_dist = BIG_DOUBLE;
            let mut found_param = 0.0;

            // If we have no candidate TrimPoints found by intersecting the 3DCurve with other
            // geometry, get a candidate TrimPoint by dropping the Vertex->Point onto the 3DCurve
            if solutions.is_empty() {
                let tol_3d = brep.brep_tolerance(base_edge)?;
                let point = brep.vertex_point(base_vertex)?;
                solutions = Self::point_solve(
                    k,
                    brep,
                    curve_3d,
                    SolverOperation::Minimize,
                    point,
                    tol_3d,
                    None,
                    SolutionRequested::All,
                )?;
            }

            // search every solution for the one closest to the BaseVertex to use as the trimPoint
            let base_point = brep.vertex_point(base_vertex)?;
            for sol in solutions.as_slice() {
                let t = sol.start.param(0)?;
                let pnt = curve_of(brep, curve_3d)?.evaluate_point(k, t)?;

                // search for the best solution
                let dist_moved = distance(pnt, base_point);
                if dist_moved < found_dist && !are_same(t, trim_ivl.min()) {
                    found_param = t;
                    found_dist = dist_moved;
                    found = true;
                }

                // If solution is a range, e.g. coincident curves, check m_vEnd
                if sol.solution_type == SolutionType::RangeOfValues {
                    let t = sol.end.param(0)?;
                    let pnt = curve_of(brep, curve_3d)?.evaluate_point(k, t)?;
                    let dist_moved = distance(pnt, base_point);

                    if dist_moved < found_dist && !are_same(t, trim_ivl.min()) {
                        found_param = t;
                        found_dist = dist_moved;
                        found = true;
                    }
                }
            }

            // BadEdge when no trim point was found for this iteration
            if !found {
                return Ok(None);
            }

            // else set Interval
            trim_ivl.add_value(found_param);
        } // end iter twice

        // `Kernel::fixes.offset_closed_curve_trim_arc` (WR-463, #403): on a closed curve the
        // two trim points bound two arcs and the interval between them is only one of them.
        // The edge takes the arc that runs where the base edge does; when a trim point lies
        // on the curve's seam the other arc is an interval too, else the edge is not trimmed.
        if k.fixes.offset_closed_curve_trim_arc {
            let nat_ivl = curve_of(brep, curve_3d)?.natural_interval();
            let seam_tol = EFF_ZERO * (1.0 + nat_ivl.max_dimension());
            let tol = self.this_approx_tol_3d;
            if trim_ivl.length() > seam_tol
                && curve_of(brep, curve_3d)?.is_closed(k, &nat_ivl, tol)?
            {
                let base_mid =
                    edgeuse_normalized_evaluate(k, brep, base_edgeuse, 0.5, false, false, true)?;
                let curve = curve_of(brep, curve_3d)?;
                let in_mid = curve.evaluate_point(k, trim_ivl.evaluate(0.5))?;
                let mut out_param = trim_ivl.max() + 0.5 * (nat_ivl.length() - trim_ivl.length());
                if out_param > nat_ivl.max() {
                    out_param -= nat_ivl.length();
                }
                let out_mid = curve.evaluate_point(k, out_param)?;
                if distance(out_mid, base_mid) < distance(in_mid, base_mid) {
                    let seam = curve.evaluate_point(k, nat_ivl.min())?;
                    let lo = curve.evaluate_point(k, trim_ivl.min())?;
                    let hi = curve.evaluate_point(k, trim_ivl.max())?;
                    if distance(lo, seam) <= tol {
                        trim_ivl = Extent1d::from_raw(trim_ivl.max(), nat_ivl.max());
                    } else if distance(hi, seam) <= tol {
                        trim_ivl = Extent1d::from_raw(nat_ivl.min(), trim_ivl.min());
                    } else {
                        // the arc runs across the seam: no interval of this curve gives it
                        return Ok(None);
                    }
                }
            }
        }

        // cull zero length curves
        let scaled_zero = EFF_ZERO * (1.0 + trim_ivl.max_dimension());
        if trim_ivl.length() > scaled_zero {
            Ok(Some(trim_ivl))
        } else {
            Ok(None)
        }
    }

    /// The neighbour classification of `TrimExtendedEdge` (repeated for each end of
    /// each base edge use): a lamina or tangent neighbour goes to the tangent lists
    /// with the base UV curve; otherwise, when the base edge and the neighbour's edge
    /// are not tangent at the vertex, the offset of the neighbour's radial edge use
    /// goes to the adjacent list. (Not upstream, WR-346: a neighbour whose radial lies
    /// on a collapsed face gives the offset of the edge use across that face, seen from
    /// `base_vertex`, instead.)
    #[allow(clippy::too_many_arguments)]
    fn collect_neighbour<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &mut impl OffsetExecutive<O>,
        base_vertex: O,
        vertex: O,
        base_edge: O,
        neighbour: O,
        base_uv: Option<O>,
        tan_tol_deg: f64,
        tan_eus: &mut Vec<O>,
        tan_uvs: &mut Vec<Option<O>>,
        off_adj_eus: &mut Vec<O>,
    ) -> Result<()> {
        // Not upstream (WR-346): across a collapsed face, trim to the extended surface
        // of the face beyond it.
        if executive.has_collapsed_faces() {
            let radial = brep.edgeuse_radial(neighbour)?;
            if let Some(across) =
                self.edgeuse_across_collapsed_face(k, brep, &*executive, radial, Some(base_vertex))?
            {
                if let Some(off_across) = executive.offset_edgeuse(across) {
                    off_adj_eus.push(off_across);
                }
                return Ok(());
            }
        }
        let n_edge = brep
            .edgeuse_edge(neighbour)?
            .ok_or(dangling("edge use without edge"))?;
        // When next edgeuse is either a tangentSector or lamina
        if brep.edge_is_lamina(n_edge)?
            || brep.edgeuse_is_tangent_sector(k, neighbour, tan_tol_deg)?
        {
            // save it and this baseEdgeuse in the Tan lists
            tan_eus.push(neighbour);
            tan_uvs.push(base_uv);
        } else {
            // Look for intersecting surface
            if !brep.vertex_are_edges_tangent(vertex, base_edge, n_edge, 1.0)? {
                let adj_eu = brep
                    .edgeuse_radial(neighbour)?
                    .ok_or(dangling("edge use without radial"))?;
                if let Some(off_adj_eu) = executive.offset_edgeuse(adj_eu) {
                    off_adj_eus.push(off_adj_eu);
                }
            }
        }
        Ok(())
    }

    /// Not upstream (WR-346, `SmOffsetExecutive::GetEdgeuseAcrossCollapsedFace`): the
    /// original edge use beyond a collapsed face, seen from its edge use `edgeuse`.
    /// Across a rail (a tangent edge of a four-sided round) it is the radial of the
    /// opposite rail; across an end edge, seen from `from` (one of that edge's
    /// vertices), the radial of the rail at the edge's other vertex. `None` when the
    /// edge use's face did not collapse, the face across collapsed too, or the face has
    /// another shape. The four-sided test walks the loop's CCW edge uses; the end edge's
    /// two vertices are `GetVertex` / `GetOtherVertex`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for dangling topology; those of the tangency test.
    pub fn edgeuse_across_collapsed_face<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &impl OffsetExecutive<O>,
        edgeuse: Option<O>,
        from: Option<O>,
    ) -> Result<Option<O>> {
        let Some(edgeuse) = edgeuse else {
            return Ok(None);
        };
        match brep.edgeuse_face(edgeuse)? {
            Some(face) if executive.is_collapsed_face(face) => {}
            _ => return Ok(None),
        }
        let tan_tol_deg = self.tangency_tol_degrees();
        let next = if brep.edgeuse_is_tangent_sector(k, edgeuse, tan_tol_deg)? {
            // a four-sided loop (bounded walk: a fifth step means more sides)
            let mut sides = 1;
            let mut cur = brep
                .edgeuse_ccw(edgeuse)?
                .ok_or(dangling("edge use without CCW"))?;
            while cur != edgeuse && sides <= 4 {
                sides += 1;
                cur = brep
                    .edgeuse_ccw(cur)?
                    .ok_or(dangling("edge use without CCW"))?;
            }
            if sides != 4 {
                return Ok(None);
            }
            let ccw = brep
                .edgeuse_ccw(edgeuse)?
                .ok_or(dangling("edge use without CCW"))?;
            brep.edgeuse_ccw(ccw)?
        } else {
            let Some(from) = from else {
                return Ok(None);
            };
            // the end edge's vertex away from `from` (by position: `from` may be a
            // vertex of the rail beside it)
            let end_edge = brep
                .edgeuse_edge(edgeuse)?
                .ok_or(dangling("edge use without edge"))?;
            let v0 = brep
                .edge_vertex(end_edge)?
                .ok_or(dangling("edge without vertex"))?;
            let v1 = brep
                .edge_other_vertex(end_edge, v0)?
                .ok_or(dangling("edge without other vertex"))?;
            let from_point = brep.vertex_point(from)?;
            let far = if brep.vertex_point(v0)?.distance(from_point)
                > brep.vertex_point(v1)?.distance(from_point)
            {
                v0
            } else {
                v1
            };
            let sides = [
                brep.edgeuse_cw(edgeuse)?
                    .ok_or(dangling("edge use without CW"))?,
                brep.edgeuse_ccw(edgeuse)?
                    .ok_or(dangling("edge use without CCW"))?,
            ];
            let mut found = None;
            for side in sides {
                if side == edgeuse {
                    continue;
                }
                let side_edge = brep
                    .edgeuse_edge(side)?
                    .ok_or(dangling("edge use without edge"))?;
                let s0 = brep.edge_vertex(side_edge)?;
                let s1 = match s0 {
                    Some(s0) => brep.edge_other_vertex(side_edge, s0)?,
                    None => None,
                };
                if s0 == Some(far) || s1 == Some(far) {
                    found = Some(side);
                    break;
                }
            }
            found
        };
        let Some(next) = next else {
            return Ok(None);
        };
        let Some(across) = brep.edgeuse_radial(next)? else {
            return Ok(None);
        };
        if across == next {
            return Ok(None);
        }
        match brep.edgeuse_face(across)? {
            Some(face) if executive.is_collapsed_face(face) => Ok(None),
            _ => Ok(Some(across)),
        }
    }

    /// `GlobalPointSolve(interval, op, point, tol, target, NULL, requested, solutions)`
    /// of the curve behind `curve` over its natural interval.
    fn point_solve<O: ClassObject + SolutionObject>(
        k: &Kernel,
        brep: &impl OffsetBrep<O>,
        curve: O,
        op: SolverOperation,
        point: Point3,
        dist_tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<SolutionArray<O>> {
        let c = curve_of(brep, curve)?;
        c.global_point_solve(
            k,
            &c.natural_interval(),
            op,
            point,
            dist_tol,
            requested,
            &GlobalPointSolveOptions {
                target_distance,
                opt_vectors: None,
                handle: Some(curve),
                analytic_handle: None,
                zone_tol_3d: None,
            },
        )
    }

    /// `ExtendTrimEdges(ctx, exec, pEdgeuse, pOtherEdgeuse, pOffsetEdgeuse,
    /// pOtherOffsetEdgeuse, eMarkType)`: extends or trims a lamina or tangent edge of
    /// the offset. The more iso-parametric of the edge use's and its radial's UV trim
    /// curves (the radial's only with an other offset edge use) is extended ten lengths
    /// each way, trimmed to the extended offset surface's domain and lifted to 3D; on a
    /// non-lamina edge that curve is dropped onto the other extended surface for the
    /// second UV curve (the piece closest to the other UV curve's midpoint, or a copy
    /// of the original UV curve when the drop fails). `TrimExtendedEdge` finds the trim
    /// interval; the three curves are trimmed to it and become the offset edge uses'
    /// geometry (`ReplaceEdgeCurves`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no trim interval is found or the second UV curve is missing;
    /// `SER` of the geometry calls (`GetCreateExtendedSurface` included);
    /// [`SolidError::Unported`] for the callees of `TrimExtendedEdge` /
    /// `ReplaceEdgeCurves`;
    /// [`SolidError::InvalidInput`] for null objects upstream dereferences.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn extend_trim_edges<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &mut impl OffsetExecutive<O>,
        edgeuse: O,
        other_edgeuse: Option<O>,
        offset_edgeuse: Option<O>,
        other_offset_edgeuse: Option<O>,
        mark: MarkType,
    ) -> Result<()> {
        // locals
        let mut uv_curve = brep
            .edgeuse_uv_trim_curve(edgeuse)?
            .ok_or(dangling("edge use without UV trim curve"))?;
        let mut other_uv_curve: Option<O> = None;
        let mut use_rad_uv = false;
        let mut other_mid = Point3::new(0.0, 0.0, 0.0);
        let edge = brep
            .edgeuse_edge(edgeuse)?
            .ok_or(dangling("edge use without edge"))?;

        // Let's first decide if edgeuse or radial UVTrimCurve is optimal TrimCurve to take.
        // We like ISO curves the best.

        // when Edgeuse is not lamina - pick the best UVTrimCurve on Edgeuse and RadialEdgeuse
        if !brep.edge_is_lamina(edge)? {
            let radial = brep
                .edgeuse_radial(edgeuse)?
                .ok_or(dangling("edge use without radial"))?;
            let uv_rad = brep
                .edgeuse_uv_trim_curve(radial)?
                .ok_or(dangling("radial edge use without UV trim curve"))?;

            // Edgeuse and RadialEdgeuse UVCurve bounding boxes and sizes (virtual calls:
            // a UV line's box is its segment's, not clamped to the natural interval)
            let uv = curve_of(brep, uv_curve)?;
            let uv_ivl = uv.natural_interval();
            let req = BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            };
            let uv_bbox = uv
                .calculate_bounding_box(k, &uv_ivl, req, true)?
                .normal
                .ok_or(dangling("no box"))?;
            // (upstream passes the edge use curve's interval to the radial's box too)
            let uv_rad_bbox = curve_of(brep, uv_rad)?
                .calculate_bounding_box(k, &uv_ivl, req, true)?
                .normal
                .ok_or(dangling("no box"))?;
            let uv_size = uv_bbox.size();
            let uv_rad_size = uv_rad_bbox.size();

            let min_uv = min2(uv_size.x, uv_size.y);
            let min_rad_uv = min2(uv_rad_size.x, uv_rad_size.y);

            // let pUVCurve be the Edgeuse or RadialEdgeuse curve that's the most isoparameter like.
            // But: if we use the radial uv curve, then we have to have pOtherOffsetEdgeuse,
            // otherwise pExtSurf will not get created (see below). [B473]
            if min_rad_uv < min_uv - EFF_ZERO && other_offset_edgeuse.is_some() {
                other_uv_curve = Some(uv_curve);
                uv_curve = uv_rad;
                use_rad_uv = true;
            } else {
                other_uv_curve = Some(uv_rad);
            }
            // refresh locals
            let other = other_uv_curve.ok_or(dangling("other UV curve"))?;
            let orig_ivl = curve_of(brep, other)?.natural_interval();
            other_mid = curve_of(brep, other)?.evaluate_point(k, orig_ivl.evaluate(0.5))?;
        } // end edgeuse is not lamina check

        // Create a greatly extended curve and trim it to the bounding box of the extended surface.
        let uv_bs = b_spline_of(brep, uv_curve)?
            .ok_or(dangling("UV trim curve is not a B-spline curve"))?;
        let ivl = uv_bs.natural_interval();
        let new_min = ivl.min() - 10.0 * ivl.length();
        let new_max = ivl.max() + 10.0 * ivl.length();
        let precise_extension = true; // Otherwise it adds another length of sIvl.

        // pCurveExtended = Extend the UVCurve to the NewMin parameter value (a null
        // extension, of a zero-length interval, is dereferenced upstream)
        let curve_extended = uv_bs
            .create_extended_curve(new_min, ContinuityType::G1G2, precise_extension)?
            .ok_or(dangling("null extended curve"))?;

        // pUVCurveExtended = ReExtend the Extended Curve to the NewMax parameter value
        let mut uv_curve_extended = curve_extended
            .create_extended_curve(new_max, ContinuityType::G1G2, precise_extension)?
            .ok_or(dangling("null extended curve"))?;

        // Clean up extended curve pUVCurveExtended
        let mut ext_ivl = uv_curve_extended.natural_interval();
        uv_curve_extended.remove_extra_knots(EFF_ZERO_SQRT / 100.0)?;

        // locals for surface extension
        let mut ext_surf: Option<(O, Extent2d)> = None;
        let mut other_ext_surf: Option<(O, Extent2d)> = None;

        // when using the Edgeuse UVTrimCurve (else branch) or the RadialEdgeuse UVTrimCurve
        if use_rad_uv {
            // when using the RadialEdgeuse UVTrimCurve
            if let Some(off) = offset_edgeuse {
                let off_face = brep
                    .edgeuse_face(off)?
                    .ok_or(dangling("edge use without face"))?;
                let surf = brep
                    .face_surface(off_face)?
                    .ok_or(dangling("face without surface"))?;
                other_ext_surf = Some(executive.get_create_extended_surface(k, brep, surf)?);
            }
            if let Some(other_off) = other_offset_edgeuse {
                let face2 = brep
                    .edgeuse_face(other_off)?
                    .ok_or(dangling("edge use without face"))?;
                let surf2 = brep
                    .face_surface(face2)?
                    .ok_or(dangling("face without surface"))?;
                ext_surf = Some(executive.get_create_extended_surface(k, brep, surf2)?);
            }
        } else {
            // extend the surface (pOffsetEdgeuse is dereferenced: null is undefined)
            let off_face = brep
                .edgeuse_face(offset_edgeuse.ok_or(dangling("null offset edge use"))?)?
                .ok_or(dangling("edge use without face"))?;
            let surf = brep
                .face_surface(off_face)?
                .ok_or(dangling("face without surface"))?;
            ext_surf = Some(executive.get_create_extended_surface(k, brep, surf)?);

            if !brep.edge_is_lamina(edge)? {
                if let Some(other_off) = other_offset_edgeuse {
                    let face2 = brep
                        .edgeuse_face(other_off)?
                        .ok_or(dangling("edge use without face"))?;
                    let surf2 = brep
                        .face_surface(face2)?
                        .ok_or(dangling("face without surface"))?;
                    other_ext_surf = Some(executive.get_create_extended_surface(k, brep, surf2)?);
                }
            }
        }

        // Trim extended UVTrimCurve to pExtDomain
        let (ext_surf_h, ext_domain) = ext_surf.ok_or(dangling("no extended surface"))?;
        let ext_curve = Curve::from(uv_curve_extended);
        let trim_extended = crate::surface::surface::p1::find_uv_curve_ivl_in_domain::<O>(
            k,
            &ext_curve,
            &ext_ivl,
            ivl.evaluate(0.5),
            &ext_domain,
        )?;
        let CurveKind::BSpline(mut uv_curve_extended) = ext_curve.into_kind() else {
            return Err(dangling("extended curve kind"));
        };
        let mut trim_extended = trim_extended;
        uv_curve_extended.trim(k, &mut trim_extended, TrimMode::Notify)?; // may snap sIvl by tol to existing knots

        // refresh sExtIvl to trimmed Ivl
        ext_ivl = uv_curve_extended.natural_interval();

        // Set pLiftedCurve = pUVCurveExtended projected through pExtSurf
        let edge_tol = brep.edge_tolerance(edge)?;
        let ext_surface = surface_of(brep, ext_surf_h)?.clone_for_evaluation()?;
        let lifted = ext_surface
            .lift_curve_with_context(
                k,
                &ext_domain,
                &Curve::from(uv_curve_extended.clone()),
                &ext_ivl,
                edge_tol / 3.0,
                false,
                &mut OffsetSurfaceSolveContext(brep),
                Some(ext_surf_h),
            )?
            .curve;
        let lifted_curve = lifted;

        // When the Edgeuse is not lamina - build the 2nd UVTrimCurve on the other Surface
        let mut uv_curve_extended_h: Option<O> = None;
        // pOtherUVCurve: the edge use's or radial's own UV trim curve until the drop below
        // replaces it (cpp:1452/1458); without an other extended surface it reaches
        // TrimExtendedEdge and is trimmed in place (cpp:1694)
        let mut other_uv_h: Option<O> = other_uv_curve;
        if let Some((other_ext_h, other_ext_domain)) = other_ext_surf {
            // (pOffsetEdgeuse->GetEdge()->GetTolerance(): null is undefined)
            let off_edge = brep
                .edgeuse_edge(offset_edgeuse.ok_or(dangling("null offset edge use"))?)?
                .ok_or(dangling("edge use without edge"))?;
            let off_tol = brep.edge_tolerance(off_edge)?;

            // Drop Lifted Curve onto other extended surface to make 2nd UVTrimCurve
            // (the drop's trace gets the surface's cache, `smsurf_GetSurfaceCache`: the
            // face's trimmed cache when a face owns the extended surface)
            let other_ext = surface_of(brep, other_ext_h)?.clone_for_evaluation()?;
            let mut face_cache = if other_ext.face()?.is_some() {
                Some(brep.prepare_surface_solve_cache(k, &other_ext, Some(other_ext_h))?)
            } else {
                None
            };
            let dropped = other_ext.drop_and_trim_curve_in::<O, O>(
                k,
                face_cache.as_mut(),
                &other_ext_domain,
                &lifted_curve,
                &lifted_curve.natural_interval(),
                off_tol / 3.0,
                true,
                true,
                None,
            )?;
            // Opt-in (WR-530, #402; the fork's source fork@23c5b0a): the edge is
            // lifted onto one face's new surface and dropped onto the other's. Faces offset
            // alike stay tangent, so the drop lands on the edge; when only one of them got a
            // new surface (a local operation moving a face beside a blend, or one piece of a
            // split face), the drop crosses the gap between them and the faces no longer
            // meet there
            if k.fixes.local_operation_refuses_loose_tangent_edges && executive.is_local_operation()
            {
                let meet_tol = 10.0 * smos_max(off_tol, self.this_approx_tol_3d());
                if dropped.max_drop_to_surf > meet_tol {
                    return Err(sm_err(OP_EXTEND));
                }
            }
            let uv_curves = dropped.uv_curves;
            let mut closest_idx = 0usize;

            // the candidate for the 2nd UVTrimCurve
            let candidate_h = if uv_curves.is_empty() {
                // LiftedCurve failed to Drop to OtherExtSurf - Unable to do dropping - just use
                // original UV and copy it (the status of the copy is ignored)
                let other = other_uv_curve.ok_or(dangling("null UV curve copied"))?;
                // (`pOtherUVCurve->Copy(crContext, pOtherUVCurve)` on an
                // `SmBSplineCurve*` binds the non-virtual `SmBSplineCurve*&` overload:
                // a plain B-spline copy, a line's analytic data is dropped)
                let copy = b_spline_overload_copy(curve_of(brep, other)?)?;
                store_curve(brep, copy)?
            } else {
                // When Lifted Curve dropped to OtherExtSurf

                // When LiftedCurve dropped in more than 1 piece to OtherExtSurf
                if uv_curves.len() > 1 {
                    // Find the UVTrimCurve closest to the original - assume curve dropped to a seam
                    let mut min_dist = BIG_DOUBLE;
                    for (j, test) in uv_curves.iter().enumerate() {
                        let tc = test.clone();
                        let sols2: SolutionArray<O> = tc.global_point_solve(
                            k,
                            &tc.natural_interval(),
                            SolverOperation::Minimize,
                            other_mid,
                            EFF_ZERO_SQRT,
                            SolutionRequested::Single,
                            &GlobalPointSolveOptions::default(),
                        )?;
                        let sol2 = sols2
                            .get(0)
                            .ok_or(dangling("point solve without solution"))?;

                        // save the UVTrimCurve with the shortest drop
                        if sol2.start.solution_value < min_dist {
                            closest_idx = j;
                            min_dist = sol2.start.solution_value;
                        }
                    } // end iter every DropCurve candidate - looking for best to use
                } // end LiftedCurve dropped in more than 1 piece check

                // Set pOtherUVCurve to best LiftedCurve dropped to OtherExtSurf UVTrimCurve
                let best = uv_curves
                    .get(closest_idx)
                    .ok_or(dangling("drop curve index"))?
                    .clone();
                store_curve(brep, best)?
            }; // end LiftedCurve failed to drop to OtherExtSurf branch

            // When using the RadialEdgeuse - swap the extended UVTrimCurves
            if use_rad_uv {
                // Swap them back
                other_uv_h = Some(store_curve(brep, Curve::from(uv_curve_extended.clone()))?);
                uv_curve_extended_h = Some(candidate_h);
            } else {
                other_uv_h = Some(candidate_h);
            }
        } // end pOtherExtSurf existence check - working with a nonLamina edge (assumes manifold edge)

        // the extended UV curve as a handle when it was not swapped in above
        let uv_curve_extended_h = match uv_curve_extended_h {
            Some(h) => h,
            None => store_curve(brep, Curve::from(uv_curve_extended.clone()))?,
        };

        // Now that we have created one or two UV curves and a 3D curve find trimming
        // interval and if we find a good one replace the geometry.
        let lifted_h = store_curve(brep, lifted_curve.clone())?;
        let trim_ivl = self.trim_extended_edge(
            k,
            brep,
            executive,
            edgeuse,
            other_edgeuse,
            offset_edgeuse,
            other_offset_edgeuse,
            lifted_h,
            Some(uv_curve_extended_h),
            other_uv_h,
            mark,
        )?;

        // when TrimExtendedEdge() call worked
        let Some(trim_ivl) = trim_ivl else {
            // TrimExtendedEdge() call did not work
            return Err(sm_err(OP_EXTEND));
        };

        // Trim the 3 curves to the common interval (may snap sIvl by tol to existing knots:
        // the snapped interval is what the next trim gets)
        let mut ti = trim_ivl;
        {
            let c = brep
                .curve_mut(lifted_h)
                .ok_or(dangling("curve handle cannot be resolved"))?;
            c.trim(k, &mut ti, TrimMode::Notify)?;
        }
        {
            let c = brep
                .curve_mut(uv_curve_extended_h)
                .ok_or(dangling("curve handle cannot be resolved"))?;
            c.trim(k, &mut ti, TrimMode::Notify)?;
        }
        if let Some(other) = other_uv_h {
            let c = brep
                .curve_mut(other)
                .ok_or(dangling("curve handle cannot be resolved"))?;
            c.trim(k, &mut ti, TrimMode::Notify)?;
        }

        let lifted_copy = curve_of(brep, lifted_h)?.copy(k)?;
        let lifted_copy_h = store_curve(brep, lifted_copy)?;

        // change the pOffsetEdgeuse geometry (ReplaceEdgeCurves dereferences it: null is
        // undefined)
        let r = self.replace_edge_curves(
            k,
            brep,
            offset_edgeuse.ok_or(dangling("null offset edge use"))?,
            Some(lifted_h),
            Some(uv_curve_extended_h),
            None,
        );
        // (the rbDelete* flags only free the candidates upstream)
        r?;

        // when appropriate - change the pOtherOffsetEdgeuse geometry
        if let Some(other_off) = other_offset_edgeuse {
            // Not Lamina
            let Some(other) = other_uv_h else {
                return Err(sm_err(OP_EXTEND));
            };
            // GWC:BUG?? These two Edgeuses point to the same edge - these two calls should
            //           have the same 3DCurve
            self.replace_edge_curves(k, brep, other_off, Some(lifted_copy_h), Some(other), None)?;
        } // end pOtherOffsetEdgeuse existence check

        // all done
        Ok(())
    }

    /// `MateEdges(ctx, exec, pEdgeuse, pOtherEdgeuse, pOffsetEdgeuse,
    /// pOtherOffsetEdgeuse, eMarkType)`: mates two offset edge uses by extending their
    /// faces' surfaces and intersecting them (at a quarter of the B-rep tolerance, 20
    /// degrees, using the surface edges); of the non-degenerate intersection curves
    /// that `TrimExtendedEdge` can trim, the one whose trimmed midpoint is closest to
    /// the original edge use's midpoint (the only one, when alone) is trimmed with its
    /// UV curves and becomes the geometry of both offset edge uses
    /// (`ReplaceEdgeCurves`, oriented by the original edge uses).
    ///
    /// With [`crate::kernel::UpstreamFixes::offset_analytic_mate_edges`] (the source
    /// fork's source fork@fddc366 and @98ab1b0), each marched intersection
    /// piece that lies on an exact line or conic of analytic copies of the two face
    /// surfaces is first replaced by that curve, with UV curves dropped from it.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a null edge use or a missing UV curve; `SM_ERR`
    /// when no intersection curve can be trimmed; `SER` of the geometry calls
    /// (`GetCreateExtendedSurface` included); [`SolidError::Unported`] for the callees
    /// of `TrimExtendedEdge` / `ReplaceEdgeCurves`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn mate_edges<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &mut impl OffsetExecutive<O>,
        edgeuse: Option<O>,
        other_edgeuse: Option<O>,
        offset_edgeuse: Option<O>,
        other_offset_edgeuse: Option<O>,
        mark: MarkType,
    ) -> Result<()> {
        // These can't be Null:
        let edgeuse = edgeuse.ok_or(SolidError::null_pointer(OP_MATE))?;
        let other_edgeuse = other_edgeuse.ok_or(SolidError::null_pointer(OP_MATE))?;
        let offset_edgeuse = offset_edgeuse.ok_or(SolidError::null_pointer(OP_MATE))?;
        let other_offset_edgeuse = other_offset_edgeuse.ok_or(SolidError::null_pointer(OP_MATE))?;

        // Extend Surfaces of faces connected to current edge.
        let face1 = brep
            .edgeuse_face(offset_edgeuse)?
            .ok_or(dangling("edge use without face"))?;
        let surf1 = brep
            .face_surface(face1)?
            .ok_or(dangling("face without surface"))?;
        let (ext_surface1, domain1) = executive.get_create_extended_surface(k, brep, surf1)?;
        let face2 = brep
            .edgeuse_face(other_offset_edgeuse)?
            .ok_or(dangling("edge use without face"))?;
        let surf2 = brep
            .face_surface(face2)?
            .ok_or(dangling("face without surface"))?;
        let (ext_surface2, domain2) = executive.get_create_extended_surface(k, brep, surf2)?;

        // locals for surf/surf intersection
        let edge = brep
            .edgeuse_edge(edgeuse)?
            .ok_or(dangling("edge use without edge"))?;
        let ssi_tol = brep.brep_tolerance(edge)? / 4.0;
        let deg20_rad = 20.0_f64.to_radians();
        let use_surface_edges = [true, true];

        // Intersect extended surfaces. The evaluation copies cannot be passed for
        // both when the two faces share one surface (`SmSurfaceIntersector` records
        // `doing_self_intersection` from pointer identity, and the C++ call passes
        // the same surface pointer twice).
        let s1 = surface_of(brep, ext_surface1)?.clone_for_evaluation()?;
        let s2 = surface_of(brep, ext_surface2)?.clone_for_evaluation()?;
        let other = if ext_surface1 == ext_surface2 {
            &s1
        } else {
            &s2
        };
        let mut ssi = s1.global_surface_intersect_with_context(
            k,
            [Some(ext_surface1), Some(ext_surface2)],
            &mut OffsetSurfaceSolveContext(brep),
            &domain1,
            other,
            &domain2,
            use_surface_edges,
            Some(ssi_tol),
            Some(deg20_rad),
        )?;

        // `Kernel::fixes.offset_analytic_mate_edges` (source fork@fddc366 and
        // @98ab1b0, #1453/#1130): two analytic surfaces meet on lines and conics. The
        // marched intersection of their B-spline extensions gives the pieces (bounded by
        // the extended domains and seams) and their UV curves; each piece that lies on
        // both extended surfaces as the exact curve between the same ends (within the
        // analytic surfaces, not on a tangent extension) is replaced by that curve, its
        // UV curves dropped from it.
        if k.fixes.offset_analytic_mate_edges {
            let ext_dist = self.offset_distance().abs() * 5.0;
            let analytic1 = create_analytic_extension(k, surface_of(brep, surf1)?, ext_dist)?;
            let analytic2 = create_analytic_extension(k, surface_of(brep, surf2)?, ext_dist)?;
            if let (Some(analytic1), Some(analytic2)) = (&analytic1, &analytic2) {
                if !ssi.curves_3d.is_empty() {
                    let exact = analytic1.global_surface_intersect(
                        k,
                        &analytic1.natural_uv_domain_with_nurb(k)?,
                        analytic2,
                        &analytic2.natural_uv_domain_with_nurb(k)?,
                        use_surface_edges,
                        Some(ssi_tol),
                        Some(deg20_rad),
                    )?;
                    let tol = brep.brep_tolerance(edge)?;
                    for ii in 0..ssi.curves_3d.len() {
                        let marched = ssi.curves_3d.get(ii).ok_or(dangling("curve index"))?;
                        if marched.is_degenerate(k, EFF_ZERO, None)? {
                            continue;
                        }
                        let Some(piece) = exact_piece(k, marched, &exact.curves_3d, tol)? else {
                            continue;
                        };
                        let mut uv: [Option<Curve>; 2] = [None, None];
                        for (i_surf, slot) in uv.iter_mut().enumerate() {
                            let (ext, dom) = if i_surf == 0 {
                                (&s1, &domain1)
                            } else {
                                (other, &domain2)
                            };
                            let dropped = ext.drop_curve::<NoObject>(
                                k,
                                dom,
                                &piece,
                                &piece.natural_interval(),
                                ssi_tol,
                                true,
                                None,
                                true,
                            )?;
                            if dropped.uv_curves.len() == 1 && dropped.max_drop_to_surf <= tol {
                                *slot = dropped.uv_curves.into_iter().next();
                            }
                        }
                        let [Some(uv1), Some(uv2)] = uv else {
                            // The piece leaves an extended surface where it is a tangent
                            // (G1R) extension, not the analytic surface: there the marched
                            // piece is these surfaces' intersection.
                            continue;
                        };
                        if let Some(c) = ssi.curves_3d.get_mut(ii) {
                            *c = piece;
                        }
                        if let Some(c) = ssi.surface1_uv_curves.get_mut(ii) {
                            *c = Some(uv1);
                        }
                        if let Some(c) = ssi.surface2_uv_curves.get_mut(ii) {
                            *c = Some(uv2);
                        }
                    }
                }
            }
        }

        let mut curves_3d: Vec<O> = Vec::new();
        let mut surface1_uv_curves: Vec<Option<O>> = Vec::new();
        let mut surface2_uv_curves: Vec<Option<O>> = Vec::new();
        for c in ssi.curves_3d {
            curves_3d.push(store_curve(brep, c)?);
        }
        for c in ssi.surface1_uv_curves {
            surface1_uv_curves.push(match c {
                Some(c) => Some(store_curve(brep, c)?),
                None => None,
            });
        }
        for c in ssi.surface2_uv_curves {
            surface2_uv_curves.push(match c {
                Some(c) => Some(store_curve(brep, c)?),
                None => None,
            });
        }

        // Now see if we have an intersection curve that has proper trimming
        let mut min_dist = BIG_DOUBLE;
        let mut min_curve: Option<usize> = None;
        let mut trim_ivl = Extent1d::new();

        // for every intersection solution
        for ii in 0..curves_3d.len() {
            let c3 = *curves_3d.get(ii).ok_or(dangling("curve index"))?;

            // skip degenerate curve solutions
            if curve_of(brep, c3)?.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }

            // get iith XSect 3D and UVTrim Curves (null UVTrimCurves are an error)
            let uv1 = surface1_uv_curves
                .get(ii)
                .copied()
                .flatten()
                .ok_or(SolidError::null_pointer(OP_MATE))?;
            let uv2 = surface2_uv_curves
                .get(ii)
                .copied()
                .flatten()
                .ok_or(SolidError::null_pointer(OP_MATE))?;
            if !is_b_spline_kind(curve_of(brep, uv1)?) || !is_b_spline_kind(curve_of(brep, uv2)?) {
                return Err(SolidError::null_pointer(OP_MATE));
            }

            // Trim the extended Surf/Surf intersection
            let my_trim = self.trim_extended_edge(
                k,
                brep,
                executive,
                edgeuse,
                Some(other_edgeuse),
                Some(offset_edgeuse),
                Some(other_offset_edgeuse),
                c3,
                Some(uv1),
                Some(uv2),
                mark,
            )?;

            // when intersection was successfully trimmed
            if let Some(my_trim) = my_trim {
                // when there is more than 1 intersection curve to check
                if curves_3d.len() > 1 {
                    // save the trimmed XSectCurve that has the smallest change from the original 3DCurve.
                    let mid_pnt = curve_of(brep, c3)?.evaluate_point(k, my_trim.evaluate(0.5))?;
                    let orig_mid =
                        edgeuse_normalized_evaluate(k, brep, edgeuse, 0.5, false, false, true)?; // FALSE = 3d Eval

                    let dist = distance(mid_pnt, orig_mid);

                    if dist < min_dist {
                        min_curve = Some(ii);
                        min_dist = dist;
                        trim_ivl = my_trim;
                    }
                } else {
                    // only 1 curve to check branch - use this curve
                    trim_ivl = my_trim;
                    min_curve = Some(0);
                }
            }
        } // end iter every extended Surf/Surf intersection

        // Error Exit - no XSectCurve found
        let Some(min_curve) = min_curve else {
            return Err(sm_err(OP_MATE));
        };

        // arrive here after finding the XSectCurve result whose midpoint that is closest
        // to the pEdgeuse->Curve->midPoint
        let curve_3d_found = *curves_3d.get(min_curve).ok_or(dangling("curve index"))?;
        let uv_curve1_found = surface1_uv_curves
            .get(min_curve)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_MATE))?;
        let uv_curve2_found = surface2_uv_curves
            .get(min_curve)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_MATE))?;

        // (the found curves are removed from the arrays upstream so that they are not deleted)

        // Trim bound best XSectCurves to sTrimIvl (may snap sIvl by tol to existing knots:
        // the snapped interval is what the next trim gets)
        let mut ti = trim_ivl;
        for h in [curve_3d_found, uv_curve1_found, uv_curve2_found] {
            let c = brep
                .curve_mut(h)
                .ok_or(dangling("curve handle cannot be resolved"))?;
            c.trim(k, &mut ti, TrimMode::Notify)?;
        }

        // Replace Curve of edge and set its new domain.
        let copy_3d = curve_of(brep, curve_3d_found)?.copy(k)?;
        let copy_3d_h = store_curve(brep, copy_3d)?;

        // when appropriate - change the pOffsetEdgeuse geometry
        self.replace_edge_curves(
            k,
            brep,
            offset_edgeuse,
            Some(curve_3d_found),
            Some(uv_curve1_found),
            Some(edgeuse),
        )?;

        // when appropriate - change the pOtherOffsetEdgeuse geometry
        // GWC:BUG?? These two Edgeuses point to the same edge - these two calls should
        //           have the same 3DCurve. It's ok: At this point they do not (yet) point to the same edge.
        self.replace_edge_curves(
            k,
            brep,
            other_offset_edgeuse,
            Some(copy_3d_h),
            Some(uv_curve2_found),
            Some(other_edgeuse),
        )?;

        // all done
        Ok(())
    }

    /// `HaveTangentGaps(lNumSamples, cpOffsetEdgeuse, cpOtherOffsetEdgeuse)`: whether
    /// any of `num_samples + 1` points along the other offset edge use is farther from
    /// the offset edge use's edge than its tolerance (a closest point was found).
    ///
    /// # Errors
    ///
    /// Those of the closest point (`SmEdgeuse::NormalizedEvaluate`'s status is ignored,
    /// [`SolidError::Unported`] aside); [`SolidError::InvalidInput`]
    /// for an edge use without edge.
    pub fn have_tangent_gaps<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        num_samples: u64,
        offset_edgeuse: O,
        other_offset_edgeuse: O,
    ) -> Result<bool> {
        let edge = brep
            .edgeuse_edge(offset_edgeuse)?
            .ok_or(dangling("edge use without edge"))?;
        let tol = brep.edge_tolerance(edge)?;

        for i in 0..=num_samples {
            let param = (i as f64 * 1.0) / (num_samples as f64 * 1.0);
            // (the evaluation's status is not checked upstream: a failed evaluation leaves
            // sPnt at SmPoint3d's SM_UNDEF_DOUBLE default)
            let pnt = ignore_soft(edgeuse_normalized_evaluate(
                k,
                brep,
                other_offset_edgeuse,
                param,
                false,
                false,
                true,
            ))? // FALSE = 3d Eval
            .unwrap_or(Vec3::new(UNDEF_DOUBLE, UNDEF_DOUBLE, UNDEF_DOUBLE));
            // (ClosestPoint's status is not checked either; only bSuccess && d > tol is)
            if let Some(Some(cp)) = ignore_soft(brep.edge_closest_point(k, edge, pnt))? {
                if cp.distance > tol {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Not upstream (`IsStepAcrossEdge`, fork source fork 5d428ae, opt-in
    /// `Kernel::fixes.local_operation_step_walls`): whether the faces of two offset edge
    /// uses of one tangent edge stayed parallel across it but apart. At each of 11 samples
    /// along the first edge use the nearest point of the other's edge must lie farther
    /// than the edges meet within (`10 * max(edge tolerances, this_approx_tol_3d)`) and the
    /// gap must run along both faces' normals within that distance. A face that turned
    /// instead (a draft) leaves its gap off its normal and is no step.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, the closest point and the normals (`SER`);
    /// [`SolidError::InvalidInput`] for missing topology or geometry (`NER`).
    pub fn is_step_across_edge<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        offset_edgeuse: O,
        other_offset_edgeuse: O,
    ) -> Result<bool> {
        // The fork's sample count: 11 points along the edge use.
        const NUM_SAMPLES: u32 = 10;
        let edge = brep
            .edgeuse_edge(offset_edgeuse)?
            .ok_or(dangling("edge use without edge"))?;
        let other_edge = brep
            .edgeuse_edge(other_offset_edgeuse)?
            .ok_or(dangling("edge use without edge"))?;
        let face = brep
            .edgeuse_face(offset_edgeuse)?
            .ok_or(dangling("edge use without face"))?;
        let other_face = brep
            .edgeuse_face(other_offset_edgeuse)?
            .ok_or(dangling("edge use without face"))?;
        let surf = brep
            .face_surface(face)?
            .ok_or(dangling("face without surface"))?;
        let other_surf = brep
            .face_surface(other_face)?
            .ok_or(dangling("face without surface"))?;
        if edge == other_edge {
            return Ok(false);
        }

        // The distance the two edges meet within (as ExtendTrimEdges judges a drop).
        let meet_tol = 10.0
            * max2(
                max2(brep.edge_tolerance(edge)?, brep.edge_tolerance(other_edge)?),
                self.this_approx_tol_3d,
            );
        let other_ivl = brep.edge_interval(other_edge)?;
        let other_len = other_ivl.length();
        let positive = other_len > 0.0;
        if !positive {
            return Ok(false);
        }
        let other_opposite =
            brep.edgeuse_orientation(other_offset_edgeuse)? == OrientType::Opposite;

        for ii in 0..=NUM_SAMPLES {
            let t = f64::from(ii) / f64::from(NUM_SAMPLES);
            let pnt = edgeuse_normalized_evaluate(k, brep, offset_edgeuse, t, false, false, true)?;
            let uv = edgeuse_normalized_evaluate(k, brep, offset_edgeuse, t, true, false, true)?;

            let Some(cp) = brep.edge_closest_point(k, other_edge, pnt)? else {
                return Ok(false);
            };
            let apart = cp.distance > meet_tol;
            if !apart {
                return Ok(false);
            }
            let mut other_t = (cp.parameter - other_ivl.min()) / other_len;
            if other_opposite {
                other_t = 1.0 - other_t;
            }
            other_t = min2(1.0, max2(0.0, other_t));
            let other_pnt = edgeuse_normalized_evaluate(
                k,
                brep,
                other_offset_edgeuse,
                other_t,
                false,
                false,
                true,
            )?;
            let other_uv = edgeuse_normalized_evaluate(
                k,
                brep,
                other_offset_edgeuse,
                other_t,
                true,
                false,
                true,
            )?;

            let normal = surface_of(brep, surf)?.evaluate_normal(
                k,
                Point2::new(uv.x, uv.y),
                Side::Right,
                Side::Right,
            )?;
            let other_normal = surface_of(brep, other_surf)?.evaluate_normal(
                k,
                Point2::new(other_uv.x, other_uv.y),
                Side::Right,
                Side::Right,
            )?;

            // The gap runs along both unit normals: what lies across each is within the
            // meet distance.
            let gap = other_pnt - pnt;
            let along = gap.project_to_plane(normal).length() <= meet_tol
                && gap.project_to_plane(other_normal).length() <= meet_tol;
            if !along {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Not upstream (`CreateStepWall`, fork source fork 5d428ae and 90aabff,
    /// opt-in `Kernel::fixes.local_operation_step_walls`): the ruled wall between two
    /// offset edge uses whose faces lie apart ([`Self::is_step_across_edge`]), stored and
    /// returned as a handle. The rails are the two edges' trimmed NURBS curves; a rail
    /// whose weights are all one value (to `SM_EFF_ZERO` relative) is made non-rational
    /// first (`MakeNonRational`), so two lines stored with different weights rule a
    /// bilinear wall. The second rail is reversed when its points at a quarter, half and
    /// three quarters along lie nearer the first's reversed (interior points: a closed
    /// rail starts where it ends).
    ///
    /// # Errors
    ///
    /// Those of the curve copies, evaluations, reversal and `CreateRuledSurface` (`SER`);
    /// [`SolidError::InvalidInput`] for missing curves (`NER`).
    pub fn create_step_wall<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        offset_edgeuse: O,
        other_offset_edgeuse: O,
    ) -> Result<O> {
        let mut rails = Vec::with_capacity(2);
        for eu in [offset_edgeuse, other_offset_edgeuse] {
            let edge = brep
                .edgeuse_edge(eu)?
                .ok_or(dangling("edge use without edge"))?;
            let h = brep
                .edge_create_trimmed_nurbs_curve(k, edge)?
                .ok_or(dangling("edge without NURBS curve"))?;
            let rail = match curve_of(brep, h)?.kind() {
                CurveKind::BSpline(b) => b.clone(),
                _ => return Err(dangling("trimmed NURBS curve is not a B-spline")),
            };
            rails.push(rail);
        }
        let [mut rail1, mut rail2]: [BSplineCurve; 2] = rails
            .try_into()
            .map_err(|_| dangling("two rails expected"))?;
        for rail in [&mut rail1, &mut rail2] {
            if !rail.is_rational()? {
                continue;
            }
            let (_, first) = rail.control_point(ControlPointForm::EuclideanRational, 0)?;
            let mut equal = true;
            let count = rail.number_control_points()?;
            let mut kk = 1;
            while kk < count && equal {
                let (_, w) = rail.control_point(ControlPointForm::EuclideanRational, kk)?;
                equal = (w - first).abs() <= EFF_ZERO * first.abs();
                kk += 1;
            }
            if equal {
                make_non_rational(rail)?;
            }
        }

        // The second rail runs the way the first does when its interior points lie nearer
        // the first's than the reversed ones do.
        let ivl1 = rail1.natural_interval();
        let ivl2 = rail2.natural_interval();
        let mut same = 0.0;
        let mut reversed = 0.0;
        for kk in 1..=3 {
            let t = 0.25 * f64::from(kk);
            let p1 = rail1.evaluate_point(ivl1.evaluate(t))?;
            let p2 = rail2.evaluate_point(ivl2.evaluate(t))?;
            let p2_reversed = rail2.evaluate_point(ivl2.evaluate(1.0 - t))?;
            same += distance(p1, p2);
            reversed += distance(p1, p2_reversed);
        }
        if reversed < same {
            rail2.reverse_parameterization(&ivl2)?;
        }

        let wall = BSplineSurface::create_ruled_surface(&rail1, &rail2, SurfParamType::U)?;
        store_surface(brep, Surface::from(wall))
    }

    /// `EdgeuseOffset(ctx, exec, cpOriginalEdgeuse, cpOtherOriginalEdgeuse,
    /// cpOffsetEdgeuse, cpOtherOffsetEdgeuse, ..)`: the cap surface of an edge of the
    /// offset. A tangent edge between an offset face and a shell face whose offset edge
    /// uses leave gaps is always bridged by a cap below. Otherwise, when the executive
    /// extends convex edges, lamina and tangent edges are
    /// extended/trimmed (`ExtendTrimEdges`) and the others mated (`MateEdges`),
    /// problems only reported. Otherwise a non-convex radial sector gets nothing
    /// (`SM_ERR` for a tangent sector with gaps between the offsets), a lamina edge
    /// nothing, an inset on a shell face mates the offset edges to the other face's
    /// surface; else the rails (the offset edges' trimmed curves, or the approximated
    /// offsets of the original UV trim curves) and the original curve (trimmed to the
    /// rails' ends) make the cap: a trimmed plane or ruled surface for a shell face, a
    /// torus for arcs, a cylinder/cone for lines, else a circular cross-section surface;
    /// rails within ten tolerances of each other only raise the offset edges' tolerances.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a tangent sector with gaps, or rails whose intervals do not line up;
    /// `SM_ERR_NULL_POINTER` for missing curves; `SER` of the geometry calls;
    /// [`SolidError::Unported`] for the callees listed in the module header;
    /// [`SolidError::InvalidInput`] for null objects upstream dereferences.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn edgeuse_offset<O: ClassObject + SolutionObject>(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        executive: &mut impl OffsetExecutive<O>,
        original_edgeuse: O,
        other_original_edgeuse: Option<O>,
        offset_edgeuse: Option<O>,
        other_offset_edgeuse: Option<O>,
        mark: MarkType,
    ) -> Result<EdgeuseOffset<O>> {
        // init output
        let mut out = EdgeuseOffset {
            surface: None,
            trimming_curves: Vec::new(),
            trim_orientations: Vec::new(),
            self_intersections: SsiData::default(),
            closing_wall: false,
        };
        let orig_edge = brep
            .edgeuse_edge(original_edgeuse)?
            .ok_or(dangling("edge use without edge"))?;

        // A tangent edge between an offset face and a shell face can't be closed by
        // extension: bridge it below.
        let shell_faces = executive.shell_faces();
        let gap_edgeuses = (other_original_edgeuse, offset_edgeuse, other_offset_edgeuse);
        let shell_tangent_gap = match gap_edgeuses {
            (Some(other_orig), Some(off), Some(other_off)) => {
                let on_shell = |face: Option<O>| face.is_some_and(|f| contains(&shell_faces, &f));
                on_shell(brep.edgeuse_face(original_edgeuse)?)
                    != on_shell(brep.edgeuse_face(other_orig)?)
                    && brep.edgeuse_is_tangent_sector(
                        k,
                        original_edgeuse,
                        self.tangency_tol_degrees(),
                    )?
                    && self.have_tangent_gaps(k, brep, 10, off, other_off)?
            }
            _ => false,
        };

        // If we are extending convex edges, extend both offset surfaces to hit each other.
        if executive.extend_convex_edges() && !shell_tangent_gap {
            let lamina = brep.edge_is_lamina(orig_edge)?;

            // No, we have to call ExtendTrimEdges() for Lamina Edges.  [cbi B01; B256 ok]
            let tan_tol_deg = self.tangency_tol_degrees();
            let tan_sec = brep.edgeuse_is_tangent_sector(k, original_edgeuse, tan_tol_deg)?;

            // Not upstream (opt-in `Kernel::fixes.local_operation_step_walls`, fork
            // source fork 5d428ae): a local operation that moved only one of two
            // tangent faces along its normal leaves them parallel, one step apart: a wall
            // between their edges closes the body.
            if k.fixes.local_operation_step_walls
                && tan_sec
                && !lamina
                && executive.is_local_operation()
            {
                if let (Some(off), Some(other_off)) = (offset_edgeuse, other_offset_edgeuse) {
                    if self.is_step_across_edge(k, brep, off, other_off)? {
                        out.surface = Some(self.create_step_wall(k, brep, off, other_off)?);
                        out.closing_wall = true;
                        return Ok(out);
                    }
                }
            }

            if tan_sec || lamina {
                // Extend or Trim Lamina or Tangent Edges (a problem is only reported)
                let extended = self.extend_trim_edges(
                    k,
                    brep,
                    executive,
                    original_edgeuse,
                    other_original_edgeuse,
                    offset_edgeuse,
                    other_offset_edgeuse,
                    mark,
                );
                // Opt-in (WR-530): a local operation has no later step that would close
                // the gap: it fails
                if k.fixes.local_operation_refuses_loose_tangent_edges
                    && executive.is_local_operation()
                {
                    extended?;
                } else {
                    let _ = ignore_soft(extended)?;
                }
                return Ok(out);
            }

            // Mate a couple of edges by extension and intersection (a problem is only reported).
            let _ = ignore_soft(self.mate_edges(
                k,
                brep,
                executive,
                Some(original_edgeuse),
                other_original_edgeuse,
                offset_edgeuse,
                other_offset_edgeuse,
                mark,
            ))?;
            return Ok(out);
        } // end when extending convex edges

        // Make sure sector being offset is convex otherwise just skip it
        if !shell_tangent_gap
            && brep.edgeuse_radial(original_edgeuse)? == other_original_edgeuse
            && !brep.edgeuse_is_convex_radial_sector(k, original_edgeuse, 10)?
        {
            if brep.edgeuse_is_tangent_sector(k, original_edgeuse, 3.0)? {
                if let (Some(off), Some(other_off)) = (offset_edgeuse, other_offset_edgeuse) {
                    if self.have_tangent_gaps(k, brep, 10, off, other_off)? {
                        return Err(sm_err(OP_EDGEUSE)); // Figure out how to recover from this outside
                    }
                }
            }
            return Ok(out);
        }

        // Now take care of case where we are insetting and hit the shell face
        // we need to extend and intersect the side face.
        // Note: if cpOtherOriginalEdgeuse was passed in NULL, then there's nothing
        // else we can do in this method.  That will happen for Lamina edges.
        let Some(other_original_edgeuse) = other_original_edgeuse else {
            return Ok(out);
        };

        let orig_face = brep
            .edgeuse_face(original_edgeuse)?
            .ok_or(dangling("edge use without face"))?;
        let other_orig_face = brep
            .edgeuse_face(other_original_edgeuse)?
            .ok_or(dangling("edge use without face"))?;

        // when Orig or OtherOrig face is a ShellFace (not offset - just copied) and we are insetting
        if !shell_tangent_gap
            && (contains(&shell_faces, &orig_face) || contains(&shell_faces, &other_orig_face))
            && executive.inset()
        {
            // when OrigFace is not a shell face and we have an OffsetEdgeuse
            if let (false, Some(off)) = (contains(&shell_faces, &orig_face), offset_edgeuse) {
                let offset_edge = brep
                    .edgeuse_edge(off)?
                    .ok_or(dangling("edge use without edge"))?;
                let other_surf = brep
                    .face_surface(other_orig_face)?
                    .ok_or(dangling("face without surface"))?;
                // a problem is only reported
                let _ = ignore_soft(brep_mate_lamina_edge_to_surface(
                    k,
                    brep,
                    offset_edge,
                    other_surf,
                    1.5,
                    10.0,
                ))?;
            }

            // when OtherFace is not a shell face and we have an OtherOffsetEdgeuse
            if let (false, Some(other_off)) = (
                contains(&shell_faces, &other_orig_face),
                other_offset_edgeuse,
            ) {
                let other_offset_edge = brep
                    .edgeuse_edge(other_off)?
                    .ok_or(dangling("edge use without edge"))?;
                let surf = brep
                    .face_surface(orig_face)?
                    .ok_or(dangling("face without surface"))?;
                let _ = ignore_soft(brep_mate_lamina_edge_to_surface(
                    k,
                    brep,
                    other_offset_edge,
                    surf,
                    1.5,
                    10.0,
                ))?;
            }

            // all done - Don't need to build one if it is an inset
            return Ok(out);
        } // end inset on shell face check

        // let pCenterLine = temporary copy of OrigEU->Edge->Curve
        // (SM_CAST_PTR(SmBSplineCurve, ..), NER, then Copy: the copy keeps the curve's kind)
        let e_curve = brep
            .edge_curve(orig_edge)?
            .ok_or(SolidError::null_pointer(OP_EDGEUSE))?;
        let mut center_line = b_spline_kind_of(brep, e_curve)?
            .ok_or(SolidError::null_pointer(OP_EDGEUSE))?
            .copy(k)?;

        // get OrigEU->Edge interval (snapped by the trims below)
        let mut edge_ivl = brep.edge_interval(orig_edge)?;

        // Let RailCurves = 3dCurve Copies of offsetEdge->Curves - trimmed to edge intervals
        let mut rail1: Option<BSplineCurve> = None;
        let mut rail2: Option<BSplineCurve> = None;
        if let Some(off) = offset_edgeuse {
            let off_edge = brep
                .edgeuse_edge(off)?
                .ok_or(dangling("edge use without edge"))?;
            if let Some(h) = brep.edge_create_trimmed_nurbs_curve(k, off_edge)? {
                rail1 = b_spline_of(brep, h)?;
            }
        }
        if let Some(other_off) = other_offset_edgeuse {
            let off_edge = brep
                .edgeuse_edge(other_off)?
                .ok_or(dangling("edge use without edge"))?;
            if let Some(h) = brep.edge_create_trimmed_nurbs_curve(k, off_edge)? {
                rail2 = b_spline_of(brep, h)?;
            }
        }

        // When not given an OffsetEdgeuse - make a Rail1Curve by approximating OrigEU->UVTrimCurve Offset
        if rail1.is_none() {
            // make temporary copy of the OrigEU->UVTrimCurve trimmed to sEdgeIvl
            let uv = brep
                .edgeuse_uv_trim_curve(original_edgeuse)?
                .ok_or(SolidError::null_pointer(OP_EDGEUSE))?;
            let mut uv_copy = curve_of(brep, uv)?.copy(k)?;
            uv_copy.trim(k, &mut edge_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots

            // make an offset surface from origEU->Face->Surface
            let surface = brep
                .face_surface(orig_face)?
                .ok_or(SolidError::null_pointer(OP_EDGEUSE))?;
            let fu = brep
                .edgeuse_faceuse(original_edgeuse)?
                .ok_or(dangling("edge use without face use"))?;
            let sign = if brep.faceuse_orientation(fu)? == OrientType::Same {
                1.0
            } else {
                -1.0
            };

            // let pRail1Curve = approximation of the 3dCurve on the offset surface
            rail1 = Some(offset_surface_rail(
                k,
                brep,
                surface,
                uv_copy,
                sign * self.offset_distance,
                false,
                rail2.as_ref(),
                self.this_approx_tol_3d,
                ApproximateCurveFlags {
                    create_analytics: true,
                    match_parameterization: false,
                    just_copy_b_splines: false,
                },
            )?);
        } // end not given an OffsetEdgeuse check

        // When not given an OtherOffsetEdgeuse - make a Rail2Curve by approximating OtherOrigEU->UVTrimCurve Offset
        if rail2.is_none() {
            let uv = brep
                .edgeuse_uv_trim_curve(other_original_edgeuse)?
                .ok_or(SolidError::null_pointer(OP_EDGEUSE))?;
            let mut uv_copy = curve_of(brep, uv)?.copy(k)?;
            uv_copy.trim(k, &mut edge_ivl, TrimMode::Notify)?;

            let surface = brep
                .face_surface(other_orig_face)?
                .ok_or(SolidError::null_pointer(OP_EDGEUSE))?;
            let fu = brep
                .edgeuse_faceuse(other_original_edgeuse)?
                .ok_or(dangling("edge use without face use"))?;
            let sign = if brep.faceuse_orientation(fu)? == OrientType::Same {
                1.0
            } else {
                -1.0
            };

            rail2 = Some(offset_surface_rail(
                k,
                brep,
                surface,
                uv_copy,
                sign * self.offset_distance,
                false,
                Some(rail1.as_ref().ok_or(dangling("rail 1"))?),
                self.this_approx_tol_3d,
                ApproximateCurveFlags {
                    create_analytics: true,
                    match_parameterization: false,
                    just_copy_b_splines: false,
                },
            )?);
        } // end not given an OtherOffsetEdgeuse check

        // Make sure intervals of rail curves are the same
        let mut rail1 = rail1.ok_or(dangling("rail 1"))?;
        let mut rail2 = rail2.ok_or(dangling("rail 2"))?;
        let ivl1 = rail1.natural_interval();
        let ivl2 = rail2.natural_interval();
        if !ivl1.is_value_on_boundary(ivl2.min(), EFF_ZERO).0
            || !ivl1.is_value_on_boundary(ivl2.max(), EFF_ZERO).0
        {
            return Err(sm_err(OP_EDGEUSE)); // Non syncronized edgeuses
        }

        // get Rail1Curve/Rail2Curve max distance
        let max_dist = Curve::from(rail1.clone())
            .curve_max_distance_between(
                k,
                &rail1.natural_interval(),
                &Curve::from(rail2.clone()),
                rail2.natural_interval().min(),
                rail2.natural_interval().max(),
                30,
                None,
            )?
            .distance;

        // When railCurves are within 10*Tolerance of each other
        let e_tol = brep.edge_tolerance(orig_edge)?;
        if max_dist < e_tol * 10.0 {
            // don't build a capSurface - just increase railCurve tolerances and return
            if let Some(off) = offset_edgeuse {
                let e = brep
                    .edgeuse_edge(off)?
                    .ok_or(dangling("edge use without edge"))?;
                brep.set_edge_tolerance(k, e, max_dist * 2.0)?;
            }
            if let Some(other_off) = other_offset_edgeuse {
                let e = brep
                    .edgeuse_edge(other_off)?
                    .ok_or(dangling("edge use without edge"))?;
                brep.set_edge_tolerance(k, e, max_dist * 2.0)?;
            }
            return Ok(out);
        }

        // regularize the railCurve knot vectors by inserting knots into large spans
        if rail1.degree()? == 3 {
            rail1.dampen_knot_spacing(2.0, 3)?;
        }
        if rail2.degree()? == 3 {
            rail2.dampen_knot_spacing(2.0, 3)?;
        }

        // The offset edgeuses must be syncronized but may only be a subset of
        // the original edge.  Therefore we do a trim just to make sure.

        // get Rail1Curve StartPoint
        let rail1_ivl = rail1.natural_interval();
        let mut pnt1 = rail1.evaluate_point(rail1_ivl.min())?;

        // get CenterLine StartPoint
        let mut center_ivl = center_line.natural_interval();
        let mut pnt2 = center_line.evaluate_point(k, center_ivl.min())?;

        // When StartPoints Gap != OffsetDistance to within tolerance
        if (distance(pnt1, pnt2) - self.offset_distance).abs() > self.this_approx_tol_3d {
            // drop Rail1Curve StartPoint to CenterLine (SmBSplineCurve::Evaluate: no
            // non-zero tangent substitution)
            let pv = rail1.evaluate(rail1_ivl.min(), 1, true, false)?;
            let p0 = pv.first().copied().ok_or(dangling("evaluation"))?;
            let p1 = pv.get(1).copied().ok_or(dangling("evaluation"))?;
            let dropped = center_line.drop_point(
                k,
                &center_ivl,
                p0,
                Some(p1),
                1.1 * self.offset_distance,
                None,
                SolverOperation::Intersect,
            )?;
            // when Rail1Curve StartPoint dropped to CenterLine
            if dropped.success {
                // Trim CenterLine to Start dropPoint (both statuses ignored; the trim may
                // snap sCenterIvl by tol to existing knots)
                let _ = ignore_soft(center_ivl.set_min_max(dropped.parameter, center_ivl.max()))?;
                let _ = ignore_soft(center_line.trim(k, &mut center_ivl, TrimMode::Notify))?;
            }
        } // end Rail1/CenterLine StartPoint gapSize check

        // get Rail1 and CenterLine EndPoints
        pnt1 = rail1.evaluate_point(rail1_ivl.max())?;
        pnt2 = center_line.evaluate_point(k, center_ivl.max())?;

        // When EndPoints Gap != OffsetDistance to within tolerance
        if (distance(pnt1, pnt2) - self.offset_distance).abs() > self.this_approx_tol_3d {
            // drop Rail1Curve EndPoint to CenterLine
            let pv = rail1.evaluate(rail1_ivl.max(), 1, true, false)?;
            let p0 = pv.first().copied().ok_or(dangling("evaluation"))?;
            let p1 = -pv.get(1).copied().ok_or(dangling("evaluation"))?; // "inward-pointing vector".

            // Call DropPoint() with INTERSECT, and pass a maximum allowable distance.
            let max_drop_dist = 1.1 * self.offset_distance;

            let dropped = center_line.drop_point(
                k,
                &center_ivl,
                p0,
                Some(p1),
                max_drop_dist,
                None,
                SolverOperation::Intersect,
            )?;
            // when Rail1Curve EndPoint dropped to CenterLine
            if dropped.success {
                // Trim CenterLine to End dropPoint
                let _ = ignore_soft(center_ivl.set_min_max(center_ivl.min(), dropped.parameter))?;
                let _ = ignore_soft(center_line.trim(k, &mut center_ivl, TrimMode::Notify))?;
            }
        } // end Rail1/CenterLine EndPoint gapSize check

        // when either railCurve is degree 3 or higher
        if rail1.degree()? > 2 || rail2.degree()? > 2 {
            // make rail1/rail2/centerLine curves compatible i.e. share a common knotVector
            // (the static routine edits the NURB part of the centre line, whatever its kind)
            let cl_part =
                b_spline_part_mut(&mut center_line)?.ok_or(dangling("centre line kind"))?;
            let mut curves = [rail1, rail2, cl_part.clone()];
            BSplineCurve::make_curves_compatible(&mut curves, EFF_ZERO_PARAM)?;
            let [r1, r2, cl] = curves;
            rail1 = r1;
            rail2 = r2;
            *cl_part = cl;
        }

        // railCurve's have been manicured - make a surface between them
        let mut new_surface: Option<BSplineSurface> = None;

        // First handle case of one face being shelling face and just
        // create a linear blend between the rails or extend the surface.
        if contains(&shell_faces, &orig_face) || contains(&shell_faces, &other_orig_face) {
            // Now that we have the curves create the geometry for the surface.
            match BSplineSurface::create_trimmed_plane_from_ruled(k, &rail1, &rail2) {
                Ok(trimmed) => {
                    for c in trimmed.trimming_curves {
                        out.trimming_curves.push(store_curve(brep, c)?);
                    }
                    out.trim_orientations = trimmed.trim_orientations;
                    new_surface = Some(trimmed.plane);
                }
                Err(e @ SolidError::Unported { .. }) => return Err(e),
                // any other failure is upstream's `!= SM_SUCCESS` (e.g. the `SM_ERR` of
                // `Unitize` on the zero cross product of two collinear-derivative rails):
                // the ruled surface between the rails instead
                Err(_) => {
                    new_surface = Some(BSplineSurface::create_ruled_surface(
                        &rail1,
                        &rail2,
                        SurfParamType::U,
                    )?);
                }
            }
        } // end one face is a shell face check

        // First do a quick check for the self intersecting torus situation.
        if new_surface.is_none() {
            if let Some((ref_frame, major_radius, start_angle, end_angle)) =
                center_line.is_arc(k, 10, self.this_approx_tol_3d)?
            {
                // (`&&`: the second IsArc runs only when the first succeeds)
                let rails = match is_arc(k, &rail1, 10, self.this_approx_tol_3d)? {
                    Some((_, radius_r1, _, _)) => is_arc(k, &rail2, 10, self.this_approx_tol_3d)?
                        .map(|(_, radius_r2, _, _)| (radius_r1, radius_r2)),
                    None => None,
                };
                if let Some((radius_r1, radius_r2)) = rails {
                    // Have a torus situation
                    let cl_point = center_line
                        .evaluate_point(k, center_line.natural_interval().evaluate(0.0))?;
                    let mut r1_point =
                        rail1.evaluate_point(rail1.natural_interval().evaluate(0.0))?;
                    let mut r2_point =
                        rail2.evaluate_point(rail2.natural_interval().evaluate(0.0))?;
                    let rad1 = distance(cl_point, r1_point);
                    let rad2 = distance(cl_point, r2_point);
                    if (rad1 - rad2).abs() < EFF_ZERO * (1.0 + r1_point.max_dimension()) {
                        let minor_radius = (rad1 + rad2) / 2.0;

                        // Swap and start arc so that it corresponds to the curve which is
                        // the same radius as the center line.
                        if (major_radius - radius_r2).abs() < (major_radius - radius_r1).abs() {
                            core::mem::swap(&mut r1_point, &mut r2_point);
                        }
                        let mut arc = BSplineCurve::create_arc_from_points(
                            cl_point,
                            r1_point,
                            r2_point,
                            NurbCircleParam::Quadratic,
                        )?;

                        // Make a ray going in the right direction
                        let ray_vector = ref_frame.z_axis();
                        let ray_point =
                            ref_frame.origin() - (minor_radius + major_radius) * ray_vector;

                        if major_radius - minor_radius < -EFF_ZERO {
                            // Self intersecting torus case - create a new point and a new circle
                            // (a copy of the arc goes to the store so that the solutions
                            // name an object, as upstream's name pArc)
                            let mut arc_ivl = arc.natural_interval();
                            let arc_h = store_curve(brep, Curve::from(arc.clone()))?;
                            let rays = [ray_vector];
                            // SmTol::GetZoneTol3d(pArc): the arc has no owner, so its
                            // curve branch gives the system zone tolerance
                            let zone_tol = |o: O| zone_tol_3d_of_curve(k, curve_of(brep, o)?);
                            let sols: SolutionArray<O> = curve_of(brep, arc_h)?
                                .global_point_solve(
                                    k,
                                    &arc_ivl,
                                    SolverOperation::Rayfire,
                                    ray_point,
                                    self.this_approx_tol_3d,
                                    SolutionRequested::Single,
                                    &GlobalPointSolveOptions {
                                        target_distance: None,
                                        opt_vectors: Some(&rays),
                                        handle: Some(arc_h),
                                        analytic_handle: None,
                                        zone_tol_3d: Some(ObjectZoneTol(&zone_tol)),
                                    },
                                )?;
                            if let Some(sol) = sols.get(0) {
                                let t = sol.start.param(0)?;
                                if (t - arc_ivl.min()).abs() > EFF_ZERO_SQRT {
                                    arc_ivl.set_min_max(arc_ivl.min(), t)?;
                                    arc.trim(k, &mut arc_ivl, TrimMode::Notify)?;
                                }
                            }
                        }
                        // Now Create torus
                        new_surface = Some(BSplineSurface::create_surf_of_revolution(
                            k,
                            &arc,
                            ref_frame.origin(),
                            ref_frame.z_axis(),
                            end_angle - start_angle,
                            false,
                            None,
                        )?);
                    }
                }
            }
        } // end quick check for self intersecting torus case

        // Check for case of cone or cylinder
        if new_surface.is_none() {
            if let Some((_line_pt, line_vec)) =
                center_line.is_line(k, 10, self.this_approx_tol_3d)?
            {
                if rail1.is_line(10, self.this_approx_tol_3d)?.is_some()
                    && rail2.is_line(10, self.this_approx_tol_3d)?.is_some()
                {
                    let cl_point = center_line
                        .evaluate_point(k, center_line.natural_interval().evaluate(0.0))?;
                    let r1_point = rail1.evaluate_point(rail1.natural_interval().evaluate(0.0))?;
                    let r2_point = rail2.evaluate_point(rail2.natural_interval().evaluate(0.0))?;
                    let rad1 = distance(cl_point, r1_point);
                    let rad2 = distance(cl_point, r2_point);
                    if (rad1 - rad2).abs() < EFF_ZERO * (1.0 + r1_point.max_dimension()) {
                        let mut arc = BSplineCurve::create_arc_from_points(
                            cl_point,
                            r2_point,
                            r1_point,
                            NurbCircleParam::Quadratic,
                        )?;
                        if is_arc(k, &arc, 10, self.this_approx_tol_3d)?.is_some() {
                            new_surface =
                                Some(BSplineSurface::create_linear_sweep(&mut arc, line_vec)?);
                        }
                    }
                }
            }
        } // end cone or cylinder case

        // when case is not already handled - build a surface to fill the curves with circular cross section
        let new_surface = match new_surface {
            Some(s) => s,
            None => circular_cross_section_surface(
                k,
                // SmCircularCrossSectionFSG sFSG(FALSE): the other arguments' defaults
                CircularCrossSectionFsg {
                    approximate_arcs: false,
                    ..CircularCrossSectionFsg::default()
                },
                &mut center_line,
                &mut rail1,
                &mut rail2,
                self.this_approx_tol_3d,
                false,
            )?,
        };

        // when working with analytics - turn NewSurface into Analytic when possible
        let final_surface = if self.make_analytics {
            // Copy pNewSurface, when possible as an analytic surface
            Surface::from(new_surface).copy_and_add_analytics(k)?
        } else {
            Surface::from(new_surface)
        };
        out.surface = Some(store_surface(brep, final_surface)?);

        // (the self-intersection block is #if 0)

        // all done
        Ok(out)
    }
}

// ---- the local-operation variant --------------------------------------------------------------

/// The geometry creator of local operations (`SmLocalOperationGeometryCreation`):
/// the offset surfaces are given per face.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalOperationGeometryCreation<O> {
    /// The base object (its `m_bMakeAnalytics` is `FALSE`).
    pub base: OffsetGeometryCreation,
    /// `m_vFaces`.
    pub faces: Vec<O>,
    /// `m_vSurfaces`: the surface replacing each face's.
    pub surfaces: Vec<O>,
}

impl<O: Copy + PartialEq> LocalOperationGeometryCreation<O> {
    /// The creator over the owned-geometry face adapter ([`FaceOffsetBrep`]) of the
    /// executive: [`LocalOperationGeometryCreation::new`] without the handle bounds.
    pub(crate) fn from_handles(
        offset_distance: f64,
        this_approx_tol_3d: f64,
        faces: Vec<O>,
        surfaces: Vec<O>,
    ) -> Self {
        let mut base = OffsetGeometryCreation::new(offset_distance, this_approx_tol_3d);
        base.make_analytics = false;
        Self {
            base,
            faces,
            surfaces,
        }
    }

    /// [`LocalOperationGeometryCreation::faceuse_offset`] over the face adapter: the
    /// surface listed for the face use's face, as it is (`m_bMakeAnalytics` is `FALSE`
    /// from the constructor on, so the reparameterisation and analytic copy are never
    /// reached; a creator that asks for them is refused as `SM_ERR`).
    pub(crate) fn faceuse_offset_with_backend(
        &self,
        brep: &mut impl FaceOffsetBrep<O>,
        original_faceuse: O,
    ) -> Result<(Option<O>, SsiData<O>)> {
        let face = brep
            .faceuse_face(original_faceuse)?
            .ok_or(dangling("face use without face"))?;
        let Some(index) = self.faces.iter().position(|f| *f == face) else {
            return Err(sm_err(OP_LOCAL));
        };
        let new_surface = self
            .surfaces
            .get(index)
            .copied()
            .ok_or(dangling("surface list shorter than the face list"))?;
        if self.base.make_analytics {
            return Err(sm_err(OP_LOCAL));
        }
        Ok((Some(new_surface), SsiData::default()))
    }
}

impl<O: ClassObject + SolutionObject> LocalOperationGeometryCreation<O> {
    /// `SmLocalOperationGeometryCreation(dOffsetDistance, dThisApproxTol3d, rFaces,
    /// rSurfaces)`.
    #[must_use]
    pub fn new(offset_distance: f64, this_approx_tol_3d: f64, faces: &[O], surfaces: &[O]) -> Self {
        let mut base = OffsetGeometryCreation::new(offset_distance, this_approx_tol_3d);
        base.make_analytics = false;
        Self {
            base,
            faces: faces.to_vec(),
            surfaces: surfaces.to_vec(),
        }
    }

    /// `SmLocalOperationGeometryCreation::FaceuseOffset`: the surface listed for the
    /// face use's face (reparameterised to the face surface's natural domain and
    /// copied as an analytic surface when `m_bMakeAnalytics`, which the constructor
    /// clears). `skip_self_intersection` is `NotUsed`; the self-intersections are
    /// always empty.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the face is not listed; `SER` of the analytic copy;
    /// [`SolidError::InvalidInput`] for a face use without face, a face without
    /// surface or a listed face without surface (upstream dereferences them).
    pub fn faceuse_offset(
        &self,
        k: &Kernel,
        brep: &mut impl OffsetBrep<O>,
        original_faceuse: O,
        skip_self_intersection: bool,
    ) -> Result<(Option<O>, SsiData<O>)> {
        let _ = skip_self_intersection;
        // init output
        let self_intersections = SsiData::default();

        // locals
        let face = brep
            .faceuse_face(original_faceuse)?
            .ok_or(dangling("face use without face"))?;

        let Some(index) = self.faces.iter().position(|f| *f == face) else {
            return Err(sm_err(OP_LOCAL));
        };

        let mut new_surface = self
            .surfaces
            .get(index)
            .copied()
            .ok_or(dangling("surface list shorter than the face list"))?;

        if self.base.make_analytics {
            // (pFace->GetSurface() is dereferenced only here)
            let sur = brep
                .face_surface(face)?
                .ok_or(dangling("face without surface"))?;
            let old_domain = surface_of(brep, sur)?.natural_uv_domain_with_nurb(k)?;
            // Reparameterize's status is ignored
            {
                let s = brep
                    .surface_mut(new_surface)
                    .ok_or(dangling("surface handle cannot be resolved"))?;
                let _ = ignore_soft(s.reparameterize(k, &old_domain))?;
            }
            // Copy pS, when possible as an analytic surface (the listed surface is deleted)
            let mut analytic = surface_of(brep, new_surface)?.copy_and_add_analytics(k)?;
            if let SurfaceKind::OffsetSurface(offset) = analytic.kind_mut() {
                // SM_CAST_PTR(SmOffsetSurface, pAnalyticS)
                offset.set_allow_extension(false);
            }
            new_surface = store_surface(brep, analytic)?;
        }

        Ok((Some(new_surface), self_intersections))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_defaults() {
        let g = OffsetGeometryCreation::new(0.25, 1e-5);
        assert_eq!(g.offset_distance(), 0.25);
        assert_eq!(g.this_approx_tol_3d(), 1e-5);
        assert!((g.tangency_tol_degrees() - 2.0).abs() < 1e-12);
        assert!((g.tangency_tol_radians() - 2.0 * PI / 180.0).abs() < 1e-18);
        assert!(g.make_analytics());
        let mut g = g;
        g.set_make_analytics(false);
        assert!(!g.make_analytics());
    }

    #[test]
    fn local_operation_clears_analytics() {
        let l: LocalOperationGeometryCreation<H> =
            LocalOperationGeometryCreation::new(1.0, 1e-4, &[H(1), H(2)], &[H(10), H(20)]);
        assert!(!l.base.make_analytics());
        assert_eq!(l.faces, vec![H(1), H(2)]);
        assert_eq!(l.surfaces, vec![H(10), H(20)]);
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct H(u32);

    impl ClassObject for H {
        fn class_kind(&self) -> crate::intersect::point_class::ClassObjectKind {
            crate::intersect::point_class::ClassObjectKind::Other
        }
    }

    impl SolutionObject for H {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Other
        }
    }

    #[test]
    fn ssi_data_reset() {
        let mut d: SsiData<H> = SsiData::default();
        d.curves_3d.push(H(1));
        d.deviations.push(0.5);
        d.reset();
        assert!(d.curves_3d.is_empty() && d.deviations.is_empty());
    }
}

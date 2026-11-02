//! Edges (`SmEdge`): the edge's accessors, its edge use, vertex use, vertex, face,
//! shell and region collections, orientation and list maintenance, the radial sector
//! searches, the seam, tangency and "topological" (removable) tests, cached gap queries,
//! the blend edge use choice, the edge length integrand and notification handling.
//!
//! Upstream: `source/SMLib/src/SmEdge.cpp`, `source/SMLib/inc/SmEdge.h`.
//!
//! | Rust                                    | Upstream                                          |
//! |-----------------------------------------|---------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `SmEdge::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`Edge::new`]                           | `SmEdge::SmEdge`                                  |
//! | [`crate::topology::Edge`] (`Drop`)      | `SmEdge::~SmEdge`                                 |
//! | [`edgeuse_count`], [`primary_edgeuse`]  | `SmEdge::GetEdgeuseCount`, `GetPrimaryEdgeuse`    |
//! | [`interval`], [`set_interval`], [`curve`], [`tolerance`] | `SmEdge::GetInterval`, `SetInterval`, `GetCurve`, `GetTolerance` (`SM_USE_OLDTOL`) |
//! | [`OdeEdgeLength`]                       | `SmODEEdgeLengthIFEO` (constructor, `Evaluate`)   |
//! | [`end_tangent`]                         | `SmEdge::GetEndTangent`                           |
//! | [`uv_trim_curve_of_surface`]            | `SmEdge::GetUVTrimCurveOfSurface`                 |
//! | [`is_closed`], [`is_wire`]              | `SmEdge::IsClosed`, `IsWire`                      |
//! | [`create_trimmed_nurbs_curve`]          | `SmEdge::CreateTrimmedNURBSCurve`                 |
//! | [`edgeuse_of_face`], [`upward_edgeuse_of_face`] | `SmEdge::GetEdgeuseOfFace`, `GetUpwardEdgeuseOfFace` |
//! | [`find_radial_sector`], [`find_radial_sector_between`], [`find_radial_sector_of_curve`], [`RadialSector`] | `SmEdge::FindRadialSector`, `FindRadialSectorOfCurve` |
//! | [`reset_primary_edgeuse`]               | `SmEdge::ResetPrimaryEdgeuse`                     |
//! | [`brep`]                                | `SmEdge::GetBrep`                                 |
//! | [`set_curve`], [`SetCurveOptions`]      | `SmEdge::SetCurve`                                |
//! | [`notify`], [`NotifyEnv`]               | `SmEdge::Notify`                                  |
//! | [`set_orientation`], [`reverse_orientation`] | `SmEdge::SetOrientation`, `ReverseOrientation` |
//! | [`orient_edge_sequence`]                | `SmEdge::OrientEdgeSequence`                      |
//! | [`faces`], [`shells`], [`regions`]      | `SmEdge::GetFaces`, `GetShells`, `GetRegions`     |
//! | [`vertices`], [`vertexuses`], [`edgeuses`], [`AttributeFilter`] | `SmEdge::GetVertices(SmTArray&, ULONG*)`, `GetVertexuses`, `GetEdgeuses` |
//! | [`max_gap_3d`], [`max_up_dim_gap_3d`], [`max_down_dim_gap_3d`], [`EdgeGap`], [`MaxGap`] | `SmEdge::GetMaxGap3d`, `GetMaxUpDimGap3d`, `GetMaxDownDimGap3d` |
//! | [`number_face_occurances`]              | `SmEdge::GetNumberFaceOccurances`                 |
//! | [`edge_form`] (submodule)               | `SmEdge::IsOnFaceEdgeForm`, `sm_AssertTestEdge13` |
//! | [`add_oriented_eu_pair`]                | `SmEdge::AddOrientedEUPair`                       |
//! | [`vertex`], [`start_vertex`], [`end_vertex`], [`other_vertex`], [`start_end_vertices`] | `SmEdge::GetVertex`, `GetStartVertex`, `GetEndVertex`, `GetOtherVertex`, `GetVertices(SmVertex*&, SmVertex*&)` |
//! | [`insert_manifold_edge_end`]            | `SmEdge::InsertManifoldEdgeEnd`                   |
//! | [`is_lamina`], [`is_manifold`], [`is_spine`] | `SmEdge::IsLamina`, `IsManifold`, `IsSpine`  |
//! | [`is_seam`], [`is_seam_of_surface`]     | the two `SmEdge::IsSeam`                          |
//! | [`is_embedded`]                         | `SmEdge::IsEmbedded`                              |
//! | `cover_other_coincident_face` (private) | `sm_CoverOtherCoincidentFace` (static)            |
//! | [`is_topological`], [`Topological`]     | `SmEdge::IsTopological`                           |
//! | [`is_tangent_edge`]                     | `SmEdge::IsTangentEdge`                           |
//! | [`is_connected_to_face`], [`is_connected_to`] | `SmEdge::IsConnectedToFace`, `IsConnectedTo` |
//! | [`set_tolerance`]                       | `SmEdge::SetTolerance` (`SM_USE_OLDTOL`)          |
//! | [`blend_edgeuse`]                       | `SmEdge::GetBlendEdgeuse`                         |
//! | [`same_oriented_edgeuse`]               | `SmEdge::GetSameOrientedEdgeuse`                  |
//! | `loopuse_edges` (private)               | `SmLoopuse::GetEdges(SmTArray&)` without filter (CO-PORTED, `topology.loopuse`) |
//!
//! Deviations:
//! - **Object model.** An edge is a record in the [`Brep`]'s arenas; the methods are
//!   free functions taking the B-rep and the edge's id. Pointer arguments and results
//!   are `Option`s of ids; out-params are return values. The edge use list `m_pList`
//!   is [`crate::topology::Edge::edgeuses`] in list order from its head (`m_pNext` is
//!   the next element, cyclically) and `m_lListSize` its length; `ReverseList` keeps
//!   the head and reverses the rest. `GetPrimaryEdgeuse`, `IsLamina`, `GetBrep` and
//!   `IsWire` were CO-PORTED by earlier units and are re-exported here.
//! - **Surface identity.** Upstream compares `SmSurface*` pointers (the target of
//!   `GetUVTrimCurveOfSurface`, `IsSeam(const SmSurface&)`, `IsTopological`'s two
//!   surfaces). A port face owns its surface, so a surface is named by the face holding
//!   it: two surfaces are the same when their faces are.
//! - **Null pointers.** Where upstream dereferences a null pointer (no primary edge
//!   use, vertex use, mate, shell, face, curve, ...) the port returns
//!   [`SolidError::InvalidInput`]. Statuses are [`SolidError::Upstream`] with the
//!   upstream code (`SM_ERR`, `SM_ERR_INVALID_INPUT`). A collection upstream fills with
//!   null entries (`GetShells`, `GetRegions`, `GetVertices`, `GetVertexuses`) holds
//!   `None`s.
//! - **Attributes.** The optional attribute filters are an [`AttributeFilter`]: the
//!   caller's attribute store, the attribute id and the identity (`ObjectKey`) each
//!   topology entity is attributed under.
//! - **Notification.** [`notify`] runs the edge's part, the edge uses' and vertex uses'
//!   `Notify` and `SmTopology::Notify` with a [`NotifyEnv`] (the caller's topology
//!   records, attribute store, context and the identity of each entity).
//!   [`NotificationStores`] instead borrows only records, attributes and identities;
//!   mixed geometry/editing operations bind those stores to the actual kernel context
//!   for each notification. Legacy environments retain their explicit optional context.
//!   `SmBrep::Notify` is [`crate::topology::brep::p2::notify`] without B-rep
//!   cache (the caller-held `SM_OC_BREP` cache is not threaded through
//!   [`NotifyEnv`]; no Rust caller can build one yet); in `Notify(SM_NO_CHANGE_GEOMETRY)` the B-rep's
//!   `data3` (`this->GetCurve()`) is the old curve's identity, as in `SetCurve`.
//!   The constructor's notifications are not modelled; the record owns its curve,
//!   which is dropped with it. [`destroy`] is `delete pEdge`: the destructor's
//!   `SM_NO_DESTRUCTION` under the context's `DoingBoolean` toggle and `~SmAObject`'s
//!   `ClearAttributes` (used by `DeleteEdge` and `GlueEdgesGeneral`).
//! - **`SetCurve`** takes the new curve by value and returns the old one when it is not
//!   deleted (its owner cleared when it was this edge, as upstream); a new curve can
//!   never be the current one, so the early return only applies when both are `None`.
//!   `bDbgWarnLeaks` only drives an `SM_DBG_WARN`, compiled out of the reference
//!   build. The curves' identities for the notifications are the caller's.
//! - **Lists.** `InsertAfter` of `AddOrientedEUPair` inserts into the list holding the
//!   anchor edge use (as upstream; the edge use count is that list's length); an edge
//!   use already on a list is [`SolidError::InvalidInput`] (upstream relinks it and
//!   corrupts its old list). A missing anchor is upstream's ignored `SM_ERR`.
//!   `InsertManifoldEdgeEnd` appends both newly connected vertex uses to the target
//!   vertex's owning list and sets their owner, in native `PostInsert` order.
//! - **Gaps.** `GetMaxGap3d` and friends return copies of the cached gaps
//!   ([`EdgeGap`]) instead of pointers; `SmGap`'s comparison operators (unit
//!   `heal.gap`) compare `m_dGap3d`, as here.
//! - **Ignored statuses.** `ReverseOrientation` ignores the curve's
//!   `ReverseParameterization` status (a failure leaves the default interval);
//!   `FindRadialSectorOfCurve` skips edge uses whose classification fails;
//!   `GetBlendEdgeuse` ignores its `Unitize` statuses (a failed `Unitize` stores
//!   `(0, 0, 1)`). Where an ignored failure would leave uninitialised outputs
//!   (`GetBlendEdgeuse`'s binormal evaluations, `IsTopological`'s `EvaluatePoint`) the
//!   port returns the error. [`SolidError::Unported`] always propagates.
//! - **Recursion.** `sm_CoverOtherCoincidentFace` recurses once (its second call has
//!   `bRefineDomains` set); no other recursion.
//! - **Unported callees** return [`SolidError::Unported`]: `SmBrep::GetInfiniteRegion`
//!   (`topology.brep_p1`) and `SmFace::CalculateUVDomainFromUVTrimCurves`
//!   (`topology.face_p1`). `SmSurface::CoverOtherCoincidentSurface` is wired to
//!   `surface.surface_p5` (and its `CoverCoincidentSurface` to `surface.surface_p3`); the
//!   two face surfaces are moved out of their faces for the call and back, so the
//!   covering surface keeps any expansion, as upstream.

pub mod edge_form;

use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, EntityRef, Result, SolidError};
use crate::geom::a_object;
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::context::Context;
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol::{are_same_to_tol, scaled_zero2};
use crate::heal::gap::{EdgeFaceGap, VertexEdgeGap};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::edge::{radial_sector_of_curve_classify, CurveSectorClass, RadialSectorClass};
use crate::surface::surface::p2::PLANE_TYPE;
use crate::surface::surface::Surface;
use crate::topology::brep::p2::{self as brep_p2, NotifyArg as BrepNotifyArg};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p2 as eu2;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p2 as face2;
use crate::topology::proto_topology::p1::owning_topology;
#[cfg(test)]
use crate::topology::topology::Topology;
use crate::topology::topology::{self as topo};
use crate::topology::vertexuse as vu1;
use crate::topology::{
    Brep, Edge, EdgeId, EdgeuseId, FaceId, FaceuseId, LoopuseId, RegionId, ShellId, VertexId,
    VertexuseId,
};

pub use crate::topology::edgeuse::p1::{
    edge_brep as brep, edge_is_lamina as is_lamina, primary_edgeuse,
};
pub use crate::topology::vertexuse::edge_is_wire as is_wire;

const OP: &str = "topology::edge";

/// `SmEdge::GetType`: `SmEdge_TYPE`.
pub const TYPE: i64 = 16016;
/// `SmEdge::GetTypeString`.
pub const TYPE_STRING: &str = "SmEdge_TYPE";
/// `SmEdge::GetClassString`.
pub const CLASS_STRING: &str = "SmEdge";

/// `SmBrep_TYPE` (the `SM_CAST_PTR(SmBrep, ..)` of `Notify`).
const BREP_TYPE: i64 = 16011;
/// `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

/// `SmEdge::IsKindOf`: `SmEdge_TYPE` or a base class type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || owning_topology::is_kind_of(t)
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn status(code: i32) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code,
        entity: None,
    }
}

impl Edge {
    /// `SmEdge()`: no curve, no edge uses, the "inside out" interval and tolerance `0`.
    /// `vertices` fills the skeleton's vertex pair (not an upstream member).
    #[must_use]
    pub const fn new(vertices: [VertexId; 2]) -> Self {
        Self {
            vertices,
            edgeuses: Vec::new(),
            interval: Extent1d::new(),
            tolerance: 0.0,
            curve: None,
        }
    }
}

// ---- edge use callees (topology.edgeuse_p3) ----------------------------------------------

/// `SmEdgeuse::NormalizedEvaluate(t, TRUE, ..)`: the UV point at the normalised
/// parameter `t` (a missing UV trim curve is made).
fn normalized_evaluate_uv(k: &Kernel, b: &mut Brep, eu: EdgeuseId, t: f64) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, eu, t, true, false, true)?.point)
}

/// `SmEdgeuse::GetOrCreateUVTrimCurve(pRet)`; the status is signalled but not returned
/// by the caller (`SE`), so only [`SolidError::Unported`] (and the null dereferences)
/// escape.
fn get_or_create_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<()> {
    eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true).map(|_| ())
}

/// `pBrep->Notify(operation, data1, pBrep, data3)` (unit `topology.brep_p2`).
fn brep_notify_with_body_identity(
    b: &Brep,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data1: Option<BrepNotifyArg>,
    data3: Option<ObjectRef>,
    body: ObjectRef,
) -> Result<()> {
    let brep = BrepNotifyArg::Object(body);
    // the caller-held SM_OC_BREP cache is not threaded through `NotifyEnv` (no Rust
    // caller can build one yet: `make_brep_cache` needs `query.brep`)
    brep_p2::notify_with_body_identity(
        b,
        env,
        None,
        operation,
        [data1, Some(brep), data3.map(BrepNotifyArg::Object)],
        body,
    )
}

/// `SmBrep::GetInfiniteRegion` (unit `topology.brep_p1`).
fn infinite_region(b: &Brep) -> Option<RegionId> {
    b.infinite_region()
}

/// `SmVertex::SetTolerance(tol)` (default arguments; unit `topology.vertex`).
fn vertex_set_tolerance(b: &mut Brep, v: VertexId, tolerance: f64) -> Result<()> {
    crate::topology::vertex::set_tolerance(b, v, tolerance, true, true)
}

/// Which of the two input surfaces `SmSurface::CoverOtherCoincidentSurface` keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covering {
    /// `this` (the first surface), possibly expanded.
    This,
    /// The other surface, possibly expanded.
    Other,
}

/// The outputs of `SmSurface::CoverOtherCoincidentSurface`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cover {
    max_dist: f64,
    covering: Option<Covering>,
    merged_domain: Extent2d,
}

/// `pSurf0->CoverOtherCoincidentSurface(pSurf1, dTol, rdMaxDist, rpCoveringSurf,
/// &sDomain0, &sDomain1, &rMergedDomain)` (unit `surface.surface_p5`) on the surfaces
/// of `face0` and `face1` (two different faces). The covering surface may be expanded
/// in place, as upstream. A merged domain that was not written stays the caller's
/// default-constructed `sMergedDomain`.
///
/// Upstream's `STEPInversion` solves inside run on the faces' own surfaces, whose
/// `smsurf_GetSurfaceCache` sees each face (`GetFace()`). The surfaces are moved out of
/// their faces for the call (keeping their owners, so the caches they build are their
/// own) and lent with the B-rep through a face solve context; while they are out, each
/// face holds an evaluation copy of its surface (same owner, face tolerance and cache
/// slot) for the B-rep reads of that cache build (UV trim curves, tolerances), whose
/// edits land on the B-rep itself. The real surfaces, possibly expanded, replace the
/// copies afterwards.
#[allow(clippy::too_many_arguments)] // upstream's argument list plus the B-rep
fn cover_other_coincident_surface(
    k: &Kernel,
    b: &mut Brep,
    face0: FaceId,
    face1: FaceId,
    tol: f64,
    domain0: &Extent2d,
    domain1: &Extent2d,
) -> Result<Cover> {
    use crate::query::brep::BrepFaceServices;
    use crate::surface::surface::p5::CoverSolveContext;
    use crate::topology::brep_cache::CacheObject;

    // the evaluation copies standing in the faces while their surfaces are out
    let stand_in = |b: &Brep, f: FaceId| -> Result<Surface> {
        b.face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("sm_CoverOtherCoincidentFace: face without surface"))?
            .clone_for_evaluation()
    };
    let (copy0, copy1) = (stand_in(b, face0)?, stand_in(b, face1)?);
    // move the surfaces out of their faces for the call (keeping their owners) and back
    let mut this = b
        .face_mut(face0)?
        .surface
        .replace(copy0)
        .ok_or_else(|| null("sm_CoverOtherCoincidentFace: face without surface"))?;
    let Some(mut other) = b.face_mut(face1)?.surface.replace(copy1) else {
        b.face_mut(face0)?.surface = Some(this);
        return Err(null("sm_CoverOtherCoincidentFace: face without surface"));
    };
    let (owner0, owner1) = (this.owner(), other.owner());
    let r = this.cover_other_coincident_surface_deferring_trims_with_context(
        k,
        &mut other,
        tol,
        Some(domain0),
        Some(domain1),
        true,
        CoverSolveContext {
            context: &mut BrepFaceServices::new(&mut *b),
            handles: [
                Some(CacheObject::Surface(face0)),
                Some(CacheObject::Surface(face1)),
            ],
        },
    );
    b.face_mut(face0)?.surface = Some(this);
    b.face_mut(face1)?.surface = Some(other);
    // a face tolerance the solves raised was recorded on the copy: the real surface
    // records its face's current one (the B-rep invariant)
    b.sync_face_surface_tolerance(face0)?;
    b.sync_face_surface_tolerance(face1)?;
    let r = r?;
    // the SmFace::RemoveUVTrimCurves (status ignored) each successful
    // CoverCoincidentSurface of a face-owned surface ends with, in call order
    for owner in &r.remove_uv_trim_curves_of {
        let face = if Some(*owner) == owner0 {
            face0
        } else if Some(*owner) == owner1 {
            face1
        } else {
            return Err(SolidError::invalid_input(
                "topology::edge::cover_other_coincident_surface",
                "covered surface owned by another face",
            ));
        };
        ignore_status(crate::topology::face::p1::remove_uv_trim_curves(b, face))?;
    }
    Ok(Cover {
        max_dist: r.max_dist,
        covering: r.covering_is_other.map(|is_other| {
            if is_other {
                Covering::Other
            } else {
                Covering::This
            }
        }),
        merged_domain: r.merged_domain.unwrap_or_else(Extent2d::new),
    })
}

/// `SmFace::CalculateUVDomainFromUVTrimCurves(dom, TRUE)` (unit `topology.face_p1`).
fn calculate_uv_domain_from_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
) -> Result<Extent2d> {
    crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves(k, b, f, true, false, false)
        .map(|r| r.domain)
}

// ---- simple accessors -------------------------------------------------------------------

/// `GetEdgeuseCount`: `m_lListSize`, the number of edge uses.
pub fn edgeuse_count(b: &Brep, e: EdgeId) -> Result<usize> {
    Ok(b.edge(e)?.edgeuses.len())
}

/// `GetInterval`: the used interval of the curve.
pub fn interval(b: &Brep, e: EdgeId) -> Result<Extent1d> {
    Ok(b.edge(e)?.interval)
}

/// `SetInterval`.
pub fn set_interval(b: &mut Brep, e: EdgeId, new_interval: &Extent1d) -> Result<()> {
    b.edge_mut(e)?.interval = *new_interval;
    Ok(())
}

/// `GetCurve`: the edge's 3D curve.
pub fn curve(b: &Brep, e: EdgeId) -> Result<Option<&Curve>> {
    Ok(b.edge(e)?.curve.as_ref())
}

/// `GetTolerance` (`SM_USE_OLDTOL`): `m_sZoneTol3d`.
pub fn tolerance(b: &Brep, e: EdgeId) -> Result<f64> {
    Ok(b.edge(e)?.tolerance)
}

fn curve_deref<'b>(b: &'b Brep, e: EdgeId, what: &'static str) -> Result<&'b Curve> {
    b.edge(e)?.curve.as_ref().ok_or_else(|| null(what))
}

fn primary_deref(b: &Brep, e: EdgeId) -> Result<EdgeuseId> {
    primary_edgeuse(b, e)?.ok_or_else(|| null("edge without primary edge use"))
}

fn mate_deref(b: &Brep, eu: EdgeuseId) -> Result<EdgeuseId> {
    eu1::mate(b, eu)?.ok_or_else(|| null("edge use without mate"))
}

/// The vertex of an edge use's vertex use (`GetVertexuse()->GetVertex()`).
fn eu_vertex(b: &Brep, eu: EdgeuseId) -> Result<Option<VertexId>> {
    let vu = eu1::vertexuse(b, eu)?.ok_or_else(|| null("edge use without vertex use"))?;
    Ok(b.vertexuse(vu)?.vertex)
}

// ---- edge length integrand --------------------------------------------------------------

/// `SmODEEdgeLengthIFEO`: the integrand of an edge's precise properties: the edge
/// treated as a tube of radius `cross_section_radius`, moments about
/// `origin_of_computation`.
#[derive(Debug, Clone, Copy)]
pub struct OdeEdgeLength<'c> {
    /// `m_dCrossSectionRadius`.
    pub cross_section_radius: f64,
    /// `m_vOriginOfComputation`.
    pub origin_of_computation: Point3,
    /// `m_crCurve`.
    pub curve: &'c Curve,
}

impl<'c> OdeEdgeLength<'c> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        curve: &'c Curve,
        origin_of_computation: Point3,
        cross_section_radius: f64,
    ) -> Self {
        Self {
            cross_section_radius,
            origin_of_computation,
            curve,
        }
    }

    /// `Evaluate`: zeroes `dydx`, then stores at `t` the speed (`[1]`), the tube's area
    /// and volume deltas (`[2]`, `[3]`) and their static and second moments (`[4..=15]`,
    /// `[16..=27]`). `rYValues` is not used upstream and not taken.
    ///
    /// # Errors
    ///
    /// The curve evaluation's; [`SolidError::InvalidInput`] when `dydx` has fewer than
    /// 28 entries (upstream writes out of bounds).
    pub fn evaluate(&self, k: &Kernel, t: f64, dydx: &mut [f64]) -> Result<()> {
        for v in dydx.iter_mut() {
            *v = 0.0;
        }

        let pv = self.curve.evaluate(k, t, 1, true, true)?;
        let (Some(&p0), Some(&p1)) = (pv.first(), pv.get(1)) else {
            return Err(null("SmODEEdgeLengthIFEO::Evaluate: short evaluation"));
        };
        let p0 = p0 - self.origin_of_computation;

        let ipnt = p0; // Point to integrate
        let Some(out) = dydx.get_mut(..28) else {
            return Err(null(
                "SmODEEdgeLengthIFEO::Evaluate: derivative array shorter than 28",
            ));
        };
        let mut put = |i: usize, v: f64| {
            if let Some(s) = out.get_mut(i) {
                *s = v;
            }
        };
        put(1, 0.0);
        if p1.length() > 0.0 {
            // We integrate over the length of the curve
            put(1, p1.length());
        }

        let circumference = 2.0 * PI * self.cross_section_radius;
        let area_delta = circumference * p1.length();

        put(2, area_delta);

        // Compute area static moments
        put(4, ipnt.x * area_delta);
        put(5, ipnt.y * area_delta);
        put(6, ipnt.z * area_delta);

        // Compute area Ixx, Iyy, Izz
        put(7, (ipnt.x * ipnt.x) * area_delta);
        put(8, (ipnt.y * ipnt.y) * area_delta);
        put(9, (ipnt.z * ipnt.z) * area_delta);

        // Compute area Iyz, Izx, Ixy
        put(10, (ipnt.y * ipnt.z) * area_delta);
        put(11, (ipnt.z * ipnt.x) * area_delta);
        put(12, (ipnt.x * ipnt.y) * area_delta);

        // Compute area second moments Ixx, Iyy, Izz about coordinate axes
        put(13, (ipnt.y * ipnt.y + ipnt.z * ipnt.z) * area_delta);
        put(14, (ipnt.x * ipnt.x + ipnt.z * ipnt.z) * area_delta);
        put(15, (ipnt.x * ipnt.x + ipnt.y * ipnt.y) * area_delta);

        // Do volumetric property
        let area = PI * self.cross_section_radius * self.cross_section_radius;
        let volume_delta = area * p1.length();

        put(3, volume_delta);

        // Compute volume static moments
        put(16, ipnt.x * volume_delta);
        put(17, ipnt.y * volume_delta);
        put(18, ipnt.z * volume_delta);

        // Compute Volume Moments Ixx, Iyy, Izz
        put(19, (ipnt.x * ipnt.x) * volume_delta);
        put(20, (ipnt.y * ipnt.y) * volume_delta);
        put(21, (ipnt.z * ipnt.z) * volume_delta);

        // Compute Volume Moments Iyz, Izx, Ixy
        put(22, (ipnt.y * ipnt.z) * volume_delta);
        put(23, (ipnt.z * ipnt.x) * volume_delta);
        put(24, (ipnt.x * ipnt.y) * volume_delta);

        // Compute Volume Second Moments Ixx, Iyy, Izz about Coordinate Axes
        put(25, (ipnt.y * ipnt.y + ipnt.z * ipnt.z) * volume_delta);
        put(26, (ipnt.x * ipnt.x + ipnt.z * ipnt.z) * volume_delta);
        put(27, (ipnt.x * ipnt.x + ipnt.y * ipnt.y) * volume_delta);

        Ok(())
    }
}

// ---- geometry queries -------------------------------------------------------------------

/// `GetEndTangent`: the curve tangent at the interval's start (`at_start`) or end.
pub fn end_tangent(k: &Kernel, b: &Brep, e: EdgeId, at_start: bool) -> Result<Vec3> {
    let c = curve_deref(b, e, "GetEndTangent: edge without curve")?;
    let ivl = b.edge(e)?.interval;
    let param = if at_start { ivl.min() } else { ivl.max() };
    let pv = c.evaluate(k, param, 1, true, true)?;
    pv.get(1)
        .copied()
        .ok_or_else(|| null("GetEndTangent: short evaluation"))
}

/// Whether the surface of face `f` is the surface named by `target` (see the module
/// docs: a surface is named by the face holding it; `None` is the null surface).
fn same_surface(b: &Brep, f: FaceId, target: Option<FaceId>) -> Result<bool> {
    let this = b.face(f)?.surface.is_some().then_some(f);
    let other = match target {
        Some(t) => b.face(t)?.surface.is_some().then_some(t),
        None => None,
    };
    Ok(this == other)
}

/// `GetUVTrimCurveOfSurface`: the first edge use on a face of `surface` (named by the
/// face holding it, `None` the null surface) gets its UV trim curve made when missing
/// (`GetOrCreateUVTrimCurve`); returns that edge use (its curve is
/// [`eu1::uv_trim_curve`]), `None` when no edge use is on the surface.
pub fn uv_trim_curve_of_surface(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    surface: Option<FaceId>,
) -> Result<Option<EdgeuseId>> {
    // for every edgeuse
    for eu in edgeuses(b, e, None, None)? {
        let f = eu1::face(b, eu)?
            .ok_or_else(|| null("GetUVTrimCurveOfSurface: edge use without face"))?;
        if same_surface(b, f, surface)? {
            get_or_create_uv_trim_curve(k, b, eu)?;
            return Ok(Some(eu));
        }
    }
    Ok(None)
}

/// `IsClosed`: the primary edge use and its mate start at the same vertex.
pub fn is_closed(b: &Brep, e: EdgeId) -> Result<bool> {
    let prim = primary_deref(b, e)?;
    let v0 = eu_vertex(b, prim)?;
    let v1 = eu_vertex(b, mate_deref(b, prim)?)?;
    Ok(v0 == v1)
}

/// `new SmBSplineCurve(*SM_CAST_PTR(SmBSplineCurve, pCurve))`: the copy constructor of
/// the `SmBSplineCurve` part of a curve of a B-spline-derived kind (a slicing copy),
/// `None` for other kinds (the cast is null).
fn bspline_slice(curve: &Curve) -> Option<Curve> {
    let base = match curve.kind() {
        CurveKind::BSpline(c) => c.clone(),
        CurveKind::Line(c) => c.base.clone(),
        CurveKind::Ellipse(c) => c.base().clone(),
        CurveKind::Hyperbola(c) => c.base().clone(),
        CurveKind::Parabola(c) => c.base().clone(),
        _ => return None,
    };
    Some(Curve::from(base))
}

/// `CreateTrimmedNURBSCurve`: a B-spline copy of the edge's curve (its
/// `SmBSplineCurve` part) trimmed to the edge interval; `None` when the edge has no
/// B-spline curve (`NERN`) or the trim fails.
pub fn create_trimmed_nurbs_curve(k: &Kernel, b: &Brep, e: EdgeId) -> Result<Option<Curve>> {
    let Some(c) = b.edge(e)?.curve.as_ref() else {
        return Ok(None);
    };
    // make temporary copy of Curve
    let Some(mut copy) = bspline_slice(c) else {
        return Ok(None);
    };

    // Trim the CurveCopy to the EdgeInterval
    let mut ivl = b.edge(e)?.interval; // may snap sIvl by tol to existing knots
    match copy.trim(k, &mut ivl, TrimMode::Notify) {
        Ok(()) => Ok(Some(copy)),
        Err(err) if err.is_unported() => Err(err),
        Err(_) => Ok(None),
    }
}

/// `GetEdgeuseOfFace`: the first [`OrientType::Same`] edge use on `face`, else the
/// last one found; `None` for no face or no edge use on it.
pub fn edgeuse_of_face(b: &Brep, e: EdgeId, face: Option<FaceId>) -> Result<Option<EdgeuseId>> {
    let mut ret = None;
    let Some(face) = face else {
        return Ok(ret);
    };
    for eu in edgeuses(b, e, None, None)? {
        if eu1::face(b, eu)? == Some(face) {
            // remember any found edgeuses
            ret = Some(eu);
            // use the 1st same oriented edgeuse found
            if eu1::orientation(b, eu)? == OrientType::Same {
                break;
            }
        }
    }
    Ok(ret)
}

/// `GetUpwardEdgeuseOfFace`: the edge use on `face` through the face's upward face
/// use.
pub fn upward_edgeuse_of_face(
    b: &Brep,
    e: EdgeId,
    face: Option<FaceId>,
) -> Result<Option<EdgeuseId>> {
    let Some(face) = face else {
        return Ok(None);
    };
    let upward = face2::upward_faceuse(b, face)?;
    for eu in edgeuses(b, e, None, None)? {
        if eu1::face(b, eu)? == Some(face) && eu1::faceuse(b, eu)? == upward {
            return Ok(Some(eu));
        }
    }
    Ok(None)
}

// ---- radial sectors ---------------------------------------------------------------------

/// The answer of [`find_radial_sector`] / [`find_radial_sector_of_curve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RadialSector {
    /// `rpRadialEdgeuse`: the edge use of the sector found (or of the coincident face).
    pub radial_edgeuse: Option<EdgeuseId>,
    /// `rpCoincidentFaceuse`: the coincident face use, `None` without coincidence.
    pub coincident_faceuse: Option<FaceuseId>,
}

/// An error a caller treats as a failed status: [`SolidError::Unported`] propagates.
fn failed(e: SolidError) -> Result<()> {
    if e.is_unported() {
        Err(e)
    } else {
        Ok(())
    }
}

/// `FindRadialSector`: the radial sector of this edge containing the face of
/// `classify_eu`, among the edge uses of `region` (all regions when `None`) with
/// orientation `required_orientation`. Up to ten edge points are tried until a
/// classification is unique. `inside_faceuses` is passed on unused, as upstream.
///
/// # Errors
///
/// `SM_ERR` when no edge use passes the filters, a classification fails or none is
/// unique; [`SolidError::InvalidInput`] where upstream dereferences null;
/// [`SolidError::Unported`] from the classification.
pub fn find_radial_sector(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    classify_eu: EdgeuseId,
    region: Option<RegionId>,
    required_orientation: OrientType,
    inside_faceuses: Option<&[FaceuseId]>,
) -> Result<RadialSector> {
    find_radial_sector_between(
        k,
        b,
        e,
        None,
        classify_eu,
        region,
        required_orientation,
        inside_faceuses,
    )
}

/// `FindRadialSector` with a classified edge use from another B-rep.
/// The returned ids belong to `b`; `classify_eu` belongs to `other` (`None` is `b`
/// itself, as [`find_radial_sector`]).
///
/// # Errors
///
/// As [`find_radial_sector`].
pub fn find_radial_sector_between(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    mut other: Option<&mut Brep>,
    classify_eu: EdgeuseId,
    region: Option<RegionId>,
    required_orientation: OrientType,
    inside_faceuses: Option<&[FaceuseId]>,
) -> Result<RadialSector> {
    let _ = inside_faceuses; // LocalMerge, unused by RadialSectorClassify
    let mut out = RadialSector::default();

    // get edgeuses that pass input region and orientation specifications
    let mut cands = Vec::new();
    for eu in edgeuses(b, e, None, None)? {
        let shell =
            eu1::shell(b, eu)?.ok_or_else(|| null("FindRadialSector: edge use without shell"))?;
        let r = eu1::shell_region(b, shell)?;
        if (region.is_none() || r == region) && eu1::orientation(b, eu)? == required_orientation {
            cands.push(eu);
        }
    }

    // no work - no candidate edgeuses after culling
    if cands.is_empty() {
        return Err(SolidError::sm_err(OP));
    }

    // Special case: one possible answer and a region
    if let (1, Some(&only), Some(_)) = (cands.len(), cands.first(), region) {
        out.radial_edgeuse = Some(only);
        return Ok(out);
    }

    // Special case: wire edge
    if cands.len() == 1 && is_wire(b, e)? {
        out.radial_edgeuse = cands.first().copied();
        return Ok(out);
    }

    let faceuse_of = |b: &Brep, eu: Option<EdgeuseId>| -> Result<Option<FaceuseId>> {
        let eu = eu.ok_or_else(|| null("FindRadialSector: null radial edge use"))?;
        eu1::faceuse(b, eu)
    };

    // for 10 points along the edge until one classifies uniquely
    for ii in 0u32..10 {
        let param = if ii == 0 { 0.5 } else { f64::from(ii) / 11.0 };

        // for every candidate edgeuse
        for &eu in &cands {
            // classify given edgeuse against current candidate edgeuse
            let class = match crate::query::edge::radial_sector_classify_between(
                k,
                b,
                e,
                param,
                eu,
                other.as_deref_mut(),
                classify_eu,
            ) {
                Ok(c) => c,
                Err(err) => {
                    failed(err)?;
                    return Err(SolidError::sm_err(OP));
                }
            };

            // branch on classification = in/out/coincident
            match class {
                RadialSectorClass::Outside => {}
                RadialSectorClass::Inside => {
                    out.radial_edgeuse = Some(eu);
                    return Ok(out);
                }
                RadialSectorClass::CoincidentSector => {
                    out.radial_edgeuse = Some(eu);
                    out.coincident_faceuse = faceuse_of(b, out.radial_edgeuse)?;
                    return Ok(out);
                }
                RadialSectorClass::CoincidentSectorMate => {
                    out.radial_edgeuse = eu1::mate(b, eu)?;
                    out.coincident_faceuse = faceuse_of(b, out.radial_edgeuse)?;
                    return Ok(out);
                }
                RadialSectorClass::CoincidentRadial => {
                    out.radial_edgeuse = eu1::radial(b, eu)?;
                    out.coincident_faceuse = faceuse_of(b, out.radial_edgeuse)?;
                    return Ok(out);
                }
                RadialSectorClass::CoincidentRadialMate => {
                    let r =
                        eu1::radial(b, eu)?.ok_or_else(|| null("FindRadialSector: no radial"))?;
                    out.radial_edgeuse = eu1::mate(b, r)?;
                    out.coincident_faceuse = faceuse_of(b, out.radial_edgeuse)?;
                    return Ok(out);
                }
                RadialSectorClass::ZeroSector | RadialSectorClass::ZeroSectorMate => {
                    // Zero sector: the faces of pEU and its radial partner are parallel
                    // (coincident); with more than two faces try a different EU.
                    if faces(b, e, None)?.len() > 2 {
                        continue;
                    }
                    out.radial_edgeuse = if class == RadialSectorClass::ZeroSector {
                        Some(eu)
                    } else {
                        eu1::radial(b, eu)?
                    };
                    out.coincident_faceuse = faceuse_of(b, out.radial_edgeuse)?;
                    return Ok(out);
                }
            }
        }
    }

    // No valid answers found is a valid case.
    Err(SolidError::sm_err(OP))
}

/// `FindRadialSectorOfCurve`: the radial sector of this edge containing `curve` (over
/// `interval`) leaving the edge at `edge_param` (see
/// [`radial_sector_of_curve_classify`]); a wire edge's primary edge use contains every
/// curve. A coincidence gives the coincident face use (unless `force_sectoring`).
/// Edge uses whose classification fails are skipped; no answer is not an error.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null;
/// [`SolidError::Unported`] from the classification.
pub fn find_radial_sector_of_curve(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    edge_param: f64,
    curve: &Curve,
    interval: &Extent1d,
    orientation_to_sector: OrientType,
    force_sectoring: bool,
) -> Result<RadialSector> {
    let mut out = RadialSector::default();

    // Wire Edges contain all curves in their primary edgeuse
    if is_wire(b, e)? {
        out.radial_edgeuse = primary_edgeuse(b, e)?;
        return Ok(out);
    }

    // for every Edgeuse
    for eu in edgeuses(b, e, None, None)? {
        // classify the curve against this edgeuse
        let class = match radial_sector_of_curve_classify(
            k,
            b,
            e,
            eu,
            edge_param,
            curve,
            interval,
            orientation_to_sector,
            force_sectoring,
        ) {
            Ok(c) => c,
            Err(err) => {
                failed(err)?;
                continue;
            }
        };

        // switch on the classification result
        match class {
            CurveSectorClass::Outside => {}
            CurveSectorClass::Inside => {
                out.radial_edgeuse = Some(eu);
                return Ok(out);
            }
            CurveSectorClass::CoincidentSector => {
                out.coincident_faceuse = eu1::faceuse(b, eu)?;
                return Ok(out);
            }
            CurveSectorClass::CoincidentRadial => {
                let r = eu1::radial(b, eu)?
                    .ok_or_else(|| null("FindRadialSectorOfCurve: no radial"))?;
                out.coincident_faceuse = eu1::faceuse(b, r)?;
                return Ok(out);
            }
        }
    }

    // No valid answers found is a valid case.
    Ok(out)
}

// ---- edge use list ----------------------------------------------------------------------

/// `ResetPrimaryEdgeuse`: makes `new_primary` the head of the edge use list, reversing
/// the rest of the list when its mate does not look like a mate (on another face, or
/// in the same loop use).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` when `new_primary` is not on the list;
/// [`SolidError::InvalidInput`] where upstream dereferences null.
pub fn reset_primary_edgeuse(
    b: &mut Brep,
    e: EdgeId,
    new_primary: Option<EdgeuseId>,
) -> Result<()> {
    let list = b.edge(e)?.edgeuses.clone();
    if new_primary == list.first().copied() {
        return Ok(()); // nothing to do.
    }

    // Given EU must be one of ours.
    let pos = new_primary.and_then(|p| list.iter().position(|&x| x == p));
    let (Some(pos), Some(new_primary)) = (pos, new_primary) else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };

    // Do the update.
    let mut rotated = list.clone();
    rotated.rotate_left(pos);
    b.edge_mut(e)?.edgeuses = rotated;

    // Check whether the Mate actually looks like the Radial.
    let prim_mate = mate_deref(b, new_primary)?;
    if eu1::face(b, new_primary)? != eu1::face(b, prim_mate)?
        || eu1::loopuse(b, new_primary)? == eu1::loopuse(b, prim_mate)?
    {
        // ReverseList: keep the head, reverse the cyclic order
        let l = &mut b.edge_mut(e)?.edgeuses;
        if let Some(rest) = l.get_mut(1..) {
            rest.reverse();
        }
    }

    // UpdateListSize: the size is the list length
    Ok(())
}

/// `InsertAfter`: inserts `obj` after `cur` on `cur`'s list and makes the list's edge
/// `obj`'s owner; `SM_ERR` (ignored by the callers) for a missing `cur` or a `cur` on
/// no list.
fn insert_after(b: &mut Brep, obj: EdgeuseId, cur: Option<EdgeuseId>) -> Result<bool> {
    let Some(cur) = cur else {
        return Ok(false);
    };
    let Some(owner) = b.edgeuse(cur)?.edge else {
        return Ok(false);
    };
    let Some(pos) = b.edge(owner)?.edgeuses.iter().position(|&x| x == cur) else {
        return Ok(false);
    };
    b.edgeuse(obj)?;
    b.edge_mut(owner)?.edgeuses.insert(pos + 1, obj);
    b.edgeuse_mut(obj)?.edge = Some(owner);
    Ok(true)
}

/// Whether `eu` is on an edge use list.
fn on_a_list(b: &Brep, eu: EdgeuseId) -> Result<bool> {
    match b.edgeuse(eu)?.edge {
        Some(owner) => Ok(b.edge(owner)?.edgeuses.contains(&eu)),
        None => Ok(false),
    }
}

/// `AddOrientedEUPair`: inserts the mated pair `eu1`, `eu2` into the radial sector of
/// `sector` (after the primary edge use's mate of a lamina edge), so that orientations
/// keep alternating along the list.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null, and for an edge use
/// already on a list (see the module docs).
pub fn add_oriented_eu_pair(
    b: &mut Brep,
    e: EdgeId,
    eu1: EdgeuseId,
    eu2: EdgeuseId,
    sector: Option<EdgeuseId>,
) -> Result<()> {
    if on_a_list(b, eu1)? || on_a_list(b, eu2)? || eu1 == eu2 {
        return Err(null("AddOrientedEUPair: edge use already on a list"));
    }
    // set pInsertAfter with edgeuse to precede the mated input pair
    let insert_after_eu = if is_lamina(b, e)? {
        let prim = primary_deref(b, e)?;
        eu1::mate(b, prim)?
    } else {
        // insert mated pair between a pair of radial mates - not between a pair of side mates
        let sector = sector.ok_or_else(|| null("AddOrientedEUPair: no sector edge use"))?;
        let next = eu1::edgeuse_next(b, sector)?;
        let radial = eu1::radial(b, sector)?;
        if next == radial {
            Some(sector)
        } else {
            radial
        }
    };
    let anchor = insert_after_eu.ok_or_else(|| null("AddOrientedEUPair: null insertion point"))?;

    // insert mated pair after InsertAfter so that orientations of edge-Edgeuses oscillate
    if eu1::orientation(b, anchor)? == eu1::orientation(b, eu1)? {
        insert_after(b, eu1, Some(anchor))?; // make list {pInsertAfter, pEU1}
        insert_after(b, eu2, Some(anchor))?; // make list {pInsertAfter, pEU2, pEU1}
    } else {
        insert_after(b, eu2, Some(anchor))?; // make list {pInsertAfter, pEU2}
        insert_after(b, eu1, Some(anchor))?; // make list {pInsertAfter, pEU1, pEU2}
    }
    Ok(())
}

/// `InsertManifoldEdgeEnd`: connects the start (`is_start_vertex`) or end of this
/// manifold edge (four edge uses) to the vertex of `vertexuse`: the edge's two edge
/// uses starting there join that vertex, and the edge uses are linked into the loop
/// at that corner (around a strut end for a loop vertex use).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` when the edge has not four edge uses;
/// [`SolidError::InvalidInput`] where upstream dereferences null.
pub fn insert_manifold_edge_end(
    b: &mut Brep,
    e: EdgeId,
    vertexuse: Option<VertexuseId>,
    is_start_vertex: bool,
) -> Result<()> {
    let list = edgeuses(b, e, None, None)?;

    // check state: ThisEdge expected to be a manifold edge with 4 Edgeuses
    let [a0, a1, a2, a3] = list.as_slice() else {
        return Err(status(SM_ERR_INVALID_INPUT));
    };
    let (top1, bot1, bot2, top2) = if is_start_vertex {
        (*a0, *a1, *a2, *a3)
    } else {
        (*a3, *a2, *a1, *a0)
    };

    // same oriented EUs starting at pVertexuse->Vertex from different sides of the Edge
    let vu_top = eu1::vertexuse(b, top1)?;
    let vu_bot = eu1::vertexuse(b, bot2)?;

    // TgtVertex
    let vu = vertexuse.ok_or_else(|| null("InsertManifoldEdgeEnd: null vertex use"))?;
    let v = b
        .vertexuse(vu)?
        .vertex
        .ok_or_else(|| null("InsertManifoldEdgeEnd: vertex use without vertex"))?;
    let vu_top =
        vu_top.ok_or_else(|| null("InsertManifoldEdgeEnd: edge use without vertex use"))?;
    let vu_bot =
        vu_bot.ok_or_else(|| null("InsertManifoldEdgeEnd: edge use without vertex use"))?;

    let loop_vu = vu1::is_loop_vertexuse(b, vu)?;
    // the old corner (EdgeuseVertex branch), read before any change
    let old = if loop_vu {
        None
    } else {
        let old_top1 = eu1::vertexuse_edgeuse(b, vu)?
            .ok_or_else(|| null("InsertManifoldEdgeEnd: vertex use without edge use"))?;
        let old_top2 = b
            .edgeuse(old_top1)?
            .cw
            .ok_or_else(|| null("InsertManifoldEdgeEnd: edge use without CW neighbour"))?;
        let old_bot1 = mate_deref(b, old_top1)?;
        let old_bot2 = mate_deref(b, old_top2)?;
        Some((old_top1, old_top2, old_bot1, old_bot2))
    };

    // Insert new start vertexuses into TgtVertex
    b.vertex_mut(v)?.vertexuses.push(vu_top);
    b.vertexuse_mut(vu_top)?.vertex = Some(v);
    b.vertex_mut(v)?.vertexuses.push(vu_bot);
    b.vertexuse_mut(vu_bot)?.vertex = Some(v);

    match old {
        None => {
            // When connecting to a LoopVertex - make a Strut edge
            b.edgeuse_mut(top1)?.cw = Some(top2);
            b.edgeuse_mut(top2)?.ccw = Some(top1);
            b.edgeuse_mut(bot1)?.ccw = Some(bot2);
            b.edgeuse_mut(bot2)?.cw = Some(bot1);
        }
        Some((old_top1, old_top2, old_bot1, old_bot2)) => {
            // insert NewEdgeuse into TgtVertex CW/CCW doubly linked list
            b.edgeuse_mut(old_top1)?.cw = Some(top2);
            b.edgeuse_mut(old_top2)?.ccw = Some(top1);

            b.edgeuse_mut(old_bot1)?.ccw = Some(bot2);
            b.edgeuse_mut(old_bot2)?.cw = Some(bot1);

            b.edgeuse_mut(top2)?.ccw = Some(old_top1);
            b.edgeuse_mut(top1)?.cw = Some(old_top2);

            b.edgeuse_mut(bot2)?.cw = Some(old_bot1);
            b.edgeuse_mut(bot1)?.ccw = Some(old_bot2);
        }
    }
    Ok(())
}

// ---- collections --------------------------------------------------------------------------

/// The optional attribute filter of the collection queries (`pOptAttributeId`): only
/// entities carrying an attribute with id `attribute_id` in `attributes`, where each
/// entity is attributed under `key_of(entity)`.
#[derive(Clone, Copy)]
pub struct AttributeFilter<'a> {
    /// The attribute store.
    pub attributes: &'a AttributeStore,
    /// `*pOptAttributeId`.
    pub attribute_id: u64,
    /// The identity each topology entity is attributed under.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectKey,
}

impl core::fmt::Debug for AttributeFilter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AttributeFilter")
            .field("attribute_id", &self.attribute_id)
            .finish_non_exhaustive()
    }
}

impl AttributeFilter<'_> {
    /// `FindAttribute(attribute_id) != NULL` of `entity`.
    #[must_use]
    pub fn matches(&self, entity: EntityRef) -> bool {
        self.attributes
            .find_attribute((self.key_of)(entity), self.attribute_id)
            .is_some()
    }
}

/// `GetEdgeuses`: the edge use list from its head; with `attribute`, those carrying the
/// attribute (and connected to `connected_to` when given); else with `connected_to`,
/// those connected to it.
pub fn edgeuses(
    b: &Brep,
    e: EdgeId,
    attribute: Option<&AttributeFilter<'_>>,
    connected_to: Option<EntityRef>,
) -> Result<Vec<EdgeuseId>> {
    let list = &b.edge(e)?.edgeuses;
    match (attribute, connected_to) {
        (None, None) => Ok(list.clone()),
        (Some(filter), _) => {
            let mut out = Vec::new();
            for &eu in list {
                if filter.matches(EntityRef::Edgeuse(eu))
                    && (connected_to.is_none() || eu1::is_connected_to(b, eu, connected_to)?)
                {
                    out.push(eu);
                }
            }
            Ok(out)
        }
        (None, Some(_)) => {
            let mut out = Vec::new();
            for &eu in list {
                if eu1::is_connected_to(b, eu, connected_to)? {
                    out.push(eu);
                }
            }
            Ok(out)
        }
    }
}

/// `GetFaces`: the distinct faces of the loop edge uses in list order (loop edge uses
/// without face are skipped), filtered by `attribute`.
pub fn faces(b: &Brep, e: EdgeId, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<FaceId>> {
    let mut out: Vec<FaceId> = Vec::new();
    for eu in edgeuses(b, e, None, None)? {
        if eu1::is_loop_edgeuse(b, eu)? {
            // Skip dangling loop-typed edgeuses
            let Some(face) = eu1::face(b, eu)? else {
                continue;
            };
            if !out.contains(&face) && attribute.is_none_or(|f| f.matches(EntityRef::Face(face))) {
                out.push(face);
            }
        }
    }
    Ok(out)
}

/// `GetShells`: the distinct shells of the edge uses (a `None` for edge uses without
/// shell), filtered by `attribute`.
pub fn shells(
    b: &Brep,
    e: EdgeId,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<Option<ShellId>>> {
    let mut out = Vec::new();
    for eu in edgeuses(b, e, None, None)? {
        let shell = eu1::shell(b, eu)?;
        let keep = match attribute {
            None => true,
            Some(f) => {
                let s = shell.ok_or_else(|| null("GetShells: edge use without shell"))?;
                f.matches(EntityRef::Shell(s))
            }
        };
        if keep && !out.contains(&shell) {
            out.push(shell);
        }
    }
    Ok(out)
}

/// `GetRegions`: the distinct regions of the edge uses' shells (a `None` for shells
/// without region), filtered by `attribute`.
pub fn regions(
    b: &Brep,
    e: EdgeId,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<Option<RegionId>>> {
    let mut out = Vec::new();
    for eu in edgeuses(b, e, None, None)? {
        let shell = eu1::shell(b, eu)?.ok_or_else(|| null("GetRegions: edge use without shell"))?;
        let region = eu1::shell_region(b, shell)?;
        let keep = match attribute {
            None => true,
            Some(f) => {
                let r = region.ok_or_else(|| null("GetRegions: shell without region"))?;
                f.matches(EntityRef::Region(r))
            }
        };
        if keep && !out.contains(&region) {
            out.push(region);
        }
    }
    Ok(out)
}

/// The start and end vertex use of the edge (`GetVertexuses`' pair), `None` without
/// primary edge use.
fn vertexuse_pair(
    b: &Brep,
    e: EdgeId,
) -> Result<Option<(Option<VertexuseId>, Option<VertexuseId>)>> {
    let Some(prim) = primary_edgeuse(b, e)? else {
        return Ok(None);
    };
    let mate = mate_deref(b, prim)?;
    let (p, m) = (eu1::vertexuse(b, prim)?, eu1::vertexuse(b, mate)?);
    Ok(Some(if eu1::orientation(b, prim)? == OrientType::Same {
        (p, m)
    } else {
        (m, p)
    }))
}

/// `GetVertices(SmTArray&, ULONG*)`: the start vertex, then the end vertex when it is
/// set and differs; filtered by `attribute`; empty without primary edge use.
pub fn vertices(
    b: &Brep,
    e: EdgeId,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<Option<VertexId>>> {
    let mut out = Vec::new();
    let Some((vu1_, vu2_)) = vertexuse_pair(b, e)? else {
        return Ok(out);
    };
    let vertex_of = |vu: Option<VertexuseId>| -> Result<Option<VertexId>> {
        let vu = vu.ok_or_else(|| null("GetVertices: edge use without vertex use"))?;
        Ok(b.vertexuse(vu)?.vertex)
    };
    let v1 = vertex_of(vu1_)?;
    let v2 = vertex_of(vu2_)?;
    let has = |v: Option<VertexId>| -> Result<bool> {
        match attribute {
            None => Ok(true),
            Some(f) => {
                let v = v.ok_or_else(|| null("GetVertices: null vertex"))?;
                Ok(f.matches(EntityRef::Vertex(v)))
            }
        }
    };
    if has(v1)? {
        out.push(v1);
    }
    if v2.is_some() && v2 != v1 && has(v2)? {
        out.push(v2);
    }
    Ok(out)
}

/// `GetVertexuses`: the start and end vertex uses (by the primary edge use's
/// orientation), filtered by `attribute`; empty without primary edge use.
pub fn vertexuses(
    b: &Brep,
    e: EdgeId,
    attribute: Option<&AttributeFilter<'_>>,
) -> Result<Vec<Option<VertexuseId>>> {
    let mut out = Vec::new();
    let Some((vu1_, vu2_)) = vertexuse_pair(b, e)? else {
        return Ok(out);
    };
    for vu in [vu1_, vu2_] {
        let keep = match attribute {
            None => true,
            Some(f) => {
                let vu = vu.ok_or_else(|| null("GetVertexuses: null vertex use"))?;
                f.matches(EntityRef::Vertexuse(vu))
            }
        };
        if keep {
            out.push(vu);
        }
    }
    Ok(out)
}

/// `GetNumberFaceOccurances`: the number of edge uses on `face`'s upward face use.
pub fn number_face_occurances(b: &Brep, e: EdgeId, face: Option<FaceId>) -> Result<usize> {
    let face = face.ok_or_else(|| null("GetNumberFaceOccurances: null face"))?;
    let fu = face2::upward_faceuse(b, face)?;
    let mut count = 0;
    for eu in edgeuses(b, e, None, None)? {
        if eu1::faceuse(b, eu)? == fu {
            count += 1;
        }
    }
    Ok(count)
}

// ---- gaps ---------------------------------------------------------------------------------

/// A cached gap of [`max_gap_3d`] and friends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeGap {
    /// An edge use's largest edge/face gap.
    EdgeFace(EdgeFaceGap),
    /// A vertex use's vertex/edge gap.
    VertexEdge(VertexEdgeGap),
}

impl EdgeGap {
    /// `GetLength`: `m_dGap3d`.
    #[must_use]
    pub const fn length(&self) -> f64 {
        match self {
            Self::EdgeFace(g) => g.gap_3d,
            Self::VertexEdge(g) => g.gap_3d,
        }
    }
}

/// The result of [`max_gap_3d`] and friends.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaxGap {
    /// The largest gap (`None`: null).
    pub max: Option<EdgeGap>,
    /// `pOptGapArray`: the gaps longer than the tolerance, when asked for.
    pub gaps: Option<Vec<EdgeGap>>,
}

/// Whether `gap` goes into the gap array (longer than `tol` when given).
fn keep_gap(gap: &EdgeGap, tol: Option<f64>) -> bool {
    tol.is_none_or(|t| gap.length() > t)
}

/// `GetMaxUpDimGap3d`: the largest cached edge/face gap of the edge uses (computed
/// when not cached), with all of them longer than `tol` when `want_gaps`.
pub fn max_up_dim_gap_3d(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    want_gaps: bool,
    tol: Option<f64>,
) -> Result<MaxGap> {
    let mut out = MaxGap {
        max: None,
        gaps: want_gaps.then(Vec::new),
    };
    for eu in edgeuses(b, e, None, None)? {
        // Max Edge/Face gap
        let Some(g) = eu1::max_edge_face_gap(k, b, eu, false)? else {
            continue;
        };
        let g = EdgeGap::EdgeFace(g);
        // build Gap List
        if let Some(gaps) = out.gaps.as_mut() {
            if keep_gap(&g, tol) {
                gaps.push(g);
            }
        }
        // set output
        if out.max.is_none_or(|m| m.length() < g.length()) {
            out.max = Some(g);
        }
    }
    Ok(out)
}

/// `GetMaxDownDimGap3d`: the larger cached vertex/edge gap of the two vertex uses
/// (computed when not cached; null when either has none), with both (longer than
/// `tol`) when `want_gaps`.
pub fn max_down_dim_gap_3d(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    want_gaps: bool,
    tol: Option<f64>,
) -> Result<MaxGap> {
    let mut out = MaxGap {
        max: None,
        gaps: want_gaps.then(Vec::new),
    };
    let vus = vertexuses(b, e, None)?;

    // when the Edge is connected to vertices - return largest Vertex/Edge gap
    if let [vu0, vu1_, ..] = vus.as_slice() {
        let vu0 = vu0.ok_or_else(|| null("GetMaxDownDimGap3d: null vertex use"))?;
        let vu1_ = vu1_.ok_or_else(|| null("GetMaxDownDimGap3d: null vertex use"))?;
        let g0 = vu1::vertex_edge_gap(k, b, vu0, false)?.map(EdgeGap::VertexEdge);
        let g1 = vu1::vertex_edge_gap(k, b, vu1_, false)?.map(EdgeGap::VertexEdge);

        // build Gap List
        if let Some(gaps) = out.gaps.as_mut() {
            for g in [g0, g1].into_iter().flatten() {
                if keep_gap(&g, tol) {
                    gaps.push(g);
                }
            }
        }

        // set output
        if let (Some(g0), Some(g1)) = (g0, g1) {
            out.max = Some(if g0.length() > g1.length() { g0 } else { g1 });
        }
    }
    Ok(out)
}

/// `GetMaxGap3d`: the larger of [`max_up_dim_gap_3d`] and [`max_down_dim_gap_3d`],
/// with the gaps of both when `want_gaps`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when either is null (upstream dereferences both).
pub fn max_gap_3d(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    want_gaps: bool,
    tol: Option<f64>,
) -> Result<MaxGap> {
    // UpDim gaps
    let up = max_up_dim_gap_3d(k, b, e, want_gaps, tol)?;
    // DownDim gaps
    let down = max_down_dim_gap_3d(k, b, e, want_gaps, tol)?;

    let gaps = match (up.gaps, down.gaps) {
        (Some(mut a), Some(c)) => {
            a.extend(c);
            Some(a)
        }
        _ => None,
    };
    let (Some(u), Some(d)) = (up.max, down.max) else {
        return Err(null("GetMaxGap3d: null up or down dimension gap"));
    };
    Ok(MaxGap {
        max: Some(if u.length() > d.length() { u } else { d }),
        gaps,
    })
}

// ---- vertices -----------------------------------------------------------------------------

/// `GetVertex`: the vertex of the primary edge use.
pub fn vertex(b: &Brep, e: EdgeId) -> Result<Option<VertexId>> {
    eu_vertex(b, primary_deref(b, e)?)
}

/// `GetVertices(SmVertex*&, SmVertex*&)`: the start and end vertex.
pub fn start_end_vertices(b: &Brep, e: EdgeId) -> Result<(Option<VertexId>, Option<VertexId>)> {
    let prim = primary_deref(b, e)?;
    if eu1::orientation(b, prim)? == OrientType::Same {
        Ok((eu_vertex(b, prim)?, eu_vertex(b, mate_deref(b, prim)?)?))
    } else {
        let end = eu_vertex(b, prim)?;
        Ok((eu_vertex(b, mate_deref(b, prim)?)?, end))
    }
}

/// `GetStartVertex`.
pub fn start_vertex(b: &Brep, e: EdgeId) -> Result<Option<VertexId>> {
    let prim = primary_deref(b, e)?;
    if eu1::orientation(b, prim)? == OrientType::Same {
        eu_vertex(b, prim)
    } else {
        eu_vertex(b, mate_deref(b, prim)?)
    }
}

/// `GetEndVertex`.
pub fn end_vertex(b: &Brep, e: EdgeId) -> Result<Option<VertexId>> {
    let prim = primary_deref(b, e)?;
    if eu1::orientation(b, prim)? == OrientType::Same {
        eu_vertex(b, mate_deref(b, prim)?)
    } else {
        eu_vertex(b, prim)
    }
}

/// `GetOtherVertex`: the edge's other vertex than `vertex`; `None` when an end is
/// missing or `vertex` is neither end (upstream signals `SM_ERR`).
pub fn other_vertex(b: &Brep, e: EdgeId, vertex: Option<VertexId>) -> Result<Option<VertexId>> {
    let mut v1 = None;
    let mut v2 = None;
    if let Some(prim) = primary_edgeuse(b, e)? {
        if let Some(vu) = eu1::vertexuse(b, prim)? {
            v1 = b.vertexuse(vu)?.vertex;
        }
        if let Some(m) = eu1::mate(b, prim)? {
            if let Some(vu) = eu1::vertexuse(b, m)? {
                v2 = b.vertexuse(vu)?.vertex;
            }
        }
    }
    let (Some(a), Some(c)) = (v1, v2) else {
        return Ok(None);
    };
    if Some(a) == vertex {
        Ok(Some(c))
    } else if Some(c) == vertex {
        Ok(Some(a))
    } else {
        Ok(None)
    }
}

/// `GetSameOrientedEdgeuse`: the primary edge use when it runs with the curve, else its
/// mate.
pub fn same_oriented_edgeuse(b: &Brep, e: EdgeId) -> Result<Option<EdgeuseId>> {
    let prim = primary_deref(b, e)?;
    if eu1::orientation(b, prim)? == OrientType::Same {
        Ok(Some(prim))
    } else {
        eu1::mate(b, prim)
    }
}

// ---- predicates ---------------------------------------------------------------------------

/// `IsManifold`: four edge uses, the first a loop edge use (two faces, both sides).
pub fn is_manifold(b: &Brep, e: EdgeId) -> Result<bool> {
    let list = &b.edge(e)?.edgeuses;
    // manifold edges connect to two sides of two faces through 4 edgeuses
    let [first, _, _, _] = list.as_slice() else {
        return Ok(false);
    };
    // Edgeuse types must be LoopEdgeuses
    eu1::is_loop_edgeuse(b, *first)
}

/// `IsSpine`: six or more edge uses, the head a loop edge use.
pub fn is_spine(b: &Brep, e: EdgeId) -> Result<bool> {
    let list = &b.edge(e)?.edgeuses;
    if list.len() < 6 {
        return Ok(false);
    }
    match list.first() {
        Some(&head) => eu1::is_loop_edgeuse(b, head),
        None => Ok(false),
    }
}

/// `IsEmbedded(pOptFace, bFaceHasSeam)`: the edge has four edge uses on `face` (on any
/// of its faces when `None`): an edge embedded within a single face, e.g. a crack. A
/// seam is not embedded; with `face_has_seam` false the (expensive) seam test is
/// skipped. An embedded edge can connect to other faces.
///
/// # Errors
///
/// Those of [`faces`], [`edgeuses`], the edge uses' faces and [`is_seam`].
pub fn is_embedded(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    face: Option<FaceId>,
    face_has_seam: bool,
) -> Result<bool> {
    // search face or all faces connected to this edge when face is None
    let tgt_faces = match face {
        None => faces(b, e, None)?,
        Some(f) => vec![f],
    };
    let uses = edgeuses(b, e, None, None)?;

    // for every face
    for tgt in tgt_faces {
        // count the edgeuses that connect to this face
        let mut count = 0usize;
        for &eu in &uses {
            if eu1::face(b, eu)? == Some(tgt) {
                count += 1;
            }
        }

        // when we found a face being embedded
        if count == 4 && (!face_has_seam || !is_seam(k, b, e)?) {
            return Ok(true);
        }
    }

    // else - edge is not embedded
    Ok(false)
}

/// `IsSeam()`: the edge is a seam of the surface of any of its faces
/// ([`is_seam_of_surface`]).
pub fn is_seam(k: &Kernel, b: &mut Brep, e: EdgeId) -> Result<bool> {
    for f in faces(b, e, None)? {
        if b.face(f)?.surface.is_some() && is_seam_of_surface(k, b, e, f)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `IsSeam(const SmSurface&)`: the surface of `surface_face` is closed (within the edge
/// tolerance) and exactly two of every other edge use lie on it, with UV trim curves
/// whose midpoints differ.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface or an edge use without
/// face (upstream dereferences them); those of the surface queries;
/// [`SolidError::Unported`] from `SmEdgeuse::NormalizedEvaluate`'s unported callees.
pub fn is_seam_of_surface(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    surface_face: FaceId,
) -> Result<bool> {
    let surface = b
        .face(surface_face)?
        .surface
        .as_ref()
        .ok_or_else(|| null("IsSeam: face without surface"))?;
    let edge_tol = tolerance(b, e)?;

    // surface must be closed
    let dom = surface.natural_uv_domain_with_nurb(k)?;
    if !(surface
        .is_closed(k, &dom, SurfParamType::U, Some(edge_tol), false)?
        .0
        || surface
            .is_closed(k, &dom, SurfParamType::V, Some(edge_tol), false)?
            .0)
    {
        return Ok(false);
    }

    // for everyother edgeuse on the edge's edgeuse list
    let mut count = 0usize;
    let mut on_face: [Option<EdgeuseId>; 2] = [None, None];
    for eu in edgeuses(b, e, None, None)?.into_iter().step_by(2) {
        let f = eu1::face(b, eu)?.ok_or_else(|| null("IsSeam: edge use without face"))?;
        // save and count those connected to the TargetSurface
        if same_surface(b, f, Some(surface_face))? {
            if count == 2 {
                return Ok(false);
            }
            if let Some(slot) = on_face.get_mut(count) {
                *slot = Some(eu);
            }
            count += 1;
        }
    }

    // must have 2 edgeuses to common face
    if let (2, [Some(a), Some(c)]) = (count, on_face) {
        let uv1 = match normalized_evaluate_uv(k, b, a, 0.5) {
            Ok(p) => p,
            Err(err) => {
                failed(err)?;
                return Ok(false);
            }
        };
        let uv2 = match normalized_evaluate_uv(k, b, c, 0.5) {
            Ok(p) => p,
            Err(err) => {
                failed(err)?;
                return Ok(false);
            }
        };
        // when mid-points are distinct
        if uv1.distance(uv2) > EFF_ZERO_SQ {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The result of [`is_topological`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Topological {
    /// The return value: the edge can be removed by merging its two faces.
    pub is_topological: bool,
    /// `rpSurfToKeep`: the surface to keep (named by its face; see the module docs).
    pub surf_to_keep: Option<FaceId>,
    /// `rdDistBetween`: the largest distance between the covered and covering surfaces.
    pub dist_between: f64,
    /// `pOptMergedFaceDomain`: the merged domain when it was written (`None`: left as
    /// the caller's).
    pub merged_face_domain: Option<Extent2d>,
}

/// `sm_CoverOtherCoincidentFace`: whether one face's surface covers the part of the
/// other's that its face uses (stored face domains first, then once more with
/// trim-derived domains), as `(max_dist, covering face, merged domain)`.
fn cover_other_coincident_face(
    k: &Kernel,
    b: &mut Brep,
    face0: FaceId,
    face1: FaceId,
    tol: f64,
    refine_domains: bool,
) -> Result<(f64, Option<FaceId>, Extent2d)> {
    for f in [face0, face1] {
        if b.face(f)?.surface.is_none() {
            return Err(null("sm_CoverOtherCoincidentFace: face without surface"));
        }
    }

    // Use the stored conservative face domains, without generating all UV trims.
    let mut domain0 = face2::uv_domain(b, face0)?;
    let mut domain1 = face2::uv_domain(b, face1)?;

    if refine_domains {
        let trim0 = calculate_uv_domain_from_uv_trim_curves(k, b, face0);
        if let Ok(d) = trim0 {
            if !d.is_degenerate(0.0) {
                domain0 = d;
            }
        } else if let Err(err) = trim0 {
            failed(err)?;
        }
        let trim1 = calculate_uv_domain_from_uv_trim_curves(k, b, face1);
        if let Ok(d) = trim1 {
            if !d.is_degenerate(0.0) {
                domain1 = d;
            }
        } else if let Err(err) = trim1 {
            failed(err)?;
        }
    }

    let cover = cover_other_coincident_surface(k, b, face0, face1, tol, &domain0, &domain1)?;
    let covering = cover.covering.map(|c| match c {
        Covering::This => face0,
        Covering::Other => face1,
    });
    if covering.is_some() {
        return Ok((cover.max_dist, covering, cover.merged_domain));
    }

    // only after the inexpensive stored-domain attempts have failed (one level of
    // recursion: the second call refines)
    if !refine_domains {
        return cover_other_coincident_face(k, b, face0, face1, tol, true);
    }
    Ok((cover.max_dist, None, cover.merged_domain))
}

/// `IsTopological`: whether this manifold edge can be removed by merging its two
/// faces: the faces are tangent and coincident along the edge at three samples, the
/// edge does not run along a seam of a closed surface, and either both sides are one
/// surface or (analytic or planar surfaces) one covers the other within
/// `approx_tol_3d_gain` times the smaller face approximation tolerance. As upstream,
/// the covering face's surface may be expanded in place by the coverage test.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (an edge use without
/// face, a face without surface); those of the surface queries;
/// [`SolidError::Unported`] from `SmEdgeuse::NormalizedEvaluate`'s unported callees and
/// the coverage test.
pub fn is_topological(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    approx_tol_3d_gain: f64,
) -> Result<Topological> {
    let mut out = Topological {
        is_topological: false,
        surf_to_keep: None,
        dist_between: crate::math::BIG_DOUBLE,
        merged_face_domain: None,
    };

    let eus = edgeuses(b, e, None, None)?; // expect 4 EUs because this->Edge is manifold

    // cheap test 1 - only Manifold edges can be "topological"
    let [eu_a, _, eu_c, _] = eus.as_slice() else {
        return Ok(out);
    };
    let (eu_a, eu_c) = (*eu_a, *eu_c);
    if !eu1::is_loop_edgeuse(b, eu_a)? {
        return Ok(out);
    }

    let face_of = |eu: EdgeuseId| -> Result<(FaceId, Surface)> {
        let f = eu1::face(b, eu)?.ok_or_else(|| null("IsTopological: edge use without face"))?;
        let s = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("IsTopological: face without surface"))?;
        // Upstream reads `pFace->GetSurface()` itself. An evaluation copy keeps the
        // face owner, whose tolerance `IsPlanar`'s pseudo box is expanded by; a plain
        // clone drops it, so a curved B-spline face could pass as planar.
        Ok((f, s.clone_for_evaluation()?))
    };
    // 1st edgeuse locals
    let (face1, surf1) = face_of(eu_a)?;
    // 3rd edgeuse locals = same oriented edgeuse on different face to edgeuse[0]
    let (face2_, surf2) = face_of(eu_c)?;
    let surfs = [&surf1, &surf2];
    let uv_domain = [
        surf1.natural_uv_domain_with_nurb(k)?,
        surf2.natural_uv_domain_with_nurb(k)?,
    ];
    let same_surf = face1 == face2_;

    let samples: [f64; 3] = [0.37, 0.57, 0.82];
    let mut uv = [[Point2::new(0.0, 0.0); 3]; 2];

    // normalized sample values
    for (ii, &t) in samples.iter().enumerate() {
        let p = match normalized_evaluate_uv(k, b, eu_a, t) {
            Ok(p) => p,
            Err(err) => {
                failed(err)?;
                return Ok(out);
            }
        };
        let uv0 = Point2::new(p.x, p.y);
        let norm1 = match surf1.evaluate_normal(k, uv0, Side::Right, Side::Right) {
            Ok(n) => n,
            Err(err) => {
                failed(err)?;
                return Ok(out);
            }
        };

        let p = match normalized_evaluate_uv(k, b, eu_c, t) {
            Ok(p) => p,
            Err(err) => {
                failed(err)?;
                return Ok(out);
            }
        };
        let uv1 = Point2::new(p.x, p.y);
        let norm2 = match surf2.evaluate_normal(k, uv1, Side::Right, Side::Right) {
            Ok(n) => n,
            Err(err) => {
                failed(err)?;
                return Ok(out);
            }
        };
        if let Some(row) = uv.get_mut(0).and_then(|r| r.get_mut(ii)) {
            *row = uv0;
        }
        if let Some(row) = uv.get_mut(1).and_then(|r| r.get_mut(ii)) {
            *row = uv1;
        }

        // cheap test 2: if the surfaces don't share the same normal (are not g1)
        if norm1.dot(norm2).abs() < 1.0 - EFF_ZERO_SQRT {
            return Ok(out);
        }

        // cheap test 3: coincidence check - gaps are within tol
        let gap_tol = face2::tolerance(b, face1)? + face2::tolerance(b, face2_)?;
        // statuses ignored upstream; a failure leaves an uninitialised point there, so
        // the port returns the error
        let pt1 = surf1.evaluate_point(k, uv0)?;
        let pt2 = surf2.evaluate_point(k, uv1)?;
        if pt1.distance(pt2) > gap_tol {
            return Ok(out);
        }
    }

    // Cheap test 4 - eliminate candidate edges running along seams
    let mut closed_u = [false; 2];
    let mut closed_v = [false; 2];
    for i in 0..2 {
        let (Some(s), Some(d)) = (surfs.get(i), uv_domain.get(i)) else {
            continue;
        };
        if let Some(c) = closed_u.get_mut(i) {
            *c = s.is_closed(k, d, SurfParamType::U, None, false)?.0;
        }
        if let Some(c) = closed_v.get_mut(i) {
            *c = s.is_closed(k, d, SurfParamType::V, None, false)?.0;
        }
    }

    if closed_u.iter().chain(closed_v.iter()).any(|&c| c) {
        for ii in 0..2usize {
            // skip testing the same surface twice
            if ii == 1 && same_surf {
                continue;
            }
            let (Some(pts), Some(dom)) = (uv.get(ii), uv_domain.get(ii)) else {
                continue;
            };
            let [p0, p1, p2] = *pts;
            let cu = closed_u.get(ii).copied().unwrap_or(false);
            let cv = closed_v.get(ii).copied().unwrap_or(false);
            let mut on_seam = true;
            let mut iso_u = true;
            let mut iso_v = true;

            // pick a tolerance
            let scaled_zero = 10000.0 * scaled_zero2(&p0, &p2);

            // for every sample point pair - see if the curve is Iso
            for (a, c) in [(p0, p1), (p1, p2)] {
                if !are_same_to_tol(a.x, c.x, scaled_zero) {
                    iso_u = false;
                }
                if !are_same_to_tol(a.y, c.y, scaled_zero) {
                    iso_v = false;
                }
                // nonIsoCurves can't be on seams
                if !iso_u && !iso_v {
                    on_seam = false;
                    break;
                }
            }
            if !on_seam {
                continue;
            }

            // when the curve is Iso in a nonClosed direction - it's not on a seam
            if (iso_u && !cu) || (iso_v && !cv) {
                continue;
            }

            // when IsoCurve is not on UVDomain boundary - it's not on a seam
            let (tu, tv) = dom.classify_point(p1, scaled_zero, Some(scaled_zero));
            let at_end =
                |t: ExtentPointType| matches!(t, ExtentPointType::Start | ExtentPointType::End);
            if iso_u && !at_end(tu) {
                continue;
            }
            if iso_v && !at_end(tv) {
                continue;
            }

            // cheap test 4: can't delete edges that run along a seam on any surface
            return Ok(out);
        }
    }

    // when edge is connected to same surface on both sides - it's topological
    if same_surf {
        out.surf_to_keep = Some(face1);
        out.dist_between = 0.0;
        out.merged_face_domain =
            Some(face2::uv_domain(b, face1)?.union(&face2::uv_domain(b, face2_)?));
    } else {
        // Get tolerance.
        let mut approx_tol = approx_tol_3d_gain
            * (0.5 * face2::tolerance(b, face1)?).min(0.5 * face2::tolerance(b, face2_)?);

        // Do this only for Analytic surfaces, because they are extendable.
        if !(surf1.is_analytic()? || surf1.is_planar(k, approx_tol)?.is_planar) {
            return Ok(out);
        }
        if !(surf2.is_analytic()? || surf2.is_planar(k, approx_tol)?.is_planar) {
            return Ok(out);
        }

        // If we have planes use a much tighter coincidence check tolerance
        if surf1.is_kind_of(PLANE_TYPE)? {
            approx_tol /= 10000.0;
        }

        // See if this surface can 'cover' the part of the other surface that its face uses
        let (dist, keep, merged) =
            match cover_other_coincident_face(k, b, face1, face2_, approx_tol, false) {
                Ok(r) => r,
                Err(err) => {
                    failed(err)?;
                    return Ok(out);
                }
            };
        out.dist_between = dist;
        out.surf_to_keep = keep;
        if keep.is_none() {
            return Ok(out);
        }
        out.merged_face_domain = Some(merged);
    }

    out.is_topological = true;
    Ok(out)
}

/// `IsTangentEdge`: a manifold edge whose primary edge use's sector is tangent
/// (`IsTangentSector(tangency_tol_deg, coincidence_test)`, UV trim curves made as
/// needed).
pub fn is_tangent_edge(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    tangency_tol_deg: f64,
    coincidence_test: bool,
) -> Result<bool> {
    if !is_manifold(b, e)? {
        return Ok(false);
    }
    let prim = primary_deref(b, e)?;
    eu2::is_tangent_sector(k, b, prim, tangency_tol_deg, coincidence_test, true)
}

/// `IsConnectedToFace`: `face` is one of the edge's faces.
pub fn is_connected_to_face(b: &Brep, e: EdgeId, face: Option<FaceId>) -> Result<bool> {
    let Some(face) = face else {
        return Ok(false);
    };
    Ok(faces(b, e, None)?.contains(&face))
}

/// `SmLoopuse::GetEdges(SmTArray&)` without filter (CO-PORTED from unit
/// `topology.loopuse`): the distinct edges of an edge loop use's edge uses in CCW order
/// (a `None` for an edge use without edge); empty for a vertex loop use.
fn loopuse_edges(b: &Brep, lu: LoopuseId) -> Result<Vec<Option<EdgeId>>> {
    let mut out = Vec::new();
    for eu in eu1::loopuse_edgeuses(b, lu)? {
        let e = eu1::edge(b, eu)?;
        if !out.contains(&e) {
            out.push(e);
        }
    }
    Ok(out)
}

/// `IsConnectedTo`: whether the edge connects to `target` (a vertex use of its edge
/// uses, one of its vertices, itself, one of its edge uses, a face or face use of one of
/// its faces, a loop or loop use through which it runs, one of its shells or regions);
/// `true` for `None` and any other kind.
pub fn is_connected_to(b: &Brep, e: EdgeId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };
    match target {
        EntityRef::Vertexuse(vu) => match eu1::vertexuse_edgeuse(b, vu)? {
            Some(eu) => Ok(eu1::edge(b, eu)? == Some(e)),
            None => Ok(false),
        },
        EntityRef::Vertex(v) => Ok(vertices(b, e, None)?.contains(&Some(v))),
        EntityRef::Edge(other) => Ok(other == e),
        EntityRef::Edgeuse(eu) => Ok(eu1::edge(b, eu)? == Some(e)),
        EntityRef::Face(f) => Ok(faces(b, e, None)?.contains(&f)),
        EntityRef::Faceuse(fu) => {
            // an Edge is always connected to both Face Faceuses
            match eu1::faceuse_face(b, fu)? {
                Some(f) => Ok(faces(b, e, None)?.contains(&f)),
                None => Ok(false),
            }
        }
        EntityRef::Loop(l) => {
            let lu = b
                .loop_(l)?
                .loopuse
                .ok_or_else(|| null("IsConnectedTo: loop without loop use"))?;
            Ok(loopuse_edges(b, lu)?.contains(&Some(e)))
        }
        EntityRef::Loopuse(lu) => Ok(loopuse_edges(b, lu)?.contains(&Some(e))),
        EntityRef::Shell(s) => Ok(shells(b, e, None)?.contains(&Some(s))),
        EntityRef::Region(r) => Ok(regions(b, e, None)?.contains(&Some(r))),
        // arrive here when cpConnectTgt is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

// ---- edits --------------------------------------------------------------------------------

/// `SetTolerance` (`SM_USE_OLDTOL`): sets the edge tolerance (only growing it when
/// `update_only_if_larger`, upstream default `TRUE`) and, with
/// `cascade_to_boundaries` (default `TRUE`), grows the vertex tolerances to it.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (cascading without
/// primary edge use or vertex use); [`SolidError::NotFound`] for a dangling id.
pub fn set_tolerance(
    b: &mut Brep,
    e: EdgeId,
    new_tolerance: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    // set tolerances - when asked skip shrinking tolerances
    if new_tolerance <= b.edge(e)?.tolerance && update_only_if_larger {
        return Ok(());
    }

    // set tolerance
    b.edge_mut(e)?.tolerance = new_tolerance;

    // when asked - make sure Edge->VertexTols at least as big as thie EdgeTol
    if cascade_to_boundaries {
        let v = vertex(b, e)?;
        let ov = other_vertex(b, e, v)?;
        for v in [v, ov].into_iter().flatten() {
            if b.vertex(v)?.tolerance < new_tolerance {
                vertex_set_tolerance(b, v, new_tolerance)?;
            }
        }
    }
    Ok(())
}

/// `SetOrientation`: gives every first edge use of each consecutive list pair
/// `orientation` and the second the opposite (unknown for an unknown orientation).
pub fn set_orientation(b: &mut Brep, e: EdgeId, orientation: OrientType) -> Result<()> {
    let list = edgeuses(b, e, None, None)?;
    for pair in list.chunks_exact(2) {
        let [a, c] = pair else {
            continue;
        };
        eu1::set_orientation(b, *a, orientation)?;
        let opposite = match orientation {
            OrientType::Same => OrientType::Opposite,
            OrientType::Opposite => OrientType::Same,
            OrientType::Unknown => OrientType::Unknown,
        };
        eu1::set_orientation(b, *c, opposite)?;
    }
    Ok(())
}

/// `ReverseOrientation`: reverses the curve's parameterisation, then installs the
/// reversed interval, flips every edge use's orientation and deletes its UV trim.
/// By default a failed curve reversal's status is ignored, as upstream: the edge
/// gets the default interval and its uses are still flipped. With the opt-in
/// [`crate::kernel::UpstreamFixes::edge_reversal_keeps_failed_edge`] (WR-260) a
/// failed reversal returns before changing the interval or edge uses, and an edge
/// without curve keeps its interval (upstream installs the default one; fork
/// source fork 47507b6).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] without edge uses (upstream walks a null list) and
/// those of [`eu1::set_uv_trim_curve`]; [`SolidError::Unported`] from the curve (with
/// the opt-in fix, every error of the curve reversal).
pub fn reverse_orientation(k: &Kernel, b: &mut Brep, e: EdgeId) -> Result<()> {
    // locals
    let old_ivl = b.edge(e)?.interval;
    let mut new_ivl = Extent1d::new();

    // reverse Curve Parameterization
    if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
        match c.reverse_parameterization(k, &old_ivl) {
            Ok(n) => new_ivl = n,
            Err(err) if k.fixes.edge_reversal_keeps_failed_edge => return Err(err),
            Err(err) => failed(err)?,
        }
    } else if k.fixes.edge_reversal_keeps_failed_edge {
        // nothing reversed the interval: the edge keeps it (WR-260 addendum)
        new_ivl = old_ivl;
    }
    b.edge_mut(e)?.interval = new_ivl;

    // invert EU orientations, and any uv trim curves.
    let list = b.edge(e)?.edgeuses.clone();
    if list.is_empty() {
        return Err(null("ReverseOrientation: edge without edge uses"));
    }
    for eu in list {
        // negate Edgeuse orientation
        let orient = match eu1::orientation(b, eu)? {
            OrientType::Same => OrientType::Opposite,
            OrientType::Opposite => OrientType::Same,
            OrientType::Unknown => OrientType::Unknown,
        };
        eu1::set_orientation(b, eu, orient)?;

        // TRUE = delete existing UVTrimCurve
        eu1::set_uv_trim_curve(b, eu, eu1::UvTrimCurveArg::Null, 0.0, true, None)?;
    }
    Ok(())
}

/// `OrientEdgeSequence`: reverses edges of the chain `edges` so that each ends where
/// the next starts (a closed two-edge chain keeps its first edge).
///
/// # Errors
///
/// `SM_ERR` when two consecutive edges share no vertex; those of
/// [`reverse_orientation`] and the vertex queries.
pub fn orient_edge_sequence(k: &Kernel, b: &mut Brep, edges: &[EdgeId]) -> Result<()> {
    let (Some(&e0), Some(&e1)) = (edges.first(), edges.get(1)) else {
        return Ok(());
    };
    let num = edges.len();

    let v00 = start_vertex(b, e0)?;
    let mut v01 = end_vertex(b, e0)?;
    let v10 = start_vertex(b, e1)?;
    let v11 = end_vertex(b, e1)?;

    if v00 == v10 || v00 == v11 {
        // Start of first matches start or end of the second:
        // Reverse the first -- unless it's a closed two-edge sequence.
        if num == 2 {
            if v00 == v11 && v01 == v10 {
                return Ok(()); // already in order.
            }
            if v00 == v10 && v01 == v11 {
                reverse_orientation(k, b, e1)?;
                return Ok(());
            }
        }

        // Not a closed 2-Edge sequence.
        reverse_orientation(k, b, e0)?;
        v01 = end_vertex(b, e0)?;
    }

    // Now the first edge should be oriented correctly.
    if v10 == v01 {
        // it's in order
    } else if v11 == v01 {
        reverse_orientation(k, b, e1)?;
    } else {
        return Err(SolidError::sm_err(OP));
    }

    // Now the rest of them.
    for pair in edges.windows(2).skip(1) {
        let [a, c] = pair else {
            continue;
        };
        let v01 = end_vertex(b, *a)?;
        let v10 = start_vertex(b, *c)?;
        let v11 = end_vertex(b, *c)?;
        if v01 == v10 {
            // it's in order
        } else if v01 == v11 {
            reverse_orientation(k, b, *c)?;
        } else {
            return Err(SolidError::sm_err(OP));
        }
    }
    Ok(())
}

/// Caller-owned notification data without a borrow of the modeling context.
///
/// Kernel-context editing operations borrow the actual [`Kernel::context`] only
/// while notifying, so geometry can use the same kernel between notifications.
/// Keys must distinguish all live objects, including different entity kinds and
/// different B-reps, and must remain stable for newly allocated entities.
pub struct NotificationStores<'a> {
    /// The topology records (`SmTopology` members) of the notified objects.
    pub records: &'a mut topo::NotifyRecords,
    /// The actual attribute store of the notified objects.
    pub attributes: &'a mut AttributeStore,
    /// The identity each topology entity is notified under.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
}

impl core::fmt::Debug for NotificationStores<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NotificationStores")
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}

/// The caller's state [`notify`] and [`set_curve`] pass notifications through.
/// A missing context means no context callbacks, not the kernel's context.
pub struct NotifyEnv<'a> {
    /// The topology records (`SmTopology` members) of the notified objects, keyed by
    /// their notification identities. The caller's own keys ([`NotifyEnv::key_of`]) are
    /// the caller's to keep distinct; the kernel-issued ones (curves, objects of callers
    /// without identities of their own) are unique only within one identity domain
    /// (`geom::context`), so a store holding them belongs to one domain: not shared
    /// across two contexts, nor across a `Context::take_notify_lease` that is not given
    /// back.
    pub records: &'a mut topo::NotifyRecords,
    /// The attribute store (keyed like [`NotifyEnv::records`], and scoped the same way).
    pub attributes: &'a mut AttributeStore,
    /// The context of the objects (`GetContext`).
    pub context: Option<&'a mut Context>,
    /// The identity each topology entity is notified under (the caller's; keys from
    /// `geom::context::KERNEL_KEY_BASE` up are the kernel's).
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
}

impl core::fmt::Debug for NotifyEnv<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NotifyEnv")
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}

/// Which real context an operation notifies; an external null stays null.
enum NotifyContext<'a> {
    External(Option<&'a mut Context>),
    Kernel,
}

/// Reborrowed notification data shared by legacy and kernel-context algorithms.
/// The kernel variant never retains a borrow of the kernel or its context.
pub(crate) struct NotifyState<'a> {
    pub(crate) records: &'a mut crate::topology::topology::NotifyRecords,
    pub(crate) attributes: &'a mut AttributeStore,
    pub(crate) key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    context: NotifyContext<'a>,
}

impl<'a> NotifyState<'a> {
    /// Reborrows the environment without changing its explicit context choice.
    pub(crate) fn from_env(env: &'a mut NotifyEnv<'_>) -> Self {
        Self {
            records: &mut *env.records,
            attributes: &mut *env.attributes,
            key_of: env.key_of,
            context: NotifyContext::External(env.context.as_deref_mut()),
        }
    }

    /// Uses these stores with the kernel passed to each notification boundary.
    pub(crate) fn from_kernel_stores(stores: &'a mut NotificationStores<'_>) -> Self {
        Self {
            records: &mut *stores.records,
            attributes: &mut *stores.attributes,
            key_of: stores.key_of,
            context: NotifyContext::Kernel,
        }
    }

    /// Changes only entity identity; data and context selection are reborrowed.
    pub(crate) fn rekey<'b>(
        &'b mut self,
        key_of: &'b dyn Fn(EntityRef) -> ObjectRef,
    ) -> NotifyState<'b> {
        NotifyState {
            records: &mut *self.records,
            attributes: &mut *self.attributes,
            key_of,
            context: match &mut self.context {
                NotifyContext::External(context) => NotifyContext::External(context.as_deref_mut()),
                NotifyContext::Kernel => NotifyContext::Kernel,
            },
        }
    }

    /// Binds a short-lived environment to the actual selected context.
    pub(crate) fn with_env<T>(
        &mut self,
        kernel: &mut Kernel,
        run: impl FnOnce(&mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_context(Some(&mut kernel.context), run)
    }

    /// Compatibility bridge for algorithms whose legacy API has no mutable kernel.
    /// Only the kernel-bound variant requires `kernel_context`; external `None`
    /// remains a genuine context-free notification, even when a kernel is supplied.
    pub(crate) fn with_context<T>(
        &mut self,
        kernel_context: Option<&mut Context>,
        run: impl FnOnce(&mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        let context = match &mut self.context {
            NotifyContext::External(context) => context.as_deref_mut(),
            NotifyContext::Kernel => Some(kernel_context.ok_or_else(|| {
                SolidError::invalid_input(OP, "kernel-bound notifications need the kernel context")
            })?),
        };
        run(&mut NotifyEnv {
            records: &mut *self.records,
            attributes: &mut *self.attributes,
            context,
            key_of: self.key_of,
        })
    }

    /// Like [`Self::with_env`] for an edit that also needs the kernel: a kernel-bound
    /// state lends the kernel context for the edit
    /// ([`Context::with_notification_context`]); an external one keeps its context.
    pub(crate) fn with_kernel_env<T>(
        &mut self,
        kernel: &mut Kernel,
        run: impl FnOnce(&mut Kernel, &mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        match self.context {
            NotifyContext::External(_) => self.with_context(None, |env| run(kernel, env)),
            NotifyContext::Kernel => Context::with_notification_context(kernel, |kernel, ctx| {
                self.with_context(Some(ctx), |env| run(kernel, env))
            })?,
        }
    }

    /// Preserves the legacy two-context Boolean scope, or scopes the single kernel
    /// context once when stores are kernel-bound. Restores state on every `Result` exit.
    pub(crate) fn with_doing_boolean<T>(
        &mut self,
        kernel: &mut Kernel,
        run: impl FnOnce(&mut Kernel, &mut Self) -> Result<T>,
    ) -> Result<T> {
        let previous = kernel.context.doing_boolean();
        kernel.context.set_doing_boolean(true);
        let previous_external = match &mut self.context {
            NotifyContext::External(Some(context)) => {
                let previous = context.doing_boolean();
                context.set_doing_boolean(true);
                Some(previous)
            }
            NotifyContext::External(None) | NotifyContext::Kernel => None,
        };
        let result = run(kernel, self);
        if let (NotifyContext::External(Some(context)), Some(previous)) =
            (&mut self.context, previous_external)
        {
            context.set_doing_boolean(previous);
        }
        kernel.context.set_doing_boolean(previous);
        result
    }
}

/// `Notify`: a geometry change first moves the edge in its B-rep's spatial cache
/// (`SmBrep::Notify`, with `data2` as the B-rep when it is one, else the edge's);
/// a geometry change, post-edit, split or merge in the B-rep is passed to every edge use
/// and both vertex uses (clearing their UV trim curves and cached gaps); then the
/// notification goes to `SmTopology::Notify`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (a missing vertex use
/// or mate, and those of the edge use notification); [`SolidError::Unported`] from
/// `SmBrep::Notify`'s unported callees; those of the notification handlers.
pub fn notify(
    b: &mut Brep,
    e: EdgeId,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<ObjectRef>; 3],
) -> Result<()> {
    notify_with_body_identity(b, e, env, operation, data, None)
}

fn notify_with_body_identity(
    b: &mut Brep,
    e: EdgeId,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<ObjectRef>; 3],
    body: Option<ObjectRef>,
) -> Result<()> {
    use NotifyOperation as N;
    let [d1, d2, d3] = data;
    if operation == N::ChangeGeometry {
        let has_brep = match d2 {
            Some(r) => r.type_code == BREP_TYPE,
            None => brep(b, e)?.is_some(),
        };
        if has_brep {
            // this->GetCurve() is still the old geometry (data3)
            let obj = |d: Option<ObjectRef>| d.map(BrepNotifyArg::Object);
            brep_notify_with_body_identity(
                b,
                env,
                N::RmFromBrep,
                obj(d3),
                d3,
                body.unwrap_or_else(|| brep_p2::brep_ref(b)),
            )?;
            brep_notify_with_body_identity(
                b,
                env,
                N::AddToBrep,
                obj(d1),
                d3,
                body.unwrap_or_else(|| brep_p2::brep_ref(b)),
            )?;
        }
    }
    if matches!(
        operation,
        N::ChangeGeometry | N::PostEdit | N::SplitInBrep | N::MergeInBrep
    ) {
        // pass the notify call along to all Vertexuses, Edgeuses, and Edgeuse->Vertexuses
        let eus = edgeuses(b, e, None, None)?;
        let vus = vertexuses(b, e, None)?;
        for eu in eus {
            let this = (env.key_of)(EntityRef::Edgeuse(eu));
            eu2::notify(
                b,
                eu,
                &mut *env.records,
                &mut *env.attributes,
                env.context.as_deref_mut(),
                operation,
                this,
                data,
            )?;
        }
        for vu in vus {
            let vu = vu.ok_or_else(|| null("Notify: null vertex use"))?;
            let this = (env.key_of)(EntityRef::Vertexuse(vu));
            vu1::notify(
                b,
                vu,
                &mut *env.records,
                &mut *env.attributes,
                env.context.as_deref_mut(),
                operation,
                this,
                data,
            )?;
        }
    }

    // Propagate notification up hierarchy
    let this = (env.key_of)(EntityRef::Edge(e));
    topo::notify(
        &mut *env.records,
        &mut *env.attributes,
        env.context.as_deref_mut(),
        operation,
        this,
        d1,
        d2,
        d3,
    )
}

/// Runs `body` with `env` bound to the object's context (`GetContext()`): the
/// environment's context, else `k`'s, as a destructor's `Notify` reaches it.
pub(crate) fn with_object_context<T>(
    k: &mut Kernel,
    env: &mut NotifyEnv<'_>,
    body: impl FnOnce(&mut NotifyEnv<'_>) -> T,
) -> T {
    let key_of = env.key_of;
    let context: &mut Context = match env.context.as_deref_mut() {
        Some(context) => context,
        None => &mut k.context,
    };
    let mut n = NotifyEnv {
        records: &mut *env.records,
        attributes: &mut *env.attributes,
        context: Some(context),
        key_of,
    };
    body(&mut n)
}

/// `delete pEdge`: `~SmEdge` sets the context's `DoingBoolean` (`GetContext()`: the
/// environment's context, else `k`'s) for the duration of its
/// `Notify(SM_NO_DESTRUCTION, this, NULL, NULL)`, whose status it ignores
/// ([`SolidError::Unported`] propagates), then restores it; the curve the edge owns is
/// dropped with its record; `~SmTopology` takes it off its owning list; `~SmAObject`
/// (`ClearAttributes`) removes the edge from every attribute it uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of the notification's
/// [`SolidError::Unported`] and of `ClearAttributes`.
pub fn destroy(k: &mut Kernel, b: &mut Brep, e: EdgeId, env: &mut NotifyEnv<'_>) -> Result<()> {
    b.edge(e)?;
    let this = (env.key_of)(EntityRef::Edge(e));
    let notified = with_object_context(k, env, |n| {
        let doing_boolean = n.context.as_deref().is_some_and(Context::doing_boolean);
        // set its DoingBoolean state to TRUE - delays SurfaceCache construction
        if let Some(context) = n.context.as_deref_mut() {
            context.set_doing_boolean(true);
        }
        let notified = ignore_status(notify(
            b,
            e,
            n,
            NotifyOperation::Destruction,
            [Some(this), None, None],
        ));
        // restore the context DoingBoolean state
        if let Some(context) = n.context.as_deref_mut() {
            context.set_doing_boolean(doing_boolean);
        }
        notified
    });
    notified?;
    b.remove_edge_record(e)?;
    env.attributes.clear_attributes(this.key)
}

/// The optional arguments of [`set_curve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCurveOptions {
    /// `bDeleteOldCurve` (default `FALSE`): drop the old curve instead of returning it.
    pub delete_old_curve: bool,
    /// `bDbgWarnLeaks` (default `TRUE`): only an `SM_DBG_WARN`, compiled out.
    pub dbg_warn_leaks: bool,
    /// `bCallNotify` (default `TRUE`): notify the change (which deletes the edge uses'
    /// UV trim curves) and update the B-rep's spatial cache.
    pub call_notify: bool,
}

impl Default for SetCurveOptions {
    fn default() -> Self {
        Self {
            delete_old_curve: false,
            dbg_warn_leaks: true,
            call_notify: true,
        }
    }
}

/// `SetCurve`: replaces the edge's curve. With `call_notify` the change is notified
/// (`SM_NO_CHANGE_GEOMETRY` with the new and old curves' identities `curve_refs`) and
/// the B-rep's spatial cache updated. The old curve is dropped with
/// `delete_old_curve`, else returned with its owner cleared when it was this edge (the
/// `SM_NO_CHANGE_OWNER` notification goes through `SmObject::Notify` when its identity
/// is known).
///
/// # Errors
///
/// Those of [`notify`] and of `SmBrep::Notify` ([`crate::topology::brep::p2::notify`]).
pub fn set_curve(
    b: &mut Brep,
    e: EdgeId,
    new_curve: Option<Curve>,
    options: SetCurveOptions,
    env: &mut NotifyEnv<'_>,
    curve_refs: [Option<ObjectRef>; 2],
) -> Result<Option<Curve>> {
    set_curve_with_identity(b, e, new_curve, options, env, curve_refs, None)
}

/// Replace the actual edge curve while notifying its body's issued identity.
/// The cache, UV invalidation and curve lifecycle share [`set_curve`]'s body.
/// Other edge APIs retain their existing identity contract.
///
/// # Errors
/// The same ordered notification errors as [`set_curve`].
pub fn set_curve_with_body_identity(
    b: &mut Brep,
    e: EdgeId,
    new_curve: Option<Curve>,
    options: SetCurveOptions,
    env: &mut NotifyEnv<'_>,
    curve_refs: [Option<ObjectRef>; 2],
    body: ObjectRef,
) -> Result<Option<Curve>> {
    set_curve_with_identity(b, e, new_curve, options, env, curve_refs, Some(body))
}

fn set_curve_with_identity(
    b: &mut Brep,
    e: EdgeId,
    new_curve: Option<Curve>,
    options: SetCurveOptions,
    env: &mut NotifyEnv<'_>,
    curve_refs: [Option<ObjectRef>; 2],
    body: Option<ObjectRef>,
) -> Result<Option<Curve>> {
    let [new_ref, old_ref] = curve_refs;
    if new_curve.is_none() && b.edge(e)?.curve.is_none() {
        return Ok(None);
    }

    let has_brep = !b.edge(e)?.edgeuses.is_empty() && brep(b, e)?.is_some();

    // side effect: delete all UVTrimCurves
    if options.call_notify {
        notify_with_body_identity(
            b,
            e,
            env,
            NotifyOperation::ChangeGeometry,
            [new_ref, None, old_ref],
            body,
        )?;
        if has_brep {
            // side effect: remove from spatial cache.
            let this = Some(BrepNotifyArg::Entity(EntityRef::Edge(e)));
            brep_notify_with_body_identity(
                b,
                env,
                NotifyOperation::RmFromBrep,
                this,
                old_ref,
                body.unwrap_or_else(|| brep_p2::brep_ref(b)),
            )?;
        }
    }

    // when preExisting Curve exists
    let this = (env.key_of)(EntityRef::Edge(e));
    let old = b.edge_mut(e)?.curve.take();
    let mut returned = None;
    if let Some(mut old) = old {
        if !options.delete_old_curve {
            // Do not leave a retained curve pointing at an edge that no longer uses it.
            if old.owner() == Some(this) {
                let (_, change) = old.set_owner(None, None);
                if let (Some(c), Some(r)) = (change, old_ref) {
                    a_object::notify(
                        &mut *env.attributes,
                        env.context.as_deref_mut(),
                        NotifyOperation::ChangeOwner,
                        r,
                        c.new_owner,
                        c.new_owner_brep,
                        c.old_owner,
                    )?;
                }
            }
            returned = Some(old);
        }
    }

    // update the value
    b.edge_mut(e)?.curve = new_curve;
    b.geometry_owner_keys.insert(EntityRef::Edge(e), this);

    // side effect: Add to spatial cache.
    if options.call_notify && has_brep {
        let this = Some(BrepNotifyArg::Entity(EntityRef::Edge(e)));
        brep_notify_with_body_identity(
            b,
            env,
            NotifyOperation::AddToBrep,
            this,
            new_ref,
            body.unwrap_or_else(|| brep_p2::brep_ref(b)),
        )?;
    }
    Ok(returned)
}

/// `GetBlendEdgeuse`: the edge use bounding the sector to fillet (an
/// [`OrientType::Same`] edge use of an interior region for a solid, of the concave
/// side for a shell); `None` for a lamina edge, an edge between tangent faces (within
/// 2 degrees) or an inconsistent edge use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null;
/// [`SolidError::Unported`] from `SmBrep::GetInfiniteRegion` and
/// `SmEdgeuse::EvaluateBinormal`.
pub fn blend_edgeuse(k: &Kernel, b: &mut Brep, e: EdgeId) -> Result<Option<EdgeuseId>> {
    if is_lamina(b, e)? {
        return Ok(None); // skip all lamina edges
    }

    let mut eu0 = primary_edgeuse(b, e)?;
    let eu0_r = eu0.ok_or_else(|| null("GetBlendEdgeuse: edge without primary edge use"))?;
    let eu1_r = mate_deref(b, eu0_r)?;
    let region_of = |eu: EdgeuseId| -> Result<Option<RegionId>> {
        let s =
            eu1::shell(b, eu)?.ok_or_else(|| null("GetBlendEdgeuse: edge use without shell"))?;
        eu1::shell_region(b, s)
    };
    let mut shell_filleting = false;

    // get an edgeuse with SM_OT_SAME orientation (from an interior region when working on a solid)
    if region_of(eu0_r)? == region_of(eu1_r)? {
        shell_filleting = true;
        if eu1::orientation(b, eu0_r)? == OrientType::Opposite {
            eu0 = eu1::mate(b, eu0_r)?;
        }
        if eu0.is_none() {
            return Ok(None);
        }
    } else {
        // primary and mate edgeuses are in different regions
        let shell = eu1::shell(b, eu0_r)?.ok_or_else(|| null("GetBlendEdgeuse: no shell"))?;
        let shell_b = eu1::shell_brep(b, shell)?
            .ok_or_else(|| null("GetBlendEdgeuse: shell without B-rep"))?;
        let infinite = infinite_region(shell_b);

        // get edgeuse for interior region
        if eu1::shell_region(b, shell)? == infinite {
            eu0 = eu1::mate(b, eu0_r)?;
        }
        let cur = eu0.ok_or_else(|| null("GetBlendEdgeuse: null edge use"))?;
        if eu1::orientation(b, cur)? == OrientType::Opposite {
            eu0 = eu1::radial(b, cur)?;
        }

        // check state - edgeuse should be part of interior region with SM_OT_SAME orientation
        match eu0 {
            None => return Ok(None),
            Some(x) => {
                if region_of(x)? == infinite || eu1::orientation(b, x)? == OrientType::Opposite {
                    return Ok(None);
                }
            }
        }
    }

    // pEU0 and peU1 now bound the sector to be filleted
    let Some(mut cur0) = eu0 else {
        return Ok(None);
    };
    let mut cur1 = eu1::radial(b, cur0)?.ok_or_else(|| null("GetBlendEdgeuse: no radial"))?;

    // test for smooth edge - don't fillet edges within 2 degrees of tangent
    let test_param = b.edge(e)?.interval.evaluate(0.34);

    // get normalized normal and binormals from the two edgeuses at a common point
    // (statuses ignored upstream, `SE`: a failure leaves the constructed
    // `SM_UNDEF_DOUBLE` locals)
    let mut ev0 = eu2::binormal_ignoring_status(k, b, cur0, test_param, false)?;
    let mut ev1 = eu2::binormal_ignoring_status(k, b, cur1, test_param, false)?;
    ev0.face_normal = ev0.face_normal.unitize_or_z();
    ev1.face_normal = ev1.face_normal.unitize_or_z();
    ev0.binormal = ev0.binormal.unitize_or_z();
    ev1.binormal = ev1.binormal.unitize_or_z();
    let (fu_normal, fu_normal2) = (ev0.face_normal, ev1.face_normal);
    let (bin_vec, bin_vec2) = (ev0.binormal, ev1.binormal);

    // set max nonTangent angle to 88 degrees
    let max_dot = (88.0 * PI / 180.0).cos();

    // the faces are tangent and can't be filleted. With `fillet_knife_edge_rounds`
    // (WR-570, fork b7ec55b) only when their binormals point away from each other (the
    // faces continue each other): binormals pointing the same way are a cusp (a knife
    // edge), whose ball lies back from the edge, filleted like any other edge
    let cusp = k.fixes.fillet_knife_edge_rounds && bin_vec.dot(bin_vec2) > 0.0;
    let dot1 = bin_vec2.dot(fu_normal);
    if !cusp && dot1.abs() < max_dot {
        return Ok(None); // Tangent: unable to fillet
    }
    let dot2 = bin_vec.dot(fu_normal2);
    if !cusp && dot2.abs() < max_dot {
        return Ok(None); // Tangent: unable to fillet
    }

    // when filleting a shell (not a solid) - get a sector which is not convex
    if shell_filleting {
        let step_off = 0.001;
        let opts = eu2::StepOffOptions::default();
        // `SE`: a failing step-off keeps what it wrote into the same locals
        for (eu, ev) in [(cur0, &mut ev0), (cur1, &mut ev1)] {
            if let Err(err) =
                eu2::evaluate_binormal_step_off_into(k, b, eu, test_param, step_off, &opts, ev)
            {
                if matches!(
                    err,
                    SolidError::Unported { .. }
                        | SolidError::InvalidInput { .. }
                        | SolidError::NotFound { .. }
                ) {
                    return Err(err);
                }
            }
        }
        let (pnt, fu_normal) = (ev0.point, ev0.face_normal);
        let (pnt2, fu_normal2) = (ev1.point, ev1.face_normal);
        let vec = (pnt2 - pnt).unitize_or_z();
        let fu_normal = fu_normal.unitize_or_z();
        let dot1 = vec.dot(fu_normal);
        let vec = -vec;
        let fu_normal2 = fu_normal2.unitize_or_z();

        // when dot products are both negative, sector is convex
        let dot2 = vec.dot(fu_normal2);
        if dot1 < 0.0 && dot2 < 0.0 {
            cur0 = mate_deref(b, cur0)?;
            if eu1::orientation(b, cur0)? == OrientType::Opposite {
                cur0 = eu1::radial(b, cur0)?.ok_or_else(|| null("GetBlendEdgeuse: no radial"))?;
            }
            cur1 = eu1::radial(b, cur0)?.ok_or_else(|| null("GetBlendEdgeuse: no radial"))?;
        }
    }
    let _ = cur1;
    Ok(Some(cur0))
}

#[cfg(test)]
pub(crate) mod notification_test_support {
    use super::*;
    use crate::geom::context::NotifyCallback;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Event {
        pub(crate) callback: &'static str,
        pub(crate) operation: NotifyOperation,
        pub(crate) caller: ObjectRef,
        pub(crate) data: [Option<ObjectRef>; 3],
    }

    #[derive(Debug)]
    struct Callback {
        name: &'static str,
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl NotifyCallback for Callback {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            caller: ObjectRef,
            data1: Option<ObjectRef>,
            data2: Option<ObjectRef>,
            data3: Option<ObjectRef>,
        ) -> Result<()> {
            self.events.lock().unwrap().push(Event {
                callback: self.name,
                operation,
                caller,
                data: [data1, data2, data3],
            });
            Ok(())
        }

        fn as_any(&self) -> Option<&dyn core::any::Any> {
            Some(self)
        }
    }

    pub(crate) fn install(context: &mut Context) -> Arc<Mutex<Vec<Event>>> {
        let events = Arc::new(Mutex::new(Vec::new()));
        context.set_user_notify_callback(Some(Box::new(Callback {
            name: "user",
            events: Arc::clone(&events),
        })));
        context.set_sys_notify_callback(Some(Box::new(Callback {
            name: "system",
            events: Arc::clone(&events),
        })));
        events
    }

    pub(crate) fn assert_installed(context: &Context, events: &Arc<Mutex<Vec<Event>>>) {
        for (name, callback) in [
            ("user", context.user_notify_callback()),
            ("system", context.sys_notify_callback()),
        ] {
            let callback = callback
                .unwrap()
                .as_any()
                .unwrap()
                .downcast_ref::<Callback>()
                .unwrap();
            assert_eq!(callback.name, name);
            assert!(Arc::ptr_eq(&callback.events, events));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::Curve as C;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::topology::{EdgeuseOwner, Region, Shell, Vertex, Vertexuse, VertexuseOwner};

    #[test]
    fn retained_curve_replacement_invalidates_real_uv_and_uses_issued_body() {
        use crate::geom::attribute::{ids, Attribute, Behavior};
        use crate::topology::edge::notification_test_support as callbacks;

        let mut k = Kernel::default();
        let mut b =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let legacy_key = b.key();
        let serial = b.notify_serial().unwrap();
        let key_of = |entity| crate::topology::brep::p3::topology_ref(serial, entity);
        let body = key_of(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
        assert_ne!(legacy_key, body.key);
        let id = b.edge_list()[0];
        let uses = edgeuses(&b, id, None, None).unwrap();
        for &eu in &uses {
            eu3::get_or_create_uv_trim_curve(&k, &mut b, eu, false, false, true)
                .unwrap()
                .status
                .unwrap();
        }
        assert!(uses
            .iter()
            .any(|&eu| b.edgeuse(eu).unwrap().uv_trim_curve.is_some()));
        let curve = b.edge_mut(id).unwrap().curve.as_mut().unwrap();
        let old_ref = ObjectRef {
            type_code: curve.type_code(),
            key: k
                .context
                .geometry_key(curve.notify_identity_slot())
                .unwrap(),
        };
        let mut copy = b
            .edge(id)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .copy(&k)
            .unwrap();
        let interval = b.edge(id).unwrap().interval;
        let before = b
            .edge(id)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .evaluate_point(&k, interval.evaluate(0.37))
            .unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        crate::topology::brep::p3::seed_records(&mut records, &b, &key_of);
        let mut attributes = AttributeStore::default();
        let marker = attributes
            .insert(Attribute::long(ids::USER_1, 917, Behavior::Reference))
            .unwrap();
        attributes.add_attribute(body.key, marker).unwrap();
        let events = callbacks::install(&mut k.context);
        k.context.set_doing_boolean(true);
        let new_ref = Context::with_notification_context(&mut k, |_k, context| {
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: Some(context),
                key_of: &key_of,
            };
            assert!(env.context.as_deref().unwrap().doing_boolean());
            let new_ref = crate::topology::brep::p9::issue_curve_identity(id, &mut copy, &mut env)?;
            set_curve_with_body_identity(
                &mut b,
                id,
                Some(copy),
                SetCurveOptions {
                    delete_old_curve: true,
                    ..Default::default()
                },
                &mut env,
                [Some(new_ref), Some(old_ref)],
                body,
            )?;
            Ok::<_, SolidError>(new_ref)
        })
        .unwrap()
        .unwrap();
        assert!(k.context.doing_boolean());
        callbacks::assert_installed(&k.context, &events);
        assert_eq!(b.key(), legacy_key);
        assert_ne!(new_ref.key, old_ref.key);
        let installed = b.edge(id).unwrap().curve.as_ref().unwrap();
        assert_eq!(installed.notify_key(), Some(new_ref.key));
        assert_eq!(
            installed
                .evaluate_point(&k, interval.evaluate(0.37))
                .unwrap(),
            before
        );
        assert!(uses
            .iter()
            .all(|&eu| b.edgeuse(eu).unwrap().uv_trim_curve.is_none()));
        assert_eq!(attributes.users(marker).unwrap(), &[body.key]);
        let events = events.lock().unwrap();
        let body_events: Vec<_> = events
            .iter()
            .filter(|event| event.caller.type_code == BREP_TYPE)
            .collect();
        assert_eq!(body_events.len(), 8);
        for event in body_events {
            assert_eq!(event.caller, body);
            assert_eq!(event.data[1], Some(body));
        }
        assert!(events
            .iter()
            .any(|event| event.caller == key_of(EntityRef::Edge(id))
                && event.operation == NotifyOperation::ChangeGeometry
                && event.data == [Some(new_ref), None, Some(old_ref)]));
        assert!(!events.iter().any(|event| event.caller.key == legacy_key));
    }

    #[test]
    fn kernel_notification_state_rekeys_real_stores_and_restores_scopes() {
        use crate::geom::context::{with_mark_lock, MarkType, ALL_MARKS};
        use crate::topology::edge::notification_test_support as callbacks;

        for previous in [false, true] {
            let mut kernel = Kernel::default();
            kernel.context.set_doing_boolean(previous);
            let context_address = core::ptr::from_ref(&kernel.context);
            let events = callbacks::install(&mut kernel.context);
            let source_key = |entity| match entity {
                EntityRef::Edge(id) => ObjectRef {
                    key: ObjectKey(100 + u64::from(id.index())),
                    type_code: TYPE,
                },
                _ => panic!("unexpected entity"),
            };
            let result_key = |entity| {
                let mut object = source_key(entity);
                object.key.0 += 1000;
                object
            };
            let id = EdgeId::from_index(0);
            let original = source_key(EntityRef::Edge(id));
            let result = result_key(EntityRef::Edge(id));
            let mut source_record = Topology::default();
            source_record.set_user_index1(71);
            let mut records =
                crate::topology::topology::NotifyRecords::from(std::collections::BTreeMap::from([
                    (original.key, source_record),
                    (result.key, Topology::default()),
                ]));
            let mut attributes = AttributeStore::default();
            let mut stores = NotificationStores {
                records: &mut records,
                attributes: &mut attributes,
                key_of: &source_key,
            };
            let mut state = NotifyState::from_kernel_stores(&mut stores);
            let error: Result<()> = with_mark_lock(&mut kernel, ALL_MARKS, |kernel, mark| {
                let mark_value = kernel.context.current_mark(mark);
                state.with_doing_boolean(kernel, |kernel, state| {
                    assert!(kernel.context.doing_boolean());
                    state.with_env(kernel, |env| {
                        let context = env.context.as_deref_mut().unwrap();
                        assert_eq!(core::ptr::from_ref(&*context), context_address);
                        assert!(context.doing_boolean());
                        assert!(context.is_mark_locked(mark));
                        assert_eq!(context.current_mark(mark), mark_value);
                        topo::notify(
                            env.records,
                            env.attributes,
                            Some(context),
                            NotifyOperation::Copy,
                            original,
                            Some(result),
                            None,
                            None,
                        )
                    })?;
                    state.rekey(&result_key).with_env(kernel, |env| {
                        assert_eq!((env.key_of)(EntityRef::Edge(id)), result);
                        crate::geom::object::notify(
                            env.context.as_deref_mut(),
                            NotifyOperation::PreEdit,
                            result,
                            None,
                            None,
                            None,
                        );
                        Err(SolidError::sm_err(OP))
                    })
                })
            })
            .unwrap();
            assert_eq!(error, Err(SolidError::sm_err(OP)));
            assert_eq!(records.get(&original.key).unwrap().user_index1(), 71);
            assert_eq!(records.get(&result.key).unwrap().user_index1(), 71);
            assert_eq!(kernel.context.doing_boolean(), previous);
            assert!(!kernel.context.is_mark_locked(MarkType::Mark));
            callbacks::assert_installed(&kernel.context, &events);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 4);
            for pair in events.chunks_exact(2) {
                assert_eq!((pair[0].callback, pair[1].callback), ("user", "system"));
                assert_eq!(pair[0].operation, pair[1].operation);
                assert_eq!(pair[0].caller, pair[1].caller);
            }
            assert_eq!(events[0].operation, NotifyOperation::Copy);
            assert_eq!(events[2].caller, result);
        }
    }

    #[test]
    fn external_notification_state_preserves_null_and_distinct_contexts() {
        use crate::topology::edge::notification_test_support as callbacks;

        for previous_kernel in [false, true] {
            for external in [None, Some(false), Some(true)] {
                let mut kernel = Kernel::default();
                kernel.context.set_doing_boolean(previous_kernel);
                let kernel_events = callbacks::install(&mut kernel.context);
                let mut context = Context::default();
                let external_events = callbacks::install(&mut context);
                if let Some(previous) = external {
                    context.set_doing_boolean(previous);
                }
                let mut records = crate::topology::topology::NotifyRecords::new();
                let mut attributes = AttributeStore::default();
                let key = |_| ObjectRef {
                    key: ObjectKey(123),
                    type_code: TYPE,
                };
                let mut env = NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: external.map(|_| &mut context),
                    key_of: &key,
                };
                let mut state = NotifyState::from_env(&mut env);
                let status: Result<()> = state.with_doing_boolean(&mut kernel, |kernel, state| {
                    assert!(kernel.context.doing_boolean());
                    state.rekey(&key).with_env(kernel, |env| {
                        assert_eq!(env.context.is_some(), external.is_some());
                        if let Some(context) = env.context.as_deref() {
                            assert!(context.doing_boolean());
                        }
                        crate::geom::object::notify(
                            env.context.as_deref_mut(),
                            NotifyOperation::PreEdit,
                            key(EntityRef::Edge(EdgeId::from_index(0))),
                            None,
                            None,
                            None,
                        );
                        Err(SolidError::unported("SmBrep::Notify", "topology.brep_p2"))
                    })
                });
                assert!(status.unwrap_err().is_unported());
                assert_eq!(kernel.context.doing_boolean(), previous_kernel);
                assert_eq!(context.doing_boolean(), external.unwrap_or(false));
                assert!(kernel_events.lock().unwrap().is_empty());
                assert_eq!(
                    external_events.lock().unwrap().len(),
                    if external.is_some() { 2 } else { 0 }
                );
                callbacks::assert_installed(&kernel.context, &kernel_events);
                callbacks::assert_installed(&context, &external_events);
            }
        }
    }

    #[test]
    fn insert_manifold_end_appends_the_new_vertexuses_in_native_order() {
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::ZERO).unwrap();
        let target = b
            .add_vertexuse_record(Vertexuse {
                vertex: Some(v),
                owner: VertexuseOwner::Loopuse(None),
                ..Vertexuse::default()
            })
            .unwrap();
        b.vertex_mut(v).unwrap().vertexuses.push(target);
        let edge = b.add_edge(v, v).unwrap();
        let mut uses = Vec::new();
        for _ in 0..4 {
            let vu = b.add_vertexuse_record(Vertexuse::default()).unwrap();
            let eu = b
                .add_edgeuse_record(crate::topology::Edgeuse {
                    edge: Some(edge),
                    vertexuse: Some(vu),
                    ..crate::topology::Edgeuse::default()
                })
                .unwrap();
            b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
            b.edge_mut(edge).unwrap().edgeuses.push(eu);
            uses.push(vu);
        }
        insert_manifold_edge_end(&mut b, edge, Some(target), true).unwrap();
        assert_eq!(
            b.vertex(v).unwrap().vertexuses,
            vec![target, uses[0], uses[2]]
        );
        assert_eq!(b.vertexuse(uses[0]).unwrap().vertex, Some(v));
        assert_eq!(b.vertexuse(uses[2]).unwrap().vertex, Some(v));
        insert_manifold_edge_end(&mut b, edge, Some(target), false).unwrap();
        assert_eq!(
            b.vertex(v).unwrap().vertexuses,
            vec![target, uses[0], uses[2], uses[3], uses[1]]
        );
    }

    /// A wire edge from (0,0,0) to (1,0,0) on a line B-spline: two shell edge uses and
    /// their vertex uses.
    fn wire() -> (Brep, EdgeId, [EdgeuseId; 2], [VertexId; 2]) {
        let mut b = Brep::new();
        let r = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let s = b
            .add_shell_record(Shell {
                faces: Vec::new(),
                region: Some(r),
                ..Shell::default()
            })
            .unwrap();
        let v: Vec<VertexId> = (0..2)
            .map(|i| {
                b.add_vertex_record(Vertex {
                    point: Vec3::new(f64::from(i), 0.0, 0.0),
                    tolerance: 1e-5,
                    vertexuses: Vec::new(),
                    poly_vertex: None,
                })
                .unwrap()
            })
            .collect();
        let nl = NlCurve::new(
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 0.0, 0.0, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let mut edge = Edge::new([v[0], v[1]]);
        edge.edgeuses = vec![EdgeuseId::from_index(0), EdgeuseId::from_index(1)];
        edge.interval = Extent1d::from_min_max(0.0, 1.0);
        edge.tolerance = 1e-5;
        edge.curve = Some(C::from(BSplineCurve::from_nurb(nl, 3, false)));
        let e = b.add_edge_record(edge).unwrap();
        let eus: Vec<EdgeuseId> = (0..2u32)
            .map(|i| {
                b.add_edgeuse_record(crate::topology::Edgeuse {
                    edge: Some(e),
                    owner: EdgeuseOwner::Shell(Some(s)),
                    vertexuse: Some(VertexuseId::from_index(i)),
                    orientation: if i == 0 {
                        OrientType::Same
                    } else {
                        OrientType::Opposite
                    },
                    ..crate::topology::Edgeuse::default()
                })
                .unwrap()
            })
            .collect();
        for i in 0..2 {
            b.add_vertexuse_record(Vertexuse {
                vertex: Some(v[i]),
                owner: VertexuseOwner::Edgeuse(Some(eus[i])),
                ..Vertexuse::default()
            })
            .unwrap();
        }
        (b, e, [eus[0], eus[1]], [v[0], v[1]])
    }

    #[test]
    fn kind_and_constructor() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(owning_topology::TYPE));
        assert!(!is_kind_of(16005));
        let e = Edge::new([VertexId::from_index(0); 2]);
        assert_eq!(e.tolerance, 0.0);
        assert!(e.curve.is_none() && e.edgeuses.is_empty());
    }

    #[test]
    fn wire_queries() {
        let k = &Kernel::default();
        let (b, e, eus, v) = wire();
        assert_eq!(edgeuse_count(&b, e).unwrap(), 2);
        assert!(is_wire(&b, e).unwrap());
        assert!(!is_lamina(&b, e).unwrap());
        assert!(!is_manifold(&b, e).unwrap());
        assert!(!is_closed(&b, e).unwrap());
        assert_eq!(start_vertex(&b, e).unwrap(), Some(v[0]));
        assert_eq!(end_vertex(&b, e).unwrap(), Some(v[1]));
        assert_eq!(other_vertex(&b, e, Some(v[0])).unwrap(), Some(v[1]));
        assert_eq!(other_vertex(&b, e, None).unwrap(), None);
        assert_eq!(vertices(&b, e, None).unwrap(), vec![Some(v[0]), Some(v[1])]);
        assert_eq!(same_oriented_edgeuse(&b, e).unwrap(), Some(eus[0]));
        assert!(faces(&b, e, None).unwrap().is_empty());
        assert_eq!(shells(&b, e, None).unwrap().len(), 1);
        assert!(is_connected_to(&b, e, Some(EntityRef::Vertex(v[1]))).unwrap());
        assert!(is_connected_to(&b, e, None).unwrap());
        let t = end_tangent(k, &b, e, true).unwrap();
        assert_eq!(t, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn orientation_and_tolerance() {
        let (mut b, e, eus, vs) = wire();
        set_orientation(&mut b, e, OrientType::Opposite).unwrap();
        assert_eq!(b.edgeuse(eus[0]).unwrap().orientation, OrientType::Opposite);
        assert_eq!(b.edgeuse(eus[1]).unwrap().orientation, OrientType::Same);
        // shrinking is skipped, growing within the vertex tolerance is fine
        set_tolerance(&mut b, e, 1e-6, true, true).unwrap();
        assert_eq!(b.edge(e).unwrap().tolerance, 1e-5);
        set_tolerance(&mut b, e, 1e-6, false, true).unwrap();
        assert_eq!(b.edge(e).unwrap().tolerance, 1e-6);
        // growing past the vertex tolerances grows them too
        set_tolerance(&mut b, e, 1.0, true, true).unwrap();
        assert_eq!(b.edge(e).unwrap().tolerance, 1.0);
        for v in vs {
            assert_eq!(b.vertex(v).unwrap().tolerance, 1.0);
        }
    }

    #[test]
    fn issue_521_failed_reversal_preserves_edge_interval_uses_and_trims() {
        let mut k = Kernel::default();
        k.fixes.edge_reversal_keeps_failed_edge = true;
        let (mut b, e, eus, _) = wire();
        let hermite = crate::curve::hermite_curve::HermiteCurve::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::X,
            Vec3::Y,
            3,
        );
        let mut curve = C::from(hermite);
        let old_ivl = b.edge(e).unwrap().interval;
        let expected = curve.reverse_parameterization(&k, &old_ivl).unwrap_err();
        b.edge_mut(e).unwrap().curve = Some(curve);
        for eu in eus {
            b.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(C::from(BSplineCurve::from_nurb(
                NlCurve::new(
                    vec![
                        CPoint::new(0.0, 0.0, 0.0, NOW),
                        CPoint::new(1.0, 0.0, 0.0, NOW),
                    ],
                    1,
                    vec![0.0, 0.0, 1.0, 1.0],
                )
                .unwrap(),
                3,
                false,
            )));
        }
        let before = format!("{b:?}");
        assert_eq!(reverse_orientation(&k, &mut b, e), Err(expected));
        assert_eq!(format!("{b:?}"), before);
    }

    #[test]
    fn failed_reversal_installs_default_interval_by_default() {
        // WR-260: upstream ignores the failed reversal's status.
        let k = Kernel::default();
        let (mut b, e, eus, _) = wire();
        b.edge_mut(e).unwrap().curve =
            Some(C::from(crate::curve::hermite_curve::HermiteCurve::new(
                Vec3::ZERO,
                Vec3::X,
                Vec3::X,
                Vec3::Y,
                3,
            )));
        for &eu in &eus {
            b.edgeuse_mut(eu).unwrap().uv_trim_curve = b.edge(e).unwrap().curve.clone();
        }
        reverse_orientation(&k, &mut b, e).unwrap();
        assert_eq!(b.edge(e).unwrap().interval, Extent1d::new());
        for eu in eus {
            assert!(b.edgeuse(eu).unwrap().uv_trim_curve.is_none());
        }
    }

    /// WR-260 addendum (fork source fork 47507b6): reversing an edge without
    /// curve installs the default interval upstream; with
    /// `edge_reversal_keeps_failed_edge` the edge keeps its own.
    #[test]
    fn reversing_an_edge_without_curve_keeps_its_interval_with_the_opt_in() {
        let mut k = Kernel::default();
        let (mut b, e, _, _) = wire();
        b.edge_mut(e).unwrap().curve = None;
        let ivl = b.edge(e).unwrap().interval;
        assert_ne!(ivl, Extent1d::new());
        reverse_orientation(&k, &mut b, e).unwrap();
        assert_eq!(b.edge(e).unwrap().interval, Extent1d::new());

        k.fixes.edge_reversal_keeps_failed_edge = true;
        let (mut b, e, _, _) = wire();
        b.edge_mut(e).unwrap().curve = None;
        reverse_orientation(&k, &mut b, e).unwrap();
        assert_eq!(b.edge(e).unwrap().interval, ivl);
    }

    #[test]
    fn reverse_and_reset_primary() {
        let (mut b, e, eus, _) = wire();
        reverse_orientation(&Kernel::default(), &mut b, e).unwrap();
        assert_eq!(b.edgeuse(eus[0]).unwrap().orientation, OrientType::Opposite);
        reset_primary_edgeuse(&mut b, e, Some(eus[1])).unwrap();
        assert_eq!(b.edge(e).unwrap().edgeuses, vec![eus[1], eus[0]]);
        assert!(reset_primary_edgeuse(&mut b, e, None).is_err());
    }

    #[test]
    fn ode_evaluate_needs_room() {
        let k = &Kernel::default();
        let (b, e, _, _) = wire();
        let c = b.edge(e).unwrap().curve.as_ref().unwrap();
        let ode = OdeEdgeLength::new(c, Vec3::new(0.0, 0.0, 0.0), 1.0);
        let mut short = vec![0.0; 10];
        assert!(ode.evaluate(k, 0.5, &mut short).is_err());
        let mut dydx = vec![1.0; 28];
        ode.evaluate(k, 0.5, &mut dydx).unwrap();
        assert_eq!(dydx[1], 1.0);
        assert_eq!(dydx[0], 0.0);
        assert!((dydx[2] - 2.0 * PI).abs() < 1e-15);
    }
}

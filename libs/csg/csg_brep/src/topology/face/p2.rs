//! `SmFace`, part 2: the face's simple accessors (surface, UV domain, rectangular-trim
//! flag, tolerance, face uses, loops), sampled interior points, the face curve
//! collections (`CreateCurvesFromFace`, `CreateCurvesByTrimming`), the integrand
//! evaluators of `ComputePreciseProperties`, and the file-static helpers of
//! `SmFace.cpp` used by loop orientation, UV trim curve building and seam repair.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.
//!
//! | Rust                                    | Upstream                                          |
//! |-----------------------------------------|---------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`class_type`], [`is_kind_of`] | `SmFace::GetType`, `GetTypeString`, `GetClassString`, `GetClassType`, `IsKindOf` |
//! | [`surface`], [`uv_domain`], [`rectangular_trim`], [`tolerance`] | `SmFace::GetSurface`, `GetUVDomain`, `GetRectangularTrim`, `GetTolerance` (`SM_USE_OLDTOL`) |
//! | [`set_uv_domain`], [`set_faceuse`]      | `SmFace::SetUVDomain`, `SetFaceuse`               |
//! | [`faceuses`], [`upward_faceuse`]        | `SmFace::GetFaceuses`, `GetUpwardFaceuse`         |
//! | [`loops`], [`outer_loops`]              | `SmFace::GetLoops`, `GetOuterLoops`               |
//! | [`check_seam_crossing`]                 | `sm_CheckSeamCrossing` (static)                   |
//! | [`calculate_an_internal_point`]         | `SmFace::CalculateAnInternalPoint`                |
//! | [`are_connected`], [`Connection`]       | `sm_AreConnected` (static)                        |
//! | [`review_start_and_stop_on_seam_crv_classifies`], [`SeamCrvReview`] | `sms_ReviewStartAndStopOnSeamCrvClassifies` |
//! | [`PrecisePropertyLayout`]               | `SmPrecisePropertyLayout` (constructor, `Add`)    |
//! | [`OdeIsoCurveLength`]                   | `SmODEIsoCurveLengthIFEO` (constructor, `Evaluate`) |
//! | [`OdeArea`]                             | `SmODEAreaIFEO::Evaluate`                         |
//! | [`surface_knots_inside`]                | `sm_SurfaceKnotsInside` (static; the source fork's source fork@08d9a14) |
//! | [`trim_curve_breaks`]                   | `sm_TrimCurveBreaks` (static; the source fork's source fork@08d9a14) |
//! | [`create_curves_by_trimming`], [`TrimmedCurves`] | `SmFace::CreateCurvesByTrimming`         |
//! | [`create_curves_from_face`], [`FaceCurves`] | `SmFace::CreateCurvesFromFace`                |
//! | [`fix_uv_trim_curves`], [`fix_uv_trim_curves_notified`] | `sm_FixUVTrimCurves`              |
//! | `notified_end_point_edit` (private)     | `SmBSplineCurve::EditEndPoint`'s `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` |
//! | [`find_best_uv_curve`]                  | `sm_FindBestUVCurve` (static)                     |
//! | [`RescueSurfaceOwner`]                  | `SmRescueSurfaceOwner` (constructor, destructor)  |
//! | [`remove_suspect_split_at_seam_uv_trim_curves`] | `sm_RemoveSuspectSplitAtSeamUVTrimCurves` |
//! | [`points_in_face`], [`PointsInFace`]    | `SmFace::GetPointsInFace`                         |
//! | `vertex_edges` (private)                | `SmVertex::GetEdges` without face filter (CO-PORTED, `topology.vertex`) |
//! | `edge_edgeuses` (private)               | `SmEdge::GetEdgeuses` without filters (CO-PORTED, `topology.edge`) |
//! | `loopuse_vertexuses` (private)          | `SmLoopuse::GetVertexuses` without filter (unit `topology.loopuse`) |
//!
//! Deviations:
//! - **Object model.** A face is a record in the [`Brep`]'s arenas; the methods are
//!   free functions taking the B-rep and the face's id. Pointer arguments and results
//!   are `Option`s of ids; out-params are return values (structs where there are
//!   several). `GetSurface`'s `SM_ASSERT_BREAK` and the other `SM_ASSERT`s are
//!   compiled out of the reference build and are not checked; where upstream then
//!   dereferences a null pointer (a face without surface or face use, an edge use
//!   without edge, vertex use or UV trim curve, an empty array's `GetLast`) the port
//!   returns [`SolidError::InvalidInput`]. `SER` / `NER` returns are
//!   [`SolidError::Upstream`] (`SM_ERR`) or the callee's error.
//! - **`sm_CheckSeamCrossing`** returns `SM_SUCCESS` (`1000`, true as an
//!   `SmBoolean`) for a surface closed in neither direction; [`check_seam_crossing`]
//!   returns `true` there (`plan/upstream-issues.md`). The unused surface argument is
//!   dropped.
//! - **`SmPrecisePropertyLayout`** indices are `usize`; `0` still means "not selected".
//!   The evaluators take the derivative array as a slice and return
//!   [`SolidError::InvalidInput`] for an index outside it (upstream writes out of
//!   bounds).
//! - **Statuses ignored upstream** (`ComputeUVPoint` in `sm_AreConnected` and
//!   `sm_FixUVTrimCurves`, the vertex/edge evaluations of `sm_FixUVTrimCurves`,
//!   `GetEnds`, the surface evaluations and the
//!   `CalculateUVDomainFromUVTrimCurves` of `GetPointsInFace`) are
//!   ignored; [`SolidError::Unported`] still propagates. A failed `ComputeUVPoint`
//!   leaves the caller's default point ([`Point2::UNDEF`]); a failed `DropPoint` is
//!   no drop point; a failed evaluation keeps the previous value (initially
//!   [`Vec3::UNDEF`]).
//! - **`sm_FixUVTrimCurves` opt-in fix** (WR-255, source issue #959/#570, the
//!   fork's source fork@5af7070). Upstream drops the vertex's UV point onto
//!   each of the two curves, `Trim`s the curve there and `EditParameterization`s it back
//!   over its old interval before moving the end point. The stretch moves every interior
//!   parameter of the trim off its edge's by the trimmed length (a 1e-6 corner gap left a
//!   thread edge's trim 5e-5 off its edge, tolerance 1e-5). That is the default here, as
//!   upstream; with [`UpstreamFixes::uv_trim_corner_end_points_only`](crate::kernel::UpstreamFixes::uv_trim_corner_end_points_only)
//!   only the end control points move, by the UV gap.
//!   Each `Trim` uses the tolerance of the edge reached through the UV curve's actual
//!   holding edge use, matching `SmTol::GetZoneTol3d(curve)`; an owner path without an
//!   edge uses the system tolerance. Notification probes use that same tolerance.
//! - **Edit notifications.** [`fix_uv_trim_curves_notified`] notifies its two
//!   `EditEndPoint`s (`SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT`, caller and first data
//!   object the UV trim curve under its kernel-issued identity) through the caller's
//!   [`NotifyEnv`]; [`fix_uv_trim_curves`] has none and notifies nobody. Upstream
//!   callbacks read the curve's owner (the edge use holding it) and that edge use's
//!   edge through the pointers; here they are reported first
//!   ([`object::report_owner_link`]). `SmCurve::Notify`'s object cache drop has no
//!   counterpart (a UV trim curve keeps no object cache here).
//! - **`EditEndPoint`** (non-virtual `SmBSplineCurve`) is applied to B-spline and line
//!   UV trim curves (a line's NURB is made first and its analytic data refreshed
//!   after, as `SmLine::RefreshAnalytics`); a UV trim curve of another kind is
//!   [`SolidError::InvalidInput`].
//! - **`SmCrvOnSurf`** preserves a face surface's concrete dispatch in
//!   [`find_best_uv_curve`].
//! - **`sm_FindBestUVCurve`** edits the chosen candidate in place and then moves it out
//!   of its slot (upstream: returns the pointer and nulls the slot).
//! - **`SmRescueSurfaceOwner`** is a value holding the saved owner; its destructor is
//!   [`RescueSurfaceOwner::restore`].
//! - **`sms_ReviewStartAndStopOnSeamCrvClassifies`** drops the unused UV trim line and
//!   face-properties (debug only) arguments, and returns which end classifications
//!   remain (upstream: pointers into the classification). An empty classification is
//!   [`SolidError::InvalidInput`] (upstream reads `[0]` and `[-1]`).
//! - **`SmVertex::GetEdges`** walks the vertex's use list; the port's vertices keep no
//!   list, so the vertex uses are the B-rep's whose `vertex` it is, in arena order.
//!   Its only caller here deletes UV trim curves, which is not order dependent.
//!   `GetEdges` also evaluates each edge use's face for the (absent) face filter; that
//!   unused call is not made.
//! - **`GetPointsInFace`** draws its V dither from `kernel.rng` instead of
//!   `srand(time(NULL))` / `rand()` (a uniform integer in `[0, RAND_MAX]`, glibc's
//!   `RAND_MAX`), so results asked for more than one point are not reproducible
//!   against upstream (`plan/findings.md`). Point counts above
//!   [`MAX_POINTS_IN_FACE`] are [`SolidError::InvalidInput`] (upstream allocates
//!   them). The rectangular branch samples from `dUInc` / `dVInc`, not from the
//!   domain minimum, as upstream does (`plan/upstream-issues.md`). It takes the
//!   B-rep mutably: its UV-domain and iso-line classifications run on it (the
//!   classification's snapshot hands back the UV trim curves, the tolerance growth
//!   that fits them and the gap caches), so what upstream's `const` queries leave on
//!   the B-rep stays there.
//! - **`CreateCurvesByTrimming`** drops its unused `bForceUVClassify` argument; the
//!   output arrays are asked for with `want_3d` / `want_uv`.
//! - **`CalculateAnInternalPoint` / `CreateCurvesByTrimming`** take the B-rep mutably:
//!   their classifications run on it in place, so what upstream's `const` queries leave
//!   on the B-rep stays there. Callers that hold only `&Brep` (the topology solver's
//!   face silhouettes, `sm_CountContainedSamples`) use the crate-private `_snapshot`
//!   entries, which classify on a private copy and drop those edits (temporary; see
//!   `query::face::p2`).
//! - **Unported callees.** `SmCurveClassification::ReSet` in the constructor of a curve
//!   (`intersect.curve_class_p2`; `SmFace::CurveOnClassify` follows it), the face member
//!   `GetUpwardFaceuse` of an opaque handle in `SmCurveInterval::GetIvlBound_EdgeuseObj`,
//!   `SmFace::CalculateUVDomainFromUVTrimCurves` (`topology.face_p1`),
//!   `SmSurface::IsSingularity` of a non-plane
//!   (`surface.surface_p5`) return [`SolidError::Unported`]. A thick face's temporary
//!   `SmOffsetSurface` is built over a copy of the face surface (upstream shares it).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::CurveKind;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, EntityRef, Result, SolidError};
use crate::geom::a_object;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::{self, NotifyOperation, ObjectRef};
use crate::geom::tol::{are_same, zone_tol3d};
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::curve_class::p1::{CurveClassification, CurveInterval};
use crate::intersect::curve_class::p4::ClassificationInit;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::point_class::{ClassGeometry, ClassObject, ClassObjectKind, PointClassType};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType};
use crate::nurbs::knots::Side;
use crate::query::face::p2::{
    curve_on_classify_cached, curve_on_classify_mut, CurveOnClassifyBrep, CurveOnClassifyCache,
    CurveOnClassifyObjects, FaceBrep,
};
use crate::query::face::p3 as qf3;
use crate::surface::offset_surface::{OffsetSurface, SS_UNKNOWN};
use crate::surface::surface::p1::map_to_2d;
use crate::surface::surface::p7::approx_derivative_lengths;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::volume::Volume;
use crate::topology::edge::NotifyEnv;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::topology as topo;
use crate::topology::vertexuse as vu1;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, LoopId, LoopuseId, LoopuseStart, VertexId,
    VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::face::p2";

/// `SmFace_TYPE` (`TOPO_BASE_TYPE + 2`).
pub const TYPE: i64 = 16002;
/// `SmFace::GetTypeString`.
pub const TYPE_STRING: &str = "SmFace_TYPE";
/// `SmFace::GetClassString`.
pub const CLASS_STRING: &str = "SmFace";

/// `UNSURE`: the "not yet known" value of an `SmBoolean` such as
/// [`crate::topology::Face::rectangular_trim`].
pub const UNSURE: i32 = crate::topology::brep_data::UNSURE;

/// `RAND_MAX` of the reference build's C library (glibc).
const RAND_MAX: f64 = 2_147_483_647.0;

/// The largest point count [`points_in_face`] accepts (upstream allocates any count).
pub const MAX_POINTS_IN_FACE: u64 = 1 << 20;

/// `SmFace::GetClassType`: [`TYPE`].
#[must_use]
pub const fn class_type() -> i64 {
    TYPE
}

/// `SmFace::IsKindOf`: [`TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// Ignores a status upstream ignores, keeping [`SolidError::Unported`].
fn ignored<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

// ---- simple data access ---------------------------------------------------------------------

/// `GetSurface`: the face's surface (`None` for a face without one).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn surface(b: &Brep, f: FaceId) -> Result<Option<&Surface>> {
    Ok(b.face(f)?.surface.as_ref())
}

/// The surface of `f`, which upstream dereferences.
fn surface_deref(b: &Brep, f: FaceId) -> Result<&Surface> {
    surface(b, f)?.ok_or_else(|| null("face without surface"))
}

/// `GetUVDomain`: the domain of the surface the face uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn uv_domain(b: &Brep, f: FaceId) -> Result<Extent2d> {
    Ok(b.face(f)?.uv_domain)
}

/// `GetRectangularTrim`: `1` when the face has one rectangular outer loop of four iso
/// UV trim curves spanning its UV domain, `0` when not, [`UNSURE`] when not known.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn rectangular_trim(b: &Brep, f: FaceId) -> Result<i32> {
    Ok(b.face(f)?.rectangular_trim)
}

/// `GetTolerance` (`SM_USE_OLDTOL`): the face's stored zone tolerance.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn tolerance(b: &Brep, f: FaceId) -> Result<f64> {
    Ok(b.face(f)?.tolerance)
}

/// `SetUVDomain`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn set_uv_domain(b: &mut Brep, f: FaceId, new_uv_domain: &Extent2d) -> Result<()> {
    b.face_mut(f)?.uv_domain = *new_uv_domain;
    Ok(())
}

/// `SetFaceuse`: sets the primary face use (`None` during deletion).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn set_faceuse(b: &mut Brep, f: FaceId, new_fu: Option<FaceuseId>) -> Result<()> {
    b.face_mut(f)?.faceuse = new_fu;
    Ok(())
}

/// `GetUpwardFaceuse`: the face use on the side of the surface normal (the primary
/// one; `None` during construction or destruction).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn upward_faceuse(b: &Brep, f: FaceId) -> Result<Option<FaceuseId>> {
    Ok(b.face(f)?.faceuse)
}

/// `GetFaceuses`: the primary face use and its mate (both `None` without face use).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuses(b: &Brep, f: FaceId) -> Result<(Option<FaceuseId>, Option<FaceuseId>)> {
    let Some(fu) = b.face(f)?.faceuse else {
        return Ok((None, None));
    };
    Ok((Some(fu), b.faceuse(fu)?.mate))
}

/// `GetLoops`: the loops of the primary face use's loop uses selected as
/// [`eu1::faceuse_loopuses`] selects them (all, the outer one, or the uses of
/// `target_loops` on this face use, in their order); none without face use.
///
/// # Errors
///
/// Those of [`eu1::faceuse_loopuses`]; [`SolidError::InvalidInput`] for a selected loop
/// use without loop (upstream stores the null pointer; it is reported instead).
pub fn loops(
    b: &Brep,
    f: FaceId,
    target_loops_only: bool,
    target_loops: Option<&[LoopId]>,
) -> Result<Vec<LoopId>> {
    // Loopuses for this one Faceuse
    let Some(fu) = b.face(f)?.faceuse else {
        return Ok(Vec::new());
    };
    let lus = eu1::faceuse_loopuses(b, fu, target_loops_only, target_loops)?;

    // for every Loopuse - get assoc Loop
    lus.into_iter()
        .map(|lu| {
            b.loopuse(lu)?
                .loop_
                .ok_or_else(|| null("GetLoops: loop use without loop"))
        })
        .collect()
}

/// `GetOuterLoops`: [`loops`] of the outer loop only.
///
/// # Errors
///
/// Those of [`loops`].
pub fn outer_loops(b: &Brep, f: FaceId) -> Result<Vec<LoopId>> {
    loops(b, f, true, None)
}

// ---- co-ported accessors of other units -----------------------------------------------------

/// `SmEdge::GetEdgeuses` without filters (CO-PORTED from unit `topology.edge`): the
/// edge's edge use list from its head.
fn edge_edgeuses(b: &Brep, e: EdgeId) -> Result<Vec<EdgeuseId>> {
    Ok(b.edge(e)?.edgeuses.clone())
}

/// `SmVertex::GetEdges` without face filter (CO-PORTED from unit `topology.vertex`): the
/// distinct edges of the vertex's edge vertex uses.
fn vertex_edges(b: &Brep, v: VertexId) -> Result<Vec<EdgeId>> {
    let mut out: Vec<EdgeId> = Vec::new();
    for (_, vu) in b.vertexuses().iter() {
        if vu.vertex != Some(v) {
            continue;
        }
        // when vertexuse is an edgeVertexuse type
        if let VertexuseOwner::Edgeuse(eu) = vu.owner {
            let eu = eu.ok_or_else(|| null("GetEdges: edge vertex use without edge use"))?;
            let e = b
                .edgeuse(eu)?
                .edge
                .ok_or_else(|| null("GetEdges: edge use without edge"))?;
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// `SmLoopuse::GetVertexuses` without filter (unit `topology.loopuse`).
pub(crate) fn loopuse_vertexuses(b: &Brep, lu: LoopuseId) -> Result<Vec<Option<VertexuseId>>> {
    crate::topology::loopuse::vertexuses(b, lu, None)
}

/// The edge of `eu`, which upstream dereferences.
fn edge_deref(b: &Brep, eu: EdgeuseId) -> Result<EdgeId> {
    eu1::edge(b, eu)?.ok_or_else(|| null("edge use without edge"))
}

/// The curve of edge `e`, which upstream dereferences.
fn edge_curve(b: &Brep, e: EdgeId) -> Result<&Curve> {
    b.edge(e)?
        .curve
        .as_ref()
        .ok_or_else(|| null("edge without curve"))
}

// ---- unported callees ---------------------------------------------------------------------

/// The handles of the UV line classifications of [`calculate_an_internal_point`]: the
/// face, its surface, the UV line and the face's boundary topology and curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UvLineHandle {
    Face,
    Surface,
    Line,
    Vertex(VertexId),
    Edge(EdgeId),
    Vertexuse(VertexuseId),
    Edgeuse(EdgeuseId),
    EdgeCurve(EdgeId),
    Trim(EdgeuseId),
}

impl ClassObject for UvLineHandle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Face => ClassObjectKind::Face,
            Self::Surface => ClassObjectKind::Surface,
            Self::Line | Self::EdgeCurve(_) | Self::Trim(_) => ClassObjectKind::Curve,
            Self::Vertex(_) => ClassObjectKind::Vertex,
            Self::Edge(_) => ClassObjectKind::Edge,
            Self::Vertexuse(_) => ClassObjectKind::Vertexuse,
            Self::Edgeuse(_) => ClassObjectKind::Edgeuse,
        }
    }
}

/// The UV line `sm_FaceClassifyUVLine` made last (`sCurveClassification.GetCurve()`),
/// and the face `f` of `b` whose objects the other handles name.
pub(crate) struct UvLineObjects<B> {
    b: B,
    f: FaceId,
    line: Option<Curve>,
    resolve_topology: bool,
}

impl<B> UvLineObjects<B> {
    /// The objects of face `f` of `b` (`&Brep`, or `&mut Brep` to classify in place),
    /// before `sm_FaceClassifyUVLine` makes its line (also used by
    /// `topology::face::p4`'s `GlobalLineIntersect`).
    pub(crate) const fn new(b: B, f: FaceId) -> Self {
        Self {
            b,
            f,
            line: None,
            resolve_topology: true,
        }
    }
}

impl<B: FaceBrep> ClassGeometry<UvLineHandle> for UvLineObjects<B> {
    fn curve(&self, o: UvLineHandle) -> Option<&Curve> {
        match o {
            UvLineHandle::Line => self.line.as_ref(),
            UvLineHandle::EdgeCurve(e) => self.b.brep().edge(e).ok()?.curve.as_ref(),
            UvLineHandle::Trim(eu) => eu1::uv_trim_curve(self.b.brep(), eu).ok()?.0,
            _ => None,
        }
    }
    fn surface(&self, o: UvLineHandle) -> Option<&Surface> {
        match o {
            UvLineHandle::Surface => self.b.brep().face(self.f).ok()?.surface.as_ref(),
            _ => None,
        }
    }
    fn volume(&self, _o: UvLineHandle) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: UvLineHandle) -> Option<Point3> {
        None
    }
    fn classification_vertex_point(&self, o: UvLineHandle) -> Option<Point3> {
        match o {
            UvLineHandle::Vertex(v) => self.b.brep().vertex(v).ok().map(|v| v.point),
            _ => None,
        }
    }
    fn topology_zone_tol_3d(&self, o: UvLineHandle) -> Option<f64> {
        match o {
            UvLineHandle::Vertex(v) => self.b.brep().vertex(v).ok().map(|v| v.tolerance),
            UvLineHandle::Edge(e) => self.b.brep().edge(e).ok().map(|e| e.tolerance),
            UvLineHandle::Face => self.b.brep().face(self.f).ok().map(|f| f.tolerance),
            _ => None,
        }
    }
    fn topology_entity(&self, o: UvLineHandle) -> Option<(&Brep, EntityRef)> {
        if !self.resolve_topology {
            return None;
        }
        let e = match o {
            UvLineHandle::Face => EntityRef::Face(self.f),
            UvLineHandle::Vertex(v) => EntityRef::Vertex(v),
            UvLineHandle::Edge(e) => EntityRef::Edge(e),
            UvLineHandle::Vertexuse(vu) => EntityRef::Vertexuse(vu),
            UvLineHandle::Edgeuse(eu) => EntityRef::Edgeuse(eu),
            UvLineHandle::Surface
            | UvLineHandle::Line
            | UvLineHandle::EdgeCurve(_)
            | UvLineHandle::Trim(_) => return None,
        };
        Some((self.b.brep(), e))
    }
    fn topology_handle(&self, e: EntityRef) -> Option<UvLineHandle> {
        if !self.resolve_topology {
            return None;
        }
        match e {
            EntityRef::Face(f) if f == self.f => Some(UvLineHandle::Face),
            EntityRef::Vertex(v) => Some(UvLineHandle::Vertex(v)),
            EntityRef::Edge(e) => Some(UvLineHandle::Edge(e)),
            EntityRef::Vertexuse(vu) => Some(UvLineHandle::Vertexuse(vu)),
            EntityRef::Edgeuse(eu) => Some(UvLineHandle::Edgeuse(eu)),
            _ => None,
        }
    }
}

impl<B: FaceBrep> CurveOnClassifyObjects<UvLineHandle> for UvLineObjects<B> {
    fn face(&self, _f: FaceId) -> UvLineHandle {
        UvLineHandle::Face
    }
    fn vertex(&self, v: VertexId) -> UvLineHandle {
        UvLineHandle::Vertex(v)
    }
    fn edge(&self, e: EdgeId) -> UvLineHandle {
        UvLineHandle::Edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> UvLineHandle {
        UvLineHandle::Vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> UvLineHandle {
        UvLineHandle::Edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> UvLineHandle {
        UvLineHandle::EdgeCurve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> UvLineHandle {
        UvLineHandle::Trim(eu)
    }
    fn face_surface(&self, _f: FaceId) -> Option<UvLineHandle> {
        // (calculate_an_internal_point returns before classifying without surface)
        Some(UvLineHandle::Surface)
    }
    fn loop_id(&self, _tgt_loop: UvLineHandle) -> Option<LoopId> {
        None
    }
}

impl<B: FaceBrep> qf3::ClassifyCurveObjects<UvLineHandle> for UvLineObjects<B> {
    fn set_classify_curves(
        &mut self,
        curve: Curve,
        _uv_curve: Option<Curve>,
    ) -> Result<(UvLineHandle, Option<UvLineHandle>)> {
        self.line = Some(curve);
        Ok((UvLineHandle::Line, None))
    }
}

impl ClassGeometryMut<UvLineHandle> for UvLineObjects<&mut Brep> {
    fn curve_mut(&mut self, _o: UvLineHandle) -> Option<&mut Curve> {
        // ClassifyFaceInterval does not edit the classified line
        None
    }
    fn topology_entity_mut(&mut self, o: UvLineHandle) -> Option<(&mut Brep, EntityRef)> {
        let (_, e) = self.topology_entity(o)?;
        Some((&mut *self.b, e))
    }
}

impl<B: FaceBrep> FaceBrep for UvLineObjects<B> {
    fn brep(&self) -> &Brep {
        self.b.brep()
    }
}

impl CurveOnClassifyBrep<UvLineHandle> for UvLineObjects<&mut Brep> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.b
    }
}

/// The handles of a curve classification of face `f` (`SmFace::CurveOnClassify`): the
/// B-rep's topology (the face, its loops, edges, vertices and their uses), the face's
/// surface, and the classified curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassHandle {
    /// A face.
    Face(FaceId),
    /// A vertex.
    Vertex(VertexId),
    /// A loop.
    Loop(LoopId),
    /// An edge.
    Edge(EdgeId),
    /// A face use.
    Faceuse(FaceuseId),
    /// An edge use.
    Edgeuse(EdgeuseId),
    /// A vertex use.
    Vertexuse(VertexuseId),
    /// The surface of a face.
    Surface(FaceId),
    /// The 3D curve of an edge.
    EdgeCurve(EdgeId),
    /// The trim curve stored on an edge use.
    Trim(EdgeuseId),
    /// The classified curve (and its UV curve: upstream passes the same curve for both).
    Curve,
}

impl ClassObject for ClassHandle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Face(_) => ClassObjectKind::Face,
            Self::Vertex(_) => ClassObjectKind::Vertex,
            Self::Loop(_) => ClassObjectKind::Loop,
            Self::Edge(_) => ClassObjectKind::Edge,
            Self::Faceuse(_) => ClassObjectKind::Faceuse,
            Self::Edgeuse(_) => ClassObjectKind::Edgeuse,
            Self::Vertexuse(_) => ClassObjectKind::Vertexuse,
            Self::Surface(_) => ClassObjectKind::Surface,
            Self::EdgeCurve(_) | Self::Trim(_) | Self::Curve => ClassObjectKind::Curve,
        }
    }
}

/// The objects behind the [`ClassHandle`]s of one `CurveOnClassify` call over the
/// face's B-rep `B` (`&Brep` for the snapshot entry, `&mut Brep` to classify in place).
struct FaceClassify<'c, B> {
    /// The B-rep of the face.
    brep: B,
    /// The classified curve.
    curve: &'c Curve,
}

/// [`FaceClassify`] over a shared B-rep (the `curve_on_classify_cached` snapshot entry).
type FaceClassifyObjects<'c> = FaceClassify<'c, &'c Brep>;

impl<B: FaceBrep> CurveOnClassifyObjects<ClassHandle> for FaceClassify<'_, B> {
    fn face(&self, f: FaceId) -> ClassHandle {
        ClassHandle::Face(f)
    }
    fn vertex(&self, v: VertexId) -> ClassHandle {
        ClassHandle::Vertex(v)
    }
    fn face_surface(&self, f: FaceId) -> Option<ClassHandle> {
        self.brep
            .brep()
            .face(f)
            .ok()
            .and_then(|r| r.surface.as_ref())
            .map(|_| ClassHandle::Surface(f))
    }
    fn edge(&self, e: EdgeId) -> ClassHandle {
        ClassHandle::Edge(e)
    }
    fn vertexuse(&self, vu: VertexuseId) -> ClassHandle {
        ClassHandle::Vertexuse(vu)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> ClassHandle {
        ClassHandle::Edgeuse(eu)
    }
    fn edge_curve(&self, e: EdgeId) -> ClassHandle {
        ClassHandle::EdgeCurve(e)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> ClassHandle {
        ClassHandle::Trim(eu)
    }
    fn loop_id(&self, tgt_loop: ClassHandle) -> Option<LoopId> {
        match tgt_loop {
            ClassHandle::Loop(l) => Some(l),
            _ => None,
        }
    }
}

impl<B: FaceBrep> ClassGeometry<ClassHandle> for FaceClassify<'_, B> {
    fn classification_vertex_point(&self, o: ClassHandle) -> Option<Point3> {
        match o {
            ClassHandle::Vertex(v) => self.brep.brep().vertex(v).ok().map(|v| v.point),
            _ => None,
        }
    }
    fn topology_zone_tol_3d(&self, o: ClassHandle) -> Option<f64> {
        match o {
            ClassHandle::Vertex(v) => self.brep.brep().vertex(v).ok().map(|v| v.tolerance),
            ClassHandle::Face(f) => self.brep.brep().face(f).ok().map(|f| f.tolerance),
            ClassHandle::Edge(e) => self.brep.brep().edge(e).ok().map(|e| e.tolerance),
            _ => None,
        }
    }
    fn curve(&self, o: ClassHandle) -> Option<&Curve> {
        match o {
            ClassHandle::Curve => Some(self.curve),
            ClassHandle::EdgeCurve(e) => self.brep.brep().edge(e).ok()?.curve.as_ref(),
            ClassHandle::Trim(eu) => eu1::uv_trim_curve(self.brep.brep(), eu).ok()?.0,
            _ => None,
        }
    }
    fn surface(&self, o: ClassHandle) -> Option<&Surface> {
        match o {
            ClassHandle::Surface(f) => self
                .brep
                .brep()
                .face(f)
                .ok()
                .and_then(|r| r.surface.as_ref()),
            _ => None,
        }
    }
    fn volume(&self, _o: ClassHandle) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: ClassHandle) -> Option<Point3> {
        None
    }
    fn topology_entity(&self, o: ClassHandle) -> Option<(&Brep, EntityRef)> {
        let e = match o {
            ClassHandle::Face(f) => EntityRef::Face(f),
            ClassHandle::Vertex(v) => EntityRef::Vertex(v),
            ClassHandle::Loop(l) => EntityRef::Loop(l),
            ClassHandle::Edge(e) => EntityRef::Edge(e),
            ClassHandle::Faceuse(fu) => EntityRef::Faceuse(fu),
            ClassHandle::Edgeuse(eu) => EntityRef::Edgeuse(eu),
            ClassHandle::Vertexuse(vu) => EntityRef::Vertexuse(vu),
            ClassHandle::Surface(_)
            | ClassHandle::EdgeCurve(_)
            | ClassHandle::Trim(_)
            | ClassHandle::Curve => return None,
        };
        Some((self.brep.brep(), e))
    }
    fn topology_handle(&self, e: EntityRef) -> Option<ClassHandle> {
        match e {
            EntityRef::Face(f) => Some(ClassHandle::Face(f)),
            EntityRef::Vertex(v) => Some(ClassHandle::Vertex(v)),
            EntityRef::Loop(l) => Some(ClassHandle::Loop(l)),
            EntityRef::Edge(e) => Some(ClassHandle::Edge(e)),
            EntityRef::Faceuse(fu) => Some(ClassHandle::Faceuse(fu)),
            EntityRef::Edgeuse(eu) => Some(ClassHandle::Edgeuse(eu)),
            EntityRef::Vertexuse(vu) => Some(ClassHandle::Vertexuse(vu)),
            _ => None,
        }
    }
}

impl ClassGeometryMut<ClassHandle> for FaceClassify<'_, &mut Brep> {
    fn curve_mut(&mut self, _o: ClassHandle) -> Option<&mut Curve> {
        // the classified curve is the caller's (read-only); ClassifyFaceInterval does
        // not edit it
        None
    }
    fn topology_entity_mut(&mut self, o: ClassHandle) -> Option<(&mut Brep, EntityRef)> {
        let (_, e) = self.topology_entity(o)?;
        Some((&mut *self.brep, e))
    }
}

impl<B: FaceBrep> FaceBrep for FaceClassify<'_, B> {
    fn brep(&self) -> &Brep {
        self.brep.brep()
    }
}

impl CurveOnClassifyBrep<ClassHandle> for FaceClassify<'_, &mut Brep> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.brep
    }
}

/// `SmCurveClassification(curve, curve->GetNaturalInterval(), uv_curve, tol, data_size,
/// data, FALSE, TRUE, surface)` (the UV curve, when given, is the curve itself; the surface
/// is the face's), followed by `SER(CurveOnClassify(TRUE, classification))` of face `f`:
/// the intervals.
#[allow(clippy::too_many_arguments)] // upstream's constructor arguments
fn classify_curve_on_face(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    curve: &Curve,
    with_uv_curve: bool,
    tol: f64,
    data_size: usize,
) -> Result<Vec<CurveInterval<ClassHandle>>> {
    let mut cache = CurveOnClassifyCache::default();
    classify_curve_on_face_cached(k, b, f, curve, with_uv_curve, tol, data_size, &mut cache)
}

/// [`classify_curve_on_face`] on the caller's B-rep in place
/// ([`curve_on_classify_mut`]): the UV trim curves (with their edge tolerance growth),
/// gap caches and trimmed surface cache the classification makes stay on `b`, as
/// upstream's `const` `CurveOnClassify` leaves them, also when it fails.
fn classify_curve_on_face_in_place(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    curve: &Curve,
    with_uv_curve: bool,
    tol: f64,
    data_size: usize,
) -> Result<Vec<CurveInterval<ClassHandle>>> {
    let mut objects = FaceClassify { brep: b, curve };
    let init = classification_init(k, &objects, f, curve, with_uv_curve, tol, data_size);
    let mut classification = CurveClassification::with_curve(k, &init)?;
    // bHasBigGaps, bDoCheck3d: FALSE; bUVSpaceOkay: TRUE (the defaults)
    curve_on_classify_mut(
        k,
        f,
        &mut objects,
        true,
        &mut classification,
        false,
        false,
        true,
    )?;
    Ok(classification.intervals().to_vec())
}

/// `SmCurveClassification(curve, curve->GetNaturalInterval(), uv_curve, tol, data_size,
/// data, FALSE, TRUE, this->GetSurface())`.
fn classification_init<B: FaceBrep>(
    k: &Kernel,
    objects: &FaceClassify<'_, B>,
    f: FaceId,
    curve: &Curve,
    with_uv_curve: bool,
    tol: f64,
    data_size: usize,
) -> ClassificationInit<ClassHandle> {
    ClassificationInit {
        curve: Some(ClassHandle::Curve),
        interval: curve.natural_interval(),
        uv_curve: with_uv_curve.then_some(ClassHandle::Curve),
        src_zone_tol_3d: zone_tol3d(k, tol),
        opt_data_size: data_size,
        is_curve_owned: false,
        is_curve_current: true,
        // this->GetSurface()
        surface1: CurveOnClassifyObjects::face_surface(objects, f),
        surface2: None,
        uv_curve2: None,
        ti: None,
    }
}

#[allow(clippy::too_many_arguments)] // upstream's constructor arguments
fn classify_curve_on_face_cached(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    curve: &Curve,
    with_uv_curve: bool,
    tol: f64,
    data_size: usize,
    cache: &mut CurveOnClassifyCache,
) -> Result<Vec<CurveInterval<ClassHandle>>> {
    let objects = FaceClassifyObjects { brep: b, curve };
    let init = classification_init(k, &objects, f, curve, with_uv_curve, tol, data_size);
    let mut classification = CurveClassification::with_curve(k, &init)?;
    // bHasBigGaps, bDoCheck3d: FALSE; bUVSpaceOkay: TRUE (the defaults)
    curve_on_classify_cached(
        k,
        b,
        f,
        &objects,
        true,
        &mut classification,
        false,
        false,
        true,
        cache,
    )?;
    Ok(classification.intervals().to_vec())
}

/// Classifies an explicit edge interval for the B-rep coincidence validator, on the
/// caller's B-rep in place ([`curve_on_classify_mut`]): the classification keeps the
/// caller's interval, and the trim/gap updates it makes stay on `b`, including those
/// made before a failed status.
pub(crate) fn classify_interval_on_face(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    curve: &Curve,
    interval: Extent1d,
    tol: f64,
) -> Result<Vec<PointClassType>> {
    let mut objects = FaceClassify { brep: b, curve };
    let init = ClassificationInit {
        curve: Some(ClassHandle::Curve),
        interval,
        uv_curve: None,
        src_zone_tol_3d: zone_tol3d(k, tol),
        opt_data_size: 0,
        is_curve_owned: false,
        is_curve_current: true,
        surface1: None,
        surface2: None,
        uv_curve2: None,
        ti: None,
    };
    let mut classification = CurveClassification::with_curve(k, &init)?;
    let result = curve_on_classify_mut(
        k,
        f,
        &mut objects,
        true,
        &mut classification,
        false,
        false,
        true,
    );
    if let Err(error) = result {
        if error.is_unported() {
            return Err(error);
        }
    }
    Ok(classification
        .intervals()
        .iter()
        .map(|i| i.mid.point_class())
        .collect())
}

/// `SmFace::CalculateUVDomainFromUVTrimCurves(rTrimUVDomain)` (unit `topology.face_p1`),
/// on the caller's B-rep: the UV trim curves the call makes (a cache upstream fills
/// through a `const` method) stay on it.
fn calculate_uv_domain_from_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
) -> Result<Extent2d> {
    crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves(k, b, f, true, false, false)
        .map(|r| r.domain)
}

/// `pEU->NormalizedEvaluate(dT, TRUE, rUV)` (UV evaluation).
fn normalized_evaluate_uv(k: &Kernel, b: &mut Brep, eu: EdgeuseId, t: f64) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, eu, t, true, false, true)?.point)
}

/// `pEU->GetOrCreateUVTrimCurve(rpUVCurve)` (its status is ignored; the curve is then
/// dereferenced).
fn get_or_create_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<Curve> {
    let _ = eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
    eu1::uv_trim_curve(b, eu)?
        .0
        .cloned()
        .ok_or_else(|| null("CreateCurvesFromFace: edge use without UV trim curve"))
}

/// `pSurf->IsSingularity(uv, eDir, dTol3d, TRUE)` (virtual: the plane override, the
/// base body otherwise).
fn is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    tol: f64,
) -> Result<(bool, SurfParamType)> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity()),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::OffsetSurface(_) => {
            let r = surface.is_singularity(k, uv, tol, false)?;
            Ok((r.singular, r.direction))
        }
    }
}

/// `SmOffsetSurface(dOffset, rSurface).EvaluateSimple(uv, 1, 1, TRUE, TRUE, TRUE, ..)`:
/// a temporary offset (not owning, default singularities, so its constructor runs the
/// surface's `IsPeriodic` and `GetSingularities`) over an exact evaluation copy of
/// `surface` that keeps its owner (upstream shares it; [`Surface`]'s `Clone` would run
/// the copy constructor's knot fix-up).
fn offset_evaluate_simple(
    k: &Kernel,
    surface: &Surface,
    offset: f64,
    uv: Point2,
) -> Result<Vec<Vec<Vec3>>> {
    let off = OffsetSurface::new(
        k,
        offset,
        surface.clone_for_evaluation()?,
        false,
        SS_UNKNOWN,
    )?;
    off.evaluate_simple(k, uv, 1, 1, simple_flags())
}

/// `EvaluateSimple(.., TRUE, TRUE, TRUE, ..)`: from the left, upper half.
const fn simple_flags() -> EvaluateFlags {
    EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: true,
    }
}

/// Copies an evaluation into the caller's first-derivative block (entries it does not
/// have keep their values).
fn copy_block(pv: &mut [[Vec3; 2]; 2], r: &[Vec<Vec3>]) {
    for (ii, row) in pv.iter_mut().enumerate() {
        for (jj, slot) in row.iter_mut().enumerate() {
            if let Some(v) = r.get(ii).and_then(|x| x.get(jj)) {
                *slot = *v;
            }
        }
    }
}

/// `SmODEIntegrator(eval).RungeKuttaODEIntegrate(dY, x1, x2, eps, h1, 0, .., pYScale)`.
fn runge_kutta_ode_integrate(
    eval: &OdeIsoCurveLength<'_>,
    y: &mut [f64],
    x1: f64,
    x2: f64,
    eps: f64,
    h1: f64,
    y_scale: &[f64],
) -> Result<()> {
    let mut scratch = vec![0.0; y.len() + 1];
    crate::mass::integrator::OdeIntegrator::default().integrate(
        y,
        x1,
        x2,
        crate::mass::integrator::OdeOptions {
            accuracy: eps,
            initial_step: h1,
            minimum_step: 0.0,
            scale: Some(y_scale),
        },
        |t, _, dy| {
            eval.evaluate(t, &mut scratch)?;
            for (d, v) in dy.iter_mut().zip(scratch.iter().skip(1)) {
                *d = *v;
            }
            Ok(())
        },
    )?;
    Ok(())
}

// ---- static helpers of loop orientation ---------------------------------------------------

/// `sm_CheckSeamCrossing`: when the step from `uv1` to `uv2` jumps more than 90% of a
/// closed direction's period of `domain`, moves `uv1` into `uv2`'s period and adds the
/// period (with the sign of the jump's opposite) to `param_shift`. Returns whether a
/// seam was crossed; `true` for a surface closed in neither direction (upstream
/// returns `SM_SUCCESS` there).
pub fn check_seam_crossing(
    uv1: &mut Point2,
    uv2: Point2,
    domain: &Extent2d,
    closed_u: bool,
    closed_v: bool,
    param_shift: &mut Vec2,
) -> bool {
    let mut ret = false;

    if !closed_u && !closed_v {
        // SM_SUCCESS as an SmBoolean
        return true;
    }

    if closed_u {
        let ivl = domain.u_interval();
        let period = ivl.length();
        let jump = uv2.x - uv1.x;

        // The caller is stepping along the curve in 0.05 increments.
        if jump.abs() > 0.90 * period {
            if uv2.x < uv1.x {
                // Jumped from high end to low.
                param_shift.x += period;
                uv1.x -= period; // Bring prev pt into same period as curr pt.
            } else {
                // Jumped from low end to high.
                param_shift.x -= period;
                uv1.x += period;
            }
            ret = true;
        }
    }

    if closed_v {
        let ivl = domain.v_interval();
        let period = ivl.length();
        let jump = uv2.y - uv1.y;

        // The caller is stepping along the curve in 0.05 increments.
        if jump.abs() > 0.90 * period {
            if uv2.y < uv1.y {
                // Jumped from high end to low.
                param_shift.y += period;
                uv1.y -= period;
            } else {
                // Jumped from low end to high.
                param_shift.y -= period;
                uv1.y += period;
            }
            ret = true;
        }
    }

    ret
}

/// The result of [`are_connected`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Connection {
    /// The end of the first edge use meets the start of the second.
    pub connected: bool,
    /// `pOptOnSingularity`: both ends lie on a singularity (with a gap above the UV
    /// tolerance).
    pub on_singularity: bool,
    /// `pOptCrossesMissingSeam`: the gap spans a missing seam.
    pub crosses_missing_seam: bool,
}

/// `sm_AreConnected`: whether the end of `eu1` (the start of its mate) meets the start
/// of `eu2` in UV space within the end vertex's tolerance mapped to UV; with
/// `singularity_ok` a gap along a singular boundary both ends lie on also connects,
/// with `crosses_missing_seam_ok` a gap of one period of a closed direction (upstream
/// defaults `TRUE` and `FALSE`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// missing mate, vertex use, vertex, face or surface (upstream dereferences them); those
/// of the surface queries.
pub fn are_connected(
    k: &Kernel,
    b: &mut Brep,
    eu1: EdgeuseId,
    eu2: EdgeuseId,
    singularity_ok: bool,
    crosses_missing_seam_ok: bool,
) -> Result<Connection> {
    // init output
    let mut out = Connection::default();

    // Get end of pEU1 and start of pEU2.
    let mate = eu1::mate(b, eu1)?.ok_or_else(|| null("AreConnected: edge use without mate"))?;
    let vu1 =
        eu1::vertexuse(b, mate)?.ok_or_else(|| null("AreConnected: mate without vertex use"))?;
    let vu2 =
        eu1::vertexuse(b, eu2)?.ok_or_else(|| null("AreConnected: edge use without vertex use"))?;

    // FALSE = don't make UVTrimCurve, if missing use GlobalSolve (statuses ignored)
    let uv1 = ignored(vu1::compute_uv_point(k, b, vu1, false))?.unwrap_or(Point2::UNDEF);
    let uv2 = ignored(vu1::compute_uv_point(k, b, vu2, false))?.unwrap_or(Point2::UNDEF);
    let gap_u = (uv2.x - uv1.x).abs();
    let gap_v = (uv2.y - uv1.y).abs();

    let f = eu1::face(b, eu1)?.ok_or_else(|| null("AreConnected: edge use without face"))?;
    let v1 = vu1::vertex(b, vu1)?.ok_or_else(|| null("AreConnected: vertex use without vertex"))?;
    let tol_3d = b.vertex(v1)?.tolerance;
    let surf = surface_deref(b, f)?;
    let uv_tol = map_to_2d(k, tol_3d, uv1, surf)?;

    if uv1.distance(uv2) < uv_tol {
        out.connected = true;
        return Ok(out);
    }

    // when gaps across singularities are allowed
    if singularity_ok {
        let (on_1, dir_1) = is_singularity(k, surf, uv1, tol_3d)?;
        let (on_2, dir_2) = is_singularity(k, surf, uv2, tol_3d)?;

        // both ends are on a singularity
        if on_1 && on_2 {
            out.on_singularity = true;
            let u_like = |d: SurfParamType| d == SurfParamType::U || d == SurfParamType::Both;
            let v_like = |d: SurfParamType| d == SurfParamType::V || d == SurfParamType::Both;

            // check for a properly oriented singularity at the gap
            if dir_1 == SurfParamType::Both && dir_2 == SurfParamType::Both {
                out.connected = true;
                return Ok(out);
            } else if u_like(dir_1) && u_like(dir_2) {
                if gap_v < uv_tol {
                    out.connected = true;
                    return Ok(out);
                }
            } else if v_like(dir_1) && v_like(dir_2) && gap_u < uv_tol {
                out.connected = true;
                return Ok(out);
            }
        }
    }

    // when gaps across MissingSeams are allowed
    if crosses_missing_seam_ok {
        // check for a properly oriented seam gap
        let domain = surf.natural_uv_domain_with_nurb(k)?;
        let (closed_u, _) = surf.is_closed(k, &domain, SurfParamType::U, Some(uv_tol), false)?;
        let (closed_v, _) = surf.is_closed(k, &domain, SurfParamType::V, Some(uv_tol), false)?;

        // when surface is closed
        let crossed = if closed_u && closed_v {
            (gap_u < uv_tol || (gap_u - domain.x_length()).abs() < uv_tol)
                && (gap_v < uv_tol || (gap_v - domain.y_length()).abs() < uv_tol)
        } else if closed_u {
            (gap_u - domain.x_length()).abs() < uv_tol && gap_v < uv_tol
        } else if closed_v {
            (gap_v - domain.y_length()).abs() < uv_tol && gap_u < uv_tol
        } else {
            false
        };
        if crossed {
            out.crosses_missing_seam = true;
            out.connected = true;
            return Ok(out);
        }
    }

    // all done
    Ok(out)
}

/// The result of [`review_start_and_stop_on_seam_crv_classifies`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeamCrvReview<O> {
    /// `rpFirstPtClassify != NULL`: the first interval's start classification is kept
    /// (it was not a false edge classification).
    pub first_kept: bool,
    /// `rpLastPtClassify != NULL`: the last interval's end classification is kept.
    pub last_kept: bool,
    /// `rpEdgeFirstSide`: the edge use bounding the first end.
    pub edge_first_side: Option<O>,
    /// `rpEdgeLastSide`: the edge use bounding the last end.
    pub edge_last_side: Option<O>,
    /// `lFalseEdgeClassifyCnt`: end classifications changed from edge to unknown.
    pub false_edge_classify_cnt: u64,
}

/// `sms_ReviewStartAndStopOnSeamCrvClassifies`: finds the edge uses bounding the two
/// ends of a seam curve classification; an end classified to an edge with no bounding
/// edge use of this face becomes [`PointClassType::Unknown`] (no object).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an empty classification; those of
/// `SmCurveInterval::GetIvlBound_EdgeuseObj`.
pub fn review_start_and_stop_on_seam_crv_classifies<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &mut CurveClassification<O>,
) -> Result<SeamCrvReview<O>> {
    // locals
    let size = classification.size();
    let last = size
        .checked_sub(1)
        .ok_or_else(|| null("ReviewStartAndStopOnSeamCrvClassifies: empty classification"))?;

    // init output
    let edge_first_side = classification.ivl_bound_edgeuse_obj(k, geometry, 0, true)?;
    let edge_last_side = classification.ivl_bound_edgeuse_obj(k, geometry, last, false)?;
    let mut out = SeamCrvReview {
        first_kept: true,
        last_kept: true,
        edge_first_side,
        edge_last_side,
        false_edge_classify_cnt: 0,
    };

    // check 1st ClassifyCrv End for error - classified to an edge but not using that edge side
    let first = &mut classification.at_mut(0)?.start;
    if first.point_class() == PointClassType::Edge && edge_first_side.is_none() {
        // remove the false CrvClassification EndPt SM_PC_EDGE classification
        first.set_class_object(PointClassType::Unknown, None);
        out.first_kept = false;
        out.false_edge_classify_cnt += 1;
    }

    // check last ClassifyCrv End
    let end = &mut classification.at_mut(last)?.end;
    if end.point_class() == PointClassType::Edge && edge_last_side.is_none() {
        end.set_class_object(PointClassType::Unknown, None);
        out.last_kept = false;
        out.false_edge_classify_cnt += 1;
    }

    // all done
    Ok(out)
}

// ---- interior points ----------------------------------------------------------------------

/// `CalculateAnInternalPoint`: a point of the face inside its boundaries, found by
/// classifying 20 constant-V then 20 constant-U UV lines across the face's UV domain
/// and evaluating the first interior interval at 45% of its range.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR` (upstream: output `(0, 0, 0)`)
/// without surface, for a surface without owner, and when no line meets the face
/// interior; [`SolidError::Unported`] from the line classification.
pub fn calculate_an_internal_point(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<Point3> {
    internal_point(k, b, f)
}

/// [`calculate_an_internal_point`] for a caller that holds only `&Brep`
/// (`query::api_queries`' `sm_CountContainedSamples`, whose sample B-rep is borrowed
/// next to the classified target): the line classifications run on a private copy of
/// the B-rep ([`qf3::face_classify_uv_line`]), so their lazy edits are dropped.
///
/// # Errors
///
/// As [`calculate_an_internal_point`].
pub(crate) fn calculate_an_internal_point_snapshot(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
) -> Result<Point3> {
    internal_point(k, b, f)
}

/// The B-rep forms the face queries `CalculateAnInternalPoint` and
/// `CreateCurvesByTrimming` classify on: `&mut Brep` in place
/// ([`curve_on_classify_mut`]), `&Brep` on the classification's snapshot.
trait QueryBrep: FaceBrep + Sized {
    /// `sm_FaceClassifyUVLine` of face `f` with `objects`.
    #[allow(clippy::too_many_arguments)] // sm_FaceClassifyUVLine's arguments
    fn classify_uv_line(
        k: &Kernel,
        f: FaceId,
        objects: &mut UvLineObjects<Self>,
        start: Point2,
        end: Point2,
        src_zone_tol_3d: f64,
        cc: &mut CurveClassification<UvLineHandle>,
    ) -> Result<()>;

    /// [`classify_curve_on_face`] of face `f`.
    #[allow(clippy::too_many_arguments)] // upstream's constructor arguments
    fn classify_curve(
        &mut self,
        k: &Kernel,
        f: FaceId,
        curve: &Curve,
        with_uv_curve: bool,
        tol: f64,
        data_size: usize,
    ) -> Result<Vec<CurveInterval<ClassHandle>>>;
}

impl QueryBrep for &Brep {
    fn classify_uv_line(
        k: &Kernel,
        f: FaceId,
        objects: &mut UvLineObjects<Self>,
        start: Point2,
        end: Point2,
        src_zone_tol_3d: f64,
        cc: &mut CurveClassification<UvLineHandle>,
    ) -> Result<()> {
        let b = objects.b;
        qf3::face_classify_uv_line(k, b, f, objects, start, end, src_zone_tol_3d, cc)
    }

    fn classify_curve(
        &mut self,
        k: &Kernel,
        f: FaceId,
        curve: &Curve,
        with_uv_curve: bool,
        tol: f64,
        data_size: usize,
    ) -> Result<Vec<CurveInterval<ClassHandle>>> {
        classify_curve_on_face(k, self, f, curve, with_uv_curve, tol, data_size)
    }
}

impl QueryBrep for &mut Brep {
    fn classify_uv_line(
        k: &Kernel,
        f: FaceId,
        objects: &mut UvLineObjects<Self>,
        start: Point2,
        end: Point2,
        src_zone_tol_3d: f64,
        cc: &mut CurveClassification<UvLineHandle>,
    ) -> Result<()> {
        qf3::face_classify_uv_line_mut(k, f, objects, start, end, src_zone_tol_3d, cc)
    }

    fn classify_curve(
        &mut self,
        k: &Kernel,
        f: FaceId,
        curve: &Curve,
        with_uv_curve: bool,
        tol: f64,
        data_size: usize,
    ) -> Result<Vec<CurveInterval<ClassHandle>>> {
        classify_curve_on_face_in_place(k, self, f, curve, with_uv_curve, tol, data_size)
    }
}

/// `CalculateAnInternalPoint` on `b` (see [`QueryBrep`]).
fn internal_point<B: QueryBrep>(k: &Kernel, b: B, f: FaceId) -> Result<Point3> {
    // locals
    let face = b.brep().face(f)?;
    let uv_dom = face.uv_domain;
    let u_min = uv_dom.u_min();
    let u_max = uv_dom.u_max();
    let v_min = uv_dom.v_min();
    let v_max = uv_dom.v_max();
    // SmTol::GetZoneTol3d(this): the face's tolerance (SM_USE_OLDTOL)
    let src_zone_tol_3d = face.tolerance;

    // no work - no surface
    let Some(surf) = face.surface.as_ref() else {
        return Err(SolidError::sm_err(OP));
    };

    // no work - Bspline surface with no owner
    if surf.owner().is_none() {
        return Err(SolidError::sm_err(OP));
    }

    let mut classification: CurveClassification<UvLineHandle> = CurveClassification::new(k);
    let mut uv_objects = UvLineObjects::new(b, f);

    // first for constant U then V isoparameter lines
    for ii in 0..2 {
        for jj in 0..20_u32 {
            let param = f64::from(jj) / 21.0;
            let (start, end) = if ii == 0 {
                // constant V
                let v = (1.0 - param) * v_min + (param) * v_max;
                (Point2::new(u_min, v), Point2::new(u_max, v))
            } else {
                // constant U
                let u = (1.0 - param) * u_min + (param) * u_max;
                (Point2::new(u, v_min), Point2::new(u, v_max))
            };

            // classify the target UVLine (status ignored)
            ignore_status(B::classify_uv_line(
                k,
                f,
                &mut uv_objects,
                start,
                end,
                src_zone_tol_3d,
                &mut classification,
            ))?;

            // when intervals were found in the face
            for ivl in classification.intervals() {
                if ivl.mid.point_class() == PointClassType::Face {
                    // Found an interior point (on the classification's UV line).
                    let Some(uv_crv) = classification
                        .curve()
                        .and_then(|h| ClassGeometry::curve(&uv_objects, h))
                    else {
                        continue;
                    };
                    let curve_parameter = ivl.interval.evaluate(0.45);
                    let Ok(uv3) = uv_crv.evaluate_point(k, curve_parameter) else {
                        continue;
                    };
                    // pSurface: the face's surface, read after the classification's
                    // edits (it stays the same surface)
                    let Some(surf) = uv_objects.brep().face(f)?.surface.as_ref() else {
                        return Err(SolidError::sm_err(OP));
                    };
                    let Ok(p) = surf.evaluate_point(k, Point2::new(uv3.x, uv3.y)) else {
                        continue;
                    };
                    return Ok(p);
                }
            }
        }
    }

    // arrive here when no curve classify to face
    Err(SolidError::sm_err(OP))
}

/// The result of [`points_in_face`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PointsInFace {
    /// `rUV`: the sampled surface parameters.
    pub uv: Vec<Point2>,
    /// `rPV`: for each UV point its position, then the surface normal.
    pub pv: Vec<Vec3>,
}

/// A dither of upstream's `((rand() - RAND_MAX/2) / (1.2*RAND_MAX))`, drawn from
/// `kernel.rng`.
fn rand_dither(k: &mut Kernel) -> f64 {
    // an integer in [0, RAND_MAX] as rand() returns
    let r = (k.rng.next_f64() * (RAND_MAX + 1.0)).floor();
    let half = (RAND_MAX / 2.0).floor(); // RAND_MAX/2 in integer arithmetic
    (r - half) / (1.2 * RAND_MAX)
}

/// `GetPointsInFace`: `point_count` points sampled inside the face with their
/// positions and normals. A rectangularly trimmed face is sampled on a grid of
/// constant-U columns; another face samples interior intervals of classified
/// constant-U UV lines. With more than one point the V values are dithered by a
/// random draw (`kernel.rng`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR` for a UV domain not longer than
/// `SM_EFF_ZERO` in either direction and when no interior line is found;
/// [`SolidError::InvalidInput`] for more than [`MAX_POINTS_IN_FACE`] points or a face
/// without surface; [`SolidError::Unported`] for the trimmed branch.
pub fn points_in_face(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    point_count: u64,
) -> Result<PointsInFace> {
    // init output
    let mut out = PointsInFace::default();

    // check input - return SM_ERR for degenerate surface
    let face = b.face(f)?;
    let dom = face.uv_domain;
    let mut u_interval = dom.u_interval();
    let v_interval = dom.v_interval();
    let mut u_inc = u_interval.length();
    let mut v_inc = v_interval.length();
    if u_inc <= EFF_ZERO || v_inc <= EFF_ZERO {
        return Err(SolidError::sm_err(OP));
    }
    if point_count == 0 {
        return Ok(out);
    }
    if point_count > MAX_POINTS_IN_FACE {
        return Err(null("GetPointsInFace: point count too large"));
    }

    // locals (the surface is cloned: the face classifications below edit `b`)
    let rectangular_trim = face.rectangular_trim;
    let surf = surface_deref(b, f)?.clone();
    let count = point_count as usize;
    let mut trim_curve_cnt = (2 * point_count).min(30);
    u_inc /= (trim_curve_cnt + 1) as f64;
    let mut u = u_inc;

    if rectangular_trim == 1 {
        // when faced is rectangularly trimmed
        trim_curve_cnt = point_count.min(30);
        let j_cnt = if (point_count / trim_curve_cnt) * trim_curve_cnt < point_count {
            (point_count / trim_curve_cnt) + 1
        } else {
            point_count / trim_curve_cnt
        };
        v_inc /= (j_cnt + 1) as f64;

        // generate an array of sample points
        for _ in 0..trim_curve_cnt {
            let mut v = v_inc;
            for _ in 0..j_cnt {
                if out.uv.len() == count {
                    break;
                }
                let v_dither = if point_count > 1 {
                    rand_dither(k) * v_inc
                } else {
                    0.0
                };
                out.uv.push(Point2::new(u, v + v_dither));
                v += v_inc;
            }
            u += u_inc;
        }
    } else {
        // face is trimmed - classify UV isoParameterUVTrimCurves
        let u_inc = 1.0 / ((trim_curve_cnt + 1) as f64);

        // set search range to face boundary domain
        // (status ignored upstream: on a failure the output is still the face's UV
        // domain, which CalculateUVDomainFromUVTrimCurves sets first)
        let trim_uv_domain = match calculate_uv_domain_from_uv_trim_curves(k, b, f) {
            Ok(d) => d,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => b.face(f)?.uv_domain,
        };
        let srf_dom = surf.natural_uv_domain_with_nurb(k)?;
        // (SmExtent2d::Intersect leaves the output unchanged when disjoint)
        let uv_dom = srf_dom
            .intersect(&trim_uv_domain)
            .unwrap_or_else(|_| Extent2d::new());
        // SetMinMax status ignored
        let _ = u_interval.set_min_max(uv_dom.min().x, uv_dom.max().x);

        // create isoParameter UVTrimCurve - we'll keep reusing it until done
        let uv_start = dom.evaluate(u_inc, 0.0);
        let uv_end = dom.evaluate(u_inc, 1.0);
        let mut uv_curve = BSplineCurve::create_line_segment(
            2,
            Point3::new(uv_start.x, uv_start.y, 0.0),
            Point3::new(uv_end.x, uv_end.y, 0.0),
        )
        .map_err(|_| SolidError::sm_err(OP))?;

        // for all u values of interest - see if any map to inside the face
        let mut classes: Vec<(f64, Vec<CurveInterval<ClassHandle>>, Curve)> = Vec::new();
        let mut ii: u64 = 0;
        while ii < trim_curve_cnt && (classes.len() as u64) < point_count {
            // start in the middle work our way out
            let half = (trim_curve_cnt / 2) as f64;
            let off = if ii % 2 == 0 {
                (ii / 2) as f64
            } else {
                -(ii.div_ceil(2) as f64)
            };
            let u_param = (half + off) * u_inc;
            let u = u_interval.evaluate(u_param);

            // move pNurb to desired U param value
            {
                let cur = uv_curve.get_or_create_gw_nurb()?;
                for pw in cur.cpts_mut().iter_mut().take(2) {
                    pw.x = u;
                }
            }

            // classify the isoParameter UVTrimCurve against the face
            let tol = b.face(f)?.tolerance;
            let crv = Curve::from(uv_curve.clone());
            let intervals = classify_curve_on_face_in_place(k, b, f, &crv, true, tol, 0)?;
            if intervals
                .iter()
                .any(|ivl| ivl.mid.point_class() == PointClassType::Face)
            {
                classes.push((u, intervals, crv));
            }
            ii += 1;
        }

        // if we failed to find any U Params inside the face quit
        if classes.is_empty() {
            return Err(SolidError::sm_err(OP));
        }

        // number of points to sample from each iso curve
        let n_classes = classes.len() as u64;
        let smp_cnt = if (point_count / n_classes) * n_classes == point_count {
            point_count / n_classes
        } else {
            (point_count / n_classes) + 1
        };

        // for every Classified Curve
        for (u_param, intervals, crv) in &classes {
            // for every curve interval that's inside the face
            if let Some(ivl) = intervals
                .iter()
                .find(|ivl| ivl.mid.point_class() == PointClassType::Face)
            {
                for kk in 0..smp_cnt {
                    let v_dither = if point_count > 1 { rand_dither(k) } else { 0.0 };
                    let v_norm = ((kk + 1) as f64 + v_dither) / ((smp_cnt + 1) as f64);
                    // status ignored: the point keeps its default value
                    let uv_point = crv
                        .evaluate_point(k, ivl.interval.evaluate(v_norm))
                        .unwrap_or(Vec3::UNDEF);
                    out.uv.push(Point2::new(*u_param, uv_point.y));
                }
            }
        }
    }

    // arrive here after picking a sUV Pnts to evaluate - set outputs (statuses ignored)
    let mut pnt = Vec3::UNDEF;
    let mut nrm = Vec3::UNDEF;
    for &uv in &out.uv {
        if let Some(p) = ignored(surf.evaluate_point(k, uv))? {
            pnt = p;
        }
        if let Some(n) = ignored(surf.evaluate_normal(k, uv, Side::Left, Side::Left))? {
            nrm = n;
        }
        out.pv.push(pnt);
        out.pv.push(nrm);
    }

    // all done
    Ok(out)
}

// ---- precise properties integrands ----------------------------------------------------------

/// `SM_PPF_AREA`.
pub const PPF_AREA: u64 = 0x01;
/// `SM_PPF_AREA_FIRST_MOMENTS`.
pub const PPF_AREA_FIRST_MOMENTS: u64 = 0x02;
/// `SM_PPF_AREA_SECOND_MOMENTS`.
pub const PPF_AREA_SECOND_MOMENTS: u64 = 0x04;
/// `SM_PPF_VOLUME`.
pub const PPF_VOLUME: u64 = 0x08;
/// `SM_PPF_VOLUME_FIRST_MOMENTS`.
pub const PPF_VOLUME_FIRST_MOMENTS: u64 = 0x10;
/// `SM_PPF_VOLUME_SECOND_MOMENTS`.
pub const PPF_VOLUME_SECOND_MOMENTS: u64 = 0x20;
/// `SM_PPF_ALL`.
pub const PPF_ALL: u64 = 0x3f;

/// The component layout of the precise property integrals (`SmPrecisePropertyLayout`):
/// one-based indices into the integrand array, `0` for a group not selected; component
/// `1` always controls the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrecisePropertyLayout {
    /// `m_lNumComponents`.
    pub num_components: usize,
    /// `m_lArea`.
    pub area: usize,
    /// `m_lVolume`.
    pub volume: usize,
    /// `m_aMoments`: area first, area second (three groups of three), volume first,
    /// volume second (three groups of three) moments.
    pub moments: [usize; 8],
}

impl PrecisePropertyLayout {
    /// The layout of the `SM_PPF_*` groups selected in `flags`.
    #[must_use]
    pub fn new(flags: u64) -> Self {
        let mut l = Self {
            num_components: 1,
            area: 0,
            volume: 0,
            moments: [0; 8],
        };
        l.area = l.add(flags & PPF_AREA, 1);
        l.volume = l.add(flags & PPF_VOLUME, 1);
        let moment_flags = [
            PPF_AREA_FIRST_MOMENTS,
            PPF_AREA_SECOND_MOMENTS,
            PPF_AREA_SECOND_MOMENTS,
            PPF_AREA_SECOND_MOMENTS,
            PPF_VOLUME_FIRST_MOMENTS,
            PPF_VOLUME_SECOND_MOMENTS,
            PPF_VOLUME_SECOND_MOMENTS,
            PPF_VOLUME_SECOND_MOMENTS,
        ];
        for (m, mf) in l.moments.iter_mut().zip(moment_flags) {
            *m = {
                let selected = flags & mf;
                if selected == 0 {
                    0
                } else {
                    let index = l.num_components + 1;
                    l.num_components += 3;
                    index
                }
            };
        }
        l
    }

    /// `Add`: the next free index for `count` components when `selected` is non-zero,
    /// else `0`.
    pub fn add(&mut self, selected: u64, count: usize) -> usize {
        if selected == 0 {
            return 0;
        }
        let index = self.num_components + 1;
        self.num_components += count;
        index
    }
}

/// `rDyDxValues[i] += v` (`set`: `=`), out-of-range indices rejected.
fn put(dydx: &mut [f64], i: usize, v: f64, set: bool) -> Result<()> {
    let slot = dydx
        .get_mut(i)
        .ok_or_else(|| null("precise properties: component index outside the array"))?;
    if set {
        *slot = v;
    } else {
        *slot += v;
    }
    Ok(())
}

/// The integrand along a constant-U iso curve of the precise property integrals
/// (`SmODEIsoCurveLengthIFEO`): area and its moments of the surface (or, for a thick
/// face, of its two offset surfaces) and the volume under it and its moments.
#[derive(Debug, Clone, Copy)]
pub struct OdeIsoCurveLength<'a> {
    kernel: &'a Kernel,
    surface: &'a Surface,
    u_iso_value: f64,
    origin: Point3,
    face_thickness: f64,
    layout: &'a PrecisePropertyLayout,
}

impl<'a> OdeIsoCurveLength<'a> {
    /// The integrand on `surface` at `u = u_iso_value` relative to `origin` (`kernel`
    /// is the context of a thick face's temporary offset surfaces).
    #[must_use]
    pub const fn new(
        kernel: &'a Kernel,
        surface: &'a Surface,
        u_iso_value: f64,
        origin: Point3,
        face_thickness: f64,
        layout: &'a PrecisePropertyLayout,
    ) -> Self {
        Self {
            kernel,
            surface,
            u_iso_value,
            origin,
            face_thickness,
            layout,
        }
    }

    /// `Evaluate`: fills `dydx` (all entries zeroed first) at `v = t`.
    ///
    /// # Errors
    ///
    /// Those of the surface evaluation (for a thick face, of constructing and
    /// evaluating its offset surfaces); [`SolidError::InvalidInput`] for a layout
    /// index outside `dydx`.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub fn evaluate(&self, t: f64, dydx: &mut [f64]) -> Result<()> {
        let lay = self.layout;
        let mut pv = [[Vec3::ZERO; 2]; 2];
        let mut pv2 = [[Vec3::ZERO; 2]; 2];

        for d in dydx.iter_mut() {
            *d = 0.0;
        }

        let mut offset_area_sum = 0.0;
        for i in 0..2 {
            if self.face_thickness < EFF_ZERO && i == 1 {
                continue;
            }

            let uv = Point2::new(self.u_iso_value, t);

            if self.face_thickness < EFF_ZERO {
                self.surface.evaluate_simple_first_into(
                    self.kernel,
                    uv,
                    simple_flags(),
                    &mut pv,
                )?;
            } else if i == 0 {
                let r = offset_evaluate_simple(
                    self.kernel,
                    self.surface,
                    self.face_thickness / 2.0,
                    uv,
                )?;
                copy_block(&mut pv, &r);
            } else {
                pv2[0][0] = pv[0][0];
                pv2[1][0] = pv[1][0];
                pv2[0][1] = pv[0][1];
                let r = offset_evaluate_simple(
                    self.kernel,
                    self.surface,
                    -self.face_thickness / 2.0,
                    uv,
                )?;
                copy_block(&mut pv, &r);
            }

            pv[0][0] -= self.origin;

            // Compute first fundamental form
            let norm = pv[1][0].cross(pv[0][1]);
            let area_delta = norm.length();
            let mut ipnt = pv[0][0]; // Point to integrate
            put(dydx, 1, 0.0, true)?;
            if pv[1][0].length() > 0.0 {
                put(dydx, 1, area_delta, false)?; // the integration function used
            }
            // to control tolerances
            offset_area_sum += area_delta; // Also needed for thick-face volume, even without AREA.
            if lay.area != 0 {
                put(dydx, lay.area, area_delta, false)?;
            }

            // Compute area static moments
            let idx = lay.moments[0];
            if idx != 0 {
                put(dydx, idx, ipnt.x * area_delta, false)?;
                put(dydx, idx + 1, ipnt.y * area_delta, false)?;
                put(dydx, idx + 2, ipnt.z * area_delta, false)?;
            }

            let idx = lay.moments[1];
            if idx != 0 {
                // Compute area Ixx, Iyy, Izz
                put(dydx, idx, (ipnt.x * ipnt.x) * area_delta, false)?;
                put(dydx, idx + 1, (ipnt.y * ipnt.y) * area_delta, false)?;
                put(dydx, idx + 2, (ipnt.z * ipnt.z) * area_delta, false)?;

                // Compute area Iyz, Izx, Ixy
                put(dydx, idx + 3, (ipnt.y * ipnt.z) * area_delta, false)?;
                put(dydx, idx + 4, (ipnt.z * ipnt.x) * area_delta, false)?;
                put(dydx, idx + 5, (ipnt.x * ipnt.y) * area_delta, false)?;

                // Compute area second moments Ixx, Iyy, Izz about coordinate axes
                put(
                    dydx,
                    idx + 6,
                    (ipnt.y * ipnt.y + ipnt.z * ipnt.z) * area_delta,
                    false,
                )?;
                put(
                    dydx,
                    idx + 7,
                    (ipnt.x * ipnt.x + ipnt.z * ipnt.z) * area_delta,
                    false,
                )?;
                put(
                    dydx,
                    idx + 8,
                    (ipnt.x * ipnt.x + ipnt.y * ipnt.y) * area_delta,
                    false,
                )?;
            }

            // Volume moments need the local volume contribution
            if lay.volume == 0 && lay.moments[4] == 0 && lay.moments[5] == 0 {
                continue;
            }

            // If we have a thick face then we need to wait until the second
            // time around the loop.
            if self.face_thickness > EFF_ZERO && i == 0 {
                continue;
            }

            // Now Do Mass Properties
            if self.face_thickness < EFF_ZERO {
                // Do volumetric property
                let dist_to_plane = pv[0][0].z;
                let mut unit_norm = norm;
                if unit_norm.length_squared() > EFF_ZERO_SQ {
                    unit_norm = unit_norm.unitize()?;
                }
                let volume_delta = area_delta * dist_to_plane * unit_norm.z;
                if lay.volume != 0 {
                    put(dydx, lay.volume, volume_delta, true)?;
                }

                // Compute volume static moments
                let idx = lay.moments[4];
                if idx != 0 {
                    put(dydx, idx, ipnt.x * volume_delta, true)?;
                    put(dydx, idx + 1, ipnt.y * volume_delta, true)?;
                    put(dydx, idx + 2, ipnt.z * volume_delta / 2.0, true)?;
                }

                let idx = lay.moments[5];
                if idx != 0 {
                    // Compute Volume Moments Ixx, Iyy, Izz
                    put(dydx, idx, (ipnt.x * ipnt.x) * volume_delta, true)?;
                    put(dydx, idx + 1, (ipnt.y * ipnt.y) * volume_delta, true)?;
                    put(dydx, idx + 2, (ipnt.z * ipnt.z / 3.0) * volume_delta, true)?;

                    // Compute Volume Moments Iyz, Izx, Ixy
                    put(dydx, idx + 3, (ipnt.y * ipnt.z) * volume_delta / 2.0, true)?;
                    put(dydx, idx + 4, (ipnt.z * ipnt.x) * volume_delta / 2.0, true)?;
                    put(dydx, idx + 5, (ipnt.x * ipnt.y) * volume_delta, true)?;

                    // Compute Volume Second Moments Ixx, Iyy, Izz about Coordinate Axes
                    put(
                        dydx,
                        idx + 6,
                        (ipnt.y * ipnt.y + ipnt.z * ipnt.z / 3.0) * volume_delta,
                        true,
                    )?;
                    put(
                        dydx,
                        idx + 7,
                        (ipnt.x * ipnt.x + ipnt.z * ipnt.z / 3.0) * volume_delta,
                        true,
                    )?;
                    put(
                        dydx,
                        idx + 8,
                        (ipnt.x * ipnt.x + ipnt.y * ipnt.y) * volume_delta,
                        true,
                    )?;
                }
            } else {
                // Volume is average of areas times thickness of face.
                let volume_delta = offset_area_sum * self.face_thickness / 2.0;
                if lay.volume != 0 {
                    put(dydx, lay.volume, volume_delta, true)?;
                }

                // The thickness-direction centroid is only needed for moments.
                if lay.moments[4] == 0 && lay.moments[5] == 0 {
                    continue;
                }

                // Find New Center of Gravity for little volume.
                let norm_first = pv2[1][0].cross(pv2[0][1]);
                let area_delta_first = norm_first.length();

                // Perform a fake integration over the area to find the COG
                let delta = self.face_thickness / 100.0;
                let mut total_volume = delta * area_delta_first;
                let mut t_value = 0.0;
                for kkk in 0..100_u32 {
                    let tol = (f64::from(kkk) + 1.0) / 100.0;
                    let curr_area = area_delta_first + tol * (area_delta - area_delta_first);
                    let curr_volume = curr_area * delta;
                    let t2 = curr_volume / (curr_volume + total_volume);
                    t_value = t_value + t2 * (tol - t_value);
                    total_volume += curr_volume;
                }

                // Compute new center point
                let vec = pv[0][0] - pv2[0][0];
                ipnt = pv2[0][0] + vec * t_value;

                // Compute volume static moments
                let idx = lay.moments[4];
                if idx != 0 {
                    put(dydx, idx, ipnt.x * volume_delta, true)?;
                    put(dydx, idx + 1, ipnt.y * volume_delta, true)?;
                    put(dydx, idx + 2, ipnt.z * volume_delta, true)?;
                }

                let idx = lay.moments[5];
                if idx != 0 {
                    put(dydx, idx, (ipnt.x * ipnt.x) * volume_delta, true)?;
                    put(dydx, idx + 1, (ipnt.y * ipnt.y) * volume_delta, true)?;
                    put(dydx, idx + 2, (ipnt.z * ipnt.z) * volume_delta, true)?;
                    put(dydx, idx + 3, (ipnt.y * ipnt.z) * volume_delta, true)?;
                    put(dydx, idx + 4, (ipnt.z * ipnt.x) * volume_delta, true)?;
                    put(dydx, idx + 5, (ipnt.x * ipnt.y) * volume_delta, true)?;
                    put(
                        dydx,
                        idx + 6,
                        (ipnt.y * ipnt.y + ipnt.z * ipnt.z) * volume_delta,
                        true,
                    )?;
                    put(
                        dydx,
                        idx + 7,
                        (ipnt.x * ipnt.x + ipnt.z * ipnt.z) * volume_delta,
                        true,
                    )?;
                    put(
                        dydx,
                        idx + 8,
                        (ipnt.x * ipnt.x + ipnt.y * ipnt.y) * volume_delta,
                        true,
                    )?;
                }
            }
        }

        Ok(())
    }
}

/// The integrand along a UV boundary curve of the precise property integrals
/// (`SmODEAreaIFEO`): the iso-curve integral from `min_v` to the curve point, scaled
/// by the curve's negated `du/dt`.
#[derive(Debug, Clone, Copy)]
pub struct OdeArea<'a> {
    /// The context of a thick face's offset surfaces (`m_crSurface`'s).
    pub kernel: &'a Kernel,
    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_crUVCurve`.
    pub uv_curve: &'a Curve,
    /// `m_dMinV`.
    pub min_v: f64,
    /// `m_dIsoCurveAccuracy`.
    pub iso_curve_accuracy: f64,
    /// `m_pYScale`.
    pub y_scale: &'a [f64],
    /// `m_vOriginOfComputation`.
    pub origin: Point3,
    /// `m_dFaceThickness`.
    pub face_thickness: f64,
    /// `m_crLayout`.
    pub layout: &'a PrecisePropertyLayout,
}

impl OdeArea<'_> {
    /// `Evaluate`: replaces `dydx` by `0` followed by the iso-curve integral times
    /// `-du/dt` when `|du/dt| > SM_EFF_ZERO`; otherwise leaves it unchanged.
    ///
    /// # Errors
    ///
    /// Those of the curve evaluation and the inner integration evaluator.
    pub fn evaluate(&self, k: &Kernel, t: f64, dydx: &mut Vec<f64>) -> Result<()> {
        let pv = self.uv_curve.evaluate(k, t, 1, true, true)?;
        let p0 = pv
            .first()
            .copied()
            .ok_or_else(|| null("OdeArea: no curve point"))?;
        let p1 = pv
            .get(1)
            .copied()
            .ok_or_else(|| null("OdeArea: no curve tangent"))?;

        let eval = OdeIsoCurveLength::new(
            self.kernel,
            self.surface,
            p0.x,
            self.origin,
            self.face_thickness,
            self.layout,
        );

        if p1.x.abs() > EFF_ZERO {
            let num_components = self.y_scale.len();
            let mut dy = Vec::new();
            dy.resize(num_components, 0.0);
            if k.fixes.precise_properties_split_at_surface_knots {
                // Along the iso-u line the integrand is smooth only within each knot
                // span of the surface: integrated span by span, as the trim curve is
                // (WR-494; the source fork's source fork@08d9a14).
                let mut breaks =
                    surface_knots_inside(k, self.surface, SurfParamType::V, self.min_v, p0.y)?;
                breaks.insert(0, self.min_v);
                breaks.push(p0.y);
                let mut piece_y = Vec::new();
                piece_y.resize(num_components, 0.0);
                for pair in breaks.windows(2) {
                    let &[a, b] = pair else {
                        continue;
                    };
                    piece_y.fill(0.0);
                    // A failed inner integration is not a zero-area contribution.
                    runge_kutta_ode_integrate(
                        &eval,
                        &mut piece_y,
                        a,
                        b,
                        self.iso_curve_accuracy,
                        (b - a) / 2.0,
                        self.y_scale,
                    )?;
                    for (total, part) in dy.iter_mut().zip(piece_y.iter()) {
                        *total += *part;
                    }
                }
            } else {
                let step = p0.y - self.min_v;

                // A failed inner integration is not a zero-area contribution.
                runge_kutta_ode_integrate(
                    &eval,
                    &mut dy,
                    self.min_v,
                    p0.y,
                    self.iso_curve_accuracy,
                    step / 2.0,
                    self.y_scale,
                )?;
            }
            dydx.clear();
            dydx.push(0.0);
            dydx.extend_from_slice(&dy);
            for v in dydx.iter_mut().skip(1) {
                // (* -1.0: an exact negation)
                *v = -(*v * p1.x);
            }
        }

        Ok(())
    }
}

/// `sm_SurfaceKnotsInside` (the source fork's source fork@08d9a14): the
/// unique knots of a B-spline-kind surface in `param`'s direction strictly inside
/// `(lo, hi)`, ascending; none for a surface not of the B-spline kind (smooth
/// everywhere). The knots are read as upstream's virtual `GetKnots` does, with the NURB
/// of a lazy analytic surface made ([`Surface::knots_with_nurb`]).
///
/// # Errors
///
/// Those of [`Surface::is_kind_of`] and [`Surface::knots_with_nurb`].
pub fn surface_knots_inside(
    k: &Kernel,
    surface: &Surface,
    param: SurfParamType,
    lo: f64,
    hi: f64,
) -> Result<Vec<f64>> {
    let mut knots = Vec::new();
    if !surface.is_kind_of(crate::surface::surface::p2::B_SPLINE_SURFACE_TYPE)? {
        return Ok(knots);
    }
    let (all, _) = surface.knots_with_nurb(k, param, None)?;
    for &knot in &all {
        if knot > lo && knot < hi {
            knots.push(knot);
        }
    }
    Ok(knots)
}

/// `sm_TrimCurveBreaks` (the source fork's source fork@08d9a14): where the
/// precise-property integrand is not smooth along the UV trim curve `uv_curve` on
/// `surface`: the curve's own knots over its natural interval, and the parameters where
/// it crosses a knot line `u = k` of a B-spline surface (found per knot span by a sign
/// change of `u(t) - k` between `4 (degree + 1)` evenly spaced points, then at most 64
/// bisections). Ascending, the curve's ends included; breaks the integrator cannot step
/// between are merged into one, a curve knot kept over a crossing.
///
/// # Errors
///
/// `SM_ERR` for fewer than two curve knots; those of the curve's knots, bounding box,
/// degree and evaluation and of [`surface_knots_inside`].
pub fn trim_curve_breaks(k: &Kernel, uv_curve: &Curve, surface: &Surface) -> Result<Vec<f64>> {
    let mut breaks = Vec::new();
    let ivl = uv_curve.natural_interval();
    let (knots, _) = uv_curve.knots(k, Some(&ivl))?;
    if knots.len() < 2 {
        return Err(SolidError::sm_err("topology::face::trim_curve_breaks"));
    }
    let req = crate::nurbs::b_spline_curve::p2::BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let bbox = uv_curve
        .calculate_bounding_box(k, &uv_curve.natural_interval(), req, true)?
        .normal
        .ok_or_else(|| null("trim_curve_breaks: bounding box not computed"))?;
    let u_lines = surface_knots_inside(k, surface, SurfParamType::U, bbox.min().x, bbox.max().x)?;

    let samples = 4 * (uv_curve.degree()? + 1);
    let u = |t: f64| -> Result<f64> { Ok(uv_curve.evaluate_point(k, t)?.x) };
    for pair in knots.windows(2) {
        let &[a, b] = pair else {
            continue;
        };
        breaks.push(a);
        for &line in &u_lines {
            let mut t0 = a;
            let mut u0 = u(t0)?;
            for i in 1..=samples {
                // (`(dB - dA) * iS / lSamples`: an unsigned count converted to double)
                let t1 = a + (b - a) * (i as f64) / (samples as f64);
                let u1 = u(t1)?;
                if (u0 - line) * (u1 - line) < 0.0 {
                    let mut lo = t0;
                    let mut hi = t1;
                    let mut u_lo = u0;
                    let mut bisections = 0;
                    while bisections < 64 && hi - lo > EFF_ZERO * (1.0 + hi.abs()) {
                        let mid = 0.5 * (lo + hi);
                        let u_mid = u(mid)?;
                        if (u_lo - line) * (u_mid - line) <= 0.0 {
                            hi = mid;
                        } else {
                            lo = mid;
                            u_lo = u_mid;
                        }
                        bisections += 1;
                    }
                    breaks.push(0.5 * (lo + hi));
                } else if u1 == line && i < samples {
                    breaks.push(t1);
                }
                t0 = t1;
                u0 = u1;
            }
        }
    }
    if let Some(&last) = knots.last() {
        breaks.push(last);
    }
    breaks.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));

    // A crossing found next to a knot (a corner of the trim curve on a knot line) is the
    // knot: breaks the integrator cannot step between are one, the curve's own knots and
    // ends kept. The integrator takes no step when its ends are `SM_ARE_SAME` or its
    // first step is `SM_ARE_SAME` as `SM_EFF_ZERO`, and the first step is half the span.
    let unsteppable = |a: f64, b: f64| {
        are_same(a, b) || are_same(0.5 * (b - a), EFF_ZERO) || 0.5 * (b - a) < EFF_ZERO
    };
    let mut distinct: Vec<f64> = Vec::new();
    for &brk in &breaks {
        if let Some(prev) = distinct.last_mut() {
            if unsteppable(*prev, brk) {
                if knots.contains(&brk) {
                    *prev = brk;
                }
                continue;
            }
        }
        distinct.push(brk);
    }
    Ok(distinct)
}

// ---- face curves ----------------------------------------------------------------------------

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

/// A trimmed slicing copy ([`bspline_slice`]) of `curve`; the trim's error after the
/// copy was made is returned with it.
fn trimmed_slice(k: &Kernel, curve: &Curve, interval: &Extent1d) -> Option<(Curve, Result<()>)> {
    let mut copy = bspline_slice(curve)?;
    let mut trim = *interval; // may snap sIvl by tol to existing knots
    let r = copy.trim(k, &mut trim, TrimMode::Notify);
    Some((copy, r))
}

/// The result of [`create_curves_by_trimming`].
#[derive(Debug, Clone, Default)]
pub struct TrimmedCurves {
    /// `cpOptNew3DCurves`: the 3D curves trimmed to the face (when asked for; B-spline
    /// copies, as the UV curves).
    pub curves_3d: Option<Vec<Curve>>,
    /// `cpOptNewUVCurves`: the UV curves trimmed to the face (when asked for; `None`
    /// entries for intervals of a non-B-spline UV curve). Upstream copies the
    /// `SmBSplineCurve` part of a derived curve (a line, conic, ...), so these are
    /// B-spline curves.
    pub uv_curves: Option<Vec<Option<Curve>>>,
}

/// `CreateCurvesByTrimming`: the parts of the given curves on the face (inside or on
/// its boundary). With UV curves the trimming classifies them in UV space (3D curves,
/// when also given, are trimmed in sync), else the 3D curves are classified in 3D.
/// `curve_tolerance` is the tolerance the curves were built to (the face tolerance is
/// used when larger or not given). `want_3d` / `want_uv` ask for the outputs.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a face
/// without surface (UV trimming) or a missing 3D curve (3D trimming) or no curves
/// (upstream dereferences them); [`SolidError::Unported`] from the classification.
#[allow(clippy::too_many_arguments)]
pub fn create_curves_by_trimming(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    curve_tolerance: Option<f64>,
    curves_3d: Option<&[Option<Curve>]>,
    uv_curves: Option<&[Option<Curve>]>,
    want_3d: bool,
    want_uv: bool,
) -> Result<TrimmedCurves> {
    curves_by_trimming(
        k,
        b,
        f,
        curve_tolerance,
        curves_3d,
        uv_curves,
        want_3d,
        want_uv,
    )
}

/// [`create_curves_by_trimming`] for a caller that holds only `&Brep`
/// (`intersect::topology_solver`'s face silhouettes, whose objects lend the face's
/// B-rep read-only): the classifications run on a private copy of the B-rep
/// ([`classify_curve_on_face`]), so their lazy edits are dropped.
///
/// # Errors
///
/// As [`create_curves_by_trimming`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_curves_by_trimming_snapshot(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    curve_tolerance: Option<f64>,
    curves_3d: Option<&[Option<Curve>]>,
    uv_curves: Option<&[Option<Curve>]>,
    want_3d: bool,
    want_uv: bool,
) -> Result<TrimmedCurves> {
    curves_by_trimming(
        k,
        b,
        f,
        curve_tolerance,
        curves_3d,
        uv_curves,
        want_3d,
        want_uv,
    )
}

/// `CreateCurvesByTrimming` on `b` (see [`QueryBrep`]).
#[allow(clippy::too_many_arguments)]
fn curves_by_trimming<B: QueryBrep>(
    k: &Kernel,
    mut b: B,
    f: FaceId,
    curve_tolerance: Option<f64>,
    curves_3d: Option<&[Option<Curve>]>,
    uv_curves: Option<&[Option<Curve>]>,
    want_3d: bool,
    want_uv: bool,
) -> Result<TrimmedCurves> {
    // locals
    let mut new_3d: Vec<Curve> = Vec::new();
    let mut new_uv: Vec<Option<Curve>> = Vec::new();

    // get max allowed distance from output 3DCurves to face
    let face_tol = tolerance(b.brep(), f)?;
    let tol_3d = match curve_tolerance {
        Some(t) => face_tol.max(t),
        None => face_tol,
    };

    if let Some(uv_list) = uv_curves {
        // a reasonable 2d tolerance based on the 3d tol
        let surf = surface_deref(b.brep(), f)?;
        let uv_scale = approx_derivative_lengths(k, surf, &uv_domain(b.brep(), f)?)?;
        let mut uv_scale_avg = 0.5 * (uv_scale.x + uv_scale.y);
        if uv_scale_avg < tol_3d / 1000.0 {
            uv_scale_avg = tol_3d / 1000.0;
        }
        let tol_uv = tol_3d / uv_scale_avg;

        // for every UVCurve
        for (i, uv) in uv_list.iter().enumerate() {
            // skip missing or degenerate UVTrimCurves
            let Some(curve) = uv else { continue };
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }

            // classify the UVCurve against the face
            let intervals = b.classify_curve(k, f, curve, true, tol_uv, 20)?;

            // for every curve classification
            for ivl in &intervals {
                let scaled_zero = EFF_ZERO * (1.0 + ivl.interval.max().abs());

                // skip degenerate parametric length intervals
                if ivl.interval.max() - ivl.interval.min() < scaled_zero {
                    continue;
                }

                // when interval classifies to face or edge
                if ivl.mid.point_class() == PointClassType::Face
                    || ivl.mid.point_class() == PointClassType::Edge
                {
                    // When saving 3DCurves - place trimmed 3DCurve copy into s3DCurves array
                    if want_3d {
                        if let Some(list) = curves_3d {
                            let c3 = list
                                .get(i)
                                .and_then(Option::as_ref)
                                .ok_or_else(|| null("CreateCurvesByTrimming: missing 3D curve"))?;
                            let (copy, r) =
                                trimmed_slice(k, c3, &ivl.interval).ok_or_else(|| {
                                    null("CreateCurvesByTrimming: 3D curve is not a B-spline")
                                })?;
                            r?;
                            new_3d.push(copy);
                        }
                    }

                    // place trimmed UVcurve copy into sUVCurves array (added before
                    // trimming)
                    match trimmed_slice(k, curve, &ivl.interval) {
                        Some((copy, r)) => {
                            new_uv.push(Some(copy));
                            r?;
                        }
                        None => new_uv.push(None),
                    }
                }
            }
        }
    } else {
        // Must be 3D trimming only
        let list = curves_3d.ok_or_else(|| null("CreateCurvesByTrimming: no curves to trim"))?;

        // for every 3DCurve
        for c in list {
            let curve = c
                .as_ref()
                .ok_or_else(|| null("CreateCurvesByTrimming: missing 3D curve"))?;

            // skip degenerate curves
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }

            // Classify the curve against the face
            let intervals = b.classify_curve(k, f, curve, false, tol_3d, 20)?;

            for ivl in &intervals {
                if ivl.mid.point_class() == PointClassType::Face
                    || ivl.mid.point_class() == PointClassType::Edge
                {
                    // place a trimmed curve copy into output
                    let (copy, r) = trimmed_slice(k, curve, &ivl.interval).ok_or_else(|| {
                        null("CreateCurvesByTrimming: 3D curve is not a B-spline")
                    })?;
                    r?;
                    new_3d.push(copy);
                }
            }
        }
    }

    // Now set output
    Ok(TrimmedCurves {
        curves_3d: want_3d.then_some(new_3d),
        uv_curves: want_uv.then_some(new_uv),
    })
}

/// The result of [`create_curves_from_face`].
#[derive(Debug, Clone, Default)]
pub struct FaceCurves {
    /// `rCurveLoops`: the number of curves of each edge loop.
    pub curve_loops: Vec<u64>,
    /// `pOpt3DCurves`: copies of the edge curves trimmed to their edges (when asked).
    pub curves_3d: Option<Vec<Curve>>,
    /// `pOptUVCurves`: copies of the UV trim curves, with gap-filling UV lines when
    /// only UV curves are asked for (when asked).
    pub uv_curves: Option<Vec<Curve>>,
    /// `rCurveOrients`: each curve's orientation in its loop.
    pub curve_orients: Vec<OrientType>,
    /// `rLoopPoints`: the point of each vertex loop.
    pub loop_points: Vec<Point3>,
}

/// `CreateCurvesFromFace`: copies of the face's loop curves (3D and/or UV) of the face
/// use with orientation `orientation`, ready for building a face from curves.
///
/// # Errors
///
/// `SM_ERR` when neither curve kind is asked for; [`SolidError::NotFound`] for a
/// dangling id; [`SolidError::InvalidInput`] for missing topology upstream dereferences;
/// those of the evaluations and copies; [`SolidError::Unported`] for edge loops
/// (`SmSurface::DropCurve`, `SmSurface::DropPoint` and the others of
/// [`crate::topology::edgeuse::p3`] when a UV trim curve must be made).
pub fn create_curves_from_face(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    orientation: OrientType,
    want_3d: bool,
    want_uv: bool,
) -> Result<FaceCurves> {
    // check state
    if !want_3d && !want_uv {
        return Err(SolidError::sm_err(OP)); // Need one or the other
    }

    // init outputs
    let mut out = FaceCurves {
        curves_3d: want_3d.then(Vec::new),
        uv_curves: want_uv.then(Vec::new),
        ..FaceCurves::default()
    };

    // select faceuse by orientation - get its loopuses
    let up =
        upward_faceuse(b, f)?.ok_or_else(|| null("CreateCurvesFromFace: face without face use"))?;
    let fu = if eu1::faceuse_orientation(b, up)? == orientation {
        up
    } else {
        eu1::faceuse_mate(b, up)?
            .ok_or_else(|| null("CreateCurvesFromFace: face use without mate"))?
    };
    let loopuses = eu1::faceuse_loopuses(b, fu, false, None)?;

    let uv_size = uv_domain(b, f)?.size();
    let hundredth_size = uv_size.length() / 100.0;

    // for every loop
    for lu in loopuses {
        // branch on Loop Type (vertex or edge loop)
        if let LoopuseStart::Vertexuse(_) = b.loopuse(lu)?.start {
            // add point locations of face's vertex loops
            let vus = loopuse_vertexuses(b, lu)?;
            let vu = vus
                .first()
                .copied()
                .flatten()
                .ok_or_else(|| null("CreateCurvesFromFace: vertex loop without vertex use"))?;
            let v = vu1::vertex(b, vu)?
                .ok_or_else(|| null("CreateCurvesFromFace: vertex use without vertex"))?;
            out.loop_points.push(b.vertex(v)?.point);
            continue;
        }

        // edge loop branch: get loop->edgeuses
        let edgeuses = eu1::loopuse_edgeuses(b, lu)?;
        out.curve_loops.push(edgeuses.len() as u64);

        let last_eu = *edgeuses
            .last()
            .ok_or_else(|| null("CreateCurvesFromFace: edge loop without edge uses"))?;
        let mut last_uv = normalized_evaluate_uv(k, b, last_eu, 1.0)?;

        // iter every edgeuse in this loop
        for &eu in &edgeuses {
            // evaluate edgeuse start point
            let curr_uv = normalized_evaluate_uv(k, b, eu, 0.0)?;

            // when saving just UVCurves
            if want_uv && !want_3d && last_uv.distance(curr_uv) > hundredth_size {
                // fill the gap with a line segment - add segment to output
                let line = BSplineCurve::create_line_segment(2, last_uv, curr_uv)?;
                if let Some(list) = out.uv_curves.as_mut() {
                    list.push(Curve::from(line));
                }
                out.curve_orients.push(OrientType::Same);
                if let Some(n) = out.curve_loops.last_mut() {
                    *n += 1;
                }
            }

            // move the lastUV point to end of this Edgeuse
            last_uv = normalized_evaluate_uv(k, b, eu, 1.0)?;

            // put this curve's orientation into output
            out.curve_orients.push(eu1::orientation(b, eu)?);

            // when saving UVCurves
            if want_uv {
                let uv_curve = get_or_create_uv_trim_curve(k, b, eu)?;
                let copy = uv_curve.copy(k)?;
                // NER(SM_CAST_PTR(SmBSplineCurve, pNewUV))
                if bspline_slice(&copy).is_none() {
                    return Err(SolidError::null_pointer(OP));
                }
                if let Some(list) = out.uv_curves.as_mut() {
                    list.push(copy);
                }
            }

            // when saving 3DCurves - place copy of 3DCurve in output
            if want_3d {
                let e = edge_deref(b, eu)?;
                let mut ivl = b.edge(e)?.interval;
                let mut copy = edge_curve(b, e)?.copy(k)?;
                copy.trim(k, &mut ivl, TrimMode::Notify)?;
                if let Some(list) = out.curves_3d.as_mut() {
                    list.push(copy);
                }
            }
        }
    }

    // all done
    Ok(out)
}

// ---- UV trim curve helpers ------------------------------------------------------------------

/// The holder of `eu`'s UV trim curve (the curve `GetUVTrimCurve` returns), which
/// upstream dereferences.
fn uv_trim_curve_holder(b: &Brep, eu: EdgeuseId) -> Result<EdgeuseId> {
    match eu1::uv_trim_curve(b, eu)? {
        (Some(_), Some(owner)) => Ok(owner),
        _ => Err(null("edge use without UV trim curve")),
    }
}

/// The UV trim curve held by `holder`.
fn held_curve(b: &Brep, holder: EdgeuseId) -> Result<&Curve> {
    b.edgeuse(holder)?
        .uv_trim_curve
        .as_ref()
        .ok_or_else(|| null("edge use without UV trim curve"))
}

/// Mutable access to the UV trim curve held by `holder`.
fn held_curve_mut(b: &mut Brep, holder: EdgeuseId) -> Result<&mut Curve> {
    b.edgeuse_mut(holder)?
        .uv_trim_curve
        .as_mut()
        .ok_or_else(|| null("edge use without UV trim curve"))
}

/// [`edit_end_point`] of the UV trim curve `holder` holds, between the
/// `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT, this, SM_NO_GET_OWNER(this), NULL)` calls
/// `SmBSplineCurve::EditEndPoint` makes (`SM_NO_GET_OWNER` is null): the curve is
/// notified under its kernel-issued identity in `env`'s context. A callback walking
/// the curve's owner reads the edge use `holder` and its edge
/// ([`object::report_owner_link`], identities from `env`'s `key_of`). An edit that
/// fails sends no `SM_NO_POST_EDIT`, as upstream's `SER` returns. The curve's object
/// cache, which `SmCurve::Notify` drops before an edit, has no counterpart on a UV trim
/// curve here. Without a context the edit notifies nobody.
///
/// # Errors
///
/// Those of [`edit_end_point`], of the attribute handlers ([`a_object::notify`]);
/// `InvalidInput` for an exhausted identity domain.
fn notified_end_point_edit(
    b: &mut Brep,
    env: Option<&mut NotifyEnv<'_>>,
    holder: EdgeuseId,
    point: Point3,
    edit_start: bool,
    weight: Option<f64>,
) -> Result<()> {
    let edit = |c: &mut Curve| edit_end_point(c, point, edit_start, weight);
    match env.filter(|e| e.context.is_some()) {
        Some(env) => notify_around_edit(b, env, holder, edit),
        None => edit(held_curve_mut(b, holder)?),
    }
}

/// `edit` of the UV trim curve `holder` holds, between `SM_NO_PRE_EDIT` and
/// `SM_NO_POST_EDIT` with the curve as caller and first data object, after reporting
/// the curve's and the edge use's owner links.
fn notify_around_edit(
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    holder: EdgeuseId,
    edit: impl Fn(&mut Curve) -> Result<()>,
) -> Result<()> {
    let holder_ref = (env.key_of)(EntityRef::Edgeuse(holder));
    let edge_ref = eu1::edge(b, holder)?.map(|e| (env.key_of)(EntityRef::Edge(e)));
    let this = {
        let curve = held_curve_mut(b, holder)?;
        let Some(ctx) = env.context.as_deref_mut() else {
            return Err(null("FixUVTrimCurves: no context"));
        };
        ObjectRef {
            type_code: curve.type_code(),
            key: ctx.geometry_key(curve.notify_identity_slot())?,
        }
    };
    // (the links a callback reads through GetOwner() / GetEdge())
    object::report_owner_link(env.context.as_deref_mut(), this.key, Some(holder_ref));
    object::report_owner_link(env.context.as_deref_mut(), holder_ref.key, edge_ref);
    let notify = |env: &mut NotifyEnv<'_>, operation| {
        a_object::notify(
            env.attributes,
            env.context.as_deref_mut(),
            operation,
            this,
            Some(this),
            None,
            None,
        )
    };
    notify(env, NotifyOperation::PreEdit)?;
    edit(held_curve_mut(b, holder)?)?;
    notify(env, NotifyOperation::PostEdit)
}

/// A `Trim` or `EditParameterization` of the UV trim curve `holder` holds, as
/// `sm_FixUVTrimCurves` makes them: when the edit does work (it changes the curve's
/// natural interval; both return before notifying otherwise), it notifies as
/// [`notified_end_point_edit`] does. Whether the edit works is found by running it on
/// a copy first.
fn notified_curve_edit(
    b: &mut Brep,
    env: Option<&mut NotifyEnv<'_>>,
    holder: EdgeuseId,
    edit: impl Fn(&mut Curve) -> Result<()>,
) -> Result<()> {
    let Some(env) = env.filter(|e| e.context.is_some()) else {
        return edit(held_curve_mut(b, holder)?);
    };
    let before = held_curve(b, holder)?.natural_interval();
    let mut probe = held_curve(b, holder)?.clone();
    edit(&mut probe)?;
    let after = probe.natural_interval();
    if before.min().to_bits() == after.min().to_bits()
        && before.max().to_bits() == after.max().to_bits()
    {
        return edit(held_curve_mut(b, holder)?);
    }
    notify_around_edit(b, env, holder, edit)
}

/// `SmBSplineCurve::EditEndPoint(point, bEditStart, pdWeight)` of a UV trim curve.
fn edit_end_point(
    curve: &mut Curve,
    point: Point3,
    edit_start: bool,
    weight: Option<f64>,
) -> Result<()> {
    match curve.kind_mut() {
        CurveKind::BSpline(c) => c.edit_end_point(point, edit_start, weight),
        CurveKind::Line(l) => {
            // MakeNurb (the SmLine override) when missing, then RefreshAnalytics
            if l.base.gw_nurb().is_none() {
                let _ = l.make_nurb();
            }
            l.base.edit_end_point(point, edit_start, weight)?;
            l.refresh_analytics()
        }
        _ => Err(null(
            "EditEndPoint: UV trim curve is not a B-spline or line",
        )),
    }
}

/// `sm_FixUVTrimCurves`: closes the UV gap between the start of `eu_current`'s UV trim
/// curve and the end of `eu_previous`'s (its clockwise neighbour in the loop) by
/// moving their end control points onto the vertex's UV point after trimming at their
/// drop points and stretching back over their intervals (unless the opt-in fix skips
/// those edits; module docs, #570). Gaps below `SM_EFF_ZERO / 10` or above a twentieth of the
/// smaller side of `uv_domain` are left alone. `vertex` is the vertex between the two
/// edge uses; `tol_3d` and `uv_dist_between` (the current UV gap) as upstream.
/// Returns whether the curves were modified.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for missing
/// topology, curves or surface upstream dereferences; those of the curve evaluations
/// and edits.
#[allow(clippy::too_many_arguments)]
pub fn fix_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    eu_current: EdgeuseId,
    eu_previous: EdgeuseId,
    vertex: VertexId,
    tol_3d: f64,
    uv_dist_between: f64,
    uv_domain: &Extent2d,
) -> Result<bool> {
    fix_uv_trim_curves_notified(
        k,
        b,
        eu_current,
        eu_previous,
        vertex,
        tol_3d,
        uv_dist_between,
        uv_domain,
        None,
    )
}

/// [`fix_uv_trim_curves`] whose curve edits notify through `env`: each
/// `EditEndPoint` sends `SM_NO_PRE_EDIT` and `SM_NO_POST_EDIT` with the edited UV trim
/// curve as caller and first data object ([`notified_end_point_edit`]), and so does each
/// `Trim` and `EditParameterization` that does work ([`notified_curve_edit`]). Without `env`
/// (or without its context) the edits notify nobody.
///
/// # Errors
///
/// Those of [`fix_uv_trim_curves`]; those of the notifications.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn fix_uv_trim_curves_notified(
    k: &Kernel,
    b: &mut Brep,
    eu_current: EdgeuseId,
    eu_previous: EdgeuseId,
    vertex: VertexId,
    tol_3d: f64,
    uv_dist_between: f64,
    uv_domain: &Extent2d,
    mut env: Option<&mut NotifyEnv<'_>>,
) -> Result<bool> {
    // no work - current gap is zero
    if uv_dist_between < EFF_ZERO / 10.0 {
        return Ok(false);
    }

    // skip relatively large edgeuse/CCWedgeuse gaps
    let uv_size = uv_domain.max() - uv_domain.min();
    let min_domain_size = uv_size.x.min(uv_size.y);
    if uv_dist_between > min_domain_size / 20.0 {
        return Ok(false);
    }

    // locals
    let vertex_point = b.vertex(vertex)?.point;
    let curr_face =
        eu1::face(b, eu_current)?.ok_or_else(|| null("FixUVTrimCurves: edge use without face"))?;
    let _ = surface_deref(b, curr_face)?.natural_uv_domain_with_nurb(k)?;

    let curr_edge = edge_deref(b, eu_current)?;
    let prev_edge = edge_deref(b, eu_previous)?;

    let curr_holder = uv_trim_curve_holder(b, eu_current)?;
    let prev_holder = uv_trim_curve_holder(b, eu_previous)?;
    let curr_ivl = held_curve(b, curr_holder)?.natural_interval();
    let prev_ivl = held_curve(b, prev_holder)?.natural_interval();

    // Get UVTrimCurve End parameters
    let (t_curr, edit_curr_start) = if eu1::orientation(b, eu_current)? == OrientType::Same {
        (curr_ivl.min(), true)
    } else {
        (curr_ivl.max(), false)
    };
    let (t_prev, edit_prev_start) = if eu1::orientation(b, eu_previous)? == OrientType::Same {
        (prev_ivl.max(), false)
    } else {
        (prev_ivl.min(), true)
    };

    // Get UVTrimCurve UV end Point positions
    let uv_curr = held_curve(b, curr_holder)?.evaluate_point(k, t_curr)?;
    let uv_prev = held_curve(b, prev_holder)?.evaluate_point(k, t_prev)?;

    // Get Vertexuse UV point. ComputeUVPoint smarter than LocalPointSolve alone.
    let vu = eu1::vertexuse(b, eu_current)?
        .ok_or_else(|| null("FixUVTrimCurves: edge use without vertex use"))?;
    let uv_vertex = match vu1::vertex_face_gap(k, b, vu, false)? {
        Some(gap) => gap.face_uv,
        None => ignored(vu1::compute_uv_point(k, b, vu, true))?.unwrap_or(Point2::UNDEF),
    };
    let uv_vertex_3d = Point3::new(uv_vertex.x, uv_vertex.y, 0.0);

    // project Vertex drop point to 3d through surface (statuses ignored)
    let surf = surface_deref(b, curr_face)?;
    let surf_vertex = ignored(surf.evaluate_point(k, uv_vertex))?.unwrap_or(Vec3::UNDEF);
    let _ = ignored(surf.evaluate_normal(k, uv_vertex, Side::Left, Side::Left))?;

    // UVGap sizes
    let uv_curr_edge_gap = uv_vertex_3d.distance(uv_curr);
    let uv_prev_edge_gap = uv_vertex_3d.distance(uv_prev);

    // edge end points (statuses ignored)
    let curr_edge_point =
        ignored(edge_curve(b, curr_edge)?.evaluate_point(k, t_curr))?.unwrap_or(Vec3::UNDEF);
    let prev_edge_point =
        ignored(edge_curve(b, prev_edge)?.evaluate_point(k, t_prev))?.unwrap_or(Vec3::UNDEF);

    // Vertex/SurfVertex and EdgeEnd/SurfVertex gaps (the out-of-tolerance branch only
    // warns upstream)
    let _vertex_gap = surf_vertex.distance(vertex_point);
    let _curr_edge_gap = surf_vertex.distance(curr_edge_point);
    let _prev_edge_gap = surf_vertex.distance(prev_edge_point);
    let _ = tol_3d;

    // drop sUVVertex to each UVTrimCurve, trim there and stretch back (upstream). The
    // opt-in WR-255 fix (#959/#570) skips this and only moves the end points below.
    if !k.fixes.uv_trim_corner_end_points_only {
        let curr_drop = ignored(held_curve(b, curr_holder)?.drop_point(
            k,
            &curr_ivl,
            uv_vertex_3d,
            None,
            1.1 * (EFF_ZERO + uv_curr_edge_gap),
            Some(t_curr),
            SolverOperation::Intersect,
        ))?;
        let prev_drop = ignored(held_curve(b, prev_holder)?.drop_point(
            k,
            &prev_ivl,
            uv_vertex_3d,
            None,
            1.1 * (EFF_ZERO + uv_prev_edge_gap),
            Some(t_prev),
            SolverOperation::Intersect,
        ))?;
        if let Some(d) = curr_drop.filter(|d| d.success) {
            let t_curr_new = d.parameter;
            if !are_same(t_curr, t_curr_new) {
                let mut trim_ivl = Extent1d::new();
                if eu1::orientation(b, eu_current)? == OrientType::Same {
                    let _ = trim_ivl.set_min_max(t_curr_new, curr_ivl.max());
                } else {
                    let _ = trim_ivl.set_min_max(curr_ivl.min(), t_curr_new);
                }
                if !are_same(trim_ivl.min(), trim_ivl.max()) {
                    // SmTol::GetZoneTol3d(curve) follows its actual holder's edge.
                    let zone_tol_3d = match eu1::edge(b, curr_holder)? {
                        Some(edge) => b.edge(edge)?.tolerance,
                        None => crate::geom::tol::zone_tol_3d(
                            k,
                            crate::geom::tol::SmallTopoType::NormalSize,
                        ),
                    };
                    notified_curve_edit(b, env.as_deref_mut(), curr_holder, |c| {
                        let mut ivl = trim_ivl;
                        c.trim_with_zone_tol(k, &mut ivl, TrimMode::Notify, zone_tol_3d)
                    })?;
                    notified_curve_edit(b, env.as_deref_mut(), curr_holder, |c| {
                        c.edit_parameterization(k, &curr_ivl)
                    })?;
                }
            }
        }
        if let Some(d) = prev_drop.filter(|d| d.success) {
            let t_prev_new = d.parameter;
            if !are_same(t_prev, t_prev_new) {
                let mut trim_ivl = Extent1d::new();
                if eu1::orientation(b, eu_previous)? == OrientType::Same {
                    let _ = trim_ivl.set_min_max(prev_ivl.min(), t_prev_new);
                } else {
                    let _ = trim_ivl.set_min_max(t_prev_new, prev_ivl.max());
                }
                if !are_same(trim_ivl.min(), trim_ivl.max()) {
                    // The previous curve can likewise be held by another edge use.
                    let zone_tol_3d = match eu1::edge(b, prev_holder)? {
                        Some(edge) => b.edge(edge)?.tolerance,
                        None => crate::geom::tol::zone_tol_3d(
                            k,
                            crate::geom::tol::SmallTopoType::NormalSize,
                        ),
                    };
                    notified_curve_edit(b, env.as_deref_mut(), prev_holder, |c| {
                        let mut ivl = trim_ivl;
                        c.trim_with_zone_tol(k, &mut ivl, TrimMode::Notify, zone_tol_3d)
                    })?;
                    notified_curve_edit(b, env.as_deref_mut(), prev_holder, |c| {
                        c.edit_parameterization(k, &prev_ivl)
                    })?;
                }
            }
        }
    }

    // get current edgeuse->EndWeights (statuses ignored)
    let end_weight = |c: &Curve, start: bool| -> f64 {
        match c.ends(k) {
            Ok(e) => {
                if start {
                    e.start_weight
                } else {
                    e.end_weight
                }
            }
            Err(_) => crate::math::UNDEF_DOUBLE,
        }
    };
    let w_curr = end_weight(held_curve(b, curr_holder)?, edit_curr_start);
    let w_prev = end_weight(held_curve(b, prev_holder)?, edit_prev_start);

    // move Currcurve endPoint to target point - no protection from introducing kinks
    if held_curve(b, curr_holder)?.is_rational()? {
        let p = uv_vertex_3d * w_curr;
        notified_end_point_edit(
            b,
            env.as_deref_mut(),
            curr_holder,
            p,
            edit_curr_start,
            Some(w_curr),
        )?;
    } else {
        notified_end_point_edit(
            b,
            env.as_deref_mut(),
            curr_holder,
            uv_vertex_3d,
            edit_curr_start,
            None,
        )?;
    }

    // move PrevCurve endPoint to target point
    if held_curve(b, prev_holder)?.is_rational()? {
        let p = uv_vertex_3d * w_prev;
        notified_end_point_edit(b, env, prev_holder, p, edit_prev_start, Some(w_prev))?;
    } else {
        notified_end_point_edit(b, env, prev_holder, uv_vertex_3d, edit_prev_start, None)?;
    }

    // all done
    Ok(true)
}

/// `sm_FindBestUVCurve`: of the candidate UV curves (`None` entries skipped) with their
/// orientations relative to the primary edge use, the one whose 3D midpoint on
/// `surface` is closest to `eu`'s edge midpoint with a tangent pointing the same way;
/// `None` when none is within 100 times the edge tolerance. The chosen curve is
/// reversed to `eu`'s orientation, reparameterised to the edge interval and moved out
/// of its slot.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for missing
/// topology or curves upstream dereferences, or an orientation missing for a
/// candidate; those of the evaluations and edits.
pub fn find_best_uv_curve(
    k: &Kernel,
    b: &Brep,
    surface: &Surface,
    eu: EdgeuseId,
    uv_curves: &mut [Option<Curve>],
    orientations: &[OrientType],
) -> Result<Option<Curve>> {
    // locals
    let e = edge_deref(b, eu)?;
    let prim_eu = eu1::primary_edgeuse(b, e)?
        .ok_or_else(|| null("FindBestUVCurve: edge without edge use"))?;
    let prim_orient = eu1::orientation(b, prim_eu)?;
    let eu_orient = eu1::orientation(b, eu)?;

    // get edgeuse->3DMidpoint and orientation adjusted tangent
    let ivl = b.edge(e)?.interval;
    let edge_pv = edge_curve(b, e)?.evaluate(k, ivl.evaluate(0.5), 1, true, true)?;
    let edge_p0 = edge_pv
        .first()
        .copied()
        .ok_or_else(|| null("FindBestUVCurve: no edge point"))?;
    let mut edge_p1 = edge_pv
        .get(1)
        .copied()
        .ok_or_else(|| null("FindBestUVCurve: no edge tangent"))?;
    if prim_orient != eu_orient {
        edge_p1 = -edge_p1;
    }

    // Find the closest UV curve which has the same orientation of tangent when lifted into 3d.
    let mut closest = 0usize;
    let mut closest_dist = BIG_DOUBLE;
    for (i, slot) in uv_curves.iter().enumerate() {
        let Some(uv_curve) = slot else { continue };

        // project UVTrimCurve into 3D space
        let crv_on_surf = CrvOnSurf::new(
            k,
            uv_curve.clone_for_evaluation()?,
            surface.clone_for_evaluation()?,
            None,
            0,
        )?;

        // get UVTrimCurve->3DMidPoint and orientation adjusted Tangent
        let interval = crv_on_surf.natural_interval();
        let pv = crv_on_surf.evaluate(k, interval.evaluate(0.5), 1, true, true)?;
        let p0 = pv
            .first()
            .copied()
            .ok_or_else(|| null("FindBestUVCurve: no curve point"))?;
        let mut p1 = pv
            .get(1)
            .copied()
            .ok_or_else(|| null("FindBestUVCurve: no curve tangent"))?;
        let orient = *orientations
            .get(i)
            .ok_or_else(|| null("FindBestUVCurve: orientation missing"))?;
        if orient != prim_orient {
            p1 = -p1;
        }

        // skip UVCurve with opposite pointing tangents
        if p1.dot(edge_p1) < 0.0 {
            continue;
        }

        // use UVCurves that are more than SM_EFF_ZERO from edge
        let dist = p0.distance(edge_p0);
        if dist < closest_dist {
            // save best hit
            closest_dist = dist;
            closest = i;

            // use exact hits
            if closest_dist < EFF_ZERO {
                break; // Should be close enough
            }
        }
    }

    // when best hit exceeds tolerance
    if closest_dist == BIG_DOUBLE || closest_dist > b.edge(e)?.tolerance * 100.0 {
        return Ok(None);
    }

    // use the found UVTrimCurve for this Edgeuse
    let slot = uv_curves
        .get_mut(closest)
        .ok_or_else(|| null("FindBestUVCurve: candidate index"))?;
    let curve = slot
        .as_mut()
        .ok_or_else(|| null("FindBestUVCurve: candidate missing"))?;
    let orient = *orientations
        .get(closest)
        .ok_or_else(|| null("FindBestUVCurve: orientation missing"))?;

    // reverse UVTrimCurve orientation as needed
    if orient != prim_orient {
        let interval = curve.natural_interval();
        curve.reverse_parameterization(k, &interval)?;
    }

    // revertex Edgeuse orientation as needed
    if eu_orient != prim_orient {
        let interval = curve.natural_interval();
        curve.reverse_parameterization(k, &interval)?;
    }

    // set UVTrimCurve parameterRange equal to Edge parameter range
    curve.edit_parameterization(k, &ivl)?;

    // all done - Set to null to prevent reuse
    Ok(slot.take())
}

/// Saves a surface's owner and puts it back (`SmRescueSurfaceOwner`: the constructor
/// saves, the destructor restores).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RescueSurfaceOwner {
    owner: Option<ObjectRef>,
    /// The face tolerance cached on the surface ([`Surface::owner_zone_tol_3d`]), put
    /// back with the owner (a caller that may change the face's tolerance meanwhile
    /// re-syncs it from the B-rep).
    zone_tol_3d: Option<f64>,
}

impl RescueSurfaceOwner {
    /// Saves `surface`'s owner.
    #[must_use]
    pub fn new(surface: &Surface) -> Self {
        Self {
            owner: surface.owner(),
            zone_tol_3d: surface.owner_zone_tol_3d(),
        }
    }

    /// The saved owner.
    #[must_use]
    pub const fn owner(&self) -> Option<ObjectRef> {
        self.owner
    }

    /// Restores the saved owner on `surface` (the destructor).
    pub fn restore(self, surface: &mut Surface) {
        surface.set_owner(self.owner);
        if self.zone_tol_3d.is_some() {
            surface.set_owner_zone_tol_3d(self.zone_tol_3d);
        }
    }
}

/// `sm_RemoveSuspectSplitAtSeamUVTrimCurves`: deletes the UV trim curves of every other
/// edge use (from the first) of each edge at a vertex the classification's interval
/// ends classify to: the first interval's start and every interval's end. `vertex_of`
/// resolves a classification's vertex object.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// vertex object `vertex_of` cannot resolve or missing topology upstream dereferences;
/// those of [`eu1::delete_uv_trim_curve`].
pub fn remove_suspect_split_at_seam_uv_trim_curves<O: ClassObject>(
    b: &mut Brep,
    classification: &CurveClassification<O>,
    vertex_of: impl Fn(O) -> Option<VertexId>,
) -> Result<()> {
    let resolve = |o: Option<O>| -> Result<Option<VertexId>> {
        match o {
            None => Ok(None),
            Some(o) => vertex_of(o)
                .map(Some)
                .ok_or_else(|| null("RemoveSuspectSplitAtSeamUVTrimCurves: unresolved vertex")),
        }
    };
    let mut ends: Vec<Option<VertexId>> = Vec::new();

    // do the 1st interval start point once
    if let Some(first) = classification.intervals().first() {
        ends.push(resolve(first.start.vertex_object())?);
    }

    // for every interval end point
    for ivl in classification.intervals() {
        ends.push(resolve(ivl.end.vertex_object())?);
    }

    // when EndPoint classifies to a Vertex - remove all vertex->Edge->UVTrimCurves
    for v in ends.into_iter().flatten() {
        for e in vertex_edges(b, v)? {
            // for everyOther Edgeuse
            for eu in edge_edgeuses(b, e)?.into_iter().step_by(2) {
                eu1::delete_uv_trim_curve(b, eu)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::ObjectKey;
    use crate::geom::object::ObjectRef;
    use crate::surface::plane::Plane;
    use crate::topology::{Face, Faceuse, Loopuse, ShellId};

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z))
    }

    #[test]
    fn type_strings_and_kind() {
        assert_eq!(TYPE, 16002);
        assert_eq!(class_type(), TYPE);
        assert_eq!(TYPE_STRING, "SmFace_TYPE");
        assert_eq!(CLASS_STRING, "SmFace");
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(topo::TYPE));
        assert!(!is_kind_of(16005));
    }

    #[test]
    fn accessors_on_a_bare_face() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        assert!(surface(&b, f).unwrap().is_none());
        assert_eq!(rectangular_trim(&b, f).unwrap(), UNSURE);
        assert_eq!(tolerance(&b, f).unwrap(), 0.0);
        assert_eq!(faceuses(&b, f).unwrap(), (None, None));
        assert!(loops(&b, f, false, None).unwrap().is_empty());
        let d = Extent2d::from_bounds(0.0, 1.0, 2.0, 3.0);
        set_uv_domain(&mut b, f, &d).unwrap();
        assert_eq!(uv_domain(&b, f).unwrap(), d);
        assert!(uv_domain(&b, FaceId::from_index(9)).is_err());
    }

    #[test]
    fn faceuses_and_loops() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let l0 = b
            .add_loop_record(crate::topology::Loop {
                face: f,
                edges: Vec::new(),
                loopuse: Some(LoopuseId::from_index(0)),
            })
            .unwrap();
        let l1 = b
            .add_loop_record(crate::topology::Loop {
                face: f,
                edges: Vec::new(),
                loopuse: Some(LoopuseId::from_index(1)),
            })
            .unwrap();
        let fu0 = b
            .add_faceuse_record(Faceuse {
                face: Some(f),
                mate: Some(FaceuseId::from_index(1)),
                orientation: OrientType::Same,
                loopuses: vec![LoopuseId::from_index(0), LoopuseId::from_index(1)],
                ..Faceuse::default()
            })
            .unwrap();
        let fu1 = b
            .add_faceuse_record(Faceuse {
                face: Some(f),
                mate: Some(fu0),
                orientation: OrientType::Opposite,
                ..Faceuse::default()
            })
            .unwrap();
        for l in [l0, l1] {
            b.add_loopuse_record(Loopuse {
                faceuse: Some(fu0),
                loop_: Some(l),
                ..Loopuse::default()
            })
            .unwrap();
        }
        set_faceuse(&mut b, f, Some(fu0)).unwrap();
        assert_eq!(faceuses(&b, f).unwrap(), (Some(fu0), Some(fu1)));
        assert_eq!(upward_faceuse(&b, f).unwrap(), Some(fu0));
        assert_eq!(loops(&b, f, false, None).unwrap(), vec![l0, l1]);
        assert_eq!(outer_loops(&b, f).unwrap(), vec![l0]);
        assert_eq!(
            loops(&b, f, true, Some(&[l1, l0, l1])).unwrap(),
            vec![l1, l0, l1]
        );
    }

    #[test]
    fn seam_crossing() {
        let dom = Extent2d::from_bounds(0.0, 0.0, 2.0, 4.0);
        let mut uv1 = Point2::new(1.95, 1.0);
        let mut shift = Vec2::new(0.0, 0.0);
        assert!(check_seam_crossing(
            &mut uv1,
            Point2::new(0.05, 1.0),
            &dom,
            true,
            false,
            &mut shift
        ));
        assert_eq!(shift, Vec2::new(2.0, 0.0));
        assert!((uv1.x - -0.05).abs() < 1e-15);
        let mut uv1 = Point2::new(1.0, 0.1);
        assert!(check_seam_crossing(
            &mut uv1,
            Point2::new(1.0, 3.9),
            &dom,
            true,
            true,
            &mut shift
        ));
        assert_eq!(shift, Vec2::new(2.0, -4.0));
        assert!(!check_seam_crossing(
            &mut uv1,
            Point2::new(1.0, 4.0),
            &dom,
            true,
            true,
            &mut shift
        ));
        // closed in neither direction: SM_SUCCESS (true)
        assert!(check_seam_crossing(
            &mut uv1,
            Point2::new(9.0, 9.0),
            &dom,
            false,
            false,
            &mut shift
        ));
    }

    #[test]
    fn precise_property_layout() {
        let l = PrecisePropertyLayout::new(PPF_ALL);
        assert_eq!(l.area, 2);
        assert_eq!(l.volume, 3);
        assert_eq!(l.moments, [4, 7, 10, 13, 16, 19, 22, 25]);
        assert_eq!(l.num_components, 27);
        let l = PrecisePropertyLayout::new(PPF_VOLUME | PPF_AREA_FIRST_MOMENTS);
        assert_eq!((l.area, l.volume, l.num_components), (0, 2, 5));
        assert_eq!(l.moments, [3, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn iso_curve_integrand_on_a_plane() {
        let p = Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        let lay = PrecisePropertyLayout::new(PPF_ALL);
        let k = Kernel::default();
        let e = OdeIsoCurveLength::new(&k, &p, 0.5, Vec3::ZERO, 0.0, &lay);
        let mut d = vec![9.0; lay.num_components + 1];
        e.evaluate(0.25, &mut d).unwrap();
        assert_eq!(d[0], 0.0);
        assert_eq!(d[1], 1.0);
        assert_eq!(d[lay.area], 1.0);
        assert_eq!(d[lay.volume], 2.0);
        // too short an array
        let mut short = vec![0.0; 3];
        assert!(e.evaluate(0.25, &mut short).is_err());
        // a thick face integrates its two offsets at +-thickness/2: twice the area
        let thick = OdeIsoCurveLength::new(&k, &p, 0.5, Vec3::ZERO, 0.1, &lay);
        thick.evaluate(0.25, &mut d).unwrap();
        assert_eq!(d[lay.area], 2.0);
    }

    #[test]
    fn rescue_surface_owner_restores() {
        let mut s = plane();
        let owner = ObjectRef {
            type_code: TYPE,
            key: ObjectKey(7),
        };
        s.set_owner(Some(owner));
        let rescue = RescueSurfaceOwner::new(&s);
        s.set_owner(None);
        rescue.restore(&mut s);
        assert_eq!(s.owner(), Some(owner));
    }

    #[test]
    fn points_in_a_rectangular_face() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b
            .add_face_record(Face {
                surface: Some(plane()),
                uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 2.0),
                rectangular_trim: 1,
                ..Face::new(ShellId::from_index(0))
            })
            .unwrap();
        let _ = s;
        let mut k = Kernel::default();
        let r = points_in_face(&mut k, &mut b, f, 1).unwrap();
        assert_eq!(r.uv, vec![Point2::new(1.0 / 3.0, 1.0)]);
        assert_eq!(r.pv.len(), 2);
        assert_eq!(r.pv[1], Vec3::Z);
        let r = points_in_face(&mut k, &mut b, f, 7).unwrap();
        assert_eq!(r.uv.len(), 7);
        assert!(points_in_face(&mut k, &mut b, f, MAX_POINTS_IN_FACE + 1).is_err());
        assert!(points_in_face(&mut k, &mut b, f, 0).unwrap().uv.is_empty());
        set_uv_domain(&mut b, f, &Extent2d::from_bounds(0.0, 0.0, 0.0, 1.0)).unwrap();
        assert!(points_in_face(&mut k, &mut b, f, 1).is_err());
    }

    #[test]
    fn internal_point_needs_surface_and_owner() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let k = Kernel::default();
        assert!(matches!(
            calculate_an_internal_point(&k, &mut b, f),
            Err(SolidError::Upstream { .. })
        ));
        b.face_mut(f).unwrap().surface = Some(plane());
        assert!(matches!(
            calculate_an_internal_point(&k, &mut b, f),
            Err(SolidError::Upstream { .. })
        ));
        b.face_mut(f)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .set_owner(Some(ObjectRef {
                type_code: TYPE,
                key: ObjectKey(1),
            }));
        // no loops: no line interval classifies into the face (the line statuses are
        // ignored), so the final SM_ERR
        assert!(matches!(
            calculate_an_internal_point(&k, &mut b, f),
            Err(SolidError::Upstream { .. })
        ));
    }

    #[test]
    fn curves_from_face_need_a_kind() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let k = Kernel::default();
        assert!(create_curves_from_face(&k, &mut b, f, OrientType::Same, false, false).is_err());
        assert!(matches!(
            create_curves_from_face(&k, &mut b, f, OrientType::Same, true, false),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn trimming_skips_missing_and_degenerate_curves() {
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        b.face_mut(f).unwrap().surface = Some(plane());
        b.face_mut(f).unwrap().uv_domain = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let k = Kernel::default();
        let uv = [None];
        let r =
            create_curves_by_trimming(&k, &mut b, f, None, None, Some(&uv), true, true).unwrap();
        assert_eq!(r.curves_3d.map(|v| v.len()), Some(0));
        assert_eq!(r.uv_curves.map(|v| v.len()), Some(0));
        let line = Curve::from(
            BSplineCurve::create_line_segment(2, Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0)).unwrap(),
        );
        let uv = [Some(line)];
        // a face without loops: CurveOnClassify's status
        assert!(matches!(
            create_curves_by_trimming(&k, &mut b, f, Some(1.0), None, Some(&uv), false, true),
            Err(SolidError::Upstream { .. })
        ));
        assert!(create_curves_by_trimming(&k, &mut b, f, None, None, None, true, false).is_err());
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct H(u32);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Vertex
        }
    }

    struct NoGeometry;

    impl ClassGeometry<H> for NoGeometry {
        fn curve(&self, _o: H) -> Option<&Curve> {
            None
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _o: H) -> Option<Point3> {
            None
        }
    }

    /// A box whose first self-holding UV trim is replaced by a three-span line that
    /// overshoots its vertex by 1e-3, after `fix_uv_trim_curves` with the WR-255 fix
    /// `end_points_only`: the trim before and after, whether its edge use runs `Same`,
    /// and the vertex's UV point.
    fn fix_overshooting_trim(end_points_only: bool) -> (Curve, Curve, bool, Point3) {
        use crate::nurbs::math::CPoint;
        let k = &mut Kernel::default();
        k.fixes.uv_trim_corner_end_points_only = end_points_only;
        let mut b = crate::primitive::api_primitives::create_box(k, Point3::ZERO, 10.0, 10.0, 10.0)
            .unwrap();
        // the first edge use holding its own UV trim curve, with its loop neighbour
        let eu = b
            .edgeuses()
            .ids()
            .find(|&e| matches!(eu1::uv_trim_curve(&b, e), Ok((Some(_), Some(h))) if h == e))
            .unwrap();
        let prev = eu1::cw_edgeuse(&b, eu).unwrap().unwrap();
        let vu = eu1::vertexuse(&b, eu).unwrap().unwrap();
        let v = b.vertexuse(vu).unwrap().vertex.unwrap();
        let f = eu1::face(&b, eu).unwrap().unwrap();
        let dom = uv_domain(&b, f).unwrap();

        // replace the trim by a three-span line that overshoots the vertex by `gap`
        let ivl = held_curve(&b, eu).unwrap().natural_interval();
        let (t0, t1) = (ivl.min(), ivl.max());
        let same = eu1::orientation(&b, eu).unwrap() == OrientType::Same;
        let (tv, tf) = if same { (t0, t1) } else { (t1, t0) };
        let at_vertex = held_curve(&b, eu).unwrap().evaluate_point(k, tv).unwrap();
        let far = held_curve(&b, eu).unwrap().evaluate_point(k, tf).unwrap();
        let gap = 1e-3;
        let dir = (at_vertex - far) * (1.0 / at_vertex.distance(far));
        let overshoot = at_vertex + dir * gap;
        let (start, end) = if same {
            (overshoot, far)
        } else {
            (far, overshoot)
        };
        let lerp = |s: f64| start + (end - start) * s;
        let pw: Vec<CPoint> = [0.0, 0.25, 0.75, 1.0]
            .iter()
            .map(|&s| {
                let p = lerp(s);
                CPoint::new(p.x, p.y, 0.0, 1.0)
            })
            .collect();
        let dt = t1 - t0;
        let u = [t0, t0, t0 + dt / 3.0, t0 + 2.0 * dt / 3.0, t1, t1];
        *held_curve_mut(&mut b, eu).unwrap() =
            Curve::from(BSplineCurve::from_data(2, 3, 5, 1, &pw, &u).unwrap());

        let before = held_curve(&b, eu).unwrap().clone();
        let fixed = fix_uv_trim_curves(k, &mut b, eu, prev, v, 1e-5, gap, &dom).unwrap();
        assert!(fixed);
        let after = held_curve(&b, eu).unwrap().clone();
        (before, after, same, at_vertex)
    }

    /// #959/#570 (WR-255): with the fix, closing a corner's UV gap moves only the trim's
    /// end control point: same knots, interior control points untouched.
    #[test]
    fn issue_959_fix_uv_trim_curves_keeps_interior_parameters() {
        let (before, after, same, at_vertex) = fix_overshooting_trim(true);
        let (CurveKind::BSpline(c0), CurveKind::BSpline(c1)) = (before.kind(), after.kind()) else {
            panic!("UV trim curve is no longer a B-spline");
        };
        // same knots: no parameter moved
        assert_eq!(c0.knots(None).unwrap(), c1.knots(None).unwrap());
        // interior control points untouched, the vertex end moved onto the vertex
        let (p0, p1) = (c0.control_points().unwrap(), c1.control_points().unwrap());
        assert_eq!(p0.get(1..3), p1.get(1..3));
        let moved = if same { p1.first() } else { p1.last() }.unwrap();
        let moved = Point3::new(moved.x, moved.y, 0.0);
        assert!(
            moved.distance(at_vertex) < 1e-12,
            "{moved:?} vs {at_vertex:?}"
        );
    }

    /// WR-255 without the fix (upstream): the trim is cut at the vertex's drop point and
    /// stretched back over its interval, so its interior knots slide.
    #[test]
    fn fix_uv_trim_curves_stretches_the_trim_by_default() {
        let (before, after, _, _) = fix_overshooting_trim(false);
        let (CurveKind::BSpline(c0), CurveKind::BSpline(c1)) = (before.kind(), after.kind()) else {
            panic!("UV trim curve is no longer a B-spline");
        };
        assert_ne!(c0.knots(None).unwrap(), c1.knots(None).unwrap());
    }

    #[test]
    fn review_of_an_empty_classification_is_rejected() {
        let k = Kernel::default();
        let mut cc: CurveClassification<H> = CurveClassification::new(&k);
        assert!(review_start_and_stop_on_seam_crv_classifies(&k, &NoGeometry, &mut cc).is_err());
    }
}

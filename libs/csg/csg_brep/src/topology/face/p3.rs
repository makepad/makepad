//! `SmFace`, part 3: loop orientation, UV trim curve creation, the face/face
//! intersection, degeneracy and lamina tests, composite boundary curves, tolerance
//! setting and change notification.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.
//!
//! | Rust                                         | Upstream                                  |
//! |----------------------------------------------|-------------------------------------------|
//! | [`compute_loop_orientation`], [`compute_loop_orientation_3d`] | `SmFace::ComputeLoopOrientation` |
//! | [`create_composites`]                        | `SmFace::CreateComposites`                |
//! | [`create_uv_trim_curves`], [`create_uv_trim_curves_notified`], [`UvTrimCurveGaps`] | `SmFace::CreateUVTrimCurves` |
//! | [`face_intersect`], [`FaceIntersectOptions`], [`FaceIntersection`], [`IntersectObject`] | `SmFace::FaceIntersect` |
//! | [`notify`]                                   | `SmFace::Notify`                          |
//! | [`set_tolerance`]                            | `SmFace::SetTolerance` (`SM_USE_OLDTOL`)  |
//! | [`is_degenerate`]                            | `SmFace::IsDegenerate`                    |
//! | [`is_closed_surface_with_vertex_loops`]      | `SmFace::IsClosedSurfaceWithVertexLoops`  |
//! | [`is_lamina`]                                | `SmFace::IsLamina`                        |
//! | [`has_consecutive_lamina_edges`]             | `SmFace::HasConsecutiveLaminaEdges`       |
//! | [`has_lamina_edge`]                          | `SmFace::HasLaminaEdge` (CO-PORTED in `topology.topology_traverser`, re-exported) |
//!
//! Deviations:
//! - **UV trim curve edit notifications.** Only the end point moves of
//!   `sm_FixUVTrimCurves` notify ([`create_uv_trim_curves_notified`], through the
//!   caller's [`NotifyEnv`]); `SmBSplineCurve::EditParameterization` of the curves
//!   `SmSurface::DropIsoCurve` builds notifies nobody (no edge use owns them yet, so no
//!   change tracker records it; `plan/findings.md`).
//! - **Object model.** A face is a record in the [`Brep`]'s arenas; the methods are free
//!   functions taking the B-rep and the face's id. Out-parameters are return values.
//!   Where upstream dereferences null (a face without surface, an edge without curve, an
//!   edge use without vertex use or clockwise neighbour, `GetLast` of an empty array, an
//!   uninitialised `SmPoint2d`) the port returns [`SolidError::InvalidInput`]. `SER` /
//!   `NER` returns are the callee's error or [`SolidError::Upstream`] `SM_ERR`.
//! - **`SmFace::GetEdges`** with default arguments is the co-ported
//!   `topology_traverser::face_edges`; `GetEdges(.., TRUE)` (outer loop only),
//!   `GetUpwardLoopuses`, `GetUpwardEdgeuses`, `GetUpwardVertexuses` (default arguments)
//!   and `GetBrep` call `topology::face::p4`.
//! - **Statuses.** `ComputeProjectedLoopOrientation`'s status is checked as upstream (a
//!   failure falls through to the 3D walk); `sm_AreConnected` and `IsDegenerate` of a
//!   curve return values; ignored as upstream: the `Trim`, `EvaluatePoint` and
//!   `Point3DClassify` calls of `FaceIntersect`, the `NormalizedEvaluate` /
//!   `Evaluate1stDerivatives` / `PointClassify` of the quick check and the
//!   `InsertTopologyIntersections` of `IsDegenerate`, the `SetUVTrimCurve` and
//!   `sm_FixUVTrimCurves` of `CreateUVTrimCurves`; [`SolidError::Unported`] always
//!   propagates. `CreateUVTrimCurves` reads the face domain, the surface poles and the
//!   natural domain before clearing the surface owner, as upstream.
//! - **`FaceIntersect`'s classifications.** With lent B-reps ([`FaceBreps::One`] /
//!   [`FaceBreps::Two`]) both `CurveOnClassify` calls run on the faces' own B-reps
//!   ([`crate::query::face::p2::curve_on_classify_mut`]), so their lazy edits stay on
//!   them in upstream order; as queries they keep the kept face caches. So do the
//!   touching points' `Point3DClassify` calls. Every caller lends the B-reps: there is no
//!   shared variant, so nothing classifies on a copy.
//! - **`ComputeLoopOrientation`** walks with a copy of the face surface. The anonymous
//!   `SmLoopOrientationSample` / `SmLoopOrientationSampler` helpers are the private
//!   `LoopOrientationSample` / `LoopOrientationSampler`; a sampling or refinement `FALSE`
//!   with a failing status is the error, with a successful one `Unknown`. A loop use of
//!   edge type without edge uses reaches `GetLast` of an empty array on the UV path
//!   (UB-189) and returns [`SolidError::InvalidInput`].
//! - **`CreateUVTrimCurves`** takes the optional UV curves and their orientations as one
//!   argument (upstream requires both or neither). The surface owner is cleared and
//!   restored around the walk ([`face2::RescueSurfaceOwner`]), also on errors, and the
//!   scope is recorded on the B-rep ([`Brep::begin_surface_owner_cleared`]): a nested
//!   `smsurf_GetSurfaceCache` of the face's surface (an edge use's `EvaluateBinormal`)
//!   then builds no face trim and does not create the face's UV trim curves itself,
//!   as upstream, whose surface has no face meanwhile. A chosen
//!   candidate curve is moved out of its slot; unused candidates stay with the caller
//!   (upstream deletes them).
//! - **`FaceIntersect`** returns its outputs in [`FaceIntersection`] (the optional ones
//!   when asked for in [`FaceIntersectOptions`]). Its classifications refer to the
//!   surface/surface intersection curves by index ([`IntersectObject::Curve`]); with
//!   `want_cc_curves` (or any classification array) those curves are returned. The coincidence solutions of
//!   `IsDegenerate` name the curves by a handle carrying their natural interval; the
//!   objects are then switched to the edges, as upstream. All returned curves are owned
//!   (upstream hands out deleted ones through `pOptCC3DCrvs` without classification
//!   arrays, UB-193). Outputs are return values, so upstream's quirk that
//!   `pOptThisCCUVCrvs` / `pOptOtherCCUVCrvs` are never reset (they append to the
//!   caller's arrays) has no counterpart; in the coincident branch the failing-copy
//!   path replaces all four vectors with the intersector's.
//! - **`Notify`** takes the face surface's trimmed surface cache (if any) for
//!   `SM_NO_PRE_EDIT` (`SmCacheMgr::GetObjectCache(SM_OC_TRIMSRF, ..)` has no counterpart:
//!   B-rep surfaces carry no object cache) and the notification state of
//!   [`edge::NotifyEnv`]. `SE_MSG` for `SM_NO_UNKNOWN` is compiled out.
//! - **Unported callees** return [`SolidError::Unported`]: the curve constructor of
//!   `SmCurveClassification` (`intersect.curve_class_p4`), `SmFace::Point3DClassify`
//!   (`query.face_p1`). `SmEdgeuse::NormalizedEvaluate`
//!   and `GetOrCreateUVTrimCurve` are wired (`topology.edgeuse_p3`; their own unported
//!   callees still return it).
//! - **`IsClosedSurfaceWithVertexLoops`** reads the cached closure when the face
//!   properties name this face and have read its surface ([`FaceProps::has_surface`]):
//!   the properties hold no surface handle to compare with `GetSurface()`.
//! - **Bounds.** `ComputeLoopOrientation`'s refinement recurses at most 20 levels deep
//!   (upstream's own bound) within a budget of 65536 samples; every other loop is over
//!   B-rep arrays or a fixed sample count.

use crate::curve::composite_curve::CompositeCurve;
use crate::curve::curve::p2::CRV_ON_SURF_TYPE;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::{Extent2d, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, MAX_TREES};
use crate::geom::tol::xsect_tol_3d_from_zones;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::sqrt as sm_sqrt;
use crate::heal::face_props::{FaceProps, LoopObject, PropStage, Tri};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p5::{ClassGeometryMut, ExtractOptions};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{BrepFaceServices, FaceCaches, SideObject, SidedFaceServices};
use crate::query::face::p2 as qf2;
use crate::surface::surface::p1::map_to_2d;
use crate::surface::surface::p2::{smos_max, TsectCurveType};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::Surface;
use crate::surface::trim_srf_cache::TrimSrfCache;
use crate::surface::volume::Volume;
use crate::topology::brep_cache::CacheObject;
use crate::topology::edge::{self, NotifyEnv};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p2 as eu2;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p2 as face2;
use crate::topology::loopuse as lu1;
use crate::topology::topology as topo;
use crate::topology::topology_traverser::face_edges;
use crate::topology::vertex;
use crate::topology::vertexuse as vu1;
use crate::topology::{
    Brep, BrepId, EdgeId, EdgeuseId, FaceId, LoopId, LoopuseId, VertexId, VertexuseId,
};

pub use crate::topology::topology_traverser::has_lamina_edge;

const OP: &str = "topology::face::p3";

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// ---- calls into other units (some not ported yet) ---------------------------------------

/// `pEU->NormalizedEvaluate(t, bUV, rPoint, pOptFirstDeriv)` (default
/// `bOkToMakeUVTrimCurve`): the point and, with `want_deriv`, the first derivative.
fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    uv: bool,
    want_deriv: bool,
) -> Result<(Point3, Option<Vec3>)> {
    let r = eu3::normalized_evaluate(k, b, eu, t, uv, want_deriv, true)?;
    Ok((r.point, r.first_deriv))
}

/// `pEU->GetOrCreateUVTrimCurve(rpUVCurve, pdOptDistToSurf)` (default
/// `bOKToMakeUVTrimCurve`, no `pdOptDeviation`; `pdOptDistToSurf` non-null with
/// `want_dist`): the returned status and `*pdOptDistToSurf` (`-1` for an existing curve
/// without a cached gap, and when not asked for). The curve is then
/// [`eu1::uv_trim_curve`].
fn get_or_create_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    want_dist: bool,
) -> Result<(Result<()>, f64)> {
    let r = eu3::get_or_create_uv_trim_curve(k, b, eu, want_dist, false, true)?;
    Ok((r.status, r.dist_to_surf))
}

/// `GetUpwardLoopuses(sLoopuses)` (default arguments).
fn upward_loopuses(b: &Brep, f: FaceId) -> Result<Vec<LoopuseId>> {
    crate::topology::face::p4::upward_loopuses(b, f, false, None)
}

/// `GetUpwardEdgeuses(sEdgeuses)` (default arguments).
fn upward_edgeuses(b: &Brep, f: FaceId) -> Result<Vec<EdgeuseId>> {
    crate::topology::face::p4::upward_edgeuses(b, f, false, None)
}

/// `GetUpwardVertexuses(sVertexuses)` (default arguments).
fn upward_vertexuses(b: &Brep, f: FaceId) -> Result<Vec<VertexuseId>> {
    crate::topology::face::p4::upward_vertexuses(b, f, false, None)
}

/// `GetEdges(sEdges, NULL, TRUE)`: the edges of the outer loop only.
fn outer_loop_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, true, None)
}

/// `this->GetBrep() ? this->GetBrep()->GetTolerance() : 0.0`.
fn brep_tolerance(b: &Brep, f: FaceId) -> Result<f64> {
    Ok(crate::topology::face::p4::brep(b, f)?.map_or(0.0, Brep::tolerance))
}

/// `new SmCurveClassification(pCurve, sInterval, pUVCurve, sSrcZoneTol3d)`.
fn curve_classification<O: ClassObject>(
    k: &Kernel,
    curve: O,
    interval: &Extent1d,
    uv_curve: Option<O>,
    src_zone_tol_3d: f64,
) -> Result<CurveClassification<O>> {
    // the remaining constructor arguments default: no interval storage, the curve not
    // owned and current, no surfaces, no topology intersector
    CurveClassification::with_curve(
        k,
        &crate::intersect::curve_class::ClassificationInit {
            curve: Some(curve),
            interval: *interval,
            uv_curve,
            src_zone_tol_3d,
            opt_data_size: 0,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )
}

/// `GetSurface()->CoincidenceCheck(other, tol, rbAreCoincident, rdDist, FALSE, ..)`:
/// whether the surfaces coincide and their largest distance.
fn coincidence_check(
    k: &Kernel,
    surface: &Surface,
    other: &Surface,
    tol: f64,
) -> Result<(bool, f64)> {
    let c = surface.coincidence_check(k, other, tol, false)?;
    Ok((c.are_coincident, c.max_distance))
}

/// `pFace->Point3DClassify(sPoint, tol, TRUE, sPC)` of a face of a B-rep lent mutably
/// (the classification makes the face's trimmed cache and UV trim curves on it): the
/// point's class. A failed classification leaves `SM_PC_UNKNOWN` (the status is ignored
/// upstream); [`SolidError::Unported`] propagates.
fn point_3d_classify(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    point: Point3,
    tol: f64,
) -> Result<PointClassType> {
    let mut pc = PointClassification::<EntityRef>::new(tol);
    ignore_status(crate::query::face::p1::point_3d_classify(
        k,
        b,
        f,
        point,
        tol,
        true,
        &mut pc,
        crate::query::face::p1::Point3dClassifyOptions::default(),
    ))?;
    Ok(pc.point_class())
}

// ---- small accessors ----------------------------------------------------------------------

fn surface_of(b: &Brep, f: FaceId) -> Result<&Surface> {
    face2::surface(b, f)?.ok_or_else(|| null("face without surface"))
}

fn edge_curve(b: &Brep, e: EdgeId) -> Result<&Curve> {
    b.edge(e)?
        .curve
        .as_ref()
        .ok_or_else(|| null("edge without curve"))
}

// ---- lamina tests ---------------------------------------------------------------------------

/// `IsLamina`: whether every edge of the face is lamina (`true` for a face without
/// edges).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for missing
/// topology links (those of the edge walk).
pub fn is_lamina(b: &Brep, f: FaceId) -> Result<bool> {
    let edges = face_edges(b, f)?;

    // Check each edge, returning False if any non-lamina edge is found.
    for e in edges {
        if !eu1::edge_is_lamina(b, e)? {
            return Ok(false);
        }
    }

    // arrive here when all face->Edges are lamina
    Ok(true)
}

/// `HasConsecutiveLaminaEdges`: whether two consecutive edges of the outer loop
/// (wrapping from the last to the first) are lamina.
///
/// # Errors
///
/// [`SolidError::Unported`]: `SmFace::GetEdges` of the outer loop (`topology.face_p4`).
pub fn has_consecutive_lamina_edges(b: &Brep, f: FaceId) -> Result<bool> {
    // JGU: taking advantage of GWC 603 updates. Not actually bug 603
    let edges = outer_loop_edges(b, f)?;
    let mut first_edge = false;
    let mut prior_edge = false;

    let Some(&first) = edges.first() else {
        return Ok(false);
    };

    // Check first edge
    if eu1::edge_is_lamina(b, first)? {
        first_edge = true;
        prior_edge = true;
    }

    // Check edges 1 to end
    for &e in edges.iter().skip(1) {
        if eu1::edge_is_lamina(b, e)? {
            if prior_edge {
                // Previous edge was lamina, so 2 in a row
                return Ok(true);
            }
            prior_edge = true;
        } else {
            prior_edge = false; // Reset SmBoolean, since not lamina
        }
    }

    // check first and last edge
    if first_edge && prior_edge {
        return Ok(true);
    }

    // arrive here when adjacent edges not lamina
    Ok(false)
}

// ---- tolerance and notification -----------------------------------------------------------

/// `SetTolerance`: sets the face tolerance to `new_zone_tol_3d` (only when larger with
/// `update_only_if_larger`); with `cascade_to_boundaries`, raises every face edge whose
/// tolerance is below it ([`edge::set_tolerance`] with the same flags). Upstream
/// defaults are `true`, `true`. `new_zone_tol_3d` is upstream's `SmZoneTol3d` argument:
/// a caller passing a plain double converts it first ([`crate::geom::tol::zone_tol3d`]).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of the edge walk and
/// [`edge::set_tolerance`].
pub fn set_tolerance(
    b: &mut Brep,
    f: FaceId,
    new_zone_tol_3d: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    // set tolerances - when asked skip shrinking tolerances
    let face = b.face_mut(f)?;
    if !update_only_if_larger || new_zone_tol_3d > face.tolerance {
        face.tolerance = new_zone_tol_3d;
    }
    // the face's surface reads it through GetFace()
    b.sync_face_surface_tolerance(f)?;

    // when asked - make sure Edge->VertexTols at least as big as thie EdgeTol
    if cascade_to_boundaries {
        for e in face_edges(b, f)? {
            if b.edge(e)?.tolerance < new_zone_tol_3d {
                edge::set_tolerance(
                    b,
                    e,
                    new_zone_tol_3d,
                    update_only_if_larger,
                    cascade_to_boundaries,
                )?;
            }
        }
    }
    Ok(())
}

/// `delete pFace`: `~SmFace` notifies `SM_NO_DESTRUCTION` (`Notify(op, this, NULL,
/// NULL)` through the face's context, `GetContext()`: the environment's, else `k`'s;
/// no trim cache), whose status it ignores ([`SolidError::Unported`] propagates); the
/// surface it owns is dropped with its record; `~SmAObject` (`ClearAttributes`)
/// removes it from every attribute it uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; the notification's
/// [`SolidError::Unported`]; those of `ClearAttributes`.
pub fn destroy(k: &mut Kernel, b: &mut Brep, f: FaceId, env: &mut NotifyEnv<'_>) -> Result<()> {
    b.face(f)?;
    let this = (env.key_of)(EntityRef::Face(f));
    edge::with_object_context(k, env, |n| {
        ignore_status(notify(
            b,
            f,
            n,
            None,
            NotifyOperation::Destruction,
            [Some(this), None, None],
        ))
    })?;
    b.remove_face_record(f)?;
    env.attributes.clear_attributes(this.key)
}

/// `Notify`: `PreEdit` resets the rectangular-trim flag to `UNSURE` and marks the
/// surface's trimmed surface cache (`trim_cache`, else the one the surface keeps) as modified; `PostEdit`,
/// `SplitInBrep`, `ChangeGeometry` and `Split` are passed to every edge use and vertex
/// use of the upward face use (clearing their cached UV trim curves and gaps); then the
/// notification goes to `SmTopology::Notify`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::Unported`] for the edit
/// notifications (`GetUpwardEdgeuses`, `topology.face_p4`); those of the edge use,
/// vertex use and topology notifications.
pub fn notify(
    b: &mut Brep,
    f: FaceId,
    env: &mut NotifyEnv<'_>,
    trim_cache: Option<&mut TrimSrfCache<EntityRef>>,
    operation: NotifyOperation,
    data: [Option<ObjectRef>; 3],
) -> Result<()> {
    use NotifyOperation as N;
    let [d1, d2, d3] = data;
    match operation {
        N::PreEdit => {
            // m_bRectangularTrim: UNSURE = Not yet set
            b.face_mut(f)?.rectangular_trim = face2::UNSURE;
            // the cache given, else the surface's own (GetObjectCache)
            if let Some(sc) = trim_cache {
                sc.base_mut().face_was_modified = true;
            } else if let Some(s) = b.face(f)?.surface.as_ref() {
                s.mark_trim_cache_face_modified();
            }
        }
        N::PostEdit | N::SplitInBrep | N::ChangeGeometry | N::Split => {
            // pass the notify call along to all Vertexuses, Edgeuses, and
            // Edgeuse->Vertexuses so that those can clean up their cached gaps
            let eus = upward_edgeuses(b, f)?;
            let vus = upward_vertexuses(b, f)?;
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
        // SM_NO_UNKNOWN: SE_MSG only (compiled out of the release build)
        _ => {}
    }

    // Propagate notification up hierarchy
    let this = (env.key_of)(EntityRef::Face(f));
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

// ---- composites ---------------------------------------------------------------------------

/// `CreateComposites`: one closed composite curve per loop of the face use with
/// orientation `SM_OT_SAME`, made of copies of the loop's 3D edge curves with their
/// senses (`false` for an edge use running opposite to its loop). The B-rep is mutable:
/// `CreateCurvesFromFace` may create missing UV trim curves.
///
/// # Errors
///
/// Those of [`face2::create_curves_from_face`] (including [`SolidError::Unported`] for
/// edge loops); [`SolidError::InvalidInput`] when the loop counts exceed the curves or
/// orientations returned (upstream reads past them).
pub fn create_composites(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<Vec<CompositeCurve>> {
    // init output
    let mut composites = Vec::new();

    // Create List of Curves from Face Loops
    let fc = face2::create_curves_from_face(k, b, f, OrientType::Same, true, false)?;
    let mut curves = fc.curves_3d.unwrap_or_default().into_iter();

    // for every loop
    let mut count = 0usize;
    for &num_crv in &fc.curve_loops {
        let mut senses = Vec::new();
        let mut loop_curves = Vec::new();

        // for every edge in the loop
        for _ in 0..num_crv {
            let curve = curves
                .next()
                .ok_or_else(|| null("CreateComposites: fewer curves than loop edges"))?;
            let orient = fc
                .curve_orients
                .get(count)
                .copied()
                .ok_or_else(|| null("CreateComposites: fewer orientations than curves"))?;
            count += 1;

            // build Curve and Senses array
            loop_curves.push(curve);
            senses.push(orient != OrientType::Opposite);
        }

        // build CompositeCurve
        composites.push(CompositeCurve::with_curves(
            3,
            loop_curves,
            true,
            Some(&senses),
            None,
        )?);
    }

    // all done
    Ok(composites)
}

// ---- loop orientation ---------------------------------------------------------------------

/// `sm_lLoopOrientationSampleLimit`: the sample budget of the 3D walk of
/// [`compute_loop_orientation`], shared by all its angular passes.
const LOOP_ORIENTATION_SAMPLE_LIMIT: usize = 65536;

/// `smos_Min(a, b)`: `a < b ? a : b`.
fn smos_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `SmLoopOrientationSample`: the raw drop stays in the surface domain; the shifted UV
/// follows the loop continuously across periodic boundaries.
#[derive(Debug, Clone, Copy)]
struct LoopOrientationSample {
    param: f64,
    raw_uv: Point2,
    uv: Point2,
    shift: Vec2,
}

impl Default for LoopOrientationSample {
    fn default() -> Self {
        // `SmPoint2d` / `SmVector2d` default-construct to `SM_UNDEF_DOUBLE`
        Self {
            param: 0.0,
            raw_uv: Point2::UNDEF,
            uv: Point2::UNDEF,
            shift: Vec2::UNDEF,
        }
    }
}

/// `SmLoopOrientationSampler`: state shared by the sampling and refinement helpers. The
/// sample budget spans all angular passes; area, error and bounds are reset for each pass.
struct LoopOrientationSampler<'a> {
    k: &'a Kernel,
    surface: &'a Surface,
    face_tol: f64,
    face_domain: Extent2d,
    surf_domain: Extent2d,
    width: f64,
    height: f64,
    closed_u: bool,
    closed_v: bool,
    samples: usize,
    angle: f64,
    area: f64,
    error: f64,
    base: Point2,
    bounds: Extent2d,
}

impl LoopOrientationSampler<'_> {
    /// `SignedArea`: the dimensionless signed area of the triangle base-start-end.
    fn signed_area(&self, start: &LoopOrientationSample, end: &LoopOrientationSample) -> f64 {
        ((start.uv.x - self.base.x) / self.width) * ((end.uv.y - self.base.y) / self.height)
            - ((start.uv.y - self.base.y) / self.height) * ((end.uv.x - self.base.x) / self.width)
    }

    /// `Sample`: drops `curve(param)` onto the surface (guessing from `previous`'s raw
    /// UV) and follows `previous`'s period shift. `Ok(None)` when the sample budget is
    /// spent or the drop fails (upstream `FALSE` with a successful status).
    fn sample(
        &mut self,
        curve: &Curve,
        param: f64,
        previous: Option<&LoopOrientationSample>,
    ) -> Result<Option<LoopOrientationSample>> {
        self.samples += 1;
        if self.samples > LOOP_ORIENTATION_SAMPLE_LIMIT {
            return Ok(None);
        }
        let point = curve.evaluate_point(self.k, param)?;
        let drop = self.surface.drop_point::<NoObject>(
            self.k,
            point,
            &self.face_domain,
            &DropPointOptions {
                guess_uv: previous.map(|p| p.raw_uv),
                ..DropPointOptions::default()
            },
        )?;
        if !drop.success {
            return Ok(None);
        }
        let mut sample = LoopOrientationSample {
            param,
            raw_uv: drop.result_uv,
            uv: Point2::UNDEF,
            shift: Vec2::new(0.0, 0.0),
        };
        if let Some(previous) = previous {
            let mut prev_raw = previous.raw_uv;
            sample.shift = previous.shift;
            face2::check_seam_crossing(
                &mut prev_raw,
                sample.raw_uv,
                &self.surf_domain,
                self.closed_u,
                self.closed_v,
                &mut sample.shift,
            );
        }
        sample.uv = sample.raw_uv + sample.shift;
        self.bounds.add_point(sample.uv);
        Ok(Some(sample))
    }

    /// `Refine`: includes quarter points (a midpoint alone can miss an S-shaped
    /// excursion) and subdivides until each piece is flat, then sums the piece's area and
    /// error. `Ok(false)` when a sample fails or past depth 20 (upstream `FALSE` with a
    /// successful status).
    fn refine(
        &mut self,
        curve: &Curve,
        start: &LoopOrientationSample,
        end: &LoopOrientationSample,
        depth: u32,
    ) -> Result<bool> {
        let Some(quarter) = self.sample(
            curve,
            start.param + (end.param - start.param) * 0.25,
            Some(start),
        )?
        else {
            return Ok(false);
        };
        let Some(mid) = self.sample(
            curve,
            start.param + (end.param - start.param) * 0.5,
            Some(&quarter),
        )?
        else {
            return Ok(false);
        };
        let Some(three_quarter) = self.sample(
            curve,
            start.param + (end.param - start.param) * 0.75,
            Some(&mid),
        )?
        else {
            return Ok(false);
        };
        let chord = Vec2::new(
            (end.uv.x - start.uv.x) / self.width,
            (end.uv.y - start.uv.y) / self.height,
        );
        let length_sq = chord.length_squared();
        let mut deviation = 0.0;
        for test in [&quarter, &mid, &three_quarter] {
            let offset = Vec2::new(
                (test.uv.x - start.uv.x) / self.width,
                (test.uv.y - start.uv.y) / self.height,
            );
            let fraction = if length_sq > 0.0 {
                smos_max(
                    0.0,
                    smos_min(1.0, (chord.x * offset.x + chord.y * offset.y) / length_sq),
                )
            } else {
                0.0
            };
            let distance = (offset - chord * fraction).length();
            deviation = smos_max(deviation, distance);
        }
        let uv_tol = map_to_2d(self.k, self.face_tol, start.raw_uv, self.surface)?;
        let flatness = smos_max(
            uv_tol / smos_min(self.width, self.height),
            sm_sqrt(length_sq) * self.angle / 360.0,
        );
        if deviation > flatness {
            if depth >= 20 {
                return Ok(false);
            }
            return Ok(self.refine(curve, start, &quarter, depth + 1)?
                && self.refine(curve, &quarter, &mid, depth + 1)?
                && self.refine(curve, &mid, &three_quarter, depth + 1)?
                && self.refine(curve, &three_quarter, end, depth + 1)?);
        }
        // Sum local magnitudes so opposite bends cannot cancel the estimate.
        self.error += (self.signed_area(start, &quarter) + self.signed_area(&quarter, &mid)
            - self.signed_area(start, &mid))
        .abs()
            + (self.signed_area(start, &mid) + self.signed_area(&mid, end)
                - self.signed_area(start, end))
            .abs()
            + (self.signed_area(&mid, &three_quarter) + self.signed_area(&three_quarter, end)
                - self.signed_area(&mid, end))
            .abs();
        self.area += self.signed_area(start, &quarter)
            + self.signed_area(&quarter, &mid)
            + self.signed_area(&mid, &three_quarter)
            + self.signed_area(&three_quarter, end);
        Ok(true)
    }
}

/// `ComputeLoopOrientation`: the orientation of loop `l` on the face: `Same` (counter-
/// clockwise in UV), `Opposite` (clockwise) or `Unknown`. A vertex loop is `Opposite`
/// without measuring an area. Without `do_3d_loop_computation`, the edge uses' existing
/// or generated UV trim curves decide when they form a connected loop. Otherwise (or
/// when they do not decide) the defining 3D curves are sampled in passes of decreasing
/// angle tolerance (10, 1, 0.1 degrees), keeping every knot and tessellating each span
/// separately; the samples are dropped onto a copy of the surface and their signed UV
/// area is refined adaptively. Unresolved area, a failed drop, an exhausted sample budget
/// or a walk ending on another periodic copy of its start is `Unknown`. With
/// `uv_bounds`, it is reset and receives the UV bounds of the deciding walk.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for missing
/// topology, a face without surface, an edge without curve, an edge loop without edge
/// uses on the UV path (upstream reads an empty array, UB-189); those of the surface
/// copy, knots, tessellations, evaluations and drops; [`SolidError::Unported`] (from
/// `GetOrCreateUVTrimCurve`).
#[allow(clippy::too_many_lines)] // one upstream routine
pub fn compute_loop_orientation(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    l: LoopId,
    mut uv_bounds: Option<&mut Extent2d>,
    do_3d_loop_computation: bool,
) -> Result<OrientType> {
    // init output
    if let Some(bounds) = uv_bounds.as_deref_mut() {
        *bounds = Extent2d::new();
    }

    // get Loopuses
    let (lu1, _lu2) = lu1::loop_loopuses(b, l)?;

    // VertexLoops are InnerLoops
    if lu1::is_vertex_loopuse(b, lu1)? {
        return Ok(OrientType::Opposite);
    }

    // arrive here for EdgeLoop type Loops - Work from the Edgeuses of pLU1.
    let edgeuses = lu1::edgeuses(b, lu1, None, None)?;

    // Integrate UVArea within the loop by walking the UVTrimCurves to determine its
    // orientation. The normal path can use or create derived trims. An explicit 3D
    // request skips this path even when cached trims are present.
    let mut uv_curves: Vec<Curve> = Vec::new();
    let mut senses: Vec<bool> = Vec::new();
    let mut uv_curves_ok = !do_3d_loop_computation;

    // for every Loop->Edgeuse
    for (ii, &eu) in edgeuses.iter().enumerate() {
        if !uv_curves_ok {
            break;
        }
        let ok = get_or_create_uv_trim_curve(k, b, eu, false)?.0.is_ok();
        let uv_curve = eu1::uv_trim_curve(b, eu)?.0.cloned();

        // no work - UVTrimCurve is missing - go try the 3d algorithm
        let Some(uv_curve) = uv_curve.filter(|_| ok) else {
            uv_curves_ok = false; // If a problem with any uv curve, force 3d computation.
            break;
        };

        // build UVTrimCurve data arrays
        uv_curves.push(uv_curve);
        senses.push(eu1::orientation(b, eu)? == OrientType::Same);

        // Check if UVTrimCurves are connected in UV space (not always near
        // singularities). If not, then don't use uv curves - try 2d algorithm
        if ii > 0 {
            let prev = edgeuses
                .get(ii - 1)
                .copied()
                .ok_or_else(|| null("ComputeLoopOrientation: no previous edge use"))?;
            if !face2::are_connected(k, b, prev, eu, true, false)?.connected {
                uv_curves_ok = false;
                break;
            }
        }
    }

    // Check LastEdge-to-FirstEdge UVSpace end-to-end connectedness.
    if uv_curves_ok {
        let (Some(&last), Some(&first)) = (edgeuses.last(), edgeuses.first()) else {
            return Err(null("ComputeLoopOrientation: loop use without edge uses"));
        };
        if !face2::are_connected(k, b, last, first, true, false)?.connected {
            uv_curves_ok = false;
        }
    }

    // when all the UVTrimCurves are available, connected, and form a closed loop
    if uv_curves_ok {
        // walk all UVTrimCurves to determine if this is a CCW or CW loop
        let normal = Vec3::new(0.0, 0.0, 1.0);
        match CompositeCurve::compute_projected_loop_orientation(
            k,
            &uv_curves,
            &senses,
            normal,
            uv_bounds.as_deref_mut(),
        ) {
            Ok(o) if o != OrientType::Unknown => return Ok(o),
            Err(e) if e.is_unported() => return Err(e),
            // a failure or an unknown orientation falls through to the 3D walk
            _ => {}
        }
    }

    // Work from defining geometry. In particular, TRUE must not trust cached trims.
    loop_orientation_3d(k, b, f, &edgeuses, uv_bounds)
}

/// [`compute_loop_orientation`] with `do_3d_loop_computation` (`TRUE`), which reads the
/// B-rep only: the UV trim curves are neither used nor made.
///
/// # Errors
///
/// As [`compute_loop_orientation`].
pub fn compute_loop_orientation_3d(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    l: LoopId,
    mut uv_bounds: Option<&mut Extent2d>,
) -> Result<OrientType> {
    // init output
    if let Some(bounds) = uv_bounds.as_deref_mut() {
        *bounds = Extent2d::new();
    }

    // get Loopuses
    let (lu1, _lu2) = lu1::loop_loopuses(b, l)?;

    // VertexLoops are InnerLoops
    if lu1::is_vertex_loopuse(b, lu1)? {
        return Ok(OrientType::Opposite);
    }
    let edgeuses = lu1::edgeuses(b, lu1, None, None)?;
    loop_orientation_3d(k, b, f, &edgeuses, uv_bounds)
}

/// The defining-geometry walk of [`compute_loop_orientation`] over the loop's edge uses.
#[allow(clippy::too_many_lines)] // one upstream routine
fn loop_orientation_3d(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    edgeuses: &[EdgeuseId],
    mut uv_bounds: Option<&mut Extent2d>,
) -> Result<OrientType> {
    let surf_copy = surface_of(b, f)?.copy(k)?;
    let face_domain = face2::uv_domain(b, f)?;
    let width = face_domain.x_length();
    let height = face_domain.y_length();
    if width <= 0.0 || height <= 0.0 || edgeuses.is_empty() {
        return Ok(OrientType::Unknown);
    }
    let surf_domain = surf_copy.natural_uv_domain_with_nurb(k)?;
    let closed_u = surf_copy
        .is_closed(k, &surf_domain, SurfParamType::U, None, false)?
        .0;
    let closed_v = surf_copy
        .is_closed(k, &surf_domain, SurfParamType::V, None, false)?
        .0;
    let mut sampler = LoopOrientationSampler {
        k,
        surface: &surf_copy,
        face_tol: face2::tolerance(b, f)?,
        face_domain,
        surf_domain,
        width,
        height,
        closed_u,
        closed_v,
        samples: 0,
        angle: 0.0,
        area: 0.0,
        error: 0.0,
        base: Point2::UNDEF,
        bounds: Extent2d::new(),
    };

    // Use dimensionless UV area, translated near the loop to avoid cancellation.
    // Error estimates below are refinement differences, not certified bounds.
    let area_floor = 1e-8;
    let mut angle = 10.0;
    while angle >= 0.1 {
        sampler.angle = angle;
        sampler.area = 0.0;
        sampler.error = 0.0;
        let mut first = LoopOrientationSample::default();
        let mut previous = LoopOrientationSample::default();
        let mut have_point = false;
        sampler.bounds.init();
        for &eu in edgeuses {
            let edge = eu1::edge(b, eu)?.ok_or_else(|| null("edge use without edge"))?;
            let curve = edge_curve(b, edge)?;
            let interval = edge::interval(b, edge)?;
            if interval.length() <= 0.0 {
                return Ok(OrientType::Unknown);
            }

            // Never step over an entire knot span, even when its parameter width is tiny.
            // Tessellate independently within each span and retain knots.
            let (knots, _) = curve.knots_with_status(k, Some(&interval))?.into_result()?;
            let mut breaks = vec![interval.min()];
            for &knot in &knots {
                if breaks.last().is_some_and(|&last| knot > last) && knot < interval.max() {
                    breaks.push(knot);
                }
            }
            breaks.push(interval.max());
            let mut params = vec![interval.min()];
            for span_ends in breaks.windows(2) {
                let &[span_min, span_max] = span_ends else {
                    continue;
                };
                let span = Extent1d::from_min_max(span_min, span_max);
                // no chord-height tolerance, the pass's angle tolerance, at least 3 segments
                let span_params = curve.tessellate(k, &span, 0.0, angle, 3)?.parameters;
                for &param in &span_params {
                    if params.last().is_some_and(|&last| param > last) && param < span.max() {
                        params.push(param);
                    }
                }
                params.push(span.max());
                if params.len() > LOOP_ORIENTATION_SAMPLE_LIMIT {
                    return Ok(OrientType::Unknown);
                }
            }

            let same = eu1::orientation(b, eu)? == OrientType::Same;
            let n = params.len();
            let param_at = |i: usize| -> Result<f64> {
                params
                    .get(if same { i } else { n - 1 - i })
                    .copied()
                    .ok_or_else(|| null("ComputeLoopOrientation: sample out of range"))
            };
            let Some(mut start) =
                sampler.sample(curve, param_at(0)?, have_point.then_some(&previous))?
            else {
                return Ok(OrientType::Unknown);
            };
            if have_point {
                sampler.area += sampler.signed_area(&previous, &start);
            } else {
                first = start;
                sampler.base = start.uv;
                have_point = true;
            }
            for i in 1..n {
                let Some(end) = sampler.sample(curve, param_at(i)?, Some(&start))? else {
                    return Ok(OrientType::Unknown);
                };
                if !sampler.refine(curve, &start, &end, 0)? {
                    return Ok(OrientType::Unknown);
                }
                start = end;
            }
            previous = start;
        }
        if let Some(bounds) = uv_bounds.as_deref_mut() {
            *bounds = sampler.bounds;
        }
        // A closed 3D loop can wind around a periodic direction without closing in
        // continuous UV. Do not assign an orientation from that open path. Compare
        // endpoints, not just the shift: a walk from the low boundary to the high
        // boundary can span a period without any raw seam jump.
        if (sampler.closed_u
            && (previous.uv.x - first.uv.x).abs() > 0.5 * sampler.surf_domain.x_length())
            || (sampler.closed_v
                && (previous.uv.y - first.uv.y).abs() > 0.5 * sampler.surf_domain.y_length())
        {
            return Ok(OrientType::Unknown);
        }
        sampler.area += sampler.signed_area(&previous, &first);
        // Not upstream (fork source fork 54ca3e0, WR-561, opt-in): the floor is
        // relative to the loop's own bounding box (dimensionless too), so a small hole in
        // a large face is oriented as a large one is; a box with no width or no height
        // bounds no area.
        let floor = if k.fixes.loop_orientation_floor_from_loop_box {
            let box_u = sampler.bounds.x_length() / sampler.width;
            let box_v = sampler.bounds.y_length() / sampler.height;
            let bounds_area = box_u > EFF_ZERO && box_v > EFF_ZERO;
            if !bounds_area {
                angle /= 10.0;
                continue;
            }
            area_floor * box_u * box_v
        } else {
            area_floor
        };
        if sampler.area.abs() > 8.0 * sampler.error + floor {
            return Ok(if sampler.area > 0.0 {
                OrientType::Same
            } else {
                OrientType::Opposite
            });
        }
        angle /= 10.0;
    }
    // Unresolved area, failed projection, or exhausted refinement stays UNKNOWN.
    Ok(OrientType::Unknown)
}

// ---- UV trim curves -----------------------------------------------------------------------

/// The gap statistics of [`create_uv_trim_curves`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UvTrimCurveGaps {
    /// `rdMeanCurveSurfaceGap`: average new UV trim curve/surface distance.
    pub mean_curve_surface_gap: f64,
    /// `rdMaxCurveSurfaceGap`: largest UV trim curve/surface distance.
    pub max_curve_surface_gap: f64,
    /// `rdMean3DVertexGap`: average vertex/edge use start distance.
    pub mean_3d_vertex_gap: f64,
    /// `rdMax3DVertexGap`: largest vertex/edge use start distance.
    pub max_3d_vertex_gap: f64,
    /// `rdMeanPSCurveCurveGap`: average UV gap between an edge use and its clockwise
    /// neighbour.
    pub mean_ps_curve_curve_gap: f64,
    /// `rdMaxPSCurveCurveGap`: largest such UV gap.
    pub max_ps_curve_curve_gap: f64,
}

/// `CreateUVTrimCurves`: gives every edge use of the face a UV trim curve (the best of
/// `opt_uv_curves` with their orientations when given, else a new drop), closes small
/// UV gaps between consecutive edge uses away from poles, and with
/// `adjust_tolerances` raises vertex tolerances to twice their 3D gap. Returns the gap
/// statistics.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for missing
/// topology or surface; those of the curve searches, edits and evaluations;
/// [`SolidError::Unported`] (`GetUpwardLoopuses`, `GetBrep`; from `NormalizedEvaluate`
/// and `GetOrCreateUVTrimCurve`).
pub fn create_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    adjust_tolerances: bool,
    opt_uv_curves: Option<(&mut [Option<Curve>], &[OrientType])>,
) -> Result<UvTrimCurveGaps> {
    create_uv_trim_curves_notified(k, b, f, adjust_tolerances, opt_uv_curves, None)
}

/// [`create_uv_trim_curves`] whose UV trim curve edits notify through `env`: the end
/// point moves of `sm_FixUVTrimCurves` ([`face2::fix_uv_trim_curves_notified`]). The
/// other geometry edits on the way (`EditParameterization` of the curves
/// `DropIsoCurve` builds, before any edge use owns them) still notify nobody.
///
/// # Errors
///
/// Those of [`create_uv_trim_curves`]; those of the notifications.
pub fn create_uv_trim_curves_notified(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    adjust_tolerances: bool,
    opt_uv_curves: Option<(&mut [Option<Curve>], &[OrientType])>,
    env: Option<&mut NotifyEnv<'_>>,
) -> Result<UvTrimCurveGaps> {
    // queried before the surface owner is cleared, as upstream: the surface is still
    // face-owned, so `GetSingularities`' approximation tolerance
    // (`SmTol::GetApproxTol3d(this)`) is the face's
    let pre = PreRescue {
        uv_domain: face2::uv_domain(b, f)?,
        srf_poles: surface_of(b, f)?
            .get_singularities_of_face(k, face2::tolerance(b, f)?, false, false)?
            .singular,
        srf_uv_domain: surface_of(b, f)?.natural_uv_domain_with_nurb(k)?,
    };
    let surface = surface_of(b, f)?;
    let rescue = face2::RescueSurfaceOwner::new(surface);
    if let Some(s) = b.face_mut(f)?.surface.as_mut() {
        s.set_owner(None);
    }
    // (recorded on the B-rep: a nested `smsurf_GetSurfaceCache` of the face's surface
    // then finds no face, so it does not build the face's UV trim curves itself)
    b.begin_surface_owner_cleared(f);
    let result = create_uv_trim_curves_body(k, b, f, adjust_tolerances, opt_uv_curves, &pre, env);
    // SmRescueSurfaceOwner's destructor restores the owner on every return
    b.end_surface_owner_cleared(f);
    if let Some(s) = b.face_mut(f)?.surface.as_mut() {
        rescue.restore(s);
    }
    // the face's tolerance may have changed while the surface had no owner
    b.sync_face_surface_tolerance(f)?;
    result
}

/// The face and surface values `CreateUVTrimCurves` reads before it clears the surface
/// owner.
struct PreRescue {
    uv_domain: Extent2d,
    srf_poles: u32,
    srf_uv_domain: Extent2d,
}

#[allow(clippy::too_many_lines)] // one upstream routine
fn create_uv_trim_curves_body(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    adjust_tolerances: bool,
    mut opt_uv_curves: Option<(&mut [Option<Curve>], &[OrientType])>,
    pre: &PreRescue,
    mut env: Option<&mut NotifyEnv<'_>>,
) -> Result<UvTrimCurveGaps> {
    let mut mean_cs_gap = 0.0;
    let mut max_cs_gap = 0.0_f64;
    let mut mean_cc_gap = 0.0;
    let mut max_cc_gap = 0.0_f64;
    let mut mean_3d_gap = 0.0;
    let mut max_3d_gap = 0.0_f64;
    let mut total_dropped: u64 = 0;

    let PreRescue {
        uv_domain,
        srf_poles,
        srf_uv_domain,
    } = *pre;

    let loopuses = upward_loopuses(b, f)?;
    for lu in loopuses {
        let edgeuses = lu1::edgeuses(b, lu, None, None)?;
        for &eu in &edgeuses {
            total_dropped += 1;

            let Some(e) = eu1::edge(b, eu)? else {
                continue;
            };
            if eu1::uv_trim_curve(b, eu)?.0.is_some() {
                continue;
            }

            let mut found: Option<Curve> = None;
            if let Some((cands, orients)) = opt_uv_curves.as_mut() {
                let bb: &Brep = b;
                found = face2::find_best_uv_curve(k, bb, surface_of(bb, f)?, eu, cands, orients)?;

                if let Some(uv_curve) = found.take() {
                    if !adjust_tolerances || edge_curve(b, e)?.is_kind_of(CRV_ON_SURF_TYPE) {
                        // TRUE = delete existing UVTrimCurve (status ignored upstream)
                        ignore_status(eu1::set_uv_trim_curve(
                            b,
                            eu,
                            eu1::UvTrimCurveArg::New(Box::new(uv_curve)),
                            0.0,
                            true,
                            None,
                        ))?;
                        continue;
                    }

                    let r = eu2::check_and_set_uv_trim_curve(
                        k,
                        b,
                        eu,
                        uv_curve.clone(),
                        adjust_tolerances,
                        None,
                    )?;
                    if r.trim_replaced {
                        let d = r
                            .max_dist_to_surf
                            .ok_or_else(|| null("CreateUVTrimCurves: distance not set"))?;
                        mean_cs_gap += d;
                        max_cs_gap = smos_max(max_cs_gap, d);
                        continue;
                    }
                    found = Some(uv_curve);
                }
            }

            if found.is_some() {
                // TRUE = delete existing UVTrimCurve (status ignored upstream)
                ignore_status(eu1::set_uv_trim_curve(
                    b,
                    eu,
                    eu1::UvTrimCurveArg::Null,
                    0.0,
                    true,
                    None,
                ))?;
            }

            let (status, max_dist_to_surf) = get_or_create_uv_trim_curve(k, b, eu, true)?;
            status?;

            mean_cs_gap += smos_max(0.0, max_dist_to_surf); // This can come back -1.0.
            max_cs_gap = smos_max(max_cs_gap, max_dist_to_surf);
        }

        for &eu in &edgeuses {
            let cw = eu1::cw_edgeuse(b, eu)?
                .ok_or_else(|| null("CreateUVTrimCurves: edge use without CW edge use"))?;
            let vu = eu1::vertexuse(b, eu)?
                .ok_or_else(|| null("CreateUVTrimCurves: edge use without vertex use"))?;
            let v = vu1::vertex(b, vu)?
                .ok_or_else(|| null("CreateUVTrimCurves: vertex use without vertex"))?;

            let mut uv_pnt0 = normalized_evaluate(k, b, eu, 0.0, true, false)?.0;
            let mut uv_pnt1 = normalized_evaluate(k, b, cw, 1.0, true, false)?.0;

            let mut uv_gap_size = uv_pnt0.distance(uv_pnt1);

            let uv_scaled_zero = EFF_ZERO * (1.0 + uv_pnt0.max_dimension());
            let u_scaled_zero = EFF_ZERO * (1.0 + uv_pnt0.x.abs());
            let v_scaled_zero = EFF_ZERO * (1.0 + uv_pnt0.y.abs());
            let mut gap_at_pole = false;

            if uv_gap_size > uv_scaled_zero {
                let uv0 = Point2::new(uv_pnt0.x, uv_pnt0.y);
                let uv1 = Point2::new(uv_pnt1.x, uv_pnt1.y);

                let (u0, v0) =
                    srf_uv_domain.classify_point(uv0, u_scaled_zero, Some(v_scaled_zero));
                let (u1, v1) =
                    srf_uv_domain.classify_point(uv1, u_scaled_zero, Some(v_scaled_zero));
                let start = |t: ExtentPointType| {
                    matches!(t, ExtentPointType::Start | ExtentPointType::Both)
                };
                let end =
                    |t: ExtentPointType| matches!(t, ExtentPointType::End | ExtentPointType::Both);
                gap_at_pole = (srf_poles & SS_UMIN != 0 && start(u0) && start(u1))
                    || (srf_poles & SS_UMAX != 0 && end(u0) && end(u1))
                    || (srf_poles & SS_VMIN != 0 && start(v0) && start(v1))
                    || (srf_poles & SS_VMAX != 0 && end(v0) && end(v1));

                if !gap_at_pole {
                    let eu_edge = eu1::edge(b, eu)?
                        .ok_or_else(|| null("CreateUVTrimCurves: edge use without edge"))?;
                    let cw_edge = eu1::edge(b, cw)?
                        .ok_or_else(|| null("CreateUVTrimCurves: edge use without edge"))?;
                    // smos_4Max(a, b, c, d) = smos_Max(smos_Max(a, b), smos_Max(c, d))
                    let tol_3d = smos_max(
                        smos_max(brep_tolerance(b, f)?, b.vertex(v)?.tolerance),
                        smos_max(b.edge(eu_edge)?.tolerance, b.edge(cw_edge)?.tolerance),
                    );

                    // status ignored upstream
                    let fixed = ignored(face2::fix_uv_trim_curves_notified(
                        k,
                        b,
                        eu,
                        cw,
                        v,
                        tol_3d,
                        uv_gap_size,
                        &uv_domain,
                        env.as_deref_mut(),
                    ))?
                    .unwrap_or(false);

                    if fixed {
                        uv_pnt0 = normalized_evaluate(k, b, eu, 0.0, true, false)?.0;
                        uv_pnt1 = normalized_evaluate(k, b, cw, 1.0, true, false)?.0;
                        uv_gap_size = uv_pnt0.distance(uv_pnt1);
                    }
                }
            }

            let pnt_3d0 = normalized_evaluate(k, b, eu, 0.0, false, false)?.0;
            let gap_3d = pnt_3d0.distance(b.vertex(v)?.point);

            mean_cc_gap += if gap_at_pole { 0.0 } else { uv_gap_size };
            max_cc_gap = smos_max(max_cc_gap, uv_gap_size);
            mean_3d_gap += gap_3d;
            max_3d_gap = smos_max(max_3d_gap, gap_3d);

            if adjust_tolerances && b.vertex(v)?.tolerance < 2.0 * gap_3d {
                vertex::set_tolerance(b, v, 2.0 * gap_3d, true, true)?;
            }
        }
    }

    let n = total_dropped as f64;
    Ok(UvTrimCurveGaps {
        mean_curve_surface_gap: mean_cs_gap / n,
        max_curve_surface_gap: max_cs_gap,
        mean_3d_vertex_gap: mean_3d_gap / n,
        max_3d_vertex_gap: max_3d_gap,
        mean_ps_curve_curve_gap: mean_cc_gap / n,
        max_ps_curve_curve_gap: max_cc_gap,
    })
}

// ---- degeneracy ---------------------------------------------------------------------------

/// The geometry behind the edge handles of [`is_degenerate`]'s classifications.
struct EdgeGeometry<'a> {
    b: &'a Brep,
}

impl ClassGeometry<EntityRef> for EdgeGeometry<'_> {
    fn curve(&self, o: EntityRef) -> Option<&Curve> {
        match o {
            EntityRef::Edge(e) => self.b.edge(e).ok()?.curve.as_ref(),
            _ => None,
        }
    }
    fn surface(&self, o: EntityRef) -> Option<&Surface> {
        match o {
            EntityRef::Face(f) => self.b.face(f).ok()?.surface.as_ref(),
            _ => None,
        }
    }
    fn volume(&self, _o: EntityRef) -> Option<&Volume> {
        None
    }
    fn point(&self, o: EntityRef) -> Option<Point3> {
        match o {
            EntityRef::Vertex(v) => Some(self.b.vertex(v).ok()?.point),
            _ => None,
        }
    }
    // the handles are the face's B-rep entities: the topology members
    // (`SmEdge::GetTolerance`, ...) read them there
    fn topology_entity(&self, o: EntityRef) -> Option<(&Brep, EntityRef)> {
        Some((self.b, o))
    }
    fn topology_handle(&self, e: EntityRef) -> Option<EntityRef> {
        Some(e)
    }
}

/// The curve objects of [`is_degenerate`]'s coincidence solutions (upstream passes the
/// edge curves): their natural intervals.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// `IsClosedSurfaceWithVertexLoops`: whether the face is a whole closed surface whose
/// only boundaries are isolated vertex loops (closed in U and V, or closed in one
/// direction with poles at both ends of the other). Isolated vertices do not trim a
/// closed surface to a smaller patch. `opt_face_props` (upstream default `None`) gives
/// the cached closure when it describes this face and stage 2 has set it.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of the loop use queries, the
/// closure tests and the singularities.
pub fn is_closed_surface_with_vertex_loops<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    opt_face_props: Option<&FaceProps<O>>,
) -> Result<bool> {
    let Some(fu) = b.face(f)?.faceuse else {
        return Ok(false);
    };
    let Some(surface) = face2::surface(b, f)? else {
        return Ok(false);
    };
    let loopuses = &b.faceuse(fu)?.loopuses;
    if loopuses.is_empty() {
        return Ok(false);
    }
    for &lu in loopuses {
        if !lu1::is_vertex_loopuse(b, lu)? {
            return Ok(false);
        }
    }

    // Stage 2 sets closure before calling HasSeamProblem, while its stage is 2a.
    if let Some(props) = opt_face_props {
        if props.face == Some(f)
            && props.has_surface
            && props.prop_stage >= PropStage::Stage2A
            && props.closed_surf != Tri::Unsure
        {
            return Ok(props.closed_surf == Tri::True);
        }
    }

    let domain = surface.natural_uv_domain_with_nurb(k)?;
    let face_tol = face2::tolerance(b, f)?;
    let xsect_tol = xsect_tol_3d_from_zones(k, face_tol, face_tol);
    let closed_u = surface
        .is_closed(k, &domain, SurfParamType::U, Some(xsect_tol), false)?
        .0;
    let closed_v = surface
        .is_closed(k, &domain, SurfParamType::V, Some(xsect_tol), false)?
        .0;
    let poles = surface
        .get_singularities_of_face(k, face_tol, false, false)?
        .singular;

    Ok((closed_u && closed_v)
        || (closed_u && (poles & SS_VMIN) != 0 && (poles & SS_VMAX) != 0)
        || (closed_v && (poles & SS_UMIN) != 0 && (poles & SS_UMAX) != 0))
}

/// `IsDegenerate`: whether every edge of the face is degenerate or completely
/// coincident with other edges of the face (all its boundaries within tolerance of
/// other boundaries). Unless `skip_quick_check`, a point stepped `2 * tol` into the
/// face from the first non-degenerate edge that classifies inside the face proves it
/// non-degenerate. `opt_zone_tol_3d` replaces the edge (and step) tolerances. Upstream
/// defaults: `false`, `None`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// without curve; those of the degeneracy tests; [`SolidError::Unported`] (the
/// classification of a curve, `intersect.curve_class_p4`; `NormalizedEvaluate`;
/// `InsertTopologyIntersections`). A failing coincidence check returns `false`, as
/// upstream.
#[allow(clippy::too_many_lines)] // one upstream routine
pub fn is_degenerate(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    skip_quick_check: bool,
    opt_zone_tol_3d: Option<f64>,
) -> Result<bool> {
    // A missing natural boundary does not make the retained closed surface a sliver.
    if is_closed_surface_with_vertex_loops::<EntityRef>(k, b, f, None)? {
        return Ok(false);
    }

    // return value
    let mut face_degenerate = true;

    // locals
    let mut non_degen_edgeuse: Option<EdgeuseId> = None;

    // get Face->edges
    let edges = face_edges(b, f)?;
    let mut classify: Vec<Option<CurveClassification<EntityRef>>> = Vec::new();

    // for every edge - check for degeneracy
    for &e in &edges {
        let interval = b.edge(e)?.interval;
        let edge_tol = b.edge(e)?.tolerance;
        let zone_tol = opt_zone_tol_3d.unwrap_or(edge_tol);

        // when edge is degenerate - its classification is NULL
        if edge_curve(b, e)?.is_degenerate(k, zone_tol, Some(&interval))? {
            classify.push(None);
        } else {
            classify.push(Some(curve_classification(
                k,
                EntityRef::Edge(e),
                &interval,
                None,
                edge_tol,
            )?));

            // init pNonDegenEdgeuse with 1st NonDegen Edge->Edgeuse connected to this face
            if non_degen_edgeuse.is_none() {
                non_degen_edgeuse = edge::upward_edgeuse_of_face(b, e, Some(f))?;
            }
        }
    }

    // no work - all Face->edges are degen
    let Some(non_degen_eu) = non_degen_edgeuse else {
        return Ok(true);
    };

    // 1st quick check - Step into face by 2*tol from 1st NonDegenEdge NearMidPt
    if !skip_quick_check {
        let face_zone_tol = face2::tolerance(b, f)?;
        let non_degen_edge = eu1::edge(b, non_degen_eu)?
            .ok_or_else(|| null("IsDegenerate: edge use without edge"))?;
        let edge_zone_tol = b.edge(non_degen_edge)?.tolerance;

        // evaluate pNonDegenEdgeuse OffsetIntoFace pt near EdgeMidPoint
        if let Some((uv_pt, uv_deriv)) =
            ignored(normalized_evaluate(k, b, non_degen_eu, 0.51, true, true))?
        {
            let uv_deriv = uv_deriv.unwrap_or(Vec3::UNDEF);
            // Rotate the UV derivative 90 degrees CCW: points 'inside' face.
            let mut uv_deriv = Vec3::new(-uv_deriv.y, uv_deriv.x, uv_deriv.z);

            // We have to get the magnitude of this vector in 3d.
            let uv_2d = Point2::new(uv_pt.x, uv_pt.y);
            let eval = ignored(surface_of(b, f)?.evaluate_1st_derivatives(k, uv_2d, false, false))?;
            if let Some((_srf_pt, su, sv)) = eval {
                // get 3d vector from edge pointing into face - sized to Edge 1st Deriv
                let vec_3d = su * uv_deriv.x + sv * uv_deriv.y;
                let len_3d = vec_3d.length();

                // when 3d vector length is larger than FaceZoneTol3d - resize it to
                // 2*FaceZoneTol3d and classify
                if len_3d > face_zone_tol {
                    uv_deriv *= match opt_zone_tol_3d {
                        Some(t) => 2.0 * t / len_3d,
                        None => 2.0 * face_zone_tol / len_3d,
                    };
                    let uv_test_pt = Point2::new(uv_pt.x + uv_deriv.x, uv_pt.y + uv_deriv.y);
                    let mut pc = PointClassification::<EntityRef>::new(face_zone_tol);
                    let classified = ignored(qf2::point_classify(
                        k,
                        b,
                        f,
                        uv_test_pt,
                        &mut pc,
                        qf2::PointClassifyOptions {
                            src_zone_tol_3d: edge_zone_tol,
                            do_boundary_intersections: false,
                            use_uv_classification: true,
                            ..qf2::PointClassifyOptions::default()
                        },
                    ))?;

                    // if the point classify is in the face - the face is not degenerate
                    if classified.is_some() && pc.point_class() == PointClassType::Face {
                        return Ok(false);
                    }
                }
            }
        }
    }

    // arrive here: Quick check was skipped or failed to identify the face as
    // NonDegenerate - find and mark every nonDegenerate edge/edge coincidence
    let mut coincidences = vec![false; edges.len()];

    // for every FaceEdge - check for coincidence with some other FaceEdge
    for ii in 0..edges.len() {
        let e1 = edges
            .get(ii)
            .copied()
            .ok_or_else(|| null("IsDegenerate: edge index"))?;
        let zone_tol1 = opt_zone_tol_3d.unwrap_or(b.edge(e1)?.tolerance);

        // skip degenerate edges
        if classify.get(ii).and_then(Option::as_ref).is_none() {
            continue;
        }

        // compare Edge1 with all other nonDegenerate edges for coincidence
        for jj in ii + 1..edges.len() {
            let e2 = edges
                .get(jj)
                .copied()
                .ok_or_else(|| null("IsDegenerate: edge index"))?;
            let zone_tol2 = opt_zone_tol_3d.unwrap_or(b.edge(e2)?.tolerance);
            let xsect_tol = xsect_tol_3d_from_zones(k, zone_tol1, zone_tol2);

            // skip degenerate edges or if they are both known to be coincident
            let c1 = coincidences.get(ii).copied().unwrap_or(false);
            let c2 = coincidences.get(jj).copied().unwrap_or(false);
            if classify.get(jj).and_then(Option::as_ref).is_none() || (c1 && c2) {
                continue;
            }

            // check for coincidence (the pre SM_TOLERANT_XSECT version, no snapping)
            let ivl1 = b.edge(e1)?.interval;
            let ivl2 = b.edge(e2)?.interval;
            let curve1 = edge_curve(b, e1)?;
            let curve2 = edge_curve(b, e2)?;
            let found = curve1.global_coincidence_checker(
                k,
                &ivl1,
                curve2,
                &ivl2,
                xsect_tol,
                false,
                [
                    Some(CurveHandle(curve1.natural_interval())),
                    Some(CurveHandle(curve2.natural_interval())),
                ],
            );

            // failure case - quit and return False as the do the least harm answer
            let found = match found {
                Ok(c) => c,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(false),
            };

            // Solution objects are SmCurve pointers. For Compatibility with upcoming
            // calls switch solution objects from SmCurve to SmEdge pointers
            let solutions: Vec<Solution<EntityRef>> = found
                .solutions
                .iter()
                .map(|sol| Solution {
                    solution_type: sol.solution_type,
                    num_variables: sol.num_variables,
                    start: sol.start,
                    end: sol.end,
                    num_objects: sol.num_objects,
                    objects: {
                        let mut o = [None; MAX_TREES];
                        if let Some(x) = o.get_mut(0) {
                            *x = Some(EntityRef::Edge(e1));
                        }
                        if let Some(x) = o.get_mut(1) {
                            *x = Some(EntityRef::Edge(e2));
                        }
                        o
                    },
                    nodes: sol.nodes,
                })
                .collect();

            if !solutions.is_empty() {
                let geometry = EdgeGeometry { b };
                // insert any GlobalCoincidenceChecker() solutions into both curve
                // classifications
                let (left, right) = classify.split_at_mut(jj);
                let cc1 = left
                    .get_mut(ii)
                    .and_then(Option::as_mut)
                    .ok_or_else(|| null("IsDegenerate: classification"))?;
                let cc2 = right
                    .first_mut()
                    .and_then(Option::as_mut)
                    .ok_or_else(|| null("IsDegenerate: classification"))?;
                // statuses ignored upstream
                ignore_status(cc1.insert_topology_intersections(k, &geometry, &solutions, 0))?;
                ignore_status(cc2.insert_topology_intersections(k, &geometry, &solutions, 1))?;

                // done with this edge once it is completely coincident
                if cc1.is_coincident(k, &geometry, &ivl1, zone_tol1)? {
                    if let Some(c) = coincidences.get_mut(ii) {
                        *c = true;
                    }
                }
                if cc2.is_coincident(k, &geometry, &ivl2, zone_tol2)? {
                    if let Some(c) = coincidences.get_mut(jj) {
                        *c = true;
                    }
                }
            }
        }

        // when CrvClassification for pEdge1 is completely coincident
        let ivl1 = b.edge(e1)?.interval;
        let cc1 = classify
            .get(ii)
            .and_then(Option::as_ref)
            .ok_or_else(|| null("IsDegenerate: classification"))?;
        if !coincidences.get(ii).copied().unwrap_or(false)
            && cc1.is_coincident(k, &EdgeGeometry { b }, &ivl1, zone_tol1)?
        {
            if let Some(c) = coincidences.get_mut(ii) {
                *c = true;
            }
        }

        // If pEdge1 is not degenerate and not completely coincident the face is not
        // degenerate
        if !coincidences.get(ii).copied().unwrap_or(false) {
            face_degenerate = false;
            break;
        }
    }

    // all done
    Ok(face_degenerate)
}

// ---- face/face intersection ---------------------------------------------------------------

/// The objects [`face_intersect`]'s classifications refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntersectObject {
    /// The surface/surface intersection curve at this index
    /// ([`FaceIntersection::cc_curves_3d`]).
    Curve(usize),
    /// The UV curve of intersection curve `.0` on this face (`.1 == false`) or the
    /// other face.
    UvCurve(usize, bool),
    /// The surface of a face. As for [`IntersectObject::UvCurve`], `.1` is the side:
    /// `false` for an object of this face's B-rep, `true` for one of the other face's
    /// (always `false` when both faces are in one B-rep).
    Surface(FaceId, bool),
    /// The 3D curve of an edge (`.1`: the side).
    EdgeCurve(EdgeId, bool),
    /// The UV trim curve stored on an edge use (`.1`: the side).
    Trim(EdgeuseId, bool),
    /// A topology entity (`.1`: the side).
    Topology(EntityRef, bool),
    /// A surface owned by no face, held by the adapter at this index: the
    /// `pOptIntersectionSurface` of `SmBrep::MergeCurvesOnSurface`
    /// ([`crate::topology::brep::p10::merge_curves_on_surface_through_surface`]).
    /// [`IntersectObjects`] resolves none; the adapter that holds such surfaces
    /// resolves them.
    FreeSurface(usize),
}

impl ClassObject for IntersectObject {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Curve(_) | Self::UvCurve(..) | Self::EdgeCurve(..) | Self::Trim(..) => {
                ClassObjectKind::Curve
            }
            Self::Surface(..) | Self::FreeSurface(_) => ClassObjectKind::Surface,
            Self::Topology(e, _) => e.class_kind(),
        }
    }
}

/// The options of [`face_intersect`]: which optional outputs are wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // upstream's optional output pointers
pub struct FaceIntersectOptions {
    /// `pOptThisUVCurves`.
    pub want_this_uv_curves: bool,
    /// `pOptOtherUVCurves`.
    pub want_other_uv_curves: bool,
    /// `pOpt3DPoints`.
    pub want_points: bool,
    /// `pOptThisCrvClasses`.
    pub want_this_classes: bool,
    /// `pOptOtherCrvClasses`.
    pub want_other_classes: bool,
    /// `pOptCC3DCrvs`, `pOptThisCCUVCrvs`, `pOptOtherCCUVCrvs`. The curves are returned
    /// also when either classification array is requested (the classifications index
    /// them).
    pub want_cc_curves: bool,
    /// `pbOptCoincidenceFlag`: whether the surfaces are known to be coincident (`None`:
    /// run the coincidence check).
    pub coincidence: Option<bool>,
}

/// The outputs of [`face_intersect`].
#[derive(Debug, Clone, Default)]
pub struct FaceIntersection {
    /// `r3DCurves`: the intersection curves lying in both faces.
    pub curves_3d: Vec<Curve>,
    /// `pOptThisUVCurves`: their UV curves on this face (when asked).
    pub this_uv_curves: Option<Vec<Option<Curve>>>,
    /// `pOptOtherUVCurves`: their UV curves on the other face (when asked).
    pub other_uv_curves: Option<Vec<Option<Curve>>>,
    /// `pOpt3DPoints`: single-point intersections (when asked).
    pub points_3d: Option<Vec<Point3>>,
    /// `pOptThisCrvClasses`: this face's classification of every surface/surface curve
    /// that yielded curves (when asked).
    pub this_classes: Option<Vec<CurveClassification<IntersectObject>>>,
    /// `pOptOtherCrvClasses`: the other face's classifications (when asked).
    pub other_classes: Option<Vec<CurveClassification<IntersectObject>>>,
    /// `pOptCC3DCrvs`: the surface/surface intersection curves (when asked).
    pub cc_curves_3d: Option<Vec<Curve>>,
    /// `pOptThisCCUVCrvs`: their UV curves on this surface (when asked).
    pub cc_this_uv_curves: Option<Vec<Option<Curve>>>,
    /// `pOptOtherCCUVCrvs`: their UV curves on the other surface (when asked).
    pub cc_other_uv_curves: Option<Vec<Option<Curve>>>,
}

/// The classification objects of [`face_intersect`] (also used by
/// `topology::brep::p7`'s `MergeSelfIntersectionCurves`).
pub(crate) struct IntersectObjects<'a> {
    /// The B-rep of this face (objects of side `false`).
    pub(crate) b: &'a Brep,
    /// The B-rep of the other face (objects of side `true`; `b` for one B-rep).
    pub(crate) other: &'a Brep,
    /// The side of the objects the classification callbacks name.
    pub(crate) side: bool,
    /// The curves behind [`IntersectObject::Curve`].
    pub(crate) curves_3d: &'a [Curve],
    /// The curves behind [`IntersectObject::UvCurve`] `(i, false)`.
    pub(crate) uv1: &'a [Option<Curve>],
    /// The curves behind [`IntersectObject::UvCurve`] `(i, true)`.
    pub(crate) uv2: &'a [Option<Curve>],
}

impl qf2::CurveOnClassifyObjects<IntersectObject> for IntersectObjects<'_> {
    fn face(&self, f: FaceId) -> IntersectObject {
        IntersectObject::Topology(EntityRef::Face(f), self.side)
    }
    fn vertex(&self, v: VertexId) -> IntersectObject {
        IntersectObject::Topology(EntityRef::Vertex(v), self.side)
    }
    fn edge(&self, e: EdgeId) -> IntersectObject {
        IntersectObject::Topology(EntityRef::Edge(e), self.side)
    }
    fn vertexuse(&self, vu: VertexuseId) -> IntersectObject {
        IntersectObject::Topology(EntityRef::Vertexuse(vu), self.side)
    }
    fn edgeuse(&self, eu: EdgeuseId) -> IntersectObject {
        IntersectObject::Topology(EntityRef::Edgeuse(eu), self.side)
    }
    fn edge_curve(&self, e: EdgeId) -> IntersectObject {
        IntersectObject::EdgeCurve(e, self.side)
    }
    fn trim_curve(&self, eu: EdgeuseId) -> IntersectObject {
        IntersectObject::Trim(eu, self.side)
    }
    fn face_surface(&self, f: FaceId) -> Option<IntersectObject> {
        Some(IntersectObject::Surface(f, self.side))
    }
    fn loop_id(&self, tgt_loop: IntersectObject) -> Option<LoopId> {
        match tgt_loop {
            IntersectObject::Topology(EntityRef::Loop(l), _) => Some(l),
            _ => None,
        }
    }
}

impl<'a> IntersectObjects<'a> {
    /// The B-rep of the objects of `side`.
    fn brep(&self, side: bool) -> &'a Brep {
        if side {
            self.other
        } else {
            self.b
        }
    }

    /// [`ClassGeometry::curve`] for the borrowed lifetime (a mutable adapter over the
    /// same objects answers from a view of its own fields).
    pub(crate) fn object_curve(&self, o: IntersectObject) -> Option<&'a Curve> {
        match o {
            IntersectObject::Curve(i) => self.curves_3d.get(i),
            IntersectObject::UvCurve(i, false) => self.uv1.get(i)?.as_ref(),
            IntersectObject::UvCurve(i, true) => self.uv2.get(i)?.as_ref(),
            IntersectObject::EdgeCurve(e, side) => self.brep(side).edge(e).ok()?.curve.as_ref(),
            IntersectObject::Trim(eu, side) => eu1::uv_trim_curve(self.brep(side), eu).ok()?.0,
            IntersectObject::Surface(..)
            | IntersectObject::Topology(..)
            | IntersectObject::FreeSurface(_) => None,
        }
    }

    /// [`ClassGeometry::surface`] for the borrowed lifetime.
    pub(crate) fn object_surface(&self, o: IntersectObject) -> Option<&'a Surface> {
        match o {
            IntersectObject::Surface(f, side) => self.brep(side).face(f).ok()?.surface.as_ref(),
            _ => None,
        }
    }

    /// [`ClassGeometry::topology_entity`] for the borrowed lifetime.
    pub(crate) fn object_entity(&self, o: IntersectObject) -> Option<(&'a Brep, EntityRef)> {
        match o {
            IntersectObject::Topology(e, side) => Some((self.brep(side), e)),
            _ => None,
        }
    }

    fn distinct_breps(&self) -> [&'a Brep; 2] {
        [self.b, self.other]
    }

    /// [`ClassGeometry::owner_entity`] for the borrowed lifetime.
    pub(crate) fn object_owner_entity(&self, owner: ObjectRef) -> Option<(&'a Brep, EntityRef)> {
        let same = core::ptr::eq(self.b, self.other);
        let breps = self.distinct_breps();
        let slice = if same { &breps[..1] } else { &breps[..] };
        if owner.type_code == edge::TYPE {
            for &b in slice {
                for e in b.edges().ids() {
                    let entity = EntityRef::Edge(e);
                    if b.geometry_owner_key(entity) == Some(owner)
                        || b.notify_serial().is_some_and(|s| {
                            crate::topology::brep::p3::topology_ref(s, entity) == owner
                        })
                    {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for &b in slice {
                for e in b.edges().ids() {
                    let actual = b.edge(e).ok().and_then(|r| r.curve.as_ref()?.owner());
                    if actual == Some(owner)
                        && crate::topology::brep::p1::default_edge_owner(b, e) == owner
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, EntityRef::Edge(e)));
                    }
                }
            }
            return fallback;
        }
        if owner.type_code == face2::TYPE {
            for &b in slice {
                for f in b.faces().ids() {
                    let entity = EntityRef::Face(f);
                    if b.geometry_owner_key(entity) == Some(owner)
                        || b.notify_serial().is_some_and(|s| {
                            crate::topology::brep::p3::topology_ref(s, entity) == owner
                        })
                    {
                        return Some((b, entity));
                    }
                }
            }
            let mut fallback = None;
            for &b in slice {
                for f in b.faces().ids() {
                    let actual = b.face(f).ok().and_then(|r| r.surface.as_ref()?.owner());
                    if actual == Some(owner)
                        && crate::topology::brep::p1::default_face_owner(b, f) == owner
                    {
                        if fallback.is_some() {
                            return None;
                        }
                        fallback = Some((b, EntityRef::Face(f)));
                    }
                }
            }
            return fallback;
        }
        None
    }

    /// [`ClassGeometry::owner_of`] for the borrowed lifetime.
    pub(crate) fn object_owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        if owner.type_code != eu1::TYPE {
            return None;
        }
        let same = core::ptr::eq(self.b, self.other);
        let breps = self.distinct_breps();
        let slice = if same { &breps[..1] } else { &breps[..] };
        let edge_owner_in = |b: &Brep, e: EdgeId| -> ObjectRef {
            b.geometry_owner_key(EntityRef::Edge(e))
                .or_else(|| b.edge(e).ok()?.curve.as_ref()?.owner())
                .unwrap_or_else(|| crate::topology::brep::p1::default_edge_owner(b, e))
        };
        for &b in slice {
            for eu in b.edgeuses().ids() {
                let entity = EntityRef::Edgeuse(eu);
                if b.notify_serial()
                    .is_some_and(|s| crate::topology::brep::p3::topology_ref(s, entity) == owner)
                {
                    let e = eu1::edge(b, eu).ok()??;
                    return Some(edge_owner_in(b, e));
                }
            }
        }
        let mut fallback = None;
        for &b in slice {
            for eu in b.edgeuses().ids() {
                let actual = b
                    .edgeuse(eu)
                    .ok()
                    .and_then(|r| r.uv_trim_curve.as_ref()?.owner());
                if actual == Some(owner) {
                    let e = eu1::edge(b, eu).ok()??;
                    if fallback.is_some() {
                        return None;
                    }
                    fallback = Some(edge_owner_in(b, e));
                }
            }
        }
        fallback
    }
}

impl ClassGeometry<IntersectObject> for IntersectObjects<'_> {
    fn classification_vertex_point(&self, o: IntersectObject) -> Option<Point3> {
        match o {
            IntersectObject::Topology(EntityRef::Vertex(v), side) => {
                self.brep(side).vertex(v).ok().map(|v| v.point)
            }
            _ => None,
        }
    }
    fn topology_zone_tol_3d(&self, o: IntersectObject) -> Option<f64> {
        match o {
            IntersectObject::Topology(EntityRef::Vertex(v), side) => {
                self.brep(side).vertex(v).ok().map(|v| v.tolerance)
            }
            IntersectObject::Topology(EntityRef::Edge(e), side) => {
                self.brep(side).edge(e).ok().map(|e| e.tolerance)
            }
            IntersectObject::Topology(EntityRef::Face(f), side) => {
                self.brep(side).face(f).ok().map(|f| f.tolerance)
            }
            _ => None,
        }
    }
    fn curve(&self, o: IntersectObject) -> Option<&Curve> {
        self.object_curve(o)
    }
    fn surface(&self, o: IntersectObject) -> Option<&Surface> {
        self.object_surface(o)
    }
    fn volume(&self, _o: IntersectObject) -> Option<&Volume> {
        None
    }
    fn point(&self, _o: IntersectObject) -> Option<Point3> {
        None
    }
    // the topology handles name entities of the B-rep of their side, so the face
    // queries of the interval classification (`SmFace::PointClassify`,
    // `FindVertexSector`, ...) reach them as upstream's pointers do
    fn topology_entity(&self, o: IntersectObject) -> Option<(&Brep, EntityRef)> {
        self.object_entity(o)
    }
    fn topology_handle(&self, e: EntityRef) -> Option<IntersectObject> {
        Some(IntersectObject::Topology(e, self.side))
    }
    fn topology_handle_in(&self, brep: &Brep, e: EntityRef) -> Option<IntersectObject> {
        // side `true` only for the other face's B-rep when it is a different one
        let side = !core::ptr::eq(brep, self.b);
        (!side || core::ptr::eq(brep, self.other)).then_some(IntersectObject::Topology(e, side))
    }
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        if core::ptr::eq(brep, self.b) {
            Some(BrepId::from_index(0))
        } else if core::ptr::eq(brep, self.other) {
            Some(BrepId::from_index(1))
        } else {
            None
        }
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        self.object_owner_entity(owner)
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        self.object_owner_of(owner)
    }
}

/// [`IntersectObjects`] over the faces' B-reps lent mutably, for the curve
/// classifications' `ExtractGeometry`: its boundary point drops make the faces'
/// trimmed surface caches on the B-reps and keep them (`smsurf_GetSurfaceCache`, which
/// creates the faces' UV trim curves), as upstream does through its face pointers.
struct IntersectObjectsMut<'a> {
    /// The B-rep of this face.
    b: &'a mut Brep,
    /// The B-rep of the other face when it is a different one.
    other: Option<&'a mut Brep>,
    side: bool,
    curves_3d: &'a [Curve],
    uv1: &'a [Option<Curve>],
    uv2: &'a [Option<Curve>],
    /// Set when an edit lent a B-rep ([`ClassGeometryMut::topology_entity_mut`]).
    edited: bool,
}

impl IntersectObjectsMut<'_> {
    /// The shared view of the same objects.
    fn view(&self) -> IntersectObjects<'_> {
        let b: &Brep = self.b;
        IntersectObjects {
            b,
            other: self.other.as_deref().unwrap_or(b),
            side: self.side,
            curves_3d: self.curves_3d,
            uv1: self.uv1,
            uv2: self.uv2,
        }
    }
}

impl ClassGeometry<IntersectObject> for IntersectObjectsMut<'_> {
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
        self.view().object_surface(o)
    }
    fn volume(&self, _o: IntersectObject) -> Option<&Volume> {
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
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        self.view().brep_id(brep)
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        self.view().object_owner_entity(owner)
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        self.view().object_owner_of(owner)
    }
}

impl ClassGeometryMut<IntersectObject> for IntersectObjectsMut<'_> {
    // the classification curves are the intersection's, which the drops do not edit
    fn curve_mut(&mut self, _o: IntersectObject) -> Option<&mut Curve> {
        None
    }
    fn topology_entity_mut(&mut self, o: IntersectObject) -> Option<(&mut Brep, EntityRef)> {
        self.edited = true;
        self.topology_entity_for_query(o)
    }
    fn topology_entity_for_query(&mut self, o: IntersectObject) -> Option<(&mut Brep, EntityRef)> {
        let IntersectObject::Topology(e, side) = o else {
            return None;
        };
        // side `true` is the other face's B-rep (this one when they are the same)
        let brep = match (side, self.other.as_deref_mut()) {
            (true, Some(other)) => other,
            _ => &mut *self.b,
        };
        Some((brep, e))
    }
}

impl qf2::CurveOnClassifyObjects<IntersectObject> for IntersectObjectsMut<'_> {
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

/// The classified face's B-rep: the other face's for side `true` when it is a
/// different one, else this face's.
impl qf2::FaceBrep for IntersectObjectsMut<'_> {
    fn brep(&self) -> &Brep {
        match (self.side, self.other.as_deref()) {
            (true, Some(other)) => other,
            _ => self.b,
        }
    }
}

impl qf2::CurveOnClassifyBrep<IntersectObject> for IntersectObjectsMut<'_> {
    fn brep_mut(&mut self) -> &mut Brep {
        match (self.side, self.other.as_deref_mut()) {
            (true, Some(other)) => other,
            _ => &mut *self.b,
        }
    }
}

/// `FaceIntersect`: the curves (and, when asked, points) where face `f` meets
/// `other`. Coincident surfaces (by `options.coincidence`, else
/// `SmSurface::CoincidenceCheck` within `approx_tol_3d`) give copies of both faces'
/// non-degenerate edge curves trimmed to their edges; otherwise the surfaces are
/// intersected (`GlobalSurfaceIntersect` over the face domains, angle tolerance
/// `crv_ang_tol_deg`). Each intersection curve is classified on both faces and the
/// pieces inside both are kept; touching points inside or on both faces are points.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a face
/// without surface; those of the intersection, copies and classifications;
/// [`SolidError::Unported`] (the curve classification constructor).
pub fn face_intersect(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    other: FaceId,
    approx_tol_3d: f64,
    crv_ang_tol_deg: f64,
    options: &FaceIntersectOptions,
) -> Result<FaceIntersection> {
    face_intersect_breps(
        k,
        FaceBreps::One(b),
        f,
        other,
        approx_tol_3d,
        crv_ang_tol_deg,
        options,
    )
}

/// The B-reps of the two faces of [`face_intersect_breps`]: one B-rep holding both,
/// or this face's and the other face's. They are lent mutably because the surface
/// intersection makes the faces' trimmed caches on them (`smsurf_GetSurfaceCache`,
/// which creates the faces' UV trim curves).
#[derive(Debug)]
pub enum FaceBreps<'a> {
    /// Both faces are of this B-rep.
    One(&'a mut Brep),
    /// This face's B-rep and the other face's.
    Two(&'a mut Brep, &'a mut Brep),
}

/// [`face_intersect`] of face `this.1` of B-rep `this.0` and face `other.1` of B-rep
/// `other.0` (upstream's faces reach their B-reps through their pointers). The
/// classification objects of the other face are of side `true` when the B-reps
/// differ (see [`IntersectObject`]); with one B-rep this is [`face_intersect`].
///
/// # Errors
///
/// Those of [`face_intersect`].
pub fn face_intersect_breps(
    k: &Kernel,
    breps: FaceBreps<'_>,
    f: FaceId,
    other: FaceId,
    approx_tol_3d: f64,
    crv_ang_tol_deg: f64,
    options: &FaceIntersectOptions,
) -> Result<FaceIntersection> {
    face_intersect_breps_cached(
        k,
        breps,
        f,
        other,
        approx_tol_3d,
        crv_ang_tol_deg,
        options,
        [None, None],
    )
}

/// [`face_intersect_breps`] keeping the trimmed surface caches the surface
/// intersection makes on the lent B-reps in `face_caches` (this face's B-rep's, then
/// the other face's; the second is unused with [`FaceBreps::One`]) for the next call
/// on the same unchanged B-reps, as upstream keeps each face's cache on its surface.
/// The caches must have been made on the B-rep they are given with, and the B-rep
/// must not have changed since.
///
/// # Errors
///
/// Those of [`face_intersect`].
#[allow(clippy::too_many_lines)] // one upstream routine
#[allow(clippy::too_many_arguments)]
pub fn face_intersect_breps_cached(
    k: &Kernel,
    breps: FaceBreps<'_>,
    f: FaceId,
    other: FaceId,
    approx_tol_3d: f64,
    crv_ang_tol_deg: f64,
    options: &FaceIntersectOptions,
    mut face_caches: [Option<&mut FaceCaches>; 2],
) -> Result<FaceIntersection> {
    let (this_brep, mut other_brep) = match breps {
        FaceBreps::One(b) => (b, None),
        FaceBreps::Two(b, ob) => (b, Some(ob)),
    };
    // the other face's objects name its own B-rep when it is not this one
    let other_side = other_brep.is_some();
    // read shared until the surface intersection lends the B-reps
    let b: &Brep = this_brep;
    let ob: &Brep = other_brep.as_deref().unwrap_or(b);
    let mut out = FaceIntersection {
        this_uv_curves: options.want_this_uv_curves.then(Vec::new),
        other_uv_curves: options.want_other_uv_curves.then(Vec::new),
        points_3d: options.want_points.then(Vec::new),
        this_classes: options.want_this_classes.then(Vec::new),
        other_classes: options.want_other_classes.then(Vec::new),
        ..FaceIntersection::default()
    };

    let use_surface_edges = [true, true];
    let ang_tol_rad = crv_ang_tol_deg * PI / 180.0;
    let other_zone_tol = face2::tolerance(ob, other)?;
    let this_zone_tol = face2::tolerance(b, f)?;
    let zone_tol = smos_max(this_zone_tol, other_zone_tol);

    let mut curves_3d: Vec<Curve> = Vec::new();
    let mut uv_curves1: Vec<Option<Curve>> = Vec::new();
    let mut uv_curves2: Vec<Option<Curve>> = Vec::new();
    let mut curve_types: Vec<TsectCurveType> = Vec::new();

    let (are_coincident, coinc_dist) = if let Some(c) = options.coincidence {
        (c, 0.0)
    } else {
        // run the Coincidence checker
        coincidence_check(k, surface_of(b, f)?, surface_of(ob, other)?, approx_tol_3d)?
    };

    let mut have_curves = false;
    if are_coincident && coinc_dist < approx_tol_3d {
        have_curves = true;
        'faces: for (fb, face) in [(b, f), (ob, other)] {
            for e in face_edges(fb, face)? {
                let edge_crv = edge_curve(fb, e)?;
                if edge_crv.is_degenerate(k, EFF_ZERO, None)? {
                    continue;
                }
                let mut new_crv = match edge_crv.copy(k) {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {
                        have_curves = false;
                        break 'faces;
                    }
                };
                let mut interval = fb.edge(e)?.interval;
                // may snap sIvl by tol to existing knots (status ignored)
                ignored(new_crv.trim(k, &mut interval, TrimMode::Notify))?;
                curves_3d.push(new_crv);
                uv_curves1.push(None);
                uv_curves2.push(None);
                curve_types.push(TsectCurveType::RegionBoundary);
            }
        }
    }

    if !have_curves {
        // the surfaces reach their faces' caches through the B-reps
        // (smsurf_GetSurfaceCache); exact evaluation copies, as the caches are made
        // on the B-reps meanwhile
        let mut surface = surface_of(b, f)?.clone_for_evaluation()?;
        let mut other_copy = surface_of(ob, other)?.clone_for_evaluation()?;
        // the copies reach their faces, whose tolerances (`GetFace()->GetTolerance()`,
        // read by the plane/plane intersector's boxes) were read above
        surface.set_owner_zone_tol_3d(Some(this_zone_tol));
        other_copy.set_owner_zone_tol_3d(Some(other_zone_tol));
        // a face intersected with itself intersects one surface object with itself
        // (the intersectors compare the two surfaces by identity)
        let other_surface = if !other_side && other == f {
            &surface
        } else {
            &other_copy
        };
        let (domain, other_domain) = (face2::uv_domain(b, f)?, face2::uv_domain(ob, other)?);
        let r = {
            let [this_caches, other_caches] = &mut face_caches;
            let taken = |slot: &mut Option<&mut FaceCaches>| {
                slot.as_deref_mut().map(core::mem::take).unwrap_or_default()
            };
            let mut services = vec![BrepFaceServices::with_caches(this_brep, taken(this_caches))];
            if let Some(ob) = other_brep.as_deref_mut() {
                services.push(BrepFaceServices::with_caches(ob, taken(other_caches)));
            }
            let other_index = usize::from(other_side);
            let mut sided = SidedFaceServices(services);
            let r = surface.global_surface_intersect_with_context(
                k,
                [
                    Some(SideObject(0, CacheObject::Surface(f))),
                    Some(SideObject(other_index, CacheObject::Surface(other))),
                ],
                &mut sided,
                &domain,
                other_surface,
                &other_domain,
                use_surface_edges,
                Some(approx_tol_3d),
                Some(ang_tol_rad),
            );
            // hand the caches back for the next call
            for (service, slot) in sided.0.into_iter().zip(face_caches.iter_mut()) {
                if let Some(slot) = slot.as_deref_mut() {
                    *slot = service.into_caches();
                }
            }
            r?
        };
        curves_3d = r.curves_3d;
        uv_curves1 = r.surface1_uv_curves;
        uv_curves2 = r.surface2_uv_curves;
        curve_types = r.curve_types;
    }
    for (ii, curve) in curves_3d.iter().enumerate() {
        let curve_type = curve_types
            .get(ii)
            .copied()
            .ok_or_else(|| null("FaceIntersect: fewer curve types than curves"))?;
        if curve_type == TsectCurveType::Touching {
            let Some(points) = out.points_3d.as_mut() else {
                continue;
            };
            // status ignored upstream
            let point = ignored(curve.evaluate_point(k, 0.5))?.unwrap_or(Vec3::UNDEF);
            // `Point3DClassify` on both faces, in place on their own B-reps
            let pct1 = point_3d_classify(k, this_brep, f, point, this_zone_tol)?;
            let ob = other_brep.as_deref_mut().unwrap_or(&mut *this_brep);
            let pct2 = point_3d_classify(k, ob, other, point, other_zone_tol)?;
            let on = |t: PointClassType| {
                matches!(
                    t,
                    PointClassType::Face | PointClassType::Edge | PointClassType::Vertex
                )
            };
            if on(pct1) && on(pct2) {
                points.push(point);
            }
            continue;
        }

        let uv1 = uv_curves1
            .get(ii)
            .and_then(Option::as_ref)
            .map(|_| IntersectObject::UvCurve(ii, false));
        let uv2 = uv_curves2
            .get(ii)
            .and_then(Option::as_ref)
            .map(|_| IntersectObject::UvCurve(ii, true));
        let ivl = curve.natural_interval();
        let mut cc1 = curve_classification(k, IntersectObject::Curve(ii), &ivl, uv1, zone_tol)?;
        let mut cc2 = curve_classification(k, IntersectObject::Curve(ii), &ivl, uv2, zone_tol)?;

        // the classifications' lazy face queries (UV trim curves, gap caches, tolerance
        // growth) edit the faces' own B-reps, as upstream's; they are queries, so the
        // kept face caches stay
        let mut objects = IntersectObjectsMut {
            b: &mut *this_brep,
            other: other_brep.as_deref_mut(),
            side: false,
            curves_3d: &curves_3d,
            uv1: &uv_curves1,
            uv2: &uv_curves2,
            edited: false,
        };
        qf2::curve_on_classify_mut(k, f, &mut objects, true, &mut cc1, false, false, true)?;
        objects.side = other_side;
        qf2::curve_on_classify_mut(k, other, &mut objects, true, &mut cc2, false, false, true)?;

        let extract = ExtractOptions {
            uv_curves1: true,
            uv_curves2: true,
            points_3d: true,
        };
        // the point drops of `Homogenize` make the faces' trimmed caches on their
        // B-reps (smsurf_GetSurfaceCache)
        let ex = {
            let mut objects = IntersectObjectsMut {
                b: &mut *this_brep,
                other: other_brep.as_deref_mut(),
                side: false,
                curves_3d: &curves_3d,
                uv1: &uv_curves1,
                uv2: &uv_curves2,
                edited: false,
            };
            let ex = cc1.extract_geometry_mut(k, &mut objects, &mut cc2, extract);
            // an edit of the B-reps (not a query) may change what the kept face
            // caches were made from
            if objects.edited {
                for slot in face_caches.iter_mut().flatten() {
                    slot.clear();
                }
            }
            ex?
        };
        let found = !ex.curves_3d.is_empty();
        out.curves_3d.extend(ex.curves_3d);
        if let Some(v) = out.this_uv_curves.as_mut() {
            v.extend(ex.uv_curves1.into_iter().map(Some));
        }
        if let Some(v) = out.other_uv_curves.as_mut() {
            v.extend(ex.uv_curves2.into_iter().map(Some));
        }
        if let Some(v) = out.points_3d.as_mut() {
            v.extend(ex.points_3d);
        }
        if found {
            if let Some(v) = out.this_classes.as_mut() {
                v.push(cc1);
            }
            if let Some(v) = out.other_classes.as_mut() {
                v.push(cc2);
            }
        }
    }

    // The classifications refer to these curves by index, so they are returned whenever
    // classifications are requested too (upstream keeps them alive through the
    // classifications; `pOptCC3DCrvs` is then only the caller's cleanup list).
    if options.want_cc_curves || options.want_this_classes || options.want_other_classes {
        out.cc_curves_3d = Some(curves_3d);
        out.cc_this_uv_curves = Some(uv_curves1);
        out.cc_other_uv_curves = Some(uv_curves2);
    }
    Ok(out)
}

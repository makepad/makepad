//! `SmSurfaceCache`, part 1: the cache object ([`SurfaceCache`]) with its simple
//! queries, the decomposition node data ([`BezierAux2d`], [`BezierPatch`]) with their
//! lazily built polar boxes, the boundary bounding planes of a patch
//! ([`PatchBoundaryBoundingPlanes`]), the alternate curve caches ([`AltSrfCache`]),
//! the Bezier approximation and size tolerances of sub-patches, the top of the
//! decomposition tree and the classification of its leaves against a face.
//!
//! Upstream: `source/SMLib/src/SmSurfaceCache.cpp`, `source/SMLib/inc/SmSurfaceCache.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`PatchBoundaryBoundingPlanes`], [`PB_SAMPLE_COUNT`] | `SmPatchBoundaryBoundingPlanes`, `SM_PB_SAMPLECOUNT` |
//! | [`PatchBoundaryBoundingPlanes::new`]              | `SmPatchBoundaryBoundingPlanes(const SmSurface*, const SmExtent2d&)` |
//! | `Drop` (implicit)                                 | `~SmPatchBoundaryBoundingPlanes`                 |
//! | [`PatchBoundaryBoundingPlanes::compute_bounding_planes`] | `ComputeBoundingPlanes`                    |
//! | [`PatchBoundaryBoundingPlanes::create_boundary_curve_for_index`] | `CreateBoundaryCurveForIndex`      |
//! | [`PatchBoundaryBoundingPlanes::boundary_uv_end_points`] | `GetBoundaryUVEndPoints`                   |
//! | [`check_bounding_plane_separating_halfspace`]     | `sm_CheckBoundingPlaneSeparatingHalfspace` (static) |
//! | [`compute_plane_from_3_points`], [`PlaneFrom3Points`] | `sm_ComputePlaneFrom3Points` (static)        |
//! | [`compute_bezier_approximation`], `compute_bezier_approximation_memo` (crate) | `sm_ComputeBezierApproximation` (static) |
//! | `PoleMemo`                                      | (none: see the singularity memo deviation)       |
//! | [`cull_array`]                                    | `sm_CullArray` (static)                          |
//! | [`BezierAux2d`], [`BezierAux2d::uv_domain`]       | `SmBezierAux2d`, `GetUVDomain`                   |
//! | [`SurfaceCache::polar_box`], [`SurfaceCache::polar_box_unstored`] | `SmBezierAux2d::GetPolarBox`     |
//! | [`SurfaceCache::build_polar_box`]                 | `SmBezierAux2d::BuildPolarBox`                   |
//! | [`BezierPatch`], [`BezierPatch::pseudo_box`], [`BezierPatch::bezier`] | `SmBezierPatch`, `GetPseudoBox`, `GetBezierPtr` |
//! | [`AltSrfCache`], [`AltSrfCacheType`], [`AltCacheKey`] | `SmAltSrfCache`, `SmAltSrfCacheType`, the user data arguments |
//! | [`AltSrfCache::new`]                              | `SmAltSrfCache::SmAltSrfCache`                   |
//! | `Drop` (implicit)                                 | `SmAltSrfCache::~SmAltSrfCache`                  |
//! | [`AltSrfCache::match_cache`]                      | `SmAltSrfCache::MatchCache`                      |
//! | [`AltSrfCache::copy_of_curves`], [`AltSrfCache::copy_of_curves_partial`], [`CopyOfCurves`] | `SmAltSrfCache::GetCopyOfCurves` |
//! | [`SurfaceCache`], [`SurfaceCacheOptions`], [`SurfaceCache::new`] | `SmSurfaceCache`, its constructor arguments, the field initialisation of `SmSurfaceCache::SmSurfaceCache` (see deviations) |
//! | [`SurfaceCache::has_cached_curves`]               | `SmSurfaceCache::HasCachedCurves`                |
//! | [`SurfaceCache::find_unmarked_leaf_node`]         | `SmSurfaceCache::FindUnmarkedLeafNode`           |
//! | [`SurfaceCache::first_bezier_patch`]              | `SmTreeNode::GetFirstBezierPatch` (unit `geom.tree`) on the cache's data |
//! | [`SurfaceCache::surface_continuity`]              | `SmSurfaceCache::GetSurfaceContinuity`           |
//! | [`SurfaceCache::connected_region`], [`ConnectedRegion`] | `SmSurfaceCache::GetConnectedRegion`       |
//! | [`SurfaceCache::mark_face_containment`]           | `SmSurfaceCache::MarkFaceContainment`            |
//! | [`SurfaceCache::mark_face_containment_with`], [`FaceContainmentClassifier`] | `MarkFaceContainment` with the virtual `PointClassify` of a derived cache |
//! | [`SurfaceCache::build_tree_top`]                  | `SmSurfaceCache::BuildTreeTop`                   |
//! | [`SurfaceCache::iso_boundary_curves`], [`IsoBoundaryCurves`] | `SmSurfaceCache::GetIsoBoundaryCurves` |
//! | [`SurfaceCache::size_tolerances_from_corners`], [`SizeTolerances`] | `ComputeSizeTolerances` (corner point version) |
//! | [`SurfaceCache::size_tolerances_from_surface`]    | `ComputeSizeTolerances` (surface version)        |
//! | [`SurfaceCache::corners`], [`Corners`]            | `SmSurfaceCache::GetCorners`                     |
//! | [`SurfaceCache::face`]                            | `SmSurfaceCache::GetFace`                        |
//! | [`SurfaceCache::point_test`]                      | `SmSurfaceCache::PointTest`                      |
//! | [`SurfaceCache::point_classify`]                  | `SmSurfaceCache::PointClassify`                  |
//! | [`SurfaceCache::point_test_with`], [`SurfaceCache::point_classify_with`] | virtual trimmed-cache point queries with an operation-scoped face service |
//! | [`SurfaceCache::is_trimmed`]                      | the retained `SmTrimSrfCache` dynamic type       |
//! | [`SurfaceCache::implant_edgeuse`], [`SurfaceCache::implant_poly_edge`] | `ImplantEdgeuse`, `ImplantPolyEdge` (base class) |
//! | [`SurfaceCache::process_boundary_curves`]         | `GetProcessBoundaryCurves`                       |
//! | [`SurfaceCache::patch_count`], [`SurfaceCache::patch_at`] | `GetPatchCount`, `GetPatchAt`            |
//! | [`SurfaceCache::u_continuities`], [`SurfaceCache::v_continuities`] | `GetUContinuitiesArray`, `GetVContinuitiesArray` |
//! | [`SurfaceCache::surface`]                         | `SmSurfaceCache::GetSurface`                     |
//! | [`SurfaceCache::patch_boundary_bounding_planes`]  | `GetPatchBoundaryBoundingPlanes`                 |
//! | [`TYPE`], [`TYPE_STRING`], [`SurfaceCache::type_code`], [`SurfaceCache::type_string`] | `GetType`, `GetTypeString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`SurfaceCache::is_kind_of`]      | `SmSurfaceCache::IsKindOf`                       |
//!
//! Deviations:
//! - **Ownership.** Upstream holds a pointer to its surface; the cache owns the
//!   [`Surface`] it is built for (moved in, so its owner is kept). The surface's
//!   handle in the caller's object space (`O`, see [`crate::geom::tree`]) is optional;
//!   it is what `mBA_pSurface` of a node compares against. Subdivision surfaces
//!   (`m_sSubdivisionSurfaces`) are kept with their handles.
//!   [`PatchBoundaryBoundingPlanes`] does not hold the surface: its methods take it.
//! - **Node data.** `SmTreeNode::m_pData` points at an `SmBezierPatch` (in the cache's
//!   `m_sBezMgr`) or an `SmBezierAux2d` (in `m_sAuxMgr`). Here the tree node keeps the
//!   fields the tree itself reads ([`SurfaceAux`]: split direction, mark, class, face,
//!   surface) and the arena slot of the rest, which lives in the cache's
//!   [`SurfaceCache::patches`] / [`SurfaceCache::auxs`] arenas in allocation order.
//!   `m_pOwningTreeNode` is [`BezierAux2d::owning_tree_node`]. The vertex, edge-use and
//!   poly-edge lists are `Vec`s of caller handles. The tree learns the surface handle
//!   ([`Tree::set_owner_surface`], `SmSurfaceCache::GetSurface` behind
//!   `SmTree::GetOwnerObject`); as the tree cannot reach the arenas,
//!   [`SurfaceCache::first_bezier_patch`] answers `SmTreeNode::GetFirstBezierPatch`
//!   for cache trees.
//! - **Construction.** `SmSurfaceCache::SmSurfaceCache` (unit `surface.surface_cache_p3`)
//!   only initialises fields; [`SurfaceCache::new`] does the same so the cache can
//!   exist before its tree is built ([`crate::surface::surface_cache::p3`] builds it). The byte-size bookkeeping (`m_lBezierSize`,
//!   `m_lTotalBezSize`, `sm_ComputeNurbSurfaceSize`) has no counterpart.
//!   `SmSurfaceCache::GetTree` is [`SurfaceCache::get_tree`] (same unit).
//! - **Virtual dispatch.** The surface calls go through the surface kind: the
//!   [`BSplineSurface`] overrides (`CreateIsoParametricCurve`, `CalculateBoundingBox`)
//!   are used; the curve-bounded surface runs the base class bodies
//!   (`SmSurface::CreateIsoParametricCurve`, `surface.surface_p5`;
//!   `SmSurface::CalculateBoundingBox`, `surface.surface_p1`), and
//!   `SmSurface::IsSingularity` (`surface.surface_p5`; only the plane overrides it,
//!   with "no singularity", wired by `surface.surface_cache_p2`).
//!   `SmSurface::GetFace` (`surface.surface_p1`) follows the surface owner chain;
//!   [`SurfaceCache::face`] additionally needs a validated caller-handle binding.
//!   `EvaluateNormal` is
//!   [`Surface::evaluate_normal`] (`surface::surface::p7`, with the plane and
//!   curve-bounded overrides). An `SmPlane` is an
//!   `SmBSplineSurface` upstream: the cast gives its B-spline part. The iso curves
//!   (`CreateIsoParametricCurve`, virtual: an `SmLine` for a plane, an `SmCircle` in
//!   the sweep direction of a surface of revolution) are held as [`Curve`], so the
//!   derived classes' overrides stay in effect. Where upstream
//!   casts with `SM_CAST_PTR(SmBSplineSurface, ...)` and `NER`s, a non-B-spline surface
//!   gives `SM_ERR_NULL_POINTER`. `PointClassify` is the base class body (`SM_ERR`); the
//!   trimmed surface cache overrides belong to `surface.trim_srf_cache`, which runs
//!   `MarkFaceContainment` through [`SurfaceCache::mark_face_containment_with`]: the
//!   [`FaceContainmentClassifier`] stands for the virtual `PointClassify` and for
//!   `SmTol::GetZoneTol3d` of a node's face (an opaque handle here).
//! - **Out-params** become return values; where upstream leaves an output untouched
//!   on a failure the port returns `None` for it. `GetIsoBoundaryCurves` returns
//!   copies of the cached curves instead of shared pointers.
//!   `SmAltSrfCache::GetCopyOfCurves` stops at a cached curve that is not a B-spline
//!   (upstream leaves the uncopied cached pointers in the output array).
//! - **Cache copies.** `map_handles` uses exact, fallible evaluation copies of
//!   surfaces, retaining their owner and face tolerance. Infallible `Clone` keeps
//!   the existing geometry-copy semantics (including NURBS knot fix-up), not an
//!   exact evaluation snapshot; its face binding is retained only when the copied
//!   surface still reaches the validated owner key.
//! - **Ignored statuses** stay ignored (`GetKnots` in `GetSurfaceContinuity`, the
//!   bounding box calls of `BuildPolarBox`, `GetCopyOfCurves` in `HasCachedCurves`,
//!   `SetMinMax` in `GetCorners`), except [`SolidError::Unported`], which is always
//!   propagated.
//! - **Undefined behaviour.** Where upstream would read through a null or mistyped
//!   `m_pData`, index past an array (`GetPatchAt`, which would also grow the patch
//!   list; a continuity array longer than the knot list) or dereference a null
//!   subdivision surface, the port returns [`SolidError::InvalidInput`].
//! - **Cyclic trees.** `FindUnmarkedLeafNode` walks child links: on a tree whose links
//!   form a cycle it never ends (its stack grows without bound). The port stops after
//!   more pops than an acyclic tree allows (the initial stack length plus one, times
//!   the node count plus one) with [`SolidError::InvalidInput`].
//! - **Singularity memo (performance only).** `sm_ComputeBezierApproximation` calls
//!   `IsSingularity` at every patch corner, which calls `GetSingularities` (36 surface
//!   evaluations) whenever the corner is not singular by its own derivatives. That
//!   result depends only on the surface and the kernel, so inside one
//!   [`SurfaceCache::subdivide_to_tolerances_with`] call (one surface, one kernel) the
//!   splits of [`SurfaceCache::subdivide_node_split`] compute it once (`PoleMemo`)
//!   and pass it to [`Surface::is_singularity_with_poles`] (upstream's overload taking
//!   the singular sides and pole points). The values are the ones the lazy call would
//!   compute; when that call fails with [`SolidError::Unported`] the memo is not used.
//!   Upstream also copies no surface per split (`subdivide_node_split` borrows it).
//! - **Not ported here.** `GetNthBoundingPlane`, `AddCurvesToCache`, the destructor
//!   (`surface.surface_cache_p2`, [`crate::surface::surface_cache::p2`]), the tree construction
//!   (`surface.surface_cache_p3`, [`crate::surface::surface_cache::p3`]),
//!   `GetBoundingPlane`, `GetBrep`, `GetMemoryUsed`, `AssertValid`, the `Dump` and
//!   `Draw` functions (other units or debug output).

use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::object;
use crate::geom::polar_box::{PolarBox, PolarBoxState};
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::{zone_tol_3d, SmallTopoType};
use crate::geom::tree::{
    AuxDataType, NodeClass, NodeData, NodeGeomType, NodeId, SplitDir, SurfaceAux, Tree, TreeNode,
};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BoundingBoxRequest, BoundingBoxes, SurfParamType,
};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::math::NOW;
use crate::nurbs::nurbs_crv::to_euclid;
use crate::nurbs::srf_basic::Surface as NurbSurface;
use crate::surface::surface::p1::SurfaceBoxes;
use crate::surface::surface::p5::SS_NONE;
use crate::surface::surface::{Surface, SurfaceKind};

/// `SmSurfaceCache_TYPE` (`SURF_BASE_TYPE + 100`).
pub const TYPE: i64 = 15100;

/// `SmSurfaceCache::GetTypeString`.
pub const TYPE_STRING: &str = "SmSurfaceCache_TYPE";

/// `SM_PB_SAMPLECOUNT`: sample points stored per bounding plane.
pub const PB_SAMPLE_COUNT: usize = 5;

/// `SM_MAX_ALT_CACHES`: alternate caches a surface cache can hold.
pub const MAX_ALT_CACHES: usize = 5;

const OP_BOUNDARY: &str =
    "surface::surface_cache::p1::PatchBoundaryBoundingPlanes::create_boundary_curve_for_index";
const OP_PLANES: &str =
    "surface::surface_cache::p1::PatchBoundaryBoundingPlanes::compute_bounding_planes";
const OP_PLANE3: &str = "surface::surface_cache::p1::compute_plane_from_3_points";
const OP_HALFSPACE: &str = "surface::surface_cache::p1::check_bounding_plane_separating_halfspace";
const OP_COPY: &str = "surface::surface_cache::p1::AltSrfCache::copy_of_curves";
const OP_AUX: &str = "surface::surface_cache::p1::SurfaceCache::aux";
const OP_POLAR: &str = "surface::surface_cache::p1::SurfaceCache::build_polar_box";
const OP_FIND: &str = "surface::surface_cache::p1::SurfaceCache::find_unmarked_leaf_node";
const OP_CONT: &str = "surface::surface_cache::p1::SurfaceCache::surface_continuity";
const OP_REGION: &str = "surface::surface_cache::p1::SurfaceCache::connected_region";
const OP_MARK: &str = "surface::surface_cache::p1::SurfaceCache::mark_face_containment";
const OP_TOP: &str = "surface::surface_cache::p1::SurfaceCache::build_tree_top";
const OP_PATCH: &str = "surface::surface_cache::p1::SurfaceCache::patch_at";
const OP_CLASSIFY: &str = "surface::surface_cache::p1::SurfaceCache::point_classify";
const OP_IMPLANT_EU: &str = "surface::surface_cache::p1::SurfaceCache::implant_edgeuse";
const OP_IMPLANT_PE: &str = "surface::surface_cache::p1::SurfaceCache::implant_poly_edge";

/// True for `SmSurfaceCache_TYPE` and its base `SmObject_TYPE`
/// (`SmSurfaceCache::IsKindOf`).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || object::is_kind_of(t)
}

pub(super) fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// `SM_CAST_PTR(SmBSplineSurface, surface)` for callers that run only non-virtual
/// `SmBSplineSurface` members: an `SmPlane` gives its B-spline part. Callers that
/// reach a virtual member deal with planes first.
pub(super) fn bspline_of(surface: &Surface) -> Option<&BSplineSurface> {
    bspline_of_kind(surface.kind())
}

/// [`bspline_of`] of a surface kind (a made kind with its NURB, say).
fn bspline_of_kind(kind: &SurfaceKind) -> Option<&BSplineSurface> {
    match kind {
        SurfaceKind::BSpline(b) => Some(b),
        SurfaceKind::Plane(p) => Some(p.base()),
        SurfaceKind::Cone(s) => Some(s.rev().base()),
        SurfaceKind::Sphere(s) => Some(s.base().base()),
        SurfaceKind::Torus(s) => Some(s.base().base()),
        SurfaceKind::SurfOfRevolution(s) => Some(s.base()),
        SurfaceKind::SurfOfExtrusion(s) => Some(s.base()),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => None,
    }
}

/// `new SmBSplineCurve(*pCurve)` for an `SmBSplineCurve*` that may point at an `SmLine` or
/// `SmCircle` (the iso curves of a plane or a sweep): the copy constructor of the base
/// class makes a plain B-spline curve, dropping the derived class.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a curve that is not of a B-spline kind.
pub fn copy_as_bspline(curve: &Curve) -> Result<Curve> {
    // (an SmLine makes its NURB when the copy constructor asks for it)
    if let CurveKind::Line(l) = curve.kind() {
        return Ok(Curve::from(l.base_with_nurb()?.into_owned()));
    }
    bspline_curve_of(curve)
        .map(|b| Curve::from(b.clone()))
        .ok_or(SolidError::null_pointer(OP_COPY))
}

/// `SM_CAST_PTR(SmBSplineCurve, curve)`: the B-spline (base) of the B-spline kinds.
const fn bspline_curve_of(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// `SmSurface::CreateIsoParametricCurve(ctx, param, iso, tol, curve, domain)`, the
/// virtual: the B-spline, plane (an `SmLine`) and surface of revolution (an
/// `SmCircle` in the sweep direction) overrides, the base class body for the other
/// kinds. The curve is held as a [`Curve`] so the derived classes' overrides
/// (evaluation, length, degeneracy) stay in effect. The caller's 3D approximation
/// tolerance is forwarded, with `0.0` selecting the automatic tolerance.
fn create_iso_parametric_curve(
    k: &Kernel,
    surface: &Surface,
    param: SurfParamType,
    iso: f64,
    approx_tol_3d: f64,
    domain: Option<Extent2d>,
) -> Result<Curve> {
    let r = surface.create_iso_parametric_curve(
        k,
        param,
        iso,
        approx_tol_3d,
        domain.as_ref(),
        false,
    )?;
    match surface.kind() {
        // the SmSurface body (surface.surface_p5) makes a B-spline
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_)
            if !matches!(r.curve.kind(), CurveKind::BSpline(_)) =>
        {
            Err(SolidError::sm_err(
                "surface::surface_cache::p1::create_iso_parametric_curve",
            ))
        }
        _ => Ok(r.curve),
    }
}

/// `SmSurface::CalculateBoundingBox(domain, ...)`: the B-spline override; other kinds
/// run the base class body.
pub(super) fn calculate_bounding_box(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    req: &BoundingBoxRequest<'_>,
) -> Result<BoundingBoxes> {
    match surface.kind_with_nurb(k)? {
        SurfaceKind::BSpline(b) => b.calculate_bounding_box(k, domain, req),
        // inherited from SmBSplineSurface, run for the derived surface (its IsKindOf
        // and virtual EvaluateNormal)
        SurfaceKind::Cone(s) => {
            s.rev()
                .base()
                .calculate_bounding_box_of(k, domain, req, Some(surface))
        }
        SurfaceKind::Sphere(s) => {
            s.base()
                .base()
                .calculate_bounding_box_of(k, domain, req, Some(surface))
        }
        SurfaceKind::Torus(s) => {
            s.base()
                .base()
                .calculate_bounding_box_of(k, domain, req, Some(surface))
        }
        SurfaceKind::SurfOfRevolution(s) => {
            s.base()
                .calculate_bounding_box_of(k, domain, req, Some(surface))
        }
        SurfaceKind::SurfOfExtrusion(s) => {
            s.base()
                .calculate_bounding_box_of(k, domain, req, Some(surface))
        }
        SurfaceKind::Plane(p) => crate::surface::surface::p1::plane_bounding_box(k, p, domain, req),
        SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) | SurfaceKind::SrfInVolume(_) => {
            // SmSurface::CalculateBoundingBox (surface::surface::p1). Its polar box asks the
            // surface's own cache for it, whose build asks this function again: upstream
            // recurses without bound (UB-84), so the request is refused.
            if req.polar {
                return Err(SolidError::invalid_input(
                    "surface::surface_cache::p1::calculate_bounding_box",
                    crate::surface::surface::p1::POLAR_RECURSION,
                ));
            }
            // The other boxes come from the base body (Surface::base_calculate_bounding_box,
            // surface::surface::p1): the top node of the surface's own cache, the cache
            // this callback is building. Not reached: the callers are the polar box of a
            // node (refused above) and the tolerance test of an SM_AD_AUX_DATA node,
            // which upstream casts to an SmBSplineSurface unchecked and the port refuses
            // for these kinds before calling here (`bspline_of`). Routing it would build
            // a second cache of the surface from inside the first build, which recurses.
            // The error names the owner of the base body.
            Err(SolidError::unported(
                "SmSurface::CalculateBoundingBox",
                "surface.surface_p1",
            ))
        }
        // SmOffsetSurface::CalculateBoundingBox forwarding the basis guesses to its base's
        // CalculateBoundingBox (this dispatcher, all three boxes)
        SurfaceKind::OffsetSurface(o) => {
            let base_req = BoundingBoxRequest {
                normal: true,
                pseudo: true,
                polar: true,
                ..*req
            };
            let boxes = o.calculate_bounding_box_with(
                k,
                surface,
                domain,
                [req.normal, req.pseudo, req.polar],
                req.expand_by_zone_tol,
                |base| {
                    let b = calculate_bounding_box(k, base, domain, &base_req)?;
                    Ok(SurfaceBoxes {
                        normal_box: b.normal,
                        pseudo_box: b.pseudo,
                        polar_box: b.polar,
                    })
                },
            )?;
            Ok(BoundingBoxes {
                normal: boxes.normal_box,
                pseudo: boxes.pseudo_box,
                polar: boxes.polar_box,
            })
        }
    }
}

/// The `GetSingularities` outputs `SmSurface::IsSingularity(uv, eSingDir)` reads when a
/// point is not singular by its own derivatives (`SM_SS_*` sides and the four pole
/// points), memoised for one surface and one kernel (see the module deviations).
/// Performance only: every use gives the values the lazy call would.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum PoleMemo {
    /// Not computed yet.
    #[default]
    Empty,
    /// The call failed with [`SolidError::Unported`]: the lazy path is used.
    Unported,
    /// The sides and pole points.
    Poles(u64, Vec<Point3>),
}

impl PoleMemo {
    /// The memoised sides and pole points of `surface`, computed on first use exactly
    /// as [`Surface::is_singularity_with_poles`] computes them without pole points
    /// (`GetSingularities(&poles)`, whose ignored failure leaves `SM_SS_NONE` and
    /// undefined poles); `None` when that call is unported.
    fn poles(&mut self, k: &Kernel, surface: &Surface) -> Option<(u64, &[Point3])> {
        if *self == Self::Empty {
            *self = match ignored(surface.get_singularities(k, true, false)) {
                Ok(Some(r)) => Self::Poles(
                    u64::from(r.singular),
                    r.pole_points
                        .map_or_else(|| vec![Vec3::UNDEF; 4], |p| p.to_vec()),
                ),
                Ok(None) => Self::Poles(SS_NONE, vec![Vec3::UNDEF; 4]),
                Err(_) => Self::Unported,
            };
        }
        match self {
            Self::Poles(sides, poles) => Some((*sides, poles.as_slice())),
            Self::Empty | Self::Unported => None,
        }
    }
}

/// `SmSurface::IsSingularity(uv, eSingDir)`: the `SmPlane` override (a plane has no
/// singularity); the other ported kinds run the base class body, with the pole data
/// from `memo` when given (see `PoleMemo`).
fn is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    memo: Option<&mut PoleMemo>,
) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity().0),
        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            if let Some((sides, poles)) = memo.and_then(|m| m.poles(k, surface)) {
                return Ok(surface
                    .is_singularity_with_poles(k, uv, sides, Some(poles), EFF_ZERO, false)?
                    .singular);
            }
            Ok(surface.is_singularity(k, uv, EFF_ZERO, false)?.singular)
        }
    }
}

/// `EvaluateNormal(uv, u_from_left, v_from_left, normal)` (virtual).
fn evaluate_normal(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    u_left: bool,
    v_left: bool,
) -> Result<Vec3> {
    use crate::nurbs::knots::Side;
    let side = |left: bool| if left { Side::Left } else { Side::Right };
    surface.evaluate_normal(k, uv, side(u_left), side(v_left))
}

/// `smos_Max`.
fn max2(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min`.
fn min2(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `smos_3Max`.
fn max3(a: f64, b: f64, c: f64) -> f64 {
    max2(max2(a, b), c)
}

/// `smos_3Min`.
fn min3(a: f64, b: f64, c: f64) -> f64 {
    min2(min2(a, b), c)
}

// ---- bounding planes ------------------------------------------------------------

/// The plane through three sample points ([`compute_plane_from_3_points`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneFrom3Points {
    /// `rbPlaneExistsArg`: the points span a plane.
    pub plane_exists: bool,
    /// `rb3PointsCollinearArg`: the points are collinear (no plane).
    pub collinear: bool,
    /// `rsPlaneBaseArg`: the points' centroid, when the plane exists.
    pub base: Option<Point3>,
    /// `rsPlaneNormalArg`: the unit plane normal, when the plane exists.
    pub normal: Option<Vec3>,
}

/// `sm_ComputePlaneFrom3Points`: the plane through three boundary curve samples,
/// unless an edge of their triangle is shorter than `tol` or the points are (nearly)
/// collinear.
///
/// # Errors
///
/// `SM_ERR` when the three corner normals disagree in sense (cannot happen for real
/// triangles).
pub fn compute_plane_from_3_points(
    p1: Point3,
    p2: Point3,
    p3: Point3,
    tol: f64,
) -> Result<PlaneFrom3Points> {
    let mut out = PlaneFrom3Points {
        plane_exists: false,
        collinear: false,
        base: None,
        normal: None,
    };

    // Avoiding insanity:
    let p12v = p1 - p2;
    let p23v = p2 - p3;
    let p31v = p3 - p1;

    if p12v.length() < tol || p23v.length() < tol || p31v.length() < tol {
        return Ok(out);
    }

    let p12v = p12v.unitize_or_z();
    let p23v = p23v.unitize_or_z();
    let p31v = p31v.unitize_or_z();

    let angle_tol = 0.01;

    let p123 = p12v.cross(p23v);
    let p231 = p23v.cross(p31v);
    let p312 = p31v.cross(p12v);

    let sin12 = p123.length();
    let sin23 = p231.length();
    let sin31 = p312.length();

    if sin12 < angle_tol || sin23 < angle_tol || sin31 < angle_tol {
        if sin12 < EFF_ZERO_SQRT && sin23 < EFF_ZERO_SQRT && sin31 < EFF_ZERO_SQRT {
            out.collinear = true;
        }
        return Ok(out);
    }

    // Check for safety (should be OK):
    if p123.dot(p231) < 0.0 || p231.dot(p312) < 0.0 || p312.dot(p123) < 0.0 {
        return Err(SolidError::sm_err(OP_PLANE3));
    }

    // If made it here, there exists a plane:
    out.plane_exists = true;
    out.base = Some((p1 + p2 + p3) / 3.0);
    out.normal = Some(((p123 + p231 + p312) / 3.0).unitize_or_z());
    Ok(out)
}

/// `sm_CheckBoundingPlaneSeparatingHalfspace`: whether every control point of
/// `bss` farther than `sqrt(SM_EFF_ZERO)` from the plane lies on one side of it
/// (`.0`), and, if so, whether that side is the normal's (`.1`, the normal must be
/// inverted to point off the surface).
///
/// # Errors
///
/// Those of `GetCanonical`.
pub fn check_bounding_plane_separating_halfspace(
    bss: &BSplineSurface,
    plane_base_point: Point3,
    plane_normal: Vec3,
) -> Result<(bool, bool)> {
    let num_control_points =
        bss.number_control_points(SurfParamType::U) * bss.number_control_points(SurfParamType::V);

    let canonical = bss.canonical()?;
    let control_points = canonical.control_points;

    if num_control_points != control_points.len() {
        return Err(SolidError::sm_err(OP_HALFSPACE));
    }

    let mut sign = 0i32;
    for cp in &control_points {
        let signed_dist = plane_normal.dot(*cp - plane_base_point);

        if signed_dist.abs() < EFF_ZERO_SQRT {
            continue;
        }

        // If there is a sign difference: the plane is not a separating halfspace
        if (signed_dist > 0.0 && sign == -1) || (signed_dist < 0.0 && sign == 1) {
            return Ok((false, false));
        }

        sign = if signed_dist > 0.0 { 1 } else { -1 };
    }

    // All points are expected to be in the 'material side' of the plane
    // (the normal pointing out of it):
    Ok((true, sign == 1))
}

/// The bounding planes of the four iso-parametric boundary curves of a rectangular
/// surface sub-domain (`SmPatchBoundaryBoundingPlanes`). Boundary `i` is
/// `0` = minimum v, `1` = maximum u, `2` = maximum v, `3` = minimum u. A boundary has a
/// plane when its 3D curve is planar and the whole surface lies on one side of that
/// plane; the normal points off the surface.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchBoundaryBoundingPlanes {
    /// `m_sUVDomain`: the sub-domain.
    uv_domain: Extent2d,
    /// `m_bPlanesComputed`.
    planes_computed: bool,
    /// `m_bBoundaryCurvePlanar`: boundary `i` has a bounding plane.
    boundary_curve_planar: [bool; 4],
    /// `m_bBoundaryCurveDegenerate`: boundary `i` is degenerate.
    boundary_curve_degenerate: [bool; 4],
    /// `m_vBasePoint`: base point of each plane.
    base_point: [Point3; 4],
    /// `m_vNormalVector`: normal of each plane.
    normal_vector: [Vec3; 4],
    /// `m_vSamplePoint`: boundary curve samples at 0, 1/4, 1/2, 3/4 and 1.
    sample_point: [[Point3; PB_SAMPLE_COUNT]; 4],
    /// `m_dPlaneTolerance`: largest distance of the curve to its plane.
    plane_tolerance: [f64; 4],
}

impl PatchBoundaryBoundingPlanes {
    /// Planes of `uv_domain`, not computed yet (`SmPatchBoundaryBoundingPlanes(const
    /// SmSurface*, const SmExtent2d&)`; the surface is passed to the methods).
    #[must_use]
    pub const fn new(uv_domain: Extent2d) -> Self {
        Self {
            uv_domain,
            planes_computed: false,
            boundary_curve_planar: [false; 4],
            boundary_curve_degenerate: [false; 4],
            base_point: [Vec3::UNDEF; 4],
            normal_vector: [Vec3::UNDEF; 4],
            sample_point: [[Vec3::UNDEF; PB_SAMPLE_COUNT]; 4],
            plane_tolerance: [0.0; 4],
        }
    }

    /// The sub-domain (`m_sUVDomain`).
    #[must_use]
    pub const fn uv_domain(&self) -> &Extent2d {
        &self.uv_domain
    }

    /// Whether the planes have been computed (`ArePlanesComputed`).
    #[must_use]
    pub const fn are_planes_computed(&self) -> bool {
        self.planes_computed
    }

    /// Whether boundary `i` has a bounding plane (`m_bBoundaryCurvePlanar[i]`).
    #[must_use]
    pub fn is_boundary_planar(&self, i: usize) -> bool {
        self.boundary_curve_planar.get(i).copied().unwrap_or(false)
    }

    /// Whether boundary `i` is degenerate (`m_bBoundaryCurveDegenerate[i]`).
    #[must_use]
    pub fn is_boundary_degenerate(&self, i: usize) -> bool {
        self.boundary_curve_degenerate
            .get(i)
            .copied()
            .unwrap_or(false)
    }

    /// The plane of boundary `i` when it has one: base point, normal, sample points
    /// and the curve's largest distance to the plane.
    #[must_use]
    pub fn plane(&self, i: usize) -> Option<(Point3, Vec3, [Point3; PB_SAMPLE_COUNT], f64)> {
        if !self.is_boundary_planar(i) {
            return None;
        }
        Some((
            *self.base_point.get(i)?,
            *self.normal_vector.get(i)?,
            *self.sample_point.get(i)?,
            *self.plane_tolerance.get(i)?,
        ))
    }

    /// The corner on boundary `index` and its iso-parameter direction: minimum v
    /// (`0`), maximum u (`1`), maximum v (`2`), minimum u (`3`).
    fn iso_of(&self, index: usize) -> (SurfParamType, f64) {
        let corner = if index == 0 || index == 3 {
            self.uv_domain.min()
        } else {
            self.uv_domain.max()
        };
        if index == 0 || index == 2 {
            (SurfParamType::V, corner.y)
        } else {
            (SurfParamType::U, corner.x)
        }
    }

    /// The 3D iso-parametric curve of boundary `index` of `surface`
    /// (`CreateBoundaryCurveForIndex`).
    ///
    /// # Errors
    ///
    /// Those of `SmBSplineSurface::CreateIsoParametricCurve`.
    pub fn create_boundary_curve_for_index(
        &self,
        k: &Kernel,
        surface: &Surface,
        index: usize,
    ) -> Result<Curve> {
        let (spt, param) = self.iso_of(index);
        // SM_CAST_PTR(SmBSplineSurface, ..) and NER: the B-spline kinds
        bspline_of(surface).ok_or(SolidError::null_pointer(OP_BOUNDARY))?;
        // make the curve through the virtual (the SmPlane and SmSurfOfRevolution
        // overrides; 3D Tol SM_BIG_DOUBLE: not used by any of them)
        create_iso_parametric_curve(k, surface, spt, param, 0.0, None)
    }

    /// The UV end points of boundary `index` (`GetBoundaryUVEndPoints`).
    #[must_use]
    pub fn boundary_uv_end_points(&self, index: usize) -> (Point2, Point2) {
        let (_, param) = self.iso_of(index);
        if index == 0 || index == 2 {
            (
                Point2::new(self.uv_domain.min().x, param),
                Point2::new(self.uv_domain.max().x, param),
            )
        } else {
            (
                Point2::new(param, self.uv_domain.min().y),
                Point2::new(param, self.uv_domain.max().y),
            )
        }
    }

    /// Computes the bounding plane of every boundary of `surface`
    /// (`ComputeBoundingPlanes`).
    ///
    /// # Errors
    ///
    /// Those of the iso curve extraction, curve evaluation, `IsOnPlane` and
    /// `GetCanonical`; `SM_ERR_INVALID_INPUT` for a zero boundary tangent at a
    /// collinear boundary; [`SolidError::Unported`] from `SmSurface::EvaluateNormal`
    /// (collinear boundary samples).
    pub fn compute_bounding_planes(&mut self, k: &Kernel, surface: &Surface) -> Result<()> {
        let tolerance = 0.0001;

        // local
        bspline_of(surface).ok_or(SolidError::null_pointer(OP_PLANES))?;
        // (the control point count and the canonical form read the NURB, made through
        // the virtual GetOrCreateGwNurbPointer when missing: a plane's, a cone's,
        // sphere's, torus's or surface of revolution's or extrusion's)
        let bs = match surface.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => p.base_for_query(),
            SurfaceKind::SurfOfExtrusion(_) => match surface.extrusion_base_with_nurb(k) {
                Ok(base) => base,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => std::borrow::Cow::Borrowed(
                    bspline_of(surface).ok_or(SolidError::null_pointer(OP_PLANES))?,
                ),
            },
            made => std::borrow::Cow::Borrowed(
                bspline_of_kind(made).ok_or(SolidError::null_pointer(OP_PLANES))?,
            ),
        };

        // for every boundary
        for ii in 0..4 {
            // create a 3D Bspline isoParameter boundary curve
            let bsp = self.create_boundary_curve_for_index(k, surface, ii)?;

            // init planar and degenerate flags to FALSE
            self.set_flags(ii, false, false);

            // test 3D boundary curve for degeneracy
            if bsp.is_degenerate(k, EFF_ZERO, None)? {
                self.set_flags(ii, false, true);
                continue;
            }

            // eval 3D boundary curve beg, mid, and end pts
            let domain = bsp.natural_interval();
            let p1 = bsp.evaluate_point(k, domain.evaluate(0.0))?;
            let p12 = bsp.evaluate_point(k, domain.evaluate(0.25))?;
            let p2 = bsp.evaluate_point(k, domain.evaluate(0.5))?;
            let p23 = bsp.evaluate_point(k, domain.evaluate(0.75))?;
            let p3 = bsp.evaluate_point(k, domain.evaluate(1.0))?;

            // compute plane from those 3 points
            let plane = compute_plane_from_3_points(p1, p2, p3, tolerance)?;
            let mut plane_base = plane.base.unwrap_or(Vec3::UNDEF);
            let mut plane_normal = plane.normal.unwrap_or(Vec3::UNDEF);

            // In case of collinearity, a plane may still be created as:
            //   Plane_normal = Cross_product(surface_normal,bndry_tangent)
            if plane.collinear {
                plane_base = (p1 + p2 + p3) / 3.0;

                let from_left = !(ii == 0 || ii == 3);
                let corner = if ii == 0 || ii == 3 {
                    self.uv_domain.min()
                } else {
                    self.uv_domain.max()
                };

                let surf_normal = evaluate_normal(k, surface, corner, from_left, from_left)?;

                let rel_curve_loc = if ii == 0 || ii == 3 { 0.0 } else { 1.0 };
                let evals = bsp.evaluate(k, domain.evaluate(rel_curve_loc), 1, from_left, false)?;
                let curve_direction = evals
                    .get(1)
                    .copied()
                    .ok_or_else(|| invalid(OP_PLANES, "missing curve derivative"))?
                    .unitize()?;

                plane_normal = curve_direction.cross(surf_normal);
            }

            // low work - degenerate boundary curve
            if !plane.plane_exists && !plane.collinear {
                continue;
            }

            // test: Is boundary curve control polygon on computed plane
            let on_plane = bsp.is_on_plane(k, plane_base, plane_normal, tolerance, None)?;

            // low work - this curve not planar - go to next boundary curve
            if !on_plane.on_plane {
                continue;
            }

            // test: does surface lie completely on one side of the bounding plane
            let (is_separating, invert_normal) =
                check_bounding_plane_separating_halfspace(&bs, plane_base, plane_normal)?;

            // low work - surface xsects bounding plane
            if !is_separating {
                continue;
            }

            // set bounding plane normal to point away from surface
            if invert_normal {
                plane_normal = -plane_normal;
            }

            // remember this boundary curve's state
            self.set_flags(ii, true, false);
            if let Some(b) = self.base_point.get_mut(ii) {
                *b = plane_base;
            }
            if let Some(n) = self.normal_vector.get_mut(ii) {
                *n = plane_normal;
            }
            if let Some(t) = self.plane_tolerance.get_mut(ii) {
                *t = on_plane.max_dist.unwrap_or(0.0);
            }
            if let Some(s) = self.sample_point.get_mut(ii) {
                *s = [p1, p12, p2, p23, p3];
            }
        }

        self.planes_computed = true;
        Ok(())
    }

    fn set_flags(&mut self, i: usize, planar: bool, degenerate: bool) {
        if let Some(p) = self.boundary_curve_planar.get_mut(i) {
            *p = planar;
        }
        if let Some(d) = self.boundary_curve_degenerate.get_mut(i) {
            *d = degenerate;
        }
    }
}

// ---- Bezier approximation and helpers ---------------------------------------------

/// `sm_ComputeBezierApproximation`: fills the preallocated bicubic `bezier` (4x4
/// net, 8 knots per direction) with the Hermite (corner point and derivative)
/// approximation of `surface` over `uv_domain`; derivatives at singular corners are
/// sampled just inside the domain.
///
/// # Errors
///
/// Those of `Evaluate`; `InvalidInput` when `bezier` is smaller than bicubic;
/// [`SolidError::Unported`] from `SmSurface::IsSingularity`.
pub fn compute_bezier_approximation(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    bezier: &mut NurbSurface,
) -> Result<()> {
    compute_bezier_approximation_memo(k, surface, uv_domain, bezier, None)
}

/// [`compute_bezier_approximation`] with the singularity test's pole data memoised in
/// `memo` (computed for `surface` and `k`; see `PoleMemo`).
pub(crate) fn compute_bezier_approximation_memo(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    bezier: &mut NurbSurface,
    mut memo: Option<&mut PoleMemo>,
) -> Result<()> {
    const OP: &str = "surface::surface_cache::p1::compute_bezier_approximation";

    // Load knot vectors with domain bounds
    let mut idx = bezier.indices();
    idx.r = 7;
    idx.s = 7;
    bezier
        .set_size_indices(idx)
        .map_err(|_| invalid(OP, "Bezier storage too small"))?;
    for i in 0..4 {
        set_knot(bezier.u_knots_mut(), i, uv_domain.min().x);
        set_knot(bezier.v_knots_mut(), i, uv_domain.min().y);
        set_knot(bezier.u_knots_mut(), i + 4, uv_domain.max().x);
        set_knot(bezier.v_knots_mut(), i + 4, uv_domain.max().y);
    }

    let size = uv_domain.size();

    // Now load control polygon at corners using hermite information.
    let corners = [
        HermiteCorner::new(
            uv_domain.min(),
            1.0,
            1.0,
            1.0,
            [(0, 0), (1, 0), (0, 1), (1, 1)],
        ),
        HermiteCorner::new(
            uv_domain.max(),
            -1.0,
            -1.0,
            1.0,
            [(3, 3), (2, 3), (3, 2), (2, 2)],
        ),
        HermiteCorner::new(
            uv_domain.evaluate(1.0, 0.0),
            -1.0,
            1.0,
            -1.0,
            [(3, 0), (2, 0), (3, 1), (2, 1)],
        ),
        HermiteCorner::new(
            uv_domain.evaluate(0.0, 1.0),
            1.0,
            -1.0,
            -1.0,
            [(0, 3), (1, 3), (0, 2), (1, 2)],
        ),
    ];
    let flags = EvaluateFlags {
        only_upper_half: false,
        ..EvaluateFlags::default()
    };
    for corner in corners {
        let HermiteCorner {
            uv,
            su,
            sv,
            twist,
            net: [c00, c10, c01, c11],
        } = corner;
        let mat = surface.evaluate(k, uv, 1, 1, flags)?;
        let d = |m: &Vec<Vec<Vec3>>, i: usize, j: usize| -> Result<Vec3> {
            m.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .ok_or_else(|| invalid(OP, "missing derivative"))
        };
        let p00 = d(&mat, 0, 0)?;
        let (du, dv, duv);
        if is_singularity(k, surface, uv, memo.as_deref_mut())? {
            let uvu = Point2::new(uv.x + su * (size.x * EFF_ZERO_SQRT), uv.y);
            let uvv = Point2::new(uv.x, uv.y + sv * (size.y * EFF_ZERO_SQRT));
            let mu = surface.evaluate(k, uvu, 1, 1, flags)?;
            du = d(&mu, 1, 0)? * size.x;
            let duv1 = d(&mu, 1, 1)? * size.x * size.y;
            let mv = surface.evaluate(k, uvv, 1, 1, flags)?;
            dv = d(&mv, 0, 1)? * size.y;
            duv = (duv1 + d(&mv, 1, 1)? * size.x * size.y) / 2.0;
        } else {
            du = d(&mat, 1, 0)? * size.x;
            dv = d(&mat, 0, 1)? * size.y;
            duv = d(&mat, 1, 1)? * size.x * size.y;
        }
        let p10 = if su > 0.0 {
            p00 + du / 3.0
        } else {
            p00 - du / 3.0
        };
        let p01 = if sv > 0.0 {
            p00 + dv / 3.0
        } else {
            p00 - dv / 3.0
        };
        let p11 = if twist > 0.0 {
            p10 + p01 - p00 + duv / 9.0
        } else {
            p10 + p01 - p00 - duv / 9.0
        };
        for (p, (i, j)) in [(p00, c00), (p10, c10), (p01, c01), (p11, c11)] {
            let cp = bezier
                .cpt_mut(i, j)
                .map_err(|_| invalid(OP, "Bezier net too small"))?;
            cp.x = p.x;
            cp.y = p.y;
            cp.z = p.z;
        }
    }

    for j in 0..4 {
        for k in 0..4 {
            bezier
                .cpt_mut(j, k)
                .map_err(|_| invalid(OP, "Bezier net too small"))?
                .w = NOW;
        }
    }
    Ok(())
}

/// One corner of [`compute_bezier_approximation`]: its uv, the signs of the u and v
/// steps into the domain and of the twist term, and the net indices of the corner
/// control point, its u and v neighbours and the twist control point.
#[derive(Debug, Clone, Copy)]
struct HermiteCorner {
    uv: Point2,
    su: f64,
    sv: f64,
    twist: f64,
    net: [(usize, usize); 4],
}

impl HermiteCorner {
    const fn new(uv: Point2, su: f64, sv: f64, twist: f64, net: [(usize, usize); 4]) -> Self {
        Self {
            uv,
            su,
            sv,
            twist,
            net,
        }
    }
}

fn set_knot(knots: &mut [f64], i: usize, value: f64) {
    if let Some(k) = knots.get_mut(i) {
        *k = value;
    }
}

/// `sm_CullArray`: removes every second element of `arr`, always keeping the first
/// and the last. An empty array becomes `[0.0]`, as upstream's `SetSize(1)` on an
/// empty `SmTArray`.
pub fn cull_array(arr: &mut Vec<f64>) {
    let size = arr.len();
    let mut ii = 2;
    let mut jj = 1;
    while ii < size {
        if let Some(&v) = arr.get(ii) {
            if let Some(slot) = arr.get_mut(jj) {
                *slot = v;
            }
        }
        ii += 2;
        jj += 1;
    }
    // Make sure the last element is retained.
    if ii == size {
        if let Some(&last) = size.checked_sub(1).and_then(|l| arr.get(l)) {
            if let Some(slot) = arr.get_mut(jj) {
                *slot = last;
            }
        }
        jj += 1;
    }
    // SetSize: shrinks, or grows an empty array by zero-filled elements (calloc)
    arr.resize(jj, 0.0);
}

// ---- decomposition node data ----------------------------------------------------------

/// One entry of a node's vertex list (`SmVertexList`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexListItem<O> {
    /// `m_pVU`: the vertex use.
    pub vertexuse: Option<O>,
    /// `m_vUVPoint`.
    pub uv_point: Point2,
    /// `m_vUVTol`.
    pub uv_tol: Vec2,
}

/// What [`SurfaceCache::mark_face_containment_with`] asks of the cache's face: the
/// virtual `PointClassify` of the derived cache and `SmTol::GetZoneTol3d` of a node's
/// face handle (the handles are opaque to the cache).
pub trait FaceContainmentClassifier<O> {
    /// `SmTol::GetZoneTol3d(pAux->m_pFace)` for a node's (non-null) face.
    ///
    /// # Errors
    ///
    /// The implementor's.
    fn zone_tol_3d(&mut self, k: &Kernel, face: O) -> Result<f64>;

    /// `PointClassify(uv, tol)`: the `SmPointClassificationType` value and the object
    /// the point is in or on.
    ///
    /// # Errors
    ///
    /// The implementor's (upstream `SER`s them).
    fn point_classify(
        &mut self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)>;
}

/// The base class answers of [`FaceContainmentClassifier`]: `PointClassify` is
/// `SM_ERR`, a node face's tolerance is unreachable.
struct BaseClassifier;

impl<O> FaceContainmentClassifier<O> for BaseClassifier {
    fn zone_tol_3d(&mut self, _k: &Kernel, _face: O) -> Result<f64> {
        // UNPORTED(geom.tol): SmTol::GetZoneTol3d(const SmObject*) on an opaque handle. Every
        // arm it dispatches to is ported (topology::{vertex, edge, face::p2, edgeuse::p1}::tolerance,
        // Brep::tolerance) but each takes `&Brep` and none is in scope here; wire once this code
        // receives the owning B-rep (justified: findings.md "stubs naming green units")
        // Proof (unreachable): BaseClassifier only fronts the SM_ERR PointClassify path; 0
        // hits in the green difftest sweep.
        Err(SolidError::unported(
            "SmTol::GetZoneTol3d(const SmObject*)",
            "geom.tol",
        ))
    }

    fn point_classify(
        &mut self,
        _k: &Kernel,
        _uv_point: Point2,
        _src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        Err(SolidError::sm_err(OP_CLASSIFY))
    }
}

/// The decomposition data of a surface subdivision tree node (`SmBezierAux2d`),
/// apart from the fields the tree reads (kept in the node's [`SurfaceAux`]: split
/// direction, mark, class, face and surface).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct BezierAux2d<O> {
    /// `m_sPolarBox`: bounds the surface normals over the node's domain (built
    /// lazily, see [`SurfaceCache::polar_box`]).
    pub polar_box: PolarBox,
    /// `m_bIsTessOnly`: node built for tessellation after the initial construction.
    pub is_tess_only: bool,
    /// `m_sUVDomain`: the node's parametric domain.
    pub uv_domain: Extent2d,
    /// `m_hasInsidePart`: the node or a child is inside.
    pub has_inside_part: bool,
    /// `m_CornerNode`: bottom-left, bottom-right, top-right, top-left node of the
    /// subtree.
    pub corner_nodes: [Option<NodeId>; 4],
    /// `m_vVertexList`.
    pub vertex_list: Vec<VertexListItem<O>>,
    /// `m_sEdgeuseList`: edge uses in the domain.
    pub edgeuse_list: Vec<O>,
    /// `m_sPolyEdgeList`: poly edges in the domain.
    pub poly_edge_list: Vec<O>,
    /// `m_bChordHeightSatisfied` (construction only).
    pub chord_height_satisfied: bool,
    /// `m_bAngleTolSatisfied` (construction only).
    pub angle_tol_satisfied: bool,
    /// `m_pOwningTreeNode`: the node owning this data.
    pub owning_tree_node: Option<NodeId>,
}

impl<O> Default for BezierAux2d<O> {
    /// Zero-filled block memory (`SmMemBlockMgr`): uninitialised polar box, zero
    /// domain, empty lists.
    fn default() -> Self {
        Self {
            polar_box: PolarBox::new(),
            is_tess_only: false,
            uv_domain: Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0),
            has_inside_part: false,
            corner_nodes: [None; 4],
            vertex_list: Vec::new(),
            edgeuse_list: Vec::new(),
            poly_edge_list: Vec::new(),
            chord_height_satisfied: false,
            angle_tol_satisfied: false,
            owning_tree_node: None,
        }
    }
}

impl<O> BezierAux2d<O> {
    /// The node's parametric domain (`GetUVDomain`).
    #[must_use]
    pub const fn uv_domain(&self) -> &Extent2d {
        &self.uv_domain
    }

    /// The polar box as stored, without building it (`GetPolarBoxPtr`).
    #[must_use]
    pub const fn polar_box_stored(&self) -> &PolarBox {
        &self.polar_box
    }

    /// Replaces the stored polar box.
    pub fn set_polar_box(&mut self, polar_box: PolarBox) {
        self.polar_box = polar_box;
    }
}

/// A leaf of the initial surface decomposition (`SmBezierPatch`): its
/// [`BezierAux2d`] data, the Bezier patch used during construction, the continuity
/// at its sides and a tight box.
#[derive(Debug, Clone, PartialEq)]
pub struct BezierPatch<O> {
    /// The `SmBezierAux2d` base.
    pub aux: BezierAux2d<O>,
    /// `m_pBezier`: the subdivision Bezier patch during construction (`None` after).
    pub bezier: Option<NurbSurface>,
    /// `m_eUCurveConts`: continuity at the minimum and maximum u of the patch.
    pub u_curve_conts: [ContinuityType; 2],
    /// `m_eVCurveConts`: continuity at the minimum and maximum v of the patch.
    pub v_curve_conts: [ContinuityType; 2],
    /// `m_sPseudoBox`: tight (parallelepiped) box.
    pub pseudo_box: PseudoBox,
}

impl<O> Default for BezierPatch<O> {
    /// Zero-filled block memory.
    fn default() -> Self {
        Self {
            aux: BezierAux2d::default(),
            bezier: None,
            u_curve_conts: [ContinuityType::Undefined; 2],
            v_curve_conts: [ContinuityType::Undefined; 2],
            pseudo_box: PseudoBox::new(),
        }
    }
}

impl<O> BezierPatch<O> {
    /// The patch's pseudo box (`GetPseudoBox`).
    #[must_use]
    pub const fn pseudo_box(&self) -> &PseudoBox {
        &self.pseudo_box
    }

    /// The patch's pseudo box, mutable (`GetPseudoBox` returns a reference).
    pub fn pseudo_box_mut(&mut self) -> &mut PseudoBox {
        &mut self.pseudo_box
    }

    /// The construction Bezier patch (`GetBezierPtr`).
    #[must_use]
    pub const fn bezier(&self) -> Option<&NurbSurface> {
        self.bezier.as_ref()
    }
}

// ---- alternate caches ---------------------------------------------------------------------

/// The kind of curves an [`AltSrfCache`] holds (`SmAltSrfCacheType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AltSrfCacheType {
    /// `SM_AS_SILHOUETTE`: silhouette curves trimmed to the natural domain.
    Silhouette,
    /// `SM_AS_TRIMMED_SILHOUETTE`: silhouette curves trimmed to the face boundaries.
    TrimmedSilhouette,
}

/// The optional user data an [`AltSrfCache`] is keyed by (`cpUserVec`, `cpUserBool`,
/// `cpUserLong`, `cpUserDouble`, `cpUserPointer`; `None` is a null pointer). The
/// user pointer is an opaque identity.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AltCacheKey {
    /// `cpUserVec`.
    pub vec: Option<Vec3>,
    /// `cpUserBool`.
    pub boolean: Option<bool>,
    /// `cpUserLong`.
    pub long: Option<i64>,
    /// `cpUserDouble`.
    pub double: Option<f64>,
    /// `cpUserPointer`.
    pub pointer: Option<u64>,
}

/// Copies of the curves of an [`AltSrfCache`] (`GetCopyOfCurves`' output arrays;
/// `None` for an array not asked for or not filled).
#[derive(Debug, Clone, Default)]
pub struct CopyOfCurves {
    /// `pCopyOf3DCurvesInCache`.
    pub curves_3d: Option<Vec<Curve>>,
    /// `pCopyOfUVCurvesInCache`.
    pub curves_uv: Option<Vec<Curve>>,
}

/// Extra curves stored with a surface cache (`SmAltSrfCache`): silhouette curves and
/// the data used to make them.
#[derive(Debug, Clone)]
pub struct AltSrfCache {
    /// `m_eSrfCacheType`.
    cache_type: AltSrfCacheType,
    /// `m_dApproxTol`.
    approx_tol: f64,
    /// `m_dAngleTol`.
    angle_tol: f64,
    /// `m_vUserVec`.
    user_vec: Vec3,
    /// `m_bUserBool`.
    user_bool: bool,
    /// `m_lUserLong`.
    user_long: i64,
    /// `m_dUserDouble`.
    user_double: f64,
    /// `m_cpUserPointer`.
    user_pointer: Option<u64>,
    /// `m_pCached3DCurves`.
    cached_3d_curves: Option<Vec<Curve>>,
    /// `m_pCachedUVCurves`.
    cached_uv_curves: Option<Vec<Curve>>,
}

impl AltSrfCache {
    /// A cache of `curves_3d` / `curves_uv` (consumed) made with the given
    /// parameters (`SmAltSrfCache::SmAltSrfCache`). Missing user data is stored as
    /// zero / `FALSE`.
    #[must_use]
    pub fn new(
        curves_3d: Option<Vec<Curve>>,
        curves_uv: Option<Vec<Curve>>,
        cache_type: AltSrfCacheType,
        approx_tol: f64,
        angle_tol: f64,
        key: &AltCacheKey,
    ) -> Self {
        Self {
            cache_type,
            approx_tol,
            angle_tol,
            user_vec: key.vec.unwrap_or(Vec3::new(0.0, 0.0, 0.0)),
            user_bool: key.boolean.unwrap_or(false),
            user_long: key.long.unwrap_or(0),
            user_double: key.double.unwrap_or(0.0),
            user_pointer: key.pointer,
            cached_3d_curves: curves_3d,
            cached_uv_curves: curves_uv,
        }
    }

    /// The cache type (`m_eSrfCacheType`).
    #[must_use]
    pub const fn cache_type(&self) -> AltSrfCacheType {
        self.cache_type
    }

    /// Whether the cache was made with the given parameters (`MatchCache`): same
    /// type, every given user value equal (vectors and doubles within a scaled
    /// `SM_EFF_ZERO`), tolerances within `SM_EFF_ZERO`.
    #[must_use]
    pub fn match_cache(
        &self,
        cache_type: AltSrfCacheType,
        approx_tol: f64,
        angle_tol: f64,
        key: &AltCacheKey,
    ) -> bool {
        let scale = key.vec.map_or(EFF_ZERO, |v| v.max_dimension() * EFF_ZERO);

        // when any AltSrfCache parameters differ - return FALSE
        !(cache_type != self.cache_type
            || key.boolean.is_some_and(|b| b != self.user_bool)
            || key.long.is_some_and(|l| l != self.user_long)
            || key.pointer.is_some_and(|p| Some(p) != self.user_pointer)
            || key
                .vec
                .is_some_and(|v| (v - self.user_vec).length_squared() > scale * scale)
            || key
                .double
                .is_some_and(|d| (d - self.user_double).abs() > EFF_ZERO)
            || (approx_tol - self.approx_tol).abs() > EFF_ZERO
            || (angle_tol - self.angle_tol).abs() > EFF_ZERO)
    }

    /// Copies (as plain B-spline curves) of the cached 3D and/or UV curves
    /// (`GetCopyOfCurves`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a list asked for is not cached or holds a curve
    /// that is not a B-spline.
    pub fn copy_of_curves(&self, want_3d: bool, want_uv: bool) -> Result<CopyOfCurves> {
        let (copies, status) = self.copy_of_curves_partial(want_3d, want_uv);
        status.map(|()| copies)
    }

    /// [`AltSrfCache::copy_of_curves`] keeping what was copied before a failure (the
    /// output arrays upstream fills in order) next to the status.
    pub fn copy_of_curves_partial(
        &self,
        want_3d: bool,
        want_uv: bool,
    ) -> (CopyOfCurves, Result<()>) {
        let mut out = CopyOfCurves::default();
        for (want, cached, slot) in [
            (want_3d, &self.cached_3d_curves, 0),
            (want_uv, &self.cached_uv_curves, 1),
        ] {
            if !want {
                continue;
            }
            let Some(cached) = cached else {
                return (out, Err(SolidError::null_pointer(OP_COPY)));
            };
            let mut list = Vec::with_capacity(cached.len());
            for c in cached {
                let Some(bsc) = bspline_curve_of(c) else {
                    return (out, Err(SolidError::null_pointer(OP_COPY)));
                };
                list.push(Curve::from(bsc.copy()));
            }
            if slot == 0 {
                out.curves_3d = Some(list);
            } else {
                out.curves_uv = Some(list);
            }
        }
        (out, Ok(()))
    }
}

// ---- the cache ---------------------------------------------------------------------------

/// The tessellation controls of a [`SurfaceCache`] (the constructor arguments of
/// `SmSurfaceCache`); a zero value is ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceCacheOptions {
    /// `dChordHeightTolerance`: largest control point to base plane distance.
    pub chord_height_tolerance: f64,
    /// `dAngTolRad`: largest control polygon turning angle, radians.
    pub ang_tol_rad: f64,
    /// `dAspectRatio3D`: largest base polygon aspect ratio.
    pub aspect_ratio_3d: f64,
    /// `dMaxSideLength3D`: largest base polygon side length.
    pub max_side_length_3d: f64,
    /// `dMinSideLength3D`: smallest base polygon side length.
    pub min_side_length_3d: f64,
    /// `dMinSideLengthRatioUV`: smallest element side / domain size ratio.
    pub min_side_length_ratio_uv: f64,
}

impl Default for SurfaceCacheOptions {
    /// Upstream's default arguments (only `dMinSideLengthRatioUV` is non-zero,
    /// `0.001`).
    fn default() -> Self {
        Self {
            chord_height_tolerance: 0.0,
            ang_tol_rad: 0.0,
            aspect_ratio_3d: 0.0,
            max_side_length_3d: 0.0,
            min_side_length_3d: 0.0,
            min_side_length_ratio_uv: 0.001,
        }
    }
}

/// The tessellation algorithm of a surface cache (`SmTessAlgorithmType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TessAlgorithmType {
    /// `SM_TA_FASTER_TESSELLATION` (default).
    #[default]
    FasterTessellation,
    /// `SM_TA_FEWER_POLYGONS`.
    FewerPolygons,
}

/// The state of `m_bFaceContainmentDone` (an `SmBoolean` that also takes `2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FaceContainment {
    /// `FALSE`: not done.
    #[default]
    NotDone,
    /// `2`: under construction.
    UnderConstruction,
    /// `TRUE`: complete.
    Done,
}

/// Size measures of a patch ([`SurfaceCache::size_tolerances_from_corners`],
/// [`SurfaceCache::size_tolerances_from_surface`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeTolerances {
    /// `rAspectRatio`: `[max u / max v, max v / max u]` (`0.5` when the divisor is
    /// below the scaled zero).
    pub aspect_ratio: Vec2,
    /// `rMaxSideLength3D`: `[max u, max v]` side lengths.
    pub max_side_length_3d: Vec2,
    /// `rMinSideLength3D`: `[min u, min v]` side lengths (the max when below the
    /// scaled zero).
    pub min_side_length_3d: Vec2,
}

/// The four corners of a patch ([`SurfaceCache::corners`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corners {
    /// `rU0V0Point`.
    pub u0v0: Point3,
    /// `rU1V0Point`.
    pub u1v0: Point3,
    /// `rU1V1Point`.
    pub u1v1: Point3,
    /// `rU0V1Point`.
    pub u0v1: Point3,
    /// `rDomain`: the domain of the corners (`None`: upstream leaves the output
    /// unchanged, for a knot vector running backwards).
    pub domain: Option<Extent2d>,
}

/// The output of [`SurfaceCache::connected_region`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectedRegion {
    /// `rConnectedNodes`: the region, starting with the start node.
    pub nodes: Vec<NodeId>,
    /// `rpClassifiedNeighborLeafNode`: the first marked neighbour classified inside
    /// or outside.
    pub classified_neighbor: Option<NodeId>,
}

/// The output of [`SurfaceCache::iso_boundary_curves`].
#[derive(Debug, Clone)]
pub struct IsoBoundaryCurves {
    /// `rbCurvesAreCached`: the curves are (now) held by the cache.
    pub cached: bool,
    /// `rIsoCurves`: minimum u, minimum v, maximum u, maximum v iso curves.
    pub curves: [Curve; 4],
}

/// The Bezier decomposition of a surface (`SmSurfaceCache`). `O` is the caller's
/// object handle type of the decomposition [`Tree`].
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct SurfaceCache<O> {
    /// Whether this cache originated from a trimmed cache; preserved by `into_base`.
    pub(crate) trimmed_dispatch: bool,
    /// Mapped face handle for trimmed point-test dispatch.
    pub(crate) trimmed_face: Option<O>,
    /// Owner key validated when `trimmed_face` was bound (never an arena index).
    trimmed_face_key: Option<object::ObjectRef>,
    /// `m_apAltSrfCaches[0..m_lNumAltCaches]`, newest first.
    alt_caches: Vec<AltSrfCache>,
    /// `m_pTree`.
    tree: Option<Tree<O>>,
    /// `m_cpSurface` (owned, see the module deviations).
    surface: Surface,
    /// The surface's handle (what `mBA_pSurface` compares against).
    surface_handle: Option<O>,
    /// `m_sUVDomain`: the domain the cache covers.
    uv_domain: Extent2d,
    /// `m_pUContinuities`: continuity at every u knot.
    u_continuities: Vec<ContinuityType>,
    /// `m_pVContinuities`: continuity at every v knot.
    v_continuities: Vec<ContinuityType>,
    /// `m_sBezMgr`: the patches of `SM_AD_BEZIER_SURFACE` nodes, in allocation order.
    patches: Vec<BezierPatch<O>>,
    /// `m_sAuxMgr`: the data of `SM_AD_BEZIER_SURFACE_HEAD` / `SM_AD_AUX_DATA` nodes.
    auxs: Vec<BezierAux2d<O>>,
    /// The tessellation controls.
    opts: SurfaceCacheOptions,
    /// `m_dIsoCurveTolerance`: tolerance of the cached iso boundary curves.
    iso_curve_tolerance: f64,
    /// `m_eTessellationAlgorithm`.
    pub tessellation_algorithm: TessAlgorithmType,
    /// `m_dStartFastSubdivisionAngleDeg`.
    pub start_fast_subdivision_angle_deg: f64,
    /// `m_bSwitchToFastSubdivision`.
    pub switch_to_fast_subdivision: bool,
    /// `m_dTrimSubdivisionFactor`.
    pub trim_subdivision_factor: f64,
    /// `m_apIsoBoundaryCurves`: the cached natural boundary iso curves.
    iso_boundary_curves: Option<[Curve; 4]>,
    /// `m_sSubdivisionSurfaces`: construction surfaces with their handles.
    subdivision_surfaces: Vec<(O, Surface)>,
    /// Names the halves of a split subdivision surface (see
    /// [`SurfaceCache::set_subdivision_handles`]).
    subdivision_handles: Option<fn(u64) -> O>,
    /// Handles named so far by `subdivision_handles`.
    subdivision_handle_count: u64,
    /// `m_sBoundingPlanes`.
    bounding_planes: PatchBoundaryBoundingPlanes,
    /// `m_bHaveTSurfaceCache`: the tree holds the face boundary lists.
    pub have_t_surface_cache: bool,
    /// `m_bFaceWasModified`.
    pub face_was_modified: bool,
    /// `m_bProcessBoundaryCurves`: local solvers also look on the boundary curves.
    pub process_boundary_curves: bool,
    /// `m_bPointTestEnabled`: keep only solutions inside the trim boundaries.
    pub point_test_enabled: bool,
    /// `m_bFaceContainmentDone`.
    pub face_containment_done: FaceContainment,
    /// The singularity memo of the running
    /// [`SurfaceCache::subdivide_to_tolerances_with`] (performance only; `None`
    /// outside it).
    pole_memo: Option<PoleMemo>,
}

impl<O: Clone> Clone for SurfaceCache<O> {
    fn clone(&self) -> Self {
        // Keep the prior infallible copy semantics; exact evaluation copies can
        // fail their nesting guard and belong in `map_handles` instead.
        let surface = self.surface.clone_keeping_owner();
        let binding_current = self
            .trimmed_face_key
            .is_some_and(|key| surface.reached_face() == Some(key));
        Self {
            trimmed_dispatch: self.trimmed_dispatch,
            trimmed_face: if binding_current {
                self.trimmed_face.clone()
            } else {
                None
            },
            trimmed_face_key: self.trimmed_face_key.filter(|_| binding_current),
            alt_caches: self.alt_caches.clone(),
            tree: self.tree.clone(),
            surface,
            surface_handle: self.surface_handle.clone(),
            uv_domain: self.uv_domain,
            u_continuities: self.u_continuities.clone(),
            v_continuities: self.v_continuities.clone(),
            patches: self.patches.clone(),
            auxs: self.auxs.clone(),
            opts: self.opts,
            iso_curve_tolerance: self.iso_curve_tolerance,
            tessellation_algorithm: self.tessellation_algorithm,
            start_fast_subdivision_angle_deg: self.start_fast_subdivision_angle_deg,
            switch_to_fast_subdivision: self.switch_to_fast_subdivision,
            trim_subdivision_factor: self.trim_subdivision_factor,
            iso_boundary_curves: self.iso_boundary_curves.clone(),
            subdivision_surfaces: self.subdivision_surfaces.clone(),
            subdivision_handles: self.subdivision_handles,
            subdivision_handle_count: self.subdivision_handle_count,
            bounding_planes: self.bounding_planes.clone(),
            have_t_surface_cache: self.have_t_surface_cache,
            face_was_modified: self.face_was_modified,
            process_boundary_curves: self.process_boundary_curves,
            point_test_enabled: self.point_test_enabled,
            face_containment_done: self.face_containment_done,
            pole_memo: None,
        }
    }
}

impl<O: Clone> SurfaceCache<O> {
    /// A copy standing for this same cache where upstream keeps using it (the
    /// boundary start-point search of `SmSurfaceIntersector::DoIntersection` runs on
    /// the intersector's own caches): the surfaces and iso boundary curves are exact
    /// evaluation copies, since [`Clone`] runs the copy constructor's knot fix-up,
    /// which moves near-coincident knots of a fillet surface.
    ///
    /// # Errors
    ///
    /// An owned geometry chain deeper than the evaluation-copy bound.
    pub(crate) fn clone_for_evaluation(&self) -> Result<Self> {
        let mut c = self.clone();
        c.surface = self.surface.clone_for_evaluation()?;
        let mut subdivision_surfaces = Vec::new();
        for (o, s) in &self.subdivision_surfaces {
            subdivision_surfaces.push((o.clone(), s.clone_for_evaluation()?));
        }
        c.subdivision_surfaces = subdivision_surfaces;
        c.iso_boundary_curves = match &self.iso_boundary_curves {
            Some([a, b, cc, d]) => Some([
                a.clone_for_evaluation()?,
                b.clone_for_evaluation()?,
                cc.clone_for_evaluation()?,
                d.clone_for_evaluation()?,
            ]),
            None => None,
        };
        Ok(c)
    }
}

impl<O: Copy + PartialEq> SurfaceCache<O> {
    /// A cache of `surface` over `uv_domain` with the tessellation controls `opts`,
    /// without tree (the field initialisation of `SmSurfaceCache::SmSurfaceCache`).
    #[must_use]
    pub fn new(surface: Surface, uv_domain: Extent2d, opts: SurfaceCacheOptions) -> Self {
        Self {
            trimmed_dispatch: false,
            trimmed_face: None,
            trimmed_face_key: None,
            alt_caches: Vec::new(),
            tree: None,
            surface,
            surface_handle: None,
            uv_domain,
            u_continuities: Vec::new(),
            v_continuities: Vec::new(),
            patches: Vec::new(),
            auxs: Vec::new(),
            opts,
            iso_curve_tolerance: 0.001,
            tessellation_algorithm: TessAlgorithmType::FasterTessellation,
            start_fast_subdivision_angle_deg: 30.0,
            switch_to_fast_subdivision: false,
            trim_subdivision_factor: 20.0,
            iso_boundary_curves: None,
            subdivision_surfaces: Vec::new(),
            subdivision_handles: None,
            subdivision_handle_count: 0,
            bounding_planes: PatchBoundaryBoundingPlanes::new(uv_domain),
            have_t_surface_cache: false,
            face_was_modified: false,
            process_boundary_curves: true,
            point_test_enabled: false,
            face_containment_done: FaceContainment::NotDone,
            pole_memo: None,
        }
    }

    /// Sets the handle of the cached surface (compared with the nodes'
    /// `mBA_pSurface`, and the tree's owner object).
    pub fn set_surface_handle(&mut self, handle: Option<O>) {
        self.surface_handle = handle;
        if let Some(t) = self.tree.as_mut() {
            t.set_owner_surface(handle);
        }
    }

    /// Synchronizes the evaluation copy with its owning face's final tolerance.
    pub fn set_owner_zone_tol_3d(&mut self, tolerance: f64) {
        self.surface.set_owner_zone_tol_3d(Some(tolerance));
    }

    /// Whether this cache retains trimmed-cache dispatch, including after unwrapping.
    #[must_use]
    pub const fn is_trimmed(&self) -> bool {
        self.trimmed_dispatch
    }

    /// Keys a face binding to the cache surface's owner as it is now: after the surface
    /// and its face were renamed together (a B-rep copied into new arenas, its handles
    /// mapped), the bound face is that surface's face again.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::face`].
    pub(crate) fn rekey_trimmed_face(&mut self) -> Result<()> {
        if self.trimmed_face_key.is_some() {
            self.trimmed_face_key = self.surface.face()?;
        }
        Ok(())
    }

    /// Records a face handle only after checking the source surface's owner key.
    pub(crate) fn bind_trimmed_face(&mut self, key: object::ObjectRef, face: O) -> Result<()> {
        if self.surface.face()? != Some(key) {
            return Err(invalid(
                OP_CLASSIFY,
                "surface owner does not match supplied face",
            ));
        }
        self.trimmed_face_key = Some(key);
        self.trimmed_face = Some(face);
        Ok(())
    }

    /// The handle of the cached surface.
    #[must_use]
    pub const fn surface_handle(&self) -> Option<O> {
        self.surface_handle
    }

    /// Copies this cache with its object handles translated by `map` (tree, node
    /// data, surface handle, subdivision surfaces), everything else unchanged. A Rust
    /// adapter like [`Tree::map_objects`]: it lets a solve whose solutions name other
    /// objects (a curve beside the surface) run over this cache. The copy names no
    /// further subdivision halves (`set_subdivision_handles`, construction only).
    /// `map` must name the same target object every time it receives the same
    /// source handle.
    ///
    /// # Errors
    ///
    /// Those of `map` and the bounded exact surface evaluation copies.
    pub fn map_handles<P: Copy + PartialEq>(
        &self,
        map: impl Fn(O) -> Result<P>,
    ) -> Result<SurfaceCache<P>> {
        let map_aux = |a: &BezierAux2d<O>| -> Result<BezierAux2d<P>> {
            let mut vertex_list = Vec::new();
            for v in &a.vertex_list {
                vertex_list.push(VertexListItem {
                    vertexuse: v.vertexuse.map(&map).transpose()?,
                    uv_point: v.uv_point,
                    uv_tol: v.uv_tol,
                });
            }
            Ok(BezierAux2d {
                polar_box: a.polar_box,
                is_tess_only: a.is_tess_only,
                uv_domain: a.uv_domain,
                has_inside_part: a.has_inside_part,
                corner_nodes: a.corner_nodes,
                vertex_list,
                edgeuse_list: a
                    .edgeuse_list
                    .iter()
                    .map(|&o| map(o))
                    .collect::<Result<_>>()?,
                poly_edge_list: a
                    .poly_edge_list
                    .iter()
                    .map(|&o| map(o))
                    .collect::<Result<_>>()?,
                chord_height_satisfied: a.chord_height_satisfied,
                angle_tol_satisfied: a.angle_tol_satisfied,
                owning_tree_node: a.owning_tree_node,
            })
        };
        let mut patches = Vec::new();
        for p in &self.patches {
            patches.push(BezierPatch {
                aux: map_aux(&p.aux)?,
                bezier: p.bezier.clone(),
                u_curve_conts: p.u_curve_conts,
                v_curve_conts: p.v_curve_conts,
                pseudo_box: p.pseudo_box,
            });
        }
        let mut auxs = Vec::new();
        for a in &self.auxs {
            auxs.push(map_aux(a)?);
        }
        let mut subdivision_surfaces = Vec::new();
        for (o, s) in &self.subdivision_surfaces {
            subdivision_surfaces.push((map(*o)?, s.clone_for_evaluation()?));
        }
        let surface = self.surface.clone_for_evaluation()?;
        let binding_current = self.trimmed_face_key == surface.face()?;
        Ok(SurfaceCache {
            trimmed_dispatch: self.trimmed_dispatch,
            trimmed_face: self
                .trimmed_face
                .filter(|_| binding_current)
                .map(&map)
                .transpose()?,
            trimmed_face_key: self.trimmed_face_key.filter(|_| binding_current),
            alt_caches: self.alt_caches.clone(),
            tree: self
                .tree
                .as_ref()
                .map(|t| t.map_objects(&map))
                .transpose()?,
            surface,
            surface_handle: self.surface_handle.map(&map).transpose()?,
            uv_domain: self.uv_domain,
            u_continuities: self.u_continuities.clone(),
            v_continuities: self.v_continuities.clone(),
            patches,
            auxs,
            opts: self.opts,
            iso_curve_tolerance: self.iso_curve_tolerance,
            tessellation_algorithm: self.tessellation_algorithm,
            start_fast_subdivision_angle_deg: self.start_fast_subdivision_angle_deg,
            switch_to_fast_subdivision: self.switch_to_fast_subdivision,
            trim_subdivision_factor: self.trim_subdivision_factor,
            iso_boundary_curves: self.iso_boundary_curves.clone(),
            subdivision_surfaces,
            subdivision_handles: None,
            subdivision_handle_count: self.subdivision_handle_count,
            bounding_planes: self.bounding_planes.clone(),
            have_t_surface_cache: self.have_t_surface_cache,
            face_was_modified: self.face_was_modified,
            process_boundary_curves: self.process_boundary_curves,
            point_test_enabled: self.point_test_enabled,
            face_containment_done: self.face_containment_done,
            pole_memo: None,
        })
    }

    /// `SmSurfaceCache_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmSurfaceCache_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// [`is_kind_of`] (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    /// The cached surface (`GetSurface`).
    #[must_use]
    pub const fn surface(&self) -> &Surface {
        &self.surface
    }

    /// The cached surface, mutable: only for the bookkeeping of the surface object it
    /// stands for (its cache slot), never its geometry.
    pub(crate) fn surface_mut(&mut self) -> &mut Surface {
        &mut self.surface
    }

    /// The cached surface and the singularity memo of the running
    /// [`SurfaceCache::subdivide_to_tolerances_with`] (`None` outside it).
    pub(crate) fn surface_and_pole_memo(&mut self) -> (&Surface, Option<&mut PoleMemo>) {
        (&self.surface, self.pole_memo.as_mut())
    }

    /// Starts a singularity memo for `self.surface` when none is running; `true` when
    /// this call started it (and must end it with [`SurfaceCache::end_pole_memo`]).
    pub(crate) fn begin_pole_memo(&mut self) -> bool {
        if self.pole_memo.is_some() {
            return false;
        }
        self.pole_memo = Some(PoleMemo::default());
        true
    }

    /// Drops the singularity memo.
    pub(crate) fn end_pole_memo(&mut self) {
        self.pole_memo = None;
    }

    /// The domain the cache covers (`GetUVDomain`).
    #[must_use]
    pub const fn uv_domain(&self) -> Extent2d {
        self.uv_domain
    }

    /// Replaces the domain the cache covers (upstream assigns `m_sUVDomain`, e.g.
    /// `SmTrimSrfCache::BuildTree` reducing it to the face domain); the boundary
    /// bounding planes keep the domain they were made with, as upstream.
    pub fn set_uv_domain(&mut self, uv_domain: Extent2d) {
        self.uv_domain = uv_domain;
    }

    /// The tessellation controls.
    #[must_use]
    pub const fn options(&self) -> &SurfaceCacheOptions {
        &self.opts
    }

    /// The tessellation controls, mutable (upstream's derived caches assign the
    /// protected tolerance members, e.g. `SmTessSrfCache::TestAgainstTolerances`).
    pub fn options_mut(&mut self) -> &mut SurfaceCacheOptions {
        &mut self.opts
    }

    /// The decomposition tree as stored (`m_pTree`), without building it.
    #[must_use]
    pub const fn tree(&self) -> Option<&Tree<O>> {
        self.tree.as_ref()
    }

    /// The decomposition tree as stored, mutable.
    pub fn tree_mut(&mut self) -> Option<&mut Tree<O>> {
        self.tree.as_mut()
    }

    /// Replaces the decomposition tree (upstream assigns `m_pTree`); the tree's
    /// owner object becomes the cache's surface handle.
    pub fn set_tree(&mut self, tree: Option<Tree<O>>) {
        self.tree = tree;
        let handle = self.surface_handle;
        if let Some(t) = self.tree.as_mut() {
            t.set_owner_surface(handle);
        }
    }

    /// The patch arena (`m_sBezMgr`).
    #[must_use]
    pub fn patches(&self) -> &[BezierPatch<O>] {
        &self.patches
    }

    /// The patch arena, mutable.
    pub fn patches_mut(&mut self) -> &mut Vec<BezierPatch<O>> {
        &mut self.patches
    }

    /// The aux arena (`m_sAuxMgr`).
    #[must_use]
    pub fn auxs(&self) -> &[BezierAux2d<O>] {
        &self.auxs
    }

    /// The aux arena, mutable.
    pub fn auxs_mut(&mut self) -> &mut Vec<BezierAux2d<O>> {
        &mut self.auxs
    }

    /// The alternate caches, newest first (`m_apAltSrfCaches`).
    #[must_use]
    pub fn alt_caches(&self) -> &[AltSrfCache] {
        &self.alt_caches
    }

    /// The alternate caches, mutable (at most [`MAX_ALT_CACHES`] are kept by
    /// `AddCurvesToCache`, unit `surface.surface_cache_p2`).
    pub fn alt_caches_mut(&mut self) -> &mut Vec<AltSrfCache> {
        &mut self.alt_caches
    }

    /// The subdivision surfaces with their handles (`m_sSubdivisionSurfaces`).
    pub fn subdivision_surfaces_mut(&mut self) -> &mut Vec<(O, Surface)> {
        &mut self.subdivision_surfaces
    }

    /// Sets how the cache names new subdivision surfaces: `make(n)` is the handle of
    /// the `n`-th surface named (`n` counts from 0 and is never reused). Upstream
    /// stores the surface pointers; a cache that holds subdivision surfaces on its
    /// nodes (`SmTessSrfCache`) needs this to split them (`SubdivideNode`).
    pub fn set_subdivision_handles(&mut self, make: Option<fn(u64) -> O>) {
        self.subdivision_handles = make;
    }

    /// A new subdivision surface handle, `None` when the cache cannot name them.
    pub fn new_subdivision_handle(&mut self) -> Option<O> {
        let make = self.subdivision_handles?;
        let n = self.subdivision_handle_count;
        self.subdivision_handle_count = n.saturating_add(1);
        Some(make(n))
    }

    /// The continuity at every u knot (`GetUContinuitiesArray`).
    #[must_use]
    pub fn u_continuities(&self) -> &[ContinuityType] {
        &self.u_continuities
    }

    /// The continuity at every v knot (`GetVContinuitiesArray`).
    #[must_use]
    pub fn v_continuities(&self) -> &[ContinuityType] {
        &self.v_continuities
    }

    /// Sets the knot continuity arrays (filled by the tree construction).
    pub fn set_continuities(&mut self, u: Vec<ContinuityType>, v: Vec<ContinuityType>) {
        self.u_continuities = u;
        self.v_continuities = v;
    }

    /// The tolerance of the cached iso boundary curves (`m_dIsoCurveTolerance`).
    #[must_use]
    pub const fn iso_curve_tolerance(&self) -> f64 {
        self.iso_curve_tolerance
    }

    /// The boundary bounding planes of the cache domain
    /// (`GetPatchBoundaryBoundingPlanes`).
    pub fn patch_boundary_bounding_planes(&mut self) -> &mut PatchBoundaryBoundingPlanes {
        &mut self.bounding_planes
    }

    /// Whether local solvers also look on the boundary curves
    /// (`GetProcessBoundaryCurves`).
    #[must_use]
    pub const fn process_boundary_curves(&self) -> bool {
        self.process_boundary_curves
    }

    /// The number of patches (`GetPatchCount`).
    #[must_use]
    pub fn patch_count(&self) -> usize {
        self.patches.len()
    }

    /// The first generation patch at `(u_index, v_index)` of a `u_count x v_count`
    /// grid (`GetPatchAt`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index outside the grid or the patch list (upstream
    /// asserts, and grows the list).
    pub fn patch_at(
        &self,
        u_index: usize,
        v_index: usize,
        u_count: usize,
        v_count: usize,
    ) -> Result<&BezierPatch<O>> {
        if u_index >= u_count || v_index >= v_count {
            return Err(invalid(OP_PATCH, "index outside the grid"));
        }
        let index = u_index
            .checked_mul(v_count)
            .and_then(|i| i.checked_add(v_index))
            .ok_or_else(|| invalid(OP_PATCH, "index overflow"))?;
        self.patches
            .get(index)
            .ok_or_else(|| invalid(OP_PATCH, "index outside the patch list"))
    }

    /// The face owning the surface (`GetFace`, through `SmSurface::GetFace`).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::face`]; [`SolidError::Unported`] when a face-owned surface
    /// has no validated caller-handle binding, or the binding's owner became stale.
    pub fn face(&self) -> Result<Option<O>> {
        match self.surface.face()? {
            None if self.trimmed_face_key.is_none() => Ok(None),
            Some(key) if self.trimmed_face_key == Some(key) => {
                self.trimmed_face.map(Some).ok_or_else(|| {
                    SolidError::unported("mapped face of trimmed cache", "topology.face_p2")
                })
            }
            // An object key is not a typed arena index or a caller handle.
            // UNPORTED(topology.face_p2): SmSurface::GetFace (the SmFace handle of a
            // face-owned surface whose cache was made without its face). Reachable: a
            // solve on a B-rep face's surface without face context lands here (seen in
            // blend.fillet_geom_p6 on own-reader targets, which now pass the face's
            // B-rep through BrepFaceServices). Callers thread it with the
            // `*_with_context` variants or `get_surface_cache_with_face`.
            Some(_) if self.trimmed_face_key.is_none() => Err(SolidError::unported(
                "SmFace handle of a surface's face",
                "topology.face_p2",
            )),
            None | Some(_) => Err(invalid(OP_CLASSIFY, "surface cache face binding is stale")),
        }
    }

    /// Whether `uv_point` passes the trim boundary test (`PointTest`): always with
    /// point testing disabled, else when it lies in the cache domain for an
    /// untrimmed cache. Trimmed classification requires [`Self::point_test_with`].
    ///
    /// # Errors
    ///
    /// A trimmed cache with testing enabled cannot fall back to its rectangle:
    /// it reports a missing face or an unported operation-scoped classifier.
    pub fn point_test(&self, uv_point: Point2) -> Result<bool> {
        if !self.point_test_enabled || (self.trimmed_dispatch && self.face_was_modified) {
            return Ok(true);
        }
        if self.trimmed_dispatch {
            self.face()?.ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
            return Err(SolidError::unported(
                "prepared face classifier for bound trim",
                "query.face_p2",
            ));
        }
        Ok(self.uv_domain.contains_point(uv_point, 0.0))
    }

    /// Point test with trimmed-cache dispatch. Bound trimmed caches fail closed when
    /// the operation-scoped face-query service is absent.
    pub fn point_test_with(
        &self,
        k: &Kernel,
        uv_point: Point2,
        opt_tol: Option<f64>,
        queries: Option<&dyn crate::surface::surface_cache::face_context::FaceQueries<O>>,
    ) -> Result<(bool, Option<O>)> {
        if !self.trimmed_dispatch {
            return Ok((self.point_test(uv_point)?, None));
        }
        if !self.point_test_enabled || self.face_was_modified {
            return Ok((true, None));
        }
        let face = self.face()?.ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
        let service = queries.ok_or_else(|| {
            SolidError::unported("prepared face classifier for bound trim", "query.face_p2")
        })?;
        let tol = match opt_tol {
            Some(t) => t,
            None => service.face_zone_tol_3d(face)?,
        };
        let (class, object) = service.face_point_classify(k, face, uv_point, tol)?;
        Ok((matches!(class, 102..=104), object))
    }

    /// Classifies a point with the operation-scoped trimmed-cache override.
    /// Unlike point testing, classification does not skip disabled or modified caches.
    pub fn point_classify_with(
        &self,
        k: &Kernel,
        uv_point: Point2,
        src_zone_tol_3d: f64,
        queries: &dyn crate::surface::surface_cache::face_context::FaceQueries<O>,
    ) -> Result<(i32, Option<O>)> {
        if !self.trimmed_dispatch {
            return self.point_classify(uv_point, src_zone_tol_3d);
        }
        let face = self.face()?.ok_or(SolidError::null_pointer(OP_CLASSIFY))?;
        let (class, object) = queries.face_point_classify(k, face, uv_point, src_zone_tol_3d)?;
        Ok((class, if class == 100 { None } else { object }))
    }

    /// `PointClassify`: the base class has no trim boundaries to classify against.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR` (only the trimmed surface caches implement it).
    pub fn point_classify(
        &self,
        _uv_point: Point2,
        _src_zone_tol_3d: f64,
    ) -> Result<(i32, Option<O>)> {
        Err(SolidError::sm_err(OP_CLASSIFY))
    }

    /// `ImplantEdgeuse`: only the trimmed surface caches implant edge uses.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR`.
    pub fn implant_edgeuse(&mut self, _edgeuse: O, _do_subdivision: bool) -> Result<()> {
        Err(SolidError::sm_err(OP_IMPLANT_EU))
    }

    /// `ImplantPolyEdge`: only the trimmed surface caches implant poly edges.
    ///
    /// # Errors
    ///
    /// Always `SM_ERR`.
    pub fn implant_poly_edge(&mut self, _poly_edge: O, _node: NodeId) -> Result<()> {
        Err(SolidError::sm_err(OP_IMPLANT_PE))
    }

    /// The copies of the curves of the first alternate cache matching the parameters
    /// (`HasCachedCurves`); `None` when no cache matches. A failing copy (ignored
    /// upstream) leaves the lists copied before it.
    ///
    /// # Errors
    ///
    /// None in practice (the copy status is ignored).
    pub fn has_cached_curves(
        &self,
        cache_type: AltSrfCacheType,
        approx_tol: f64,
        angle_tol: f64,
        key: &AltCacheKey,
        want_3d: bool,
        want_uv: bool,
    ) -> Result<Option<CopyOfCurves>> {
        // for every AltCache
        for alt in &self.alt_caches {
            // when cache parameters match input values
            if alt.match_cache(cache_type, approx_tol, angle_tol, key) {
                // get copies of curves and return
                let (copies, _status) = alt.copy_of_curves_partial(want_3d, want_uv);
                return Ok(Some(copies));
            }
        }
        // didn't find a matching altCache
        Ok(None)
    }

    // ---- node data ----------------------------------------------------------------

    /// The tree, or an error (upstream dereferences a null `m_pTree`).
    fn tree_ref(&self, operation: &'static str) -> Result<&Tree<O>> {
        self.tree
            .as_ref()
            .ok_or_else(|| invalid(operation, "cache has no tree"))
    }

    /// The tree-held subdivision fields of `node`; `None` for a null `m_pData`.
    fn surface_fields(
        &self,
        node: NodeId,
        operation: &'static str,
    ) -> Result<Option<SurfaceAux<O>>> {
        match self.tree_ref(operation)?.node(node)?.data {
            NodeData::Surface(a) => Ok(Some(a)),
            NodeData::None => Ok(None),
            NodeData::ObjectList(_) | NodeData::Curve(_) => Err(invalid(
                operation,
                "node data is not surface subdivision data",
            )),
        }
    }

    /// The `SmBezierAux2d` of `node` (its patch's base for `SM_AD_BEZIER_SURFACE`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the node has no surface data in this cache.
    pub fn aux(&self, node: NodeId) -> Result<&BezierAux2d<O>> {
        let tree = self.tree_ref(OP_AUX)?;
        let n = tree.node(node)?;
        let NodeData::Surface(SurfaceAux {
            slot: Some(slot), ..
        }) = n.data
        else {
            return Err(invalid(OP_AUX, "node has no surface data in this cache"));
        };
        let found = if n.aux_type == AuxDataType::BezierSurface {
            self.patches.get(slot).map(|p| &p.aux)
        } else {
            self.auxs.get(slot)
        };
        found.ok_or_else(|| invalid(OP_AUX, "node data slot out of range"))
    }

    /// Mutable [`SurfaceCache::aux`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the node has no surface data in this cache.
    pub fn aux_mut(&mut self, node: NodeId) -> Result<&mut BezierAux2d<O>> {
        let tree = self.tree_ref(OP_AUX)?;
        let n = tree.node(node)?;
        let NodeData::Surface(SurfaceAux {
            slot: Some(slot), ..
        }) = n.data
        else {
            return Err(invalid(OP_AUX, "node has no surface data in this cache"));
        };
        let found = if n.aux_type == AuxDataType::BezierSurface {
            self.patches.get_mut(slot).map(|p| &mut p.aux)
        } else {
            self.auxs.get_mut(slot)
        };
        found.ok_or_else(|| invalid(OP_AUX, "node data slot out of range"))
    }

    /// The `SmBezierPatch` of an `SM_AD_BEZIER_SURFACE` node.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for other nodes.
    pub fn patch(&self, node: NodeId) -> Result<&BezierPatch<O>> {
        let tree = self.tree_ref(OP_AUX)?;
        let n = tree.node(node)?;
        match (n.aux_type, n.data) {
            (AuxDataType::BezierSurface, NodeData::Surface(SurfaceAux { slot: Some(s), .. })) => {
                self.patches
                    .get(s)
                    .ok_or_else(|| invalid(OP_AUX, "node data slot out of range"))
            }
            _ => Err(invalid(OP_AUX, "node is not a Bezier patch node")),
        }
    }

    /// The surface a node's `mBA_pSurface` handle refers to.
    ///
    /// # Errors
    ///
    /// `InvalidInput` without a handle or for a handle the cache does not hold.
    pub fn node_surface(&self, handle: Option<O>) -> Result<&Surface> {
        let h = handle.ok_or_else(|| invalid(OP_POLAR, "node has no surface"))?;
        if Some(h) == self.surface_handle {
            return Ok(&self.surface);
        }
        self.subdivision_surfaces
            .iter()
            .find(|(sh, _)| *sh == h)
            .map(|(_, s)| s)
            .ok_or_else(|| invalid(OP_POLAR, "node surface not held by the cache"))
    }

    /// The Bezier patch describing `node`'s shape, falling back to its ancestors
    /// (`SmTreeNode::GetFirstBezierPatch` answered from the cache's data): the
    /// construction Bezier of a patch node, else the NURB of the node's subdivision
    /// surface (of the owner surface only at the top); `None` for nodes that are not
    /// subdivision nodes or have no shape.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for nodes without surface data, unknown surface handles or
    /// cyclic parent links.
    pub fn first_bezier_patch(&self, node: NodeId) -> Result<Option<&NurbSurface>> {
        const OP: &str = "surface::surface_cache::p1::SurfaceCache::first_bezier_patch";
        let tree = self.tree_ref(OP)?;
        // upstream recurses into the parent; a loop with a cycle guard here
        let mut cur = node;
        for _ in 0..=tree.nodes().len() {
            let n = tree.node(cur)?;
            // check the AuxData Object for type SmBezier Patch
            if n.aux_type == AuxDataType::BezierSurface {
                if let Some(b) = self.patch(cur)?.bezier() {
                    return Ok(Some(b));
                }
            }
            // check AuxData object for type SmBezierAux2d
            if !n.aux_type.is_surface_subdivision() {
                return Ok(None);
            }
            let owner_surface = tree.owner_object()?;
            let fields = self
                .surface_fields(cur, OP)?
                .ok_or_else(|| invalid(OP, "node has no surface data"))?;
            // if the AuxData has a temporary Subdivision Surface pointer - use it
            if fields.surface.is_some() && (n.parent.is_none() || owner_surface != fields.surface) {
                // (upstream casts to SmBSplineSurface* unchecked)
                let bss = bspline_of(self.node_surface(fields.surface)?)
                    .ok_or_else(|| invalid(OP, "subdivision surface is not a B-spline"))?;
                return Ok(bss.gw_nurb());
            }
            // else try to get the parent's shape - skip the owner surface
            match n.parent {
                Some(p) if owner_surface != fields.surface => cur = p,
                _ => return Ok(None),
            }
        }
        Err(invalid(OP, "tree node links are cyclic"))
    }

    // ---- polar boxes --------------------------------------------------------------

    /// The polar box of `node`, built first when uninitialised
    /// (`SmBezierAux2d::GetPolarBox`; the build status is ignored).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a node without surface data; [`SolidError::Unported`] from
    /// the build.
    pub fn polar_box(&mut self, k: &Kernel, node: NodeId) -> Result<PolarBox> {
        if self.aux(node)?.polar_box.state() == PolarBoxState::Uninitialized {
            match self.build_polar_box(k, node) {
                Err(e) if e.is_unported() => return Err(e),
                Err(_) | Ok(()) => {}
            }
        }
        Ok(self.aux(node)?.polar_box)
    }

    /// Builds the polar boxes from the root, or from the nearest ancestor that has
    /// one, down to `node` (`SmBezierAux2d::BuildPolarBox`), each with its parent's
    /// box as basis guess (unless that one is unbounded).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for nodes without surface data or surface, cyclic parent
    /// links; [`SolidError::Unported`] from `CalculateBoundingBox`.
    pub fn build_polar_box(&mut self, k: &Kernel, node: NodeId) -> Result<()> {
        // collect chain of nodes up the tree, terminating in either the root
        // or something which has a polar box already
        let mut chain = vec![node];
        let mut found_polar = false;
        let mut cur = node;
        let limit = self.tree_ref(OP_POLAR)?.nodes().len();
        loop {
            if chain.len() > limit + 1 {
                return Err(invalid(OP_POLAR, "tree node links are cyclic"));
            }
            let parent = self.tree_ref(OP_POLAR)?.node(cur)?.parent;
            let Some(p) = parent else {
                break; // found the root
            };
            chain.push(p);
            if self.aux(p)?.polar_box.state() != PolarBoxState::Uninitialized {
                found_polar = true;
                break;
            }
            cur = p;
        }

        // now we work backwards through the list
        let last = chain
            .pop()
            .ok_or_else(|| invalid(OP_POLAR, "empty node chain"))?;
        let mut last_polar = if found_polar {
            self.aux(last)?.polar_box
        } else {
            // the root (can't have a box, else we would have stopped when we saw it)
            let b = self.compute_polar_box(k, last, None)?;
            if let Some(b) = b {
                self.aux_mut(last)?.polar_box = b;
            }
            self.aux(last)?.polar_box
        };

        // build boxes toward the start of the array (our starting node)
        while let Some(n) = chain.pop() {
            // this addresses a bunch of assert messages by not using a bad initial guess
            let guess = (last_polar.state() != PolarBoxState::Unbounded).then_some(last_polar);
            if let Some(b) = self.compute_polar_box(k, n, guess.as_ref())? {
                self.aux_mut(n)?.polar_box = b;
            }
            last_polar = self.aux(n)?.polar_box;
        }
        Ok(())
    }

    /// The polar box [`Self::polar_box`] returns for `node`, for callers that only
    /// borrow the cache: a missing box is built as [`Self::build_polar_box`] builds it
    /// (from the nearest ancestor with a box, or the root, down to `node`, each with
    /// its parent's box as guess) without storing the chain; the values are the
    /// same.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a node without surface data; [`SolidError::Unported`] from
    /// the build (its other failures leave the stored box, as [`Self::polar_box`]).
    pub fn polar_box_unstored(&self, k: &Kernel, node: NodeId) -> Result<PolarBox> {
        if self.aux(node)?.polar_box.state() == PolarBoxState::Uninitialized {
            match self.build_polar_box_unstored(k, node) {
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {}
                Ok(b) => return Ok(b),
            }
        }
        Ok(self.aux(node)?.polar_box)
    }

    /// The box [`Self::build_polar_box`] would store at `node`.
    fn build_polar_box_unstored(&self, k: &Kernel, node: NodeId) -> Result<PolarBox> {
        // collect chain of nodes up the tree, as build_polar_box
        let mut chain = vec![node];
        let mut found_polar = false;
        let mut cur = node;
        let limit = self.tree_ref(OP_POLAR)?.nodes().len();
        loop {
            if chain.len() > limit + 1 {
                return Err(invalid(OP_POLAR, "tree node links are cyclic"));
            }
            let parent = self.tree_ref(OP_POLAR)?.node(cur)?.parent;
            let Some(p) = parent else {
                break; // found the root
            };
            chain.push(p);
            if self.aux(p)?.polar_box.state() != PolarBoxState::Uninitialized {
                found_polar = true;
                break;
            }
            cur = p;
        }

        let last = chain
            .pop()
            .ok_or_else(|| invalid(OP_POLAR, "empty node chain"))?;
        let mut last_polar = if found_polar {
            self.aux(last)?.polar_box
        } else {
            // the root (can't have a box, else we would have stopped when we saw it)
            match self.compute_polar_box(k, last, None)? {
                Some(b) => b,
                None => self.aux(last)?.polar_box,
            }
        };

        // build boxes toward the start of the array (our starting node)
        while let Some(n) = chain.pop() {
            let guess = (last_polar.state() != PolarBoxState::Unbounded).then_some(last_polar);
            last_polar = match self.compute_polar_box(k, n, guess.as_ref())? {
                Some(b) => b,
                None => self.aux(n)?.polar_box,
            };
        }
        Ok(last_polar)
    }

    /// `mBA_pSurface->CalculateBoundingBox(m_sUVDomain, NULL, NULL, &m_sPolarBox,
    /// NULL, guess)`; `None` when the (ignored) status is a failure.
    fn compute_polar_box(
        &self,
        k: &Kernel,
        node: NodeId,
        guess: Option<&PolarBox>,
    ) -> Result<Option<PolarBox>> {
        let fields = self
            .surface_fields(node, OP_POLAR)?
            .ok_or_else(|| invalid(OP_POLAR, "node has no surface data"))?;
        let surface = self.node_surface(fields.surface)?;
        let domain = self.aux(node)?.uv_domain;
        let req = BoundingBoxRequest {
            normal: false,
            pseudo: false,
            polar: true,
            pseudo_guess: None,
            polar_guess: guess,
            expand_by_zone_tol: true,
            expand_tolerance: None,
        };
        let boxes = ignored(calculate_bounding_box(k, surface, &domain, &req))?;
        Ok(boxes.and_then(|b| b.polar))
    }

    // ---- tree queries -------------------------------------------------------------

    /// Pops nodes off `stack` until it finds an unmarked leaf of class `class`
    /// (`FindUnmarkedLeafNode`); unmarked parents push their children. Nodes
    /// without data are skipped. The stack is left in an unknown state.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a node with non-surface data or a missing node.
    pub fn find_unmarked_leaf_node(
        &self,
        class: NodeClass,
        stack: &mut Vec<NodeId>,
    ) -> Result<Option<NodeId>> {
        let tree = self.tree_ref(OP_FIND)?;
        // each entry of the stack expands to at most every node of an acyclic tree
        let visit_limit = stack
            .len()
            .saturating_add(1)
            .saturating_mul(tree.nodes().len().saturating_add(1));
        let mut visits = 0usize;
        // while nodes are to be processed
        while let Some(id) = stack.pop() {
            visits += 1;
            if visits > visit_limit {
                return Err(invalid(OP_FIND, "tree node links are cyclic"));
            }
            let node = tree.node(id)?;
            let aux = match node.data {
                NodeData::Surface(a) => a,
                // ERR_MSG "Null node"
                NodeData::None => continue,
                NodeData::ObjectList(_) | NodeData::Curve(_) => {
                    return Err(invalid(
                        OP_FIND,
                        "node data is not surface subdivision data",
                    ));
                }
            };

            // skip already searched nodes
            if aux.is_marked {
                continue;
            }

            // Add child Nodes to stack and continue. Check leafNodes for containment
            if let Some(c1) = node.child1 {
                stack.push(c1);
                if let Some(c2) = node.child2 {
                    stack.push(c2);
                }
            } else if aux.node_class == class {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// The lowest continuity at an internal knot of the surface inside `uv_domain`
    /// (`GetSurfaceContinuity`); `SM_CT_CINFINITY` without internal knots. Keeps
    /// upstream's v loop bound, which skips the last internal v knot.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a continuity array is longer than the knot list (upstream
    /// reads past it); [`SolidError::Unported`] from `GetKnots`.
    pub fn surface_continuity(&self, uv_domain: &Extent2d) -> Result<ContinuityType> {
        // init return
        let mut ret = ContinuityType::CInfinity;

        let u_domain = uv_domain.u_interval();
        let v_domain = uv_domain.v_interval();

        // fetch U knots and cached continuity array
        let mut knots: Vec<f64> = Vec::new();
        if let Some((k, _)) = ignored(self.surface.knots(SurfParamType::U, None))? {
            knots = k;
        }
        let u_conts = &self.u_continuities;

        // remember lowest internal U continuity value
        if !u_conts.is_empty() {
            let mut iu = 1;
            while iu + 1 < u_conts.len() {
                let c = *u_conts.get(iu).ok_or_else(|| invalid(OP_CONT, "index"))?;
                let knot = *knots
                    .get(iu)
                    .ok_or_else(|| invalid(OP_CONT, "continuity array longer than the knots"))?;
                if (c as i32) <= (ret as i32) && u_domain.contains_value(knot, 0.0) {
                    ret = c;
                }
                iu += 1;
            }
        }

        // fetch V knots and cached continuity array
        if let Some((k, _)) = ignored(self.surface.knots(SurfParamType::V, None))? {
            knots = k;
        }
        let v_conts = &self.v_continuities;

        // remember lowest internal V continuity value
        if !v_conts.is_empty() {
            let mut iv = 1;
            while iv + 1 < v_conts.len() - 1 {
                let c = *v_conts.get(iv).ok_or_else(|| invalid(OP_CONT, "index"))?;
                let knot = *knots
                    .get(iv)
                    .ok_or_else(|| invalid(OP_CONT, "continuity array longer than the knots"))?;
                if (c as i32) <= (ret as i32) && v_domain.contains_value(knot, 0.0) {
                    ret = c;
                }
                iv += 1;
            }
        }

        Ok(ret)
    }

    /// The unmarked leaves connected to the leaf `start` through shared sides that
    /// have its class, marking them (`GetConnectedRegion`), and the first marked
    /// neighbour classified inside or outside. Nothing for a node that is not a
    /// subdivision leaf.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for nodes with non-surface data, missing nodes or links;
    /// those of the neighbour search.
    pub fn connected_region(&mut self, start: NodeId) -> Result<ConnectedRegion> {
        let mut out = ConnectedRegion::default();
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| invalid(OP_REGION, "cache has no tree"))?;

        let start_node = *tree.node(start)?;

        // no work - Node is not from a surface spatial decomposition tree
        if !start_node.aux_type.is_surface_subdivision() {
            return Ok(out);
        }

        // no work - pStartLeafNode is not a leaf node
        if start_node.child1.is_some() || start_node.child2.is_some() {
            return Ok(out);
        }

        let node_class = match start_node.data {
            NodeData::Surface(a) => a.node_class,
            _ => {
                return Err(invalid(
                    OP_REGION,
                    "start node has no surface subdivision data",
                ));
            }
        };

        // start stack and output list with start node
        let mut stack = vec![start];
        out.nodes.push(start);

        // mark all nodes placed in the connectedNodes list
        set_marked(tree, start)?;

        // while stack continues to hold nodes
        while let Some(id) = stack.pop() {
            // get node's neighbors
            let neighbors = tree.neighbors(id)?;

            // for every neighbor node
            for nb in neighbors {
                let nb_aux = match tree.node(nb)?.data {
                    NodeData::Surface(a) => a,
                    // ERR_MSG "Null node"
                    NodeData::None => continue,
                    NodeData::ObjectList(_) | NodeData::Curve(_) => {
                        return Err(invalid(
                            OP_REGION,
                            "neighbour has no surface subdivision data",
                        ));
                    }
                };

                // skip marked nodes - save 1st INSIDE/OUTSIDE pointer
                if nb_aux.is_marked {
                    if out.classified_neighbor.is_none()
                        && (nb_aux.node_class == NodeClass::Inside
                            || nb_aux.node_class == NodeClass::Outside)
                    {
                        out.classified_neighbor = Some(nb);
                    }
                    continue;
                }

                // for nodes of the same type as the start node
                if nb_aux.node_class == node_class {
                    stack.push(nb);
                    out.nodes.push(nb);
                    set_marked(tree, nb)?;
                }
            }
        }
        Ok(out)
    }

    /// Classifies every unclassified leaf inside or outside the face
    /// (`MarkFaceContainment`): each connected region of unknown leaves takes the
    /// class (and face) of a classified neighbour, else that of `PointClassify` at
    /// the middle of its first node; classes and marks are propagated to the
    /// parents.
    ///
    /// # Errors
    ///
    /// `SM_ERR` from the base class `PointClassify` (a region without classified
    /// neighbour); `InvalidInput` for malformed trees; [`SolidError::Unported`] from
    /// `GetTree` (a tree build needs `SmSurface::CalculateContinuities`) and
    /// `SmTol::GetZoneTol3d` of a face.
    pub fn mark_face_containment(&mut self, k: &Kernel) -> Result<()> {
        self.mark_face_containment_with(k, &mut BaseClassifier)
    }

    /// [`SurfaceCache::mark_face_containment`] of a derived cache: `classifier`
    /// answers the virtual `PointClassify` and the zone tolerance of a node's face.
    ///
    /// # Errors
    ///
    /// Those of `classifier`; `InvalidInput` for malformed trees; those of `GetTree`.
    pub fn mark_face_containment_with(
        &mut self,
        k: &Kernel,
        classifier: &mut dyn FaceContainmentClassifier<O>,
    ) -> Result<()> {
        self.get_tree(k)?;
        let top = self
            .tree_ref(OP_MARK)?
            .top_node()
            .ok_or_else(|| invalid(OP_MARK, "tree has no top node"))?;
        let mut stack = vec![top];

        // find a 1st unMarked leafNode with classification == SM_NC_UNKNOWN
        let mut curr = self.find_unmarked_leaf_node(NodeClass::Unknown, &mut stack)?;

        // while unMarked Nodes remain with classification SM_NC_UNKNOWN
        while let Some(curr_node) = curr {
            let curr_fields = self
                .surface_fields(curr_node, OP_MARK)?
                .ok_or_else(|| invalid(OP_MARK, "node has no surface data"))?;
            let mut found_class = NodeClass::Unknown;
            let mut found_face: Option<O> = None;

            // get all nodes in connected region of SM_NC_UNKNOWN nodes
            let region = self.connected_region(curr_node)?;

            // get classification from the classified neighbor leaf node if possible
            if let Some(nb) = region.classified_neighbor {
                if let Some(a) = self.surface_fields(nb, OP_MARK)? {
                    found_class = a.node_class;
                    found_face = a.face;
                }
            }

            // when no classified neighbor was found - cast a ray
            if found_class == NodeClass::Unknown {
                let src_zone_tol_3d = match curr_fields.face {
                    Some(face) => classifier.zone_tol_3d(k, face)?,
                    None => zone_tol_3d(k, SmallTopoType::NormalSize),
                };

                let test_pt = self.aux(curr_node)?.uv_domain.evaluate(0.5, 0.5);
                let (classification, object_in_or_on) =
                    classifier.point_classify(k, test_pt, src_zone_tol_3d)?;

                // remember the Node is in or out of the Face (SM_PC_FACE)
                if classification == POINT_CLASS_FACE {
                    found_class = NodeClass::Inside;
                    found_face = object_in_or_on;
                } else {
                    found_class = NodeClass::Outside;
                }
            }

            // propagate this node's classification to all its connected nodes
            let tree = self
                .tree
                .as_mut()
                .ok_or_else(|| invalid(OP_MARK, "cache has no tree"))?;
            for &n in &region.nodes {
                match &mut tree.node_mut(n)?.data {
                    NodeData::Surface(a) => {
                        a.node_class = found_class;
                        a.face = found_face;
                    }
                    _ => return Err(invalid(OP_MARK, "node has no surface data")),
                }
                // propagate information up tree
                tree.propagate_to_parents(n)?;
            }

            // search for the next unmarked and unclassified node
            stack.clear();
            stack.push(top);
            curr = self.find_unmarked_leaf_node(NodeClass::Unknown, &mut stack)?;
        }
        Ok(())
    }

    /// Builds the top of the tree over the `u_count x v_count` first generation
    /// nodes `nodes` (v index fastest), pairing neighbours row- or column-wise
    /// (the shorter combined box first) into `SM_AD_BEZIER_SURFACE_HEAD` parents
    /// until one node is left, which becomes the top node (`BuildTreeTop`). The aux
    /// arena is reset first.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a missing tree, too few nodes, or nodes without surface
    /// data.
    pub fn build_tree_top(
        &mut self,
        nodes: &[NodeId],
        u_count: usize,
        v_count: usize,
    ) -> Result<()> {
        let mut num_u = u_count;
        let mut num_v = v_count;

        // init aux memory manager
        self.auxs.clear();

        let mut curr: Vec<NodeId> = nodes.to_vec();
        let mut next: Vec<NodeId> = Vec::with_capacity(curr.len());
        let at = |v: &[NodeId], i: usize| -> Result<NodeId> {
            v.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_TOP, "fewer nodes than the grid"))
        };

        // while current generation has more than 1 node
        while num_u > 1 || num_v > 1 {
            let mut combine = SplitDir::U;
            if num_u == 1 {
                combine = SplitDir::V;
            } else if num_v != 1 {
                // more than 1 U and more than 1 V nodes: combine the shorter way
                let tree = self.tree_ref(OP_TOP)?;
                let n00 = tree.node(at(&curr, 0)?)?.bbox;
                let n01 = tree.node(at(&curr, 1)?)?.bbox;
                let n10 = tree.node(at(&curr, num_v)?)?.bbox;
                let ext_v = n00.union(&n01);
                let ext_u = n00.union(&n10);
                let diff_u = ext_u.max() - ext_u.min();
                let diff_v = ext_v.max() - ext_v.min();
                if diff_v.length_squared() < diff_u.length_squared() {
                    combine = SplitDir::V;
                }
            }

            if combine == SplitDir::V {
                // Combine along rows of V: this combination shrinks the rows
                for ii in 0..num_u {
                    let mut kk = 1;
                    while kk < num_v {
                        let n1 = at(&curr, ii * num_v + kk - 1)?;
                        let n2 = at(&curr, ii * num_v + kk)?;
                        next.push(self.combine_nodes(n1, n2, SplitDir::V)?);
                        kk += 2;
                    }
                    if num_v % 2 == 1 {
                        // Add on last element if had odd number
                        next.push(at(&curr, ii * num_v + num_v - 1)?);
                    }
                }
                num_v = num_v.div_ceil(2);
            } else {
                // Combine in V direction
                let mut ii = 1;
                while ii < num_u {
                    for kk in 0..num_v {
                        let n1 = at(&curr, (ii - 1) * num_v + kk)?;
                        let n2 = at(&curr, ii * num_v + kk)?;
                        next.push(self.combine_nodes(n1, n2, SplitDir::U)?);
                    }
                    ii += 2;
                }
                // If have odd number of rows add on last one
                if num_u % 2 == 1 {
                    for kk in 0..num_v {
                        next.push(at(&curr, (num_u - 1) * num_v + kk)?);
                    }
                }
                num_u = num_u.div_ceil(2);
            }

            // Swap arrays
            std::mem::swap(&mut curr, &mut next);
            next.clear();
        }

        let top = at(&curr, 0)?;
        self.tree
            .as_mut()
            .ok_or_else(|| invalid(OP_TOP, "cache has no tree"))?
            .set_top_node(Some(top));
        Ok(())
    }

    /// A new `SM_AD_BEZIER_SURFACE_HEAD` parent of `n1` and `n2` split in `split`.
    fn combine_nodes(&mut self, n1: NodeId, n2: NodeId, split: SplitDir) -> Result<NodeId> {
        let uv = {
            let a1 = self.aux(n1)?;
            let a2 = self.aux(n2)?;
            a1.uv_domain.union(&a2.uv_domain)
        };
        let aux_slot = self.auxs.len();
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| invalid(OP_TOP, "cache has no tree"))?;
        let b1 = tree.node(n1)?.bbox;
        let b2 = tree.node(n2)?.bbox;
        let new_id = tree.add_node(TreeNode {
            parent: None,
            child1: Some(n1),
            child2: Some(n2),
            bbox: b1.union(&b2),
            aux_type: AuxDataType::BezierSurfaceHead,
            data: NodeData::Surface(SurfaceAux {
                split_dir: split,
                is_marked: false,
                node_class: NodeClass::Unknown,
                face: None,
                surface: None,
                slot: Some(aux_slot),
            }),
            geom_type: NodeGeomType::Default,
        });
        tree.node_mut(n1)?.parent = Some(new_id);
        tree.node_mut(n2)?.parent = Some(new_id);
        self.auxs.push(BezierAux2d {
            uv_domain: uv,
            owning_tree_node: Some(new_id),
            ..BezierAux2d::default()
        });
        Ok(new_id)
    }

    // ---- iso curves and sizes -----------------------------------------------------

    /// The four iso-parametric boundary curves of `uv_domain` within `approx_tol_3d`
    /// of the surface (`GetIsoBoundaryCurves`): the cached ones when `uv_domain` is
    /// the cache domain and they were made with a tolerance no larger; else new
    /// ones, cached when `uv_domain` is the cache domain.
    ///
    /// # Errors
    ///
    /// Those of `CreateIsoParametricCurve`.
    pub fn iso_boundary_curves(
        &mut self,
        k: &Kernel,
        uv_domain: &Extent2d,
        approx_tol_3d: f64,
    ) -> Result<IsoBoundaryCurves> {
        // See if crUVDomain matches the domain of the cache
        let diff_min = uv_domain.min() - self.uv_domain.min();
        let diff_max = uv_domain.max() - self.uv_domain.max();
        let same_domain = !(diff_min.length_squared() > EFF_ZERO_SQ
            || diff_max.length_squared() > EFF_ZERO_SQ
            || approx_tol_3d < self.iso_curve_tolerance);

        // If the domains match and there are cached boundary curves use them
        if same_domain {
            if let Some(c) = &self.iso_boundary_curves {
                return Ok(IsoBoundaryCurves {
                    cached: true,
                    curves: c.clone(),
                });
            }
        }

        // remember the ApproxTol3d used to Create these IsoParamCurves
        self.iso_curve_tolerance = approx_tol_3d;

        // build isoParameter curves
        let d = Some(*uv_domain);
        let c0 = create_iso_parametric_curve(
            k,
            &self.surface,
            SurfParamType::U,
            uv_domain.min().x,
            approx_tol_3d,
            d,
        )?;
        let c1 = create_iso_parametric_curve(
            k,
            &self.surface,
            SurfParamType::V,
            uv_domain.min().y,
            approx_tol_3d,
            d,
        )?;
        let c2 = create_iso_parametric_curve(
            k,
            &self.surface,
            SurfParamType::U,
            uv_domain.max().x,
            approx_tol_3d,
            d,
        )?;
        let c3 = create_iso_parametric_curve(
            k,
            &self.surface,
            SurfParamType::V,
            uv_domain.max().y,
            approx_tol_3d,
            d,
        )?;
        let curves = [c0, c1, c2, c3];

        // when domain is same as cache domain - cache curves
        if same_domain {
            self.iso_boundary_curves = Some(curves.clone());
        }
        Ok(IsoBoundaryCurves {
            cached: same_domain,
            curves,
        })
    }

    /// Side lengths and aspect ratios of the quad with corners `u0v0`, `u1v0`,
    /// `u1v1`, `u0v1` (`ComputeSizeTolerances`, corner point version).
    #[must_use]
    pub fn size_tolerances_from_corners(
        u0v0: Point3,
        u1v0: Point3,
        u1v1: Point3,
        u0v1: Point3,
    ) -> SizeTolerances {
        let bot_dist = (u0v0 - u1v0).length();
        let top_dist = (u0v1 - u1v1).length();
        let left_dist = (u0v0 - u0v1).length();
        let right_dist = (u1v0 - u1v1).length();

        let max_u = max2(bot_dist, top_dist);
        let max_v = max2(left_dist, right_dist);
        let min_u = min2(bot_dist, top_dist);
        let min_v = min2(left_dist, right_dist);

        // Scaled Zero
        let scaled_zero = EFF_ZERO * (1.0 + u0v0.max_dimension() + u1v1.max_dimension());

        size_output(max_u, max_v, min_u, min_v, scaled_zero)
    }

    /// Side lengths and aspect ratios of `surface` over `domain` from the lengths of
    /// its iso curves at the low, middle and high parameters
    /// (`ComputeSizeTolerances`, surface version).
    ///
    /// # Errors
    ///
    /// Those of `CreateIsoParametricCurve`; [`SolidError::Unported`] from the curve
    /// evaluation.
    pub fn size_tolerances_from_surface(
        k: &Kernel,
        surface: &Surface,
        domain: &Extent2d,
    ) -> Result<SizeTolerances> {
        let u_domain = domain.u_interval();
        let v_domain = domain.v_interval();

        // UVPoints
        let uv_bot = domain.evaluate(0.0, 0.0);
        let uv_mid = domain.evaluate(0.5, 0.5);
        let uv_top = domain.evaluate(1.0, 1.0);

        // IsoParamCurves
        let d = Some(*domain);
        let bot_u = create_iso_parametric_curve(k, surface, SurfParamType::U, uv_bot.x, 0.0, d)?;
        let bot_v = create_iso_parametric_curve(k, surface, SurfParamType::V, uv_bot.y, 0.0, d)?;
        let mid_u = create_iso_parametric_curve(k, surface, SurfParamType::U, uv_mid.x, 0.0, d)?;
        let mid_v = create_iso_parametric_curve(k, surface, SurfParamType::V, uv_mid.y, 0.0, d)?;
        let top_u = create_iso_parametric_curve(k, surface, SurfParamType::U, uv_top.x, 0.0, d)?;
        let top_v = create_iso_parametric_curve(k, surface, SurfParamType::V, uv_top.y, 0.0, d)?;

        // 3D Udir and Vdir IsoParamCurve lengths
        let len = |c: &Curve, ivl: &Extent1d| -> Result<f64> {
            Ok(c.approximate_length(k, ivl, 8, false)?.0)
        };
        let bot_v_size = len(&bot_u, &v_domain)?;
        let bot_u_size = len(&bot_v, &u_domain)?;
        let mid_v_size = len(&mid_u, &v_domain)?;
        let mid_u_size = len(&mid_v, &u_domain)?;
        let top_v_size = len(&top_u, &v_domain)?;
        let top_u_size = len(&top_v, &u_domain)?;

        // Max / Min IsoParamCurve lengths
        let max_u = max3(bot_u_size, mid_u_size, top_u_size);
        let max_v = max3(bot_v_size, mid_v_size, top_v_size);
        let min_u = min3(bot_u_size, mid_u_size, top_u_size);
        let min_v = min3(bot_v_size, mid_v_size, top_v_size);

        // Scaled Zero
        let scaled_zero = EFF_ZERO * (1.0 + max_u + max_v);

        Ok(size_output(max_u, max_v, min_u, min_v, scaled_zero))
    }

    /// The corner points of a patch (`GetCorners`): of the Bezier `bezier` when given
    /// (its net corners and knot domain), else evaluated on the cached surface at the
    /// corners of `patch_domain`.
    ///
    /// # Errors
    ///
    /// Those of `EvaluatePoint`; `InvalidInput` for an empty Bezier net.
    pub fn corners(
        &self,
        k: &Kernel,
        bezier: Option<&NurbSurface>,
        patch_domain: &Extent2d,
    ) -> Result<Corners> {
        if let Some(sur) = bezier {
            let (n, m) = (sur.n(), sur.m());
            let u = sur.u_knots();
            let v = sur.v_knots();
            let mut domain = Extent2d::new();
            let ok = match (u.first(), v.first(), u.last(), v.last()) {
                (Some(&u0), Some(&v0), Some(&u1), Some(&v1)) => {
                    domain.set_min_max_coords(u0, v0, u1, v1).is_ok()
                }
                _ => false,
            };
            Ok(Corners {
                u0v0: to_euclid(sur.cpt(0, 0)?),
                u1v0: to_euclid(sur.cpt(n, 0)?),
                u1v1: to_euclid(sur.cpt(n, m)?),
                u0v1: to_euclid(sur.cpt(0, m)?),
                domain: ok.then_some(domain),
            })
        } else {
            // no gw_SURFACE available
            let u0v0 = self
                .surface
                .evaluate_point(k, patch_domain.evaluate(0.0, 0.0))?;
            let u1v0 = self
                .surface
                .evaluate_point(k, patch_domain.evaluate(1.0, 0.0))?;
            let u0v1 = self
                .surface
                .evaluate_point(k, patch_domain.evaluate(0.0, 1.0))?;
            let u1v1 = self
                .surface
                .evaluate_point(k, patch_domain.evaluate(1.0, 1.0))?;
            Ok(Corners {
                u0v0,
                u1v0,
                u1v1,
                u0v1,
                domain: Some(*patch_domain),
            })
        }
    }
}

/// `SM_PC_FACE` (`SmPointClassificationType`).
pub const POINT_CLASS_FACE: i32 = 102;

/// The shared tail of the `ComputeSizeTolerances` versions.
fn size_output(max_u: f64, max_v: f64, min_u: f64, min_v: f64, scaled_zero: f64) -> SizeTolerances {
    let mut min_u = min_u;
    let mut min_v = min_v;
    // snap MinU and MinV values to MaxV when less than dTol
    if min_u < scaled_zero {
        min_u = max_u;
    }
    if min_v < scaled_zero {
        min_v = max_v;
    }
    SizeTolerances {
        aspect_ratio: Vec2::new(
            if max_v > scaled_zero {
                max_u / max_v
            } else {
                0.5
            },
            if max_u > scaled_zero {
                max_v / max_u
            } else {
                0.5
            },
        ),
        max_side_length_3d: Vec2::new(max_u, max_v),
        min_side_length_3d: Vec2::new(min_u, min_v),
    }
}

/// Sets the mark of a subdivision node.
fn set_marked<O>(tree: &mut Tree<O>, node: NodeId) -> Result<()>
where
    O: Copy + PartialEq,
{
    match &mut tree.node_mut(node)?.data {
        NodeData::Surface(a) => {
            a.is_marked = true;
            Ok(())
        }
        _ => Err(invalid(OP_REGION, "node has no surface subdivision data")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent3d::Extent3d;

    #[test]
    fn plane_boundary_planes() {
        use crate::surface::plane::Plane;
        let k = Kernel::default();
        let dom = Extent2d::from_bounds(0.0, 0.0, 10.0, 6.0);
        let surface = Surface::from(Plane::new(
            Vec3::new(-5.0, -3.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &dom,
        ));
        let mut bp = PatchBoundaryBoundingPlanes::new(dom);
        // the iso curve of a plane is its SmLine
        let c = bp.create_boundary_curve_for_index(&k, &surface, 0).unwrap();
        assert!(matches!(c.kind(), CurveKind::Line(_)));
        bp.compute_bounding_planes(&k, &surface).unwrap();
    }

    #[test]
    fn cull_keeps_first_and_last() {
        let mut a = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        cull_array(&mut a);
        assert_eq!(a, vec![0.0, 2.0, 4.0]);
        let mut b = vec![0.0, 1.0, 2.0, 3.0];
        cull_array(&mut b);
        assert_eq!(b, vec![0.0, 2.0, 3.0]);
        let mut c: Vec<f64> = vec![];
        cull_array(&mut c);
        assert_eq!(c, vec![0.0]);
        let mut d = vec![5.0];
        cull_array(&mut d);
        assert_eq!(d, vec![5.0]);
    }

    #[test]
    fn plane_from_points() {
        let p = compute_plane_from_3_points(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1e-4,
        )
        .unwrap();
        assert!(p.plane_exists && !p.collinear);
        let n = p.normal.unwrap();
        assert!((n.z.abs() - 1.0).abs() < 1e-12);
        let c = compute_plane_from_3_points(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            1e-4,
        )
        .unwrap();
        assert!(c.collinear && !c.plane_exists);
    }

    #[test]
    fn alt_cache_matching() {
        let key = AltCacheKey {
            vec: Some(Vec3::new(1.0, 2.0, 3.0)),
            long: Some(7),
            ..AltCacheKey::default()
        };
        let c = AltSrfCache::new(None, None, AltSrfCacheType::Silhouette, 0.1, 0.2, &key);
        assert!(c.match_cache(AltSrfCacheType::Silhouette, 0.1, 0.2, &key));
        assert!(c.match_cache(
            AltSrfCacheType::Silhouette,
            0.1,
            0.2,
            &AltCacheKey::default()
        ));
        assert!(!c.match_cache(AltSrfCacheType::TrimmedSilhouette, 0.1, 0.2, &key));
        let other = AltCacheKey {
            long: Some(8),
            ..key
        };
        assert!(!c.match_cache(AltSrfCacheType::Silhouette, 0.1, 0.2, &other));
        assert!(c.copy_of_curves(true, false).is_err());
        assert!(c.copy_of_curves(false, false).is_ok());
    }

    /// One `PoleMemo` reused over many sub-domains gives the Bezier approximations
    /// of the lazy singularity test, on a surface with a pole (a degenerate v-min
    /// side) and without one.
    #[test]
    fn pole_memo_matches_lazy_singularity_test() {
        use crate::nurbs::math::CPoint;
        use crate::nurbs::nurbs_srf::allocate_nurb_surface;
        let k = Kernel::default();
        let net = |pole: bool| {
            (0..3_i32)
                .map(|i| {
                    (0..3_i32)
                        .map(|j| {
                            let (x, y) = if pole && j == 0 {
                                (0.0, 0.0)
                            } else {
                                (f64::from(i) - 1.0, f64::from(j))
                            };
                            CPoint::new(x, y, 0.1 * f64::from(i * j), NOW)
                        })
                        .collect()
                })
                .collect()
        };
        for pole in [true, false] {
            let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            let nurb = NurbSurface::new(net(pole), 2, 2, knots.clone(), knots).unwrap();
            let surface = Surface::from(BSplineSurface::from_nurb(nurb, false));
            let mut memo = PoleMemo::default();
            for (u0, v0, u1, v1) in [
                (0.0, 0.0, 1.0, 1.0),
                (0.0, 0.0, 0.5, 0.5),
                (0.5, 0.0, 1.0, 0.25),
                (0.25, 0.5, 0.75, 1.0),
                (0.0, 0.75, 0.125, 1.0),
            ] {
                let dom = Extent2d::from_bounds(u0, v0, u1, v1);
                let mut lazy = allocate_nurb_surface(3, 3, 3, 3, 7, 7).unwrap();
                let mut memoised = allocate_nurb_surface(3, 3, 3, 3, 7, 7).unwrap();
                let r1 = compute_bezier_approximation(&k, &surface, &dom, &mut lazy);
                let r2 = compute_bezier_approximation_memo(
                    &k,
                    &surface,
                    &dom,
                    &mut memoised,
                    Some(&mut memo),
                );
                assert_eq!(format!("{r1:?}"), format!("{r2:?}"));
                assert_eq!(format!("{lazy:?}"), format!("{memoised:?}"));
            }
            assert!(matches!(memo, PoleMemo::Poles(sides, _) if (sides != SS_NONE) == pole));
        }
    }

    fn bilinear_cache() -> SurfaceCache<i64> {
        use crate::nurbs::math::CPoint;
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ];
        let nurb = NurbSurface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let surface = Surface::from(BSplineSurface::from_nurb(nurb, false));
        let d = surface.natural_uv_domain_if_built();
        let mut c = SurfaceCache::new(surface, d, SurfaceCacheOptions::default());
        c.set_surface_handle(Some(-1));
        c
    }

    /// A 2x2 grid of first generation patches combined into a tree.
    #[test]
    fn tree_top_and_classification() {
        let mut c = bilinear_cache();
        let mut tree: Tree<i64> = Tree::new(Some(crate::geom::tree::TreeOwner::SurfaceCache));
        let mut first = Vec::new();
        for (k, (u, v)) in [(0.0, 0.0), (0.0, 0.5), (0.5, 0.0), (0.5, 0.5)]
            .into_iter()
            .enumerate()
        {
            let id = tree.add_node(TreeNode {
                parent: None,
                child1: None,
                child2: None,
                bbox: Extent3d::from_bounds(u, v, 0.0, u + 0.5, v + 0.5, 0.0),
                aux_type: AuxDataType::BezierSurface,
                data: NodeData::Surface(SurfaceAux {
                    split_dir: SplitDir::Unknown,
                    is_marked: k == 0,
                    node_class: if k == 0 {
                        NodeClass::Inside
                    } else {
                        NodeClass::Unknown
                    },
                    face: (k == 0).then_some(4),
                    surface: Some(-1),
                    slot: Some(k),
                }),
                geom_type: NodeGeomType::Default,
            });
            c.patches_mut().push(BezierPatch {
                aux: BezierAux2d {
                    uv_domain: Extent2d::from_bounds(u, v, u + 0.5, v + 0.5),
                    owning_tree_node: Some(id),
                    ..BezierAux2d::default()
                },
                ..BezierPatch::default()
            });
            first.push(id);
        }
        c.set_tree(Some(tree));
        c.build_tree_top(&first, 2, 2).unwrap();
        let t = c.tree().unwrap();
        assert_eq!(t.nodes().len(), 7);
        let top = t.top_node().unwrap();
        assert_eq!(
            c.aux(top).unwrap().uv_domain,
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
        );
        assert_eq!(t.owner_object().unwrap(), Some(-1));
        // the top's surface is null: no shape anywhere
        assert!(c.first_bezier_patch(first[0]).unwrap().is_none());

        // every unknown leaf is reached from the inside one
        let k = Kernel::default();
        c.mark_face_containment(&k).unwrap();
        for id in &first {
            match c.tree().unwrap().node(*id).unwrap().data {
                NodeData::Surface(a) => {
                    assert_eq!(a.node_class, NodeClass::Inside);
                    assert_eq!(a.face, Some(4));
                    assert!(a.is_marked);
                }
                _ => panic!("no data"),
            }
        }
        assert_eq!(
            c.patch_at(1, 1, 2, 2).unwrap().aux.owning_tree_node,
            Some(first[3])
        );
        assert!(c.patch_at(2, 0, 2, 2).is_err());
    }

    #[test]
    fn base_class_queries() {
        let mut c = bilinear_cache();
        assert!(c.point_test(Point2::new(5.0, 5.0)).unwrap());
        c.point_test_enabled = true;
        assert!(!c.point_test(Point2::new(5.0, 5.0)).unwrap());
        assert!(c.point_classify(Point2::new(0.5, 0.5), 0.0).is_err());
        assert_eq!(c.face().unwrap(), None);
        assert!(c.is_kind_of(TYPE) && c.is_kind_of(object::OBJECT_TYPE) && !c.is_kind_of(15001));
        let k = Kernel::default();
        let d = c.uv_domain();
        let first = c.iso_boundary_curves(&k, &d, 0.001).unwrap();
        assert!(first.cached);
        let again = c.iso_boundary_curves(&k, &d, 0.01).unwrap();
        assert!(again.cached);
        let finer = c.iso_boundary_curves(&k, &d, 0.0001).unwrap();
        assert!(!finer.cached);
        assert_eq!(c.iso_curve_tolerance(), 0.0001);
    }

    #[test]
    fn iso_boundary_curves_forward_approximation_tolerance() {
        use crate::nurbs::math::CPoint;
        use crate::surface::offset_surface::OffsetSurface;

        let k = Kernel::default();
        let net = (0..3_i32)
            .map(|i| {
                (0..3_i32)
                    .map(|j| {
                        CPoint::new(
                            f64::from(i),
                            f64::from(j),
                            0.5 * f64::from(i * i)
                                + 0.3 * f64::from(j * j)
                                + 0.2 * f64::from(i * j),
                            NOW,
                        )
                    })
                    .collect()
            })
            .collect();
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let nurb = NurbSurface::new(net, 2, 2, knots.clone(), knots).unwrap();
        let base = Surface::from(BSplineSurface::from_nurb(nurb, false));
        // An offset uses the approximating base implementation, not the exact
        // B-spline or plane iso-curve overrides.
        let surface = Surface::from(
            OffsetSurface::new(&k, 0.4, base, true, crate::geom::extent2d::SS_NONE).unwrap(),
        );
        let domain = Extent2d::from_bounds(0.1, 0.15, 0.85, 0.9);
        let boundaries = [
            (SurfParamType::U, domain.min().x),
            (SurfParamType::V, domain.min().y),
            (SurfParamType::U, domain.max().x),
            (SurfParamType::V, domain.max().y),
        ];
        let curve_data = |curve: &Curve| {
            let spline = bspline_curve_of(curve).unwrap();
            (
                spline.control_points().unwrap().to_vec(),
                spline.knots(None).unwrap(),
            )
        };
        let direct = |tol| {
            boundaries.map(|(param, iso)| {
                let curve = surface
                    .create_iso_parametric_curve(&k, param, iso, tol, Some(&domain), false)
                    .unwrap()
                    .curve;
                curve_data(&curve)
            })
        };
        let automatic = direct(0.0);
        let tight = direct(1e-9);
        let loose = direct(1e-5);
        for i in 0..4 {
            assert_ne!(tight[i], loose[i], "boundary {i} must depend on tolerance");
            assert_ne!(
                tight[i], automatic[i],
                "tight boundary {i} must differ from default"
            );
            assert_ne!(
                loose[i], automatic[i],
                "loose boundary {i} must differ from default"
            );
        }
        for (tol, expected) in [(1e-9, tight), (1e-5, loose)] {
            // Fresh caches prevent reuse of a tighter curve for a looser request.
            let mut cache =
                SurfaceCache::<u32>::new(surface.clone(), domain, SurfaceCacheOptions::default());
            let actual = cache.iso_boundary_curves(&k, &domain, tol).unwrap();
            for (i, curve) in actual.curves.iter().enumerate() {
                assert_eq!(
                    curve_data(curve),
                    expected[i],
                    "boundary {i}, tolerance {tol}"
                );
            }
        }
    }

    #[test]
    fn size_tolerances() {
        let s = SurfaceCache::<u32>::size_tolerances_from_corners(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert_eq!(s.max_side_length_3d, Vec2::new(2.0, 1.0));
        assert_eq!(s.aspect_ratio, Vec2::new(2.0, 0.5));
        let o = Vec3::new(0.0, 0.0, 0.0);
        let z = SurfaceCache::<u32>::size_tolerances_from_corners(o, o, o, o);
        assert_eq!(z.aspect_ratio, Vec2::new(0.5, 0.5));
    }

    /// A node box of an offset surface with basis guesses (the polar box chain of
    /// `BuildPolarBox` passes the parent's box) runs `SmOffsetSurface::
    /// CalculateBoundingBox`, which hands the guesses to its base surface: the polar
    /// box is the base's, computed with the same guess.
    #[test]
    fn offset_node_boxes_pass_the_basis_guesses_to_the_base() {
        use crate::geom::axis2_placement::Axis2Placement;
        use crate::nurbs::b_spline_surface::BoundingBoxRequest;
        use crate::surface::offset_surface::OffsetSurface;
        use crate::surface::torus::{Torus, TorusOptions};
        let k = Kernel::default();
        let torus = Torus::create_canonical(
            &k,
            &Axis2Placement::IDENTITY,
            3.0,
            1.0,
            TorusOptions::default(),
            None,
        )
        .unwrap();
        let base = Surface::from(torus);
        let natural = base.natural_uv_domain_with_nurb(&k).unwrap();
        let sub = |a: f64, b: f64, c: f64, d: f64| {
            Extent2d::from_points(natural.evaluate(a, b), natural.evaluate(c, d))
        };
        let wide = sub(0.1, 0.1, 0.6, 0.6);
        let node = sub(0.2, 0.25, 0.35, 0.4);
        let guess = base
            .calculate_bounding_box(&k, &wide, [false, false, true], true)
            .unwrap()
            .polar_box
            .unwrap();
        let offset = Surface::from(
            OffsetSurface::new(&k, 0.5, base.clone(), true, crate::geom::extent2d::SS_NONE)
                .unwrap(),
        );
        let req = BoundingBoxRequest {
            normal: false,
            pseudo: false,
            polar: true,
            pseudo_guess: None,
            polar_guess: Some(&guess),
            expand_by_zone_tol: true,
            expand_tolerance: None,
        };
        let boxes = calculate_bounding_box(&k, &offset, &node, &req).unwrap();
        let from_base = base
            .calculate_bounding_box_with_guesses(
                &k,
                &node,
                [true, true, true],
                true,
                None,
                Some(&guess),
            )
            .unwrap();
        assert_eq!(boxes.polar, from_base.polar_box);
    }
}

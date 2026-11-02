//! `SmSurfaceCache`, part 2: the n-th boundary bounding plane of a patch, adding
//! alternate curve caches, the tolerance test of a subdivision node, the block
//! subdivision of a node along tessellation splits and the two Bezier patch block
//! builders (exact decomposition of a B-spline, bicubic approximation of any surface).
//!
//! Upstream: `source/SMLib/src/SmSurfaceCache.cpp`.
//!
//! | Rust                                                   | Upstream                                         |
//! |--------------------------------------------------------|--------------------------------------------------|
//! | [`PatchBoundaryBoundingPlanes::nth_bounding_plane`], [`NthBoundingPlane`], [`BoundingPlane`] | `SmPatchBoundaryBoundingPlanes::GetNthBoundingPlane` |
//! | [`SurfaceCache::add_curves_to_cache`]                  | `SmSurfaceCache::AddCurvesToCache`               |
//! | [`SurfaceCache::test_against_tolerances`], [`ToleranceTest`] | `SmSurfaceCache::TestAgainstTolerances`    |
//! | [`SurfaceCache::subdivide_by_block`]                   | `SmSurfaceCache::SubdivideByBlock`               |
//! | [`create_bezier_patch_block`], [`BezierPatchBlock`]    | `sm_CreateBezierPatchBlock` (static)             |
//! | [`create_approximate_bezier_patch_block`], [`ApproximateBezierPatchBlock`] | `sm_CreateApproximateBezierPatchBlock` (static) |
//! | `Drop` (implicit)                                      | `SmSurfaceCache::~SmSurfaceCache`                |
//!
//! Deviations:
//! - **Out-params** become return values. `GetNthBoundingPlane` only writes the plane
//!   outputs when the plane exists: [`NthBoundingPlane::plane`] is `None` otherwise.
//!   The sample point array is copied instead of referenced.
//! - **Memory blocks.** The Bezier patch builders allocate their patches in
//!   `SmMemBlockMgr` blocks and return pointers into them; here they return the
//!   patches ([`crate::nurbs::srf_basic::Surface`]) in upstream's flat order
//!   `[patch00, patch01, .. patch0N, patch10, .. patchMN]`. The block sizing
//!   (`sm_ComputeNurbSurfaceSize`, `lNumPerBlock`) has no counterpart.
//! - **Tree parameter.** `SubdivideByBlock(pTree, pNode)` is only called with the
//!   cache's own tree; [`SurfaceCache::subdivide_by_block`] works on it. New nodes and
//!   their [`BezierAux2d`] data come from zero-filled block memory upstream: their
//!   boxes start as all-zero boxes (not "inside out"), as upstream.
//! - **Ignored statuses** stay ignored (`sm_ComputeBezierApproximation` in the
//!   approximate block builder, `N_SrfDerivs` in the tolerance test, `CopySubPatch` and
//!   `CalculateBoundingBox` in the block subdivision), except
//!   [`SolidError::Unported`], which is always propagated. Where an ignored
//!   `N_SrfDerivs` failure would leave upstream reading uninitialised vectors the port
//!   returns [`SolidError::InvalidInput`].
//! - **Undefined behaviour.** Where upstream would read through a null or mistyped
//!   `m_pData`, a null Bezier pointer, a NURB-less B-spline or a bounding plane index
//!   above 3, the port returns [`SolidError::InvalidInput`].
//! - **Approximation angle arithmetic.** For a surface tested by evaluations rather
//!   than an exact control net, an angle walk can divide a finite segment by an
//!   overflowed length, producing a zero direction and spurious turning angles.
//!   If refinement is still requested after the upstream minimum-UV-size checks,
//!   the port returns `InvalidInput` instead of growing an approximation tree from
//!   that invalid measurement. Passing leaves, exact control-net tests, infinite
//!   chord tolerances alone, and custom testers that do not call this test are
//!   unchanged. No coordinate or box-size cutoff is used.
//! - **Virtual dispatch.** `SmSurface::FindTessellationSplits` (unit
//!   `surface.surface_p9`) is not ported: [`SurfaceCache::subdivide_by_block`] returns
//!   [`SolidError::Unported`] once it reaches it. The surface kinds are dispatched as
//!   in [`crate::surface::surface_cache::p1`] (`SmIsoCurve::IsLinear` over the B-spline
//!   kind only, which is all upstream reaches: the diagonal test runs for
//!   `SmBSplineSurface_TYPE` and `SmSurfOfExtrusion_TYPE`, whose iso curves run the
//!   inherited B-spline evaluation).
//! - **Destructor.** `~SmSurfaceCache` deletes the tree, the continuity arrays, the
//!   iso boundary curves, the alternate caches and the subdivision surfaces: all owned
//!   by [`SurfaceCache`], dropped with it.

use crate::curve::curve::Curve;
use crate::curve::iso_curve::IsoCurve;
use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::tree::{
    AuxDataType, NodeClass, NodeData, NodeGeomType, NodeId, SplitDir, SurfaceAux, TreeNode,
};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_surface::{BoundingBoxRequest, SurfParamType};
use crate::nurbs::knots::Side;
use crate::nurbs::nurbs_srf::{
    compute_net_constants, compute_surf_constants, decompose_srf, ConstantsRequest, PointEvaluator,
};
use crate::nurbs::srf_basic::Surface as NurbSurface;
use crate::surface::surface::{Surface, SurfaceKind};

use super::p1::{
    bspline_of, calculate_bounding_box, compute_bezier_approximation, cull_array, invalid,
    AltCacheKey, AltSrfCache, AltSrfCacheType, BezierAux2d, PatchBoundaryBoundingPlanes,
    SurfaceCache, MAX_ALT_CACHES, PB_SAMPLE_COUNT,
};

const OP_NTH: &str = "surface::surface_cache::p2::PatchBoundaryBoundingPlanes::nth_bounding_plane";
const OP_ADD: &str = "surface::surface_cache::p2::SurfaceCache::add_curves_to_cache";
const OP_TEST: &str = "surface::surface_cache::p2::SurfaceCache::test_against_tolerances";
const OP_BLOCK: &str = "surface::surface_cache::p2::SurfaceCache::subdivide_by_block";
const OP_BEZ_BLOCK: &str = "surface::surface_cache::p2::create_bezier_patch_block";
const OP_APPROX_BLOCK: &str = "surface::surface_cache::p2::create_approximate_bezier_patch_block";

/// `slMaxTotalFacets` of `SubdivideByBlock`: largest number of split point pairs.
const MAX_TOTAL_FACETS: usize = 100_000;

// ---- GetNthBoundingPlane ------------------------------------------------------------

/// A boundary bounding plane ([`NthBoundingPlane::plane`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingPlane {
    /// `rsPlaneBasePointArg`: the plane's base point.
    pub base_point: Point3,
    /// `rsPlaneNormalArg`: the plane's normal (pointing off the surface).
    pub normal: Vec3,
    /// `pSamplePoint`: the boundary curve samples.
    pub sample_points: [Point3; PB_SAMPLE_COUNT],
    /// `rdToleranceArg`: the boundary curve's largest distance to the plane.
    pub tolerance: f64,
}

/// The output of [`PatchBoundaryBoundingPlanes::nth_bounding_plane`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NthBoundingPlane {
    /// `rbPlaneExistsArg`: the boundary is planar and the surface lies on one side of
    /// its plane.
    pub plane_exists: bool,
    /// `rbBoundaryDegenerateArg`: the boundary curve is degenerate.
    pub boundary_degenerate: bool,
    /// The plane outputs, written only when the plane exists.
    pub plane: Option<BoundingPlane>,
}

impl PatchBoundaryBoundingPlanes {
    /// The bounding plane of boundary `index` (`0` = minimum v, `1` = maximum u, `2` =
    /// maximum v, `3` = minimum u) of `surface`, computing the planes first when they
    /// are not computed yet (`GetNthBoundingPlane`). A failing computation gives "no
    /// plane" (and is retried by the next call).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index above 3 (upstream reads past its arrays);
    /// [`SolidError::Unported`] from the plane computation.
    pub fn nth_bounding_plane(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        index: usize,
    ) -> Result<NthBoundingPlane> {
        // init output
        let mut out = NthBoundingPlane {
            plane_exists: false,
            boundary_degenerate: false,
            plane: None,
        };

        if !self.are_planes_computed() {
            // Does not seem to make sense crash in a boxtest-like function:
            match self.compute_bounding_planes(k, surface) {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(out),
            }
        }

        if index >= 4 {
            return Err(invalid(OP_NTH, "bounding plane index above 3"));
        }

        match self.plane(index) {
            None => {
                out.boundary_degenerate = self.is_boundary_degenerate(index);
                Ok(out)
            }
            Some((base_point, normal, sample_points, tolerance)) => {
                // set output
                out.plane = Some(BoundingPlane {
                    base_point,
                    normal,
                    sample_points,
                    tolerance,
                });
                out.plane_exists = true;
                Ok(out)
            }
        }
    }
}

// ---- Bezier patch blocks ------------------------------------------------------------

/// The output of [`create_bezier_patch_block`].
#[derive(Debug, Clone)]
pub struct BezierPatchBlock {
    /// `rlNumUSpans`: non-zero u spans of the source.
    pub num_u_spans: usize,
    /// `rlNumVSpans`: non-zero v spans of the source.
    pub num_v_spans: usize,
    /// `rBeziers`: one Bezier patch per non-zero span pair, ordered
    /// `[patch00, patch01, .. patch0N, patch10, .. patchMN]`.
    pub beziers: Vec<NurbSurface>,
}

/// Decomposes the B-spline surface `source` into its Bezier patches, one for every
/// non-zero span pair (`sm_CreateBezierPatchBlock`).
///
/// # Errors
///
/// `SM_ERR` when the decomposition fails (upstream `GW_SER` of `sm_DecomposeSrf`: no
/// non-zero span, more breakpoints than spans); those of the knot vector reads.
pub fn create_bezier_patch_block(source: &NurbSurface) -> Result<BezierPatchBlock> {
    // get span counts
    let (p, q) = source.degrees();
    let (knu, knv) = source.knot_vectors()?;
    let num_u_spans = knu.span_count(p);
    let num_v_spans = knv.span_count(q);

    // store a bezier patch for every non-zero span
    // order: [patch00, patch01, ... patch0N, patch10, ... patchMN]
    let rows = decompose_srf(source).map_err(SolidError::nl_ser(OP_BEZ_BLOCK))?;
    let beziers: Vec<NurbSurface> = rows.into_iter().flatten().collect();
    if beziers.len() != num_u_spans * num_v_spans {
        return Err(invalid(OP_BEZ_BLOCK, "decomposition patch count mismatch"));
    }

    // all done
    Ok(BezierPatchBlock {
        num_u_spans,
        num_v_spans,
        beziers,
    })
}

/// The output of [`create_approximate_bezier_patch_block`].
#[derive(Debug, Clone)]
pub struct ApproximateBezierPatchBlock {
    /// `rlNumUSpans`: number of u spans (`u_splits.len() - 1`).
    pub num_u_spans: usize,
    /// `rlNumVSpans`: number of v spans (`v_splits.len() - 1`).
    pub num_v_spans: usize,
    /// `rUSplits`: the u split points.
    pub u_splits: Vec<f64>,
    /// `rVSplits`: the v split points.
    pub v_splits: Vec<f64>,
    /// `rBeziers`: one bicubic patch per span pair, ordered
    /// `[patch00, patch01, .. patch0N, patch10, .. patchMN]`.
    pub beziers: Vec<NurbSurface>,
}

/// Limits a knot list to 6 values: the ends and 4 equally spaced values between.
fn limit_splits(splits: &mut Vec<f64>) {
    if splits.len() > 6 {
        let (Some(&min), Some(&max)) = (splits.first(), splits.last()) else {
            return;
        };
        splits.truncate(6);
        let mut out = [0.0; 6];
        out[0] = min;
        out[5] = max;
        for (i, slot) in out.iter_mut().enumerate().take(5).skip(1) {
            let i = i as f64;
            *slot = min + (i / 5.0) * (max - min);
        }
        splits.clear();
        splits.extend_from_slice(&out);
    }
}

/// Approximates `surface` by bicubic Bezier patches over its knot spans, at most 5 x 5
/// of them (more knots are replaced by 6 equally spaced split values)
/// (`sm_CreateApproximateBezierPatchBlock`).
///
/// # Errors
///
/// Those of `GetKnots` (the base class default, `SM_ERR`, for kinds without knots);
/// [`SolidError::Unported`] from the Bezier approximation (the approximation's other
/// failures are ignored, as upstream: the patch keeps what was written).
pub fn create_approximate_bezier_patch_block(
    k: &Kernel,
    surface: &Surface,
) -> Result<ApproximateBezierPatchBlock> {
    // Note we are creating approximate cubic beziers
    let (mut u_splits, _) = surface.knots_with_nurb(k, SurfParamType::U, None)?;
    // limit initial U knot count to 6
    limit_splits(&mut u_splits);

    // limit initial V knot count to 6
    let (mut v_splits, _) = surface.knots_with_nurb(k, SurfParamType::V, None)?;
    limit_splits(&mut v_splits);

    // get non-zero span count
    let num_u_spans = u_splits
        .len()
        .checked_sub(1)
        .ok_or_else(|| invalid(OP_APPROX_BLOCK, "no u knots"))?;
    let num_v_spans = v_splits
        .len()
        .checked_sub(1)
        .ok_or_else(|| invalid(OP_APPROX_BLOCK, "no v knots"))?;

    let mut beziers = Vec::with_capacity(num_u_spans * num_v_spans);

    // for every U span
    for (u_lo, u_hi) in u_splits.iter().zip(u_splits.iter().skip(1)) {
        // for every V Span
        for (v_lo, v_hi) in v_splits.iter().zip(v_splits.iter().skip(1)) {
            // set next patch extent
            let uv_domain = Extent2d::from_bounds(*u_lo, *v_lo, *u_hi, *v_hi);

            // zero-filled degree 3x3 patch memory
            let mut bezier = bicubic_block_patch()?;

            // approximate the surface shape with a degree 3x3 bezier patch
            match compute_bezier_approximation(k, surface, &uv_domain, &mut bezier) {
                Err(e) if e.is_unported() => return Err(e),
                Err(_) | Ok(()) => {}
            }

            // make the surface pointer array
            beziers.push(bezier);
        }
    }

    // all done
    Ok(ApproximateBezierPatchBlock {
        num_u_spans,
        num_v_spans,
        u_splits,
        v_splits,
        beziers,
    })
}

/// A zero-filled bicubic patch (`sm_InitNurbSurfaceMemory(pSurface, 3, 3, 3, 3, 7,
/// 7)` on calloc'ed block memory).
fn bicubic_block_patch() -> Result<NurbSurface> {
    crate::nurbs::nurbs_srf::allocate_nurb_surface(3, 3, 3, 3, 7, 7)
}

// ---- the cache ------------------------------------------------------------------------

/// The output of [`SurfaceCache::test_against_tolerances`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToleranceTest {
    /// `rbNeedsSubdivision`: the patch fails some tessellation test.
    pub needs_subdivision: bool,
    /// `reSubdivisionDirection`: suggested split direction ([`SurfParamType::U`]:
    /// child 1 left, child 2 right; [`SurfParamType::V`]: child 1 bottom, child 2 top).
    pub direction: SurfParamType,
}

/// A `TestAgainstTolerances` for `SubdivideToTolerances` (upstream's virtual call:
/// [`SurfaceCache::test_against_tolerances`] for the base class, a derived cache's
/// override otherwise).
pub type ToleranceTester<'a, O> =
    dyn FnMut(&mut SurfaceCache<O>, &Kernel, NodeId) -> Result<ToleranceTest> + 'a;

/// Turns an [`SolidError::Unported`] or any other error of an ignored status into
/// "ignored": `Ok(None)` for ignored failures, the error for `Unported`.
fn ignore<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// The chord height outputs of `sm_ComputeNetConstants` (U, V and diagonal).
const CHORD_HEIGHTS: ConstantsRequest = ConstantsRequest {
    u_chord_height: true,
    v_chord_height: true,
    u_angle_deg: false,
    v_angle_deg: false,
    uv_chord_height: true,
};

/// The turning angle outputs of `sm_ComputeNetConstants` (U and V).
const ANGLES: ConstantsRequest = ConstantsRequest {
    u_chord_height: false,
    v_chord_height: false,
    u_angle_deg: true,
    v_angle_deg: true,
    uv_chord_height: false,
};

/// `sm_ComputeNetConstants(pSurToTest, pUVDomain, pSur, ..)`: with a surface to test
/// (not a B-spline) the values come from evaluations of it over `uv_domain` (and
/// `pSur` is not read); else from the net of `sur`.
fn net_constants(
    k: &Kernel,
    surface_to_test: Option<&Surface>,
    uv_domain: &Extent2d,
    sur: Option<&NurbSurface>,
    zone_tol_3d: f64,
    req: ConstantsRequest,
) -> Result<crate::nurbs::nurbs_srf::NetConstants> {
    if let Some(s) = surface_to_test {
        let mut eval = |uv: Point2| s.evaluate_point(k, uv);
        let ev: &mut PointEvaluator<'_> = &mut eval;
        compute_surf_constants(ev, uv_domain, req)
    } else {
        let sur = sur.ok_or_else(|| invalid(OP_TEST, "null Bezier patch"))?;
        compute_net_constants(None, sur, zone_tol_3d, req)
    }
}

impl<O: Copy + PartialEq> SurfaceCache<O> {
    /// Adds an alternate cache of `curves_3d` / `curves_uv` (consumed) made with the
    /// given parameters as the newest cache; when the cache is full the oldest one is
    /// deleted first (`AddCurvesToCache`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when more than `SM_MAX_ALT_CACHES` caches are held (cannot happen
    /// through this method).
    pub fn add_curves_to_cache(
        &mut self,
        cache_type: AltSrfCacheType,
        approx_tol: f64,
        angle_tol: f64,
        key: &AltCacheKey,
        curves_3d: Option<Vec<Curve>>,
        curves_uv: Option<Vec<Curve>>,
    ) -> Result<()> {
        let caches = self.alt_caches_mut();
        if caches.len() > MAX_ALT_CACHES {
            return Err(SolidError::sm_err(OP_ADD));
        }

        if caches.len() == MAX_ALT_CACHES {
            caches.pop();
        }

        // Move caches up by one to make space at beginning.
        let new_cache =
            AltSrfCache::new(curves_3d, curves_uv, cache_type, approx_tol, angle_tol, key);
        caches.insert(0, new_cache);
        Ok(())
    }

    /// Tests the leaf `node` against the cache's tessellation controls
    /// (`TestAgainstTolerances`): chord height (pseudo box height, then the net or
    /// sampled chord heights for the direction), turning angle, then the maximum side
    /// length and aspect ratio (which can force a split) and the minimum side length
    /// (which can stop one); a split of a side below the minimum uv size ratio is
    /// turned into the other direction or dropped. Updates the node's
    /// `m_bChordHeightSatisfied` / `m_bAngleTolSatisfied`, the bounding box of an
    /// `SM_AD_AUX_DATA` node and [`SurfaceCache::switch_to_fast_subdivision`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a node that is not an `SM_AD_BEZIER_SURFACE` / `SM_AD_AUX_DATA` leaf;
    /// those of `CalculateBoundingBox`, `sm_ComputeNetConstants`,
    /// `sm_ComputeSurfConstants`, `GetCorners` and `ComputeSizeTolerances`;
    /// `InvalidInput` where upstream reads through null or mistyped data;
    /// [`SolidError::Unported`] from `SmTol::GetZoneTol3d` of a face-owned surface whose
    /// face tolerance is not cached on it ([`Surface::owner_zone_tol_3d`]) and from the
    /// surface calls.
    /// `InvalidInput` if an approximation still requires refinement after a sampled
    /// angle segment's length was non-finite (see the module deviations).
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn test_against_tolerances(&mut self, k: &Kernel, node: NodeId) -> Result<ToleranceTest> {
        // init output
        let mut out = ToleranceTest {
            needs_subdivision: false,
            direction: SurfParamType::U,
        };

        // check state - nodes are properly labeled and have not yet been split
        let (aux_type, has_child, fields) = {
            let tree = self
                .tree()
                .ok_or_else(|| invalid(OP_TEST, "cache has no tree"))?;
            let n = tree.node(node)?;
            let fields = match n.data {
                NodeData::Surface(a) => Some(a),
                _ => None,
            };
            (n.aux_type, n.child1.is_some(), fields)
        };
        if aux_type != AuxDataType::BezierSurface && aux_type != AuxDataType::AuxData {
            return Err(SolidError::sm_err(OP_TEST));
        }
        if has_child {
            return Err(SolidError::sm_err(OP_TEST));
        }
        let fields = fields.ok_or_else(|| invalid(OP_TEST, "node has no surface data"))?;

        // local tolerances
        let opts = *self.options();
        let chord_tol = opts.chord_height_tolerance;
        let chord_height_squared = chord_tol * chord_tol;
        let ang_tol_deg = opts.ang_tol_rad * 180.0 / PI;
        // SmTol::GetZoneTol3d(m_cpSurface): GetFace() ? its tolerance : the system's
        // (the face's tolerance as its B-rep cached it on the surface;
        // UNPORTED(topology.face_p2): SmFace::GetTolerance for a bare face handle)
        // Proof (unreachable): a B-rep face's surface carries the face tolerance
        // (Brep::sync_face_surface_tolerance invariant, see Surface::face_zone_tol_3d); 0
        // hits in the green difftest sweep.
        let zone_tol = crate::surface::surface::p1::zone_tol_3d(k, self.surface())?;

        // local tolerance state - a zero ChordHeight or Angle Tol value means to ignore the test
        {
            let aux = self.aux_mut(node)?;
            if chord_height_squared < EFF_ZERO {
                aux.chord_height_satisfied = true;
            }
            if ang_tol_deg < EFF_ZERO {
                aux.angle_tol_satisfied = true;
            }
        }
        let both_satisfied = {
            let aux = self.aux(node)?;
            aux.chord_height_satisfied && aux.angle_tol_satisfied
        };
        let node_domain = self.aux(node)?.uv_domain;

        // Get a pseudo box for this patch's uv-domain
        let force_angle_test;
        let pseudo_box: PseudoBox;
        let sur: Option<NurbSurface>;
        if aux_type == AuxDataType::AuxData {
            force_angle_test = true;
            let node_surface = self.node_surface(fields.surface)?;
            // (upstream casts unchecked; a NURB-less surface gives a null pSur; a plane
            // has its NURB, made here on a copy when the port has not made it yet)
            sur = match node_surface.kind() {
                SurfaceKind::Plane(p) => p.base_for_query().gw_nurb().cloned(),
                _ => bspline_of(node_surface)
                    .ok_or_else(|| invalid(OP_TEST, "node surface is not a B-spline"))?
                    .gw_nurb()
                    .cloned(),
            };
            if both_satisfied {
                pseudo_box = PseudoBox::new();
            } else {
                let req = BoundingBoxRequest {
                    normal: true,
                    pseudo: true,
                    polar: false,
                    pseudo_guess: None,
                    polar_guess: None,
                    expand_by_zone_tol: true,
                    expand_tolerance: None,
                };
                let boxes = calculate_bounding_box(k, node_surface, &node_domain, &req)?;
                let bbox = boxes
                    .normal
                    .ok_or_else(|| invalid(OP_TEST, "no bounding box"))?;
                pseudo_box = boxes
                    .pseudo
                    .ok_or_else(|| invalid(OP_TEST, "no pseudo box"))?;
                self.tree_mut()
                    .ok_or_else(|| invalid(OP_TEST, "cache has no tree"))?
                    .node_mut(node)?
                    .bbox = bbox;
            }
        } else {
            let patch = self.patch(node)?;
            force_angle_test = false;
            pseudo_box = *patch.pseudo_box();
            sur = patch.bezier().cloned();
        }

        // When the surface is not a SmBSplineSurface - test the surface over the
        // node's subdomain. (The cache surface is copied only where the copy is read:
        // the B-spline based kinds test the node's patch, and read the copy only in
        // the surface-evaluation fallbacks below.)
        let compound_copy: Option<Surface> = match self.surface().kind() {
            SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_) => None,
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => Some(self.surface().clone()),
        };
        let surface_to_test: Option<&Surface> = compound_copy.as_ref();

        let mut subdivide = false;
        let mut direction = SurfParamType::U;

        // chordheight
        if chord_height_squared >= EFF_ZERO {
            let mut height = pseudo_box.interval(2).map_or(0.0, |i| i.length());

            // Bezier patch boxes are padded by SM_ZONE_TOL_3D on each side, so a chord
            // height tolerance no larger than that could never be met; test the
            // unpadded height.
            if aux_type == AuxDataType::BezierSurface && chord_tol <= 2.0 * ZONE_TOL_3D {
                let unpadded = height - 2.0 * ZONE_TOL_3D;
                height = if unpadded > 0.0 { unpadded } else { 0.0 };
            }

            if height * height <= chord_height_squared {
                self.aux_mut(node)?.chord_height_satisfied = true;
            } else {
                subdivide = true;

                let nc = net_constants(
                    k,
                    surface_to_test,
                    &node_domain,
                    sur.as_ref(),
                    zone_tol,
                    CHORD_HEIGHTS,
                )?;
                let u_ch = nc.u_chord_height.unwrap_or(0.0);
                let v_ch = nc.v_chord_height.unwrap_or(0.0);
                let uv_ch = nc.uv_chord_height.unwrap_or(0.0);

                // If U- and V- chord heights are within tol but the diagonal is not,
                // split the direction with the larger derivative x parameter range.
                if u_ch <= chord_tol && v_ch <= chord_tol && uv_ch > chord_tol {
                    // evaluate first derivatives at center of patch
                    let sur = sur
                        .as_ref()
                        .ok_or_else(|| invalid(OP_TEST, "null Bezier patch"))?;
                    let r = sur.parameter_bounds();
                    let (us, ue, vs, ve) = (r.ul, r.ur, r.vb, r.vt);
                    let sd = sur
                        .derivs(
                            (us + ue) / 2.0,
                            (vs + ve) / 2.0,
                            Side::Left,
                            Side::Left,
                            true,
                            1,
                            1,
                        )
                        .map_err(|_| invalid(OP_TEST, "N_SrfDerivs failed"))?;
                    let du = sd
                        .get(1)
                        .and_then(|r| r.first())
                        .copied()
                        .ok_or_else(|| invalid(OP_TEST, "missing derivative"))?;
                    let dv = sd
                        .first()
                        .and_then(|r| r.get(1))
                        .copied()
                        .ok_or_else(|| invalid(OP_TEST, "missing derivative"))?;

                    // split the longer direction
                    if du.length_squared() * (ue - us) < dv.length_squared() * (ve - vs) {
                        direction = SurfParamType::V;
                    }
                }
                // Otherwise split the direction with the larger directional chordheight error.
                else if v_ch > u_ch {
                    direction = SurfParamType::V;
                }
            }
        } // end ChordHeight

        // AngleTol
        let mut u_ang_deg = -1.0;
        let mut v_ang_deg = -1.0;
        let mut nonfinite_angle_length = false;

        if force_angle_test || (!subdivide && ang_tol_deg >= EFF_ZERO) {
            let nc = net_constants(
                k,
                surface_to_test,
                &node_domain,
                sur.as_ref(),
                zone_tol,
                ANGLES,
            )?;
            u_ang_deg = nc.u_angle_deg.unwrap_or(0.0);
            v_ang_deg = nc.v_angle_deg.unwrap_or(0.0);
            nonfinite_angle_length |= nc.nonfinite_angle_length;

            // Returning more than 1-1/2 full revolutions indicates that the control
            // net is not suitable for estimating the angles: use surface evaluations.
            if u_ang_deg > 540.0 || v_ang_deg > 540.0 {
                let cache_surface;
                let surf: &Surface = if let Some(s) = surface_to_test {
                    s
                } else {
                    cache_surface = self.surface().clone();
                    &cache_surface
                };
                let mut eval = |uv: Point2| surf.evaluate_point(k, uv);
                let sc = compute_surf_constants(&mut eval, &node_domain, ANGLES)?;
                u_ang_deg = sc.u_angle_deg.unwrap_or(0.0);
                v_ang_deg = sc.v_angle_deg.unwrap_or(0.0);
                nonfinite_angle_length |= sc.nonfinite_angle_length;
            }

            // Optimization for SmTessSrfCache::BuildTree(): switch to block subdivision
            if force_angle_test
                && u_ang_deg < self.start_fast_subdivision_angle_deg
                && v_ang_deg < self.start_fast_subdivision_angle_deg
            {
                self.switch_to_fast_subdivision = true;
            }

            // look for passing/failing patches
            if ang_tol_deg < EFF_ZERO || (u_ang_deg <= ang_tol_deg && v_ang_deg <= ang_tol_deg) {
                self.aux_mut(node)?.angle_tol_satisfied = true;
            } else {
                // When direction is not yet picked - Split the direction with the larger angle.
                if !subdivide && v_ang_deg > u_ang_deg {
                    direction = SurfParamType::V;
                }
                subdivide = true;
            }
        } // end AngleTol

        // node property locals
        let mut aspect_ratio = Vec2::new(0.0, 0.0);
        let mut max_side_length = Vec2::new(0.0, 0.0);
        let mut min_side_length = Vec2::new(0.0, 0.0);

        // when further testing is required - compute node properties
        if (!subdivide && (opts.aspect_ratio_3d > 0.0 || opts.max_side_length_3d > 0.0))
            || (subdivide && opts.min_side_length_3d > 0.0)
        {
            // compute dUAngDeg and dVAngDeg to see if patch is flat
            if u_ang_deg < 0.0 || v_ang_deg < 0.0 {
                let nc = net_constants(
                    k,
                    surface_to_test,
                    &node_domain,
                    sur.as_ref(),
                    zone_tol,
                    ANGLES,
                )?;
                u_ang_deg = nc.u_angle_deg.unwrap_or(0.0);
                v_ang_deg = nc.v_angle_deg.unwrap_or(0.0);
                nonfinite_angle_length |= nc.nonfinite_angle_length;
            }

            let sizes = if u_ang_deg <= ang_tol_deg && v_ang_deg <= ang_tol_deg {
                // when patch is Flat enough, compute from surface corner points - cheap
                let c = self.corners(k, sur.as_ref(), &node_domain)?;
                Self::size_tolerances_from_corners(c.u0v0, c.u1v0, c.u1v1, c.u0v1)
            } else {
                // compute from sample surface IsoCurves - expensive
                if let Some(s) = surface_to_test {
                    Self::size_tolerances_from_surface(k, s, &node_domain)?
                } else {
                    let cache_surface = self.surface().clone();
                    Self::size_tolerances_from_surface(k, &cache_surface, &node_domain)?
                }
            };
            aspect_ratio = sizes.aspect_ratio;
            max_side_length = sizes.max_side_length_3d;
            min_side_length = sizes.min_side_length_3d;
        }

        // when asked - force division until MaxSideLengths < limit
        if !subdivide
            && opts.max_side_length_3d > 0.0
            && max_side_length.length() > opts.max_side_length_3d
        {
            subdivide = true;
            // always divide biggest dimension
            direction = if max_side_length.x > max_side_length.y {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
        }

        // when asked - force division until AspectRatio < limit
        if !subdivide
            && opts.aspect_ratio_3d > 0.0
            && (aspect_ratio.x > opts.aspect_ratio_3d || aspect_ratio.y > opts.aspect_ratio_3d)
        {
            subdivide = true;
            // always divide biggest dimension
            direction = if max_side_length.x > max_side_length.y {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
        }

        // when asked - stop division when MinSideLength < limit
        if subdivide
            && opts.min_side_length_3d > 0.0
            && min_side_length.x <= opts.min_side_length_3d
            && min_side_length.y <= opts.min_side_length_3d
        {
            subdivide = false;
        }

        // no more work - patch passes all subdivision tests
        if !subdivide {
            return Ok(out);
        }

        // Make sure that we eventually stop subdivision: don't let either node
        // direction get less than the minimum ratio of the original UV domain
        let uv_size = self.uv_domain().size();
        let node_size = node_domain.size();
        let min_slr = if opts.min_side_length_ratio_uv == 0.0 {
            0.001
        } else {
            opts.min_side_length_ratio_uv
        };

        // USide or VSide too small When being split
        if (direction == SurfParamType::U && node_size.x < uv_size.x * min_slr)
            || (direction == SurfParamType::V && node_size.y < uv_size.y * min_slr)
        {
            // if USide or VSide is big enough - split it, otherwise stop splitting
            if direction == SurfParamType::U && node_size.y > uv_size.y / 100.0 {
                direction = SurfParamType::V;
            } else if direction == SurfParamType::V && node_size.x > uv_size.x / 100.0 {
                direction = SurfParamType::U;
            } else {
                out.needs_subdivision = false;
                return Ok(out);
            }
        }

        // set output for split
        if surface_to_test.is_some() && nonfinite_angle_length {
            return Err(invalid(
                OP_TEST,
                "non-finite approximation angle segment length",
            ));
        }
        out.needs_subdivision = true;
        out.direction = direction;
        Ok(out)
    }

    /// Subdivides the `SM_AD_AUX_DATA` node `node` in one pass: the tessellation
    /// splits of its surface in u and v (thinned until at most 100000 split pairs
    /// remain) make a grid of base nodes (boxed by sub-patches of the surface), which
    /// are combined pairwise, the longer direction first, into a binary tree whose
    /// root is `node` (`SubdivideByBlock`). Nothing changes when neither direction
    /// has an interior split.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a node without surface; `SM_ERR` for a malformed
    /// node grid; `InvalidInput` for a node without surface data;
    /// Errors from tessellation split selection and the iso curve linearity tests.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn subdivide_by_block(&mut self, k: &Kernel, node: NodeId) -> Result<()> {
        // locals
        let fields = {
            let tree = self
                .tree()
                .ok_or_else(|| invalid(OP_BLOCK, "cache has no tree"))?;
            match tree.node(node)?.data {
                NodeData::Surface(a) => a,
                _ => return Err(invalid(OP_BLOCK, "node has no surface data")),
            }
        };
        let handle = fields.surface.ok_or(SolidError::null_pointer(OP_BLOCK))?;
        // upstream works on the node's surface itself (`pAux->mBA_pSurface`); the cache is
        // edited below, so an evaluation copy (exact knots, owner kept) stands for it: the
        // copy constructor's knot fix-up would move near-coincident knots and so the splits.
        // An `SmPlane` keeps the copy constructor: its copy carries the NURB upstream has
        // made on the plane by now (the copy constructor makes and keeps it there first),
        // and a freshly
        // made plane NURB has no knots for the fix-up to move.
        let node_surface = self.node_surface(Some(handle))?;
        let surface = if let SurfaceKind::Plane(_) = node_surface.kind() {
            node_surface.clone()
        } else {
            node_surface.clone_for_evaluation()?
        };
        let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
        let opts = *self.options();

        // remember need to check diagonal when all natural isoParam Curves are linear
        let mut check_diagonals = false;
        // (SmBSplineSurface_TYPE or SmSurfOfExtrusion_TYPE: the extrusion's iso curves
        // evaluate its inherited B-spline evaluation)
        let diagonal_candidate = match surface.kind_with_nurb(k)? {
            SurfaceKind::BSpline(bss) => Some(bss),
            SurfaceKind::SurfOfExtrusion(s) => Some(s.base()),
            _ => None,
        };
        if let Some(bss) = diagonal_candidate {
            let tol = if opts.chord_height_tolerance > 0.0 {
                opts.chord_height_tolerance / 2.0
            } else {
                EFF_ZERO_SQRT
            };
            let iso_linear = |param: SurfParamType, value: f64| -> Result<bool> {
                let c = Curve::from(IsoCurve::new(bss.copy(), param, value, true, true));
                Ok(c.is_linear(k, tol, false)?.linear)
            };
            if iso_linear(SurfParamType::U, uv_domain.min().x)?
                && iso_linear(SurfParamType::V, uv_domain.min().y)?
                && iso_linear(SurfParamType::U, uv_domain.max().x)?
                && iso_linear(SurfParamType::V, uv_domain.max().y)?
            {
                check_diagonals = true;
            }
        }

        let ang_deg = opts.ang_tol_rad * 180.0 / PI;
        // UDir tessellation points
        let mut v_splits = surface.find_tessellation_splits(
            k,
            SurfParamType::U,
            &uv_domain,
            opts.chord_height_tolerance,
            ang_deg,
            opts.max_side_length_3d,
            check_diagonals,
        )?;
        // VDir tessellation points
        let mut u_splits = surface.find_tessellation_splits(
            k,
            SurfParamType::V,
            &uv_domain,
            opts.chord_height_tolerance,
            ang_deg,
            opts.max_side_length_3d,
            check_diagonals,
        )?;

        self.block_from_splits(k, node, handle, &surface, &mut u_splits, &mut v_splits)
    }

    /// The part of [`SurfaceCache::subdivide_by_block`] after the splits are known:
    /// culls them, makes the base node grid and combines it into a tree under `node`.
    fn block_from_splits(
        &mut self,
        k: &Kernel,
        node: NodeId,
        handle: O,
        surface: &Surface,
        u_splits: &mut Vec<f64>,
        v_splits: &mut Vec<f64>,
    ) -> Result<()> {
        let mut num_splits_u = u_splits.len();
        let mut num_splits_v = v_splits.len();
        let mut total = num_splits_u.saturating_mul(num_splits_v);

        // no work - no splits
        if num_splits_u <= 2 && num_splits_v <= 2 {
            return Ok(());
        }

        // cull split points until total facet count is small enough to proceed
        while total > MAX_TOTAL_FACETS {
            if num_splits_u > 20 {
                cull_array(u_splits);
                num_splits_u = u_splits.len();
            }
            if num_splits_v > 20 {
                cull_array(v_splits);
                num_splits_v = v_splits.len();
            }
            total = num_splits_u.saturating_mul(num_splits_v);
        }

        let split = |s: &[f64], i: usize| -> Result<f64> {
            s.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_BLOCK, "split index out of range"))
        };

        // create the base nodes
        let mut base_nodes: Vec<NodeId> = Vec::new();
        for i in 1..num_splits_u {
            for j in 1..num_splits_v {
                let node_uv = Extent2d::from_bounds(
                    split(u_splits, i - 1)?,
                    split(v_splits, j - 1)?,
                    split(u_splits, i)?,
                    split(v_splits, j)?,
                );
                let slot = self.auxs().len();
                let tree = self
                    .tree_mut()
                    .ok_or_else(|| invalid(OP_BLOCK, "cache has no tree"))?;
                let base = tree.add_node(TreeNode {
                    parent: None,
                    child1: None,
                    child2: None,
                    bbox: zero_box(),
                    aux_type: AuxDataType::AuxData,
                    data: NodeData::Surface(SurfaceAux {
                        split_dir: SplitDir::U,
                        is_marked: false,
                        node_class: NodeClass::Unknown,
                        face: None,
                        surface: Some(handle),
                        slot: Some(slot),
                    }),
                    geom_type: NodeGeomType::Default,
                });
                base_nodes.push(base);
                self.auxs_mut().push(BezierAux2d {
                    owning_tree_node: Some(base),
                    uv_domain: node_uv,
                    ..BezierAux2d::default()
                });

                // for the node's bounding box
                let patch = match surface.kind_with_nurb(k)? {
                    SurfaceKind::BSpline(b) => b.copy_sub_patch(&node_uv),
                    SurfaceKind::Plane(p) => p.base().copy_sub_patch(&node_uv),
                    SurfaceKind::Cone(s) => s.rev().base().copy_sub_patch(&node_uv),
                    SurfaceKind::Sphere(s) => s.base().base().copy_sub_patch(&node_uv),
                    SurfaceKind::Torus(s) => s.base().base().copy_sub_patch(&node_uv),
                    SurfaceKind::SurfOfRevolution(s) => s.base().copy_sub_patch(&node_uv),
                    SurfaceKind::SurfOfExtrusion(s) => s.base().copy_sub_patch(&node_uv),
                    SurfaceKind::CurveBounded(_)
                    | SurfaceKind::Step(_)
                    | SurfaceKind::OffsetSurface(_)
                    | SurfaceKind::SrfInVolume(_) => {
                        return Err(invalid(OP_BLOCK, "node surface is not a B-spline"));
                    }
                };
                // If that fails, do what we can and continue.
                let Some(patch) = ignore(patch)? else {
                    continue;
                };
                let patch_domain = patch.natural_uv_domain();
                let req = BoundingBoxRequest {
                    normal: true,
                    pseudo: false,
                    polar: false,
                    pseudo_guess: None,
                    polar_guess: None,
                    expand_by_zone_tol: true,
                    expand_tolerance: None,
                };
                if let Some(boxes) = ignore(patch.calculate_bounding_box(k, &patch_domain, &req))? {
                    if let Some(b) = boxes.normal {
                        self.tree_mut()
                            .ok_or_else(|| invalid(OP_BLOCK, "cache has no tree"))?
                            .node_mut(base)?
                            .bbox = b;
                    }
                }
            }
        }

        // assemble the base nodes into a binary tree with a sequence of parent node generations
        let mut num_u = u_splits.len().saturating_sub(1);
        let mut num_v = v_splits.len().saturating_sub(1);
        let mut curr: Vec<NodeId> = base_nodes.clone();
        let mut next: Vec<NodeId> = Vec::with_capacity(base_nodes.len());
        let at = |v: &[NodeId], i: usize| -> Result<NodeId> {
            v.get(i).copied().ok_or(SolidError::sm_err(OP_BLOCK))
        };

        // assemble U and V splits until there is just one
        while num_u > 1 || num_v > 1 {
            let combine_v = num_v > num_u;
            let last_time = num_u + num_v == 3;

            if combine_v {
                // Combine along rows of V: this combination shrinks the rows
                for ii in 0..num_u {
                    let mut kk = 1;
                    while kk < num_v {
                        let n1 = at(&curr, ii * num_v + kk - 1)?;
                        let n2 = at(&curr, ii * num_v + kk)?;
                        let new_node = self.block_parent(node, n1, n2, last_time, SplitDir::V)?;
                        next.push(new_node);
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
                        let new_node = self.block_parent(node, n1, n2, last_time, SplitDir::U)?;
                        next.push(new_node);
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

        // all done
        Ok(())
    }

    /// The parent of `n1` and `n2` in the block subdivision: `root` the last time,
    /// else a new `SM_AD_AUX_DATA` node over the union of their domains and boxes;
    /// its split direction is `split`.
    fn block_parent(
        &mut self,
        root: NodeId,
        n1: NodeId,
        n2: NodeId,
        last_time: bool,
        split: SplitDir,
    ) -> Result<NodeId> {
        let new_node = if last_time {
            root
        } else {
            let uv = {
                let a1 = self.aux(n1)?;
                let a2 = self.aux(n2)?;
                a1.uv_domain.union(&a2.uv_domain)
            };
            let slot = self.auxs().len();
            let tree = self
                .tree_mut()
                .ok_or_else(|| invalid(OP_BLOCK, "cache has no tree"))?;
            let b1 = tree.node(n1)?.bbox;
            let b2 = tree.node(n2)?.bbox;
            let id = tree.add_node(TreeNode {
                parent: None,
                child1: None,
                child2: None,
                bbox: b1.union(&b2),
                aux_type: AuxDataType::AuxData,
                data: NodeData::Surface(SurfaceAux {
                    split_dir: SplitDir::U,
                    is_marked: false,
                    node_class: NodeClass::Unknown,
                    face: None,
                    surface: None,
                    slot: Some(slot),
                }),
                geom_type: NodeGeomType::Default,
            });
            self.auxs_mut().push(BezierAux2d {
                owning_tree_node: Some(id),
                uv_domain: uv,
                ..BezierAux2d::default()
            });
            id
        };
        let tree = self
            .tree_mut()
            .ok_or_else(|| invalid(OP_BLOCK, "cache has no tree"))?;
        match &mut tree.node_mut(new_node)?.data {
            NodeData::Surface(a) => a.split_dir = split,
            _ => return Err(invalid(OP_BLOCK, "node has no surface data")),
        }
        let nn = tree.node_mut(new_node)?;
        nn.child1 = Some(n1);
        nn.child2 = Some(n2);
        tree.node_mut(n1)?.parent = Some(new_node);
        tree.node_mut(n2)?.parent = Some(new_node);
        Ok(new_node)
    }
}

/// An all-zero box (zero-filled `SmMemBlockMgr` memory).
const fn zero_box() -> Extent3d {
    Extent3d::from_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tree::Tree;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::surface_cache::p1::{BezierPatch, SurfaceCacheOptions};

    fn bilinear(z: f64) -> NurbSurface {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, z, NOW),
            ],
        ];
        NurbSurface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .expect("surface")
    }

    fn biquad() -> NurbSurface {
        let net = (0..4)
            .map(|i| {
                (0..3)
                    .map(|j| CPoint::new(f64::from(i), f64::from(j), f64::from(i * j) * 0.3, NOW))
                    .collect()
            })
            .collect();
        NurbSurface::new(
            net,
            2,
            2,
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .expect("surface")
    }

    #[test]
    fn bezier_block_counts_spans() {
        let b = create_bezier_patch_block(&biquad()).expect("block");
        assert_eq!((b.num_u_spans, b.num_v_spans), (2, 1));
        assert_eq!(b.beziers.len(), 2);
        assert_eq!(b.beziers[0].degrees(), (2, 2));
    }

    #[test]
    fn approximate_block_limits_splits() {
        let mut k = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 1.0];
        limit_splits(&mut k);
        assert_eq!(k.len(), 6);
        assert_eq!(k[0], 0.0);
        assert_eq!(k[5], 1.0);
        assert_eq!(k[1], 0.0 + (1.0 / 5.0) * 1.0);
        let mut short = vec![0.0, 1.0];
        limit_splits(&mut short);
        assert_eq!(short, vec![0.0, 1.0]);
    }

    #[test]
    fn add_curves_evicts_oldest() {
        let s = Surface::from(BSplineSurface::from_nurb(bilinear(0.0), false));
        let d = s.natural_uv_domain_if_built();
        let mut c: SurfaceCache<i64> = SurfaceCache::new(s, d, SurfaceCacheOptions::default());
        for i in 0..7 {
            let key = AltCacheKey {
                long: Some(i),
                ..AltCacheKey::default()
            };
            c.add_curves_to_cache(AltSrfCacheType::Silhouette, 0.1, 0.2, &key, None, None)
                .expect("add");
        }
        assert_eq!(c.alt_caches().len(), MAX_ALT_CACHES);
        let newest = AltCacheKey {
            long: Some(6),
            ..AltCacheKey::default()
        };
        assert!(c.alt_caches()[0].match_cache(AltSrfCacheType::Silhouette, 0.1, 0.2, &newest));
    }

    fn patch_cache(z: f64, opts: SurfaceCacheOptions) -> SurfaceCache<i64> {
        let s = Surface::from(BSplineSurface::from_nurb(bilinear(z), false));
        let d = s.natural_uv_domain_if_built();
        let mut c = SurfaceCache::new(s, d, opts);
        c.set_surface_handle(Some(-7));
        let mut tree: Tree<i64> = Tree::new(None);
        tree.add_node(TreeNode {
            aux_type: AuxDataType::BezierSurface,
            data: NodeData::Surface(SurfaceAux {
                split_dir: SplitDir::U,
                is_marked: false,
                node_class: NodeClass::Unknown,
                face: None,
                surface: Some(-7),
                slot: Some(0),
            }),
            ..TreeNode::new()
        });
        let mut pb = PseudoBox::new();
        pb.set_intervals(
            crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0),
            crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0),
            crate::geom::extent1d::Extent1d::from_raw(0.0, z),
        );
        c.patches_mut().push(BezierPatch {
            aux: BezierAux2d {
                uv_domain: d,
                ..BezierAux2d::default()
            },
            bezier: Some(bilinear(z)),
            pseudo_box: pb,
            ..BezierPatch::default()
        });
        c.set_tree(Some(tree));
        c
    }

    #[test]
    fn flat_patch_passes_chord_height() {
        let k = Kernel::default();
        let opts = SurfaceCacheOptions {
            chord_height_tolerance: 0.01,
            ..SurfaceCacheOptions::default()
        };
        let mut c = patch_cache(0.0, opts);
        let t = c.test_against_tolerances(&k, NodeId(0)).expect("test");
        assert!(!t.needs_subdivision);
        assert!(c.aux(NodeId(0)).expect("aux").chord_height_satisfied);
    }

    #[test]
    fn twisted_patch_needs_subdivision() {
        let k = Kernel::default();
        let opts = SurfaceCacheOptions {
            chord_height_tolerance: 0.01,
            ..SurfaceCacheOptions::default()
        };
        let mut c = patch_cache(1.0, opts);
        let t = c.test_against_tolerances(&k, NodeId(0)).expect("test");
        assert!(t.needs_subdivision);
    }

    #[test]
    fn block_subdivision_without_surface_is_null_pointer() {
        let mut c = patch_cache(0.0, SurfaceCacheOptions::default());
        if let Some(t) = c.tree_mut() {
            if let NodeData::Surface(a) = &mut t.node_mut(NodeId(0)).expect("node").data {
                a.surface = None;
            }
        }
        let e = c
            .subdivide_by_block(&Kernel::default(), NodeId(0))
            .expect_err("null");
        assert!(matches!(e, SolidError::Upstream { code: 1006, .. }));
    }

    #[test]
    fn nth_bounding_plane_rejects_large_index() {
        let k = Kernel::default();
        let s = Surface::from(BSplineSurface::from_nurb(bilinear(0.0), false));
        let mut bp = PatchBoundaryBoundingPlanes::new(s.natural_uv_domain_if_built());
        let r = bp.nth_bounding_plane(&k, &s, 4);
        assert!(r.is_err() || !r.expect("ok").plane_exists);
    }
}

//! `SmBrep`, part 8: making faces from surfaces and from loops of curves.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                                   | Upstream                                  |
//! |----------------------------------------|-------------------------------------------|
//! | [`create_faces_with_curves`]           | `SmBrep::CreateFacesWithCurves`           |
//! | [`create_planar_faces_with_3d_curves`] | `SmBrep::CreatePlanarFacesWith3DCurves`   |
//! | [`create_planar_face_with_3d_curves`]  | `SmBrep::CreatePlanarFaceWith3DCurves`    |
//! | [`create_face_in_region_from_surface`] | `SmBrep::CreateFaceInRegionFromSurface`   |
//! | [`create_face_from_surface`]           | `SmBrep::CreateFaceFromSurface`           |
//! | [`create_faces_from_surface`]          | `SmBrep::CreateFacesFromSurface`          |
//! | [`make_faces_with_curves`]             | `SmBrep::MakeFacesWithCurves`             |
//! | [`FaceBuildEnv`]                       | (the notification and mark state the Euler operators take) |
//! | [`NewFaces`], [`FaceInRegion`]         | (the out-parameters)                      |
//!
//! Deviations:
//! - **Object model.** Free functions over a [`Brep`]; pointer arguments are `Option`s
//!   (a `None` region is the infinite region, as upstream's `NULL`), out-parameters are
//!   return values and are only returned on success. Curves and surfaces are values the
//!   functions consume (upstream: "consumed"). Where upstream shares one surface between
//!   several faces (`CreatePlanarFaceWith3DCurves` with several outer loops), each face
//!   gets a copy. `SmTemporaryChangeValue<m_bEditingEnabled>` sets the flag and restores
//!   it on every return.
//! - **Notifications.** The Euler operators notify through the caller's [`NotifyEnv`];
//!   the new face's surface owner is named by the caller's `this_face`, as in
//!   [`p7::make_face_with_curves`].
//! - **Dead code.** `CreateFaceInRegionFromSurface`'s "new algorithm" branch is not
//!   ported: `sbUseOldAlgorithm` is a `static constexpr TRUE`, so it is unreachable.
//! - **Null dereferences** upstream are [`SolidError::InvalidInput`]: a null surface
//!   for `CreateFacesWithCurves` (`GetNaturalUVDomain` before the check in
//!   `CreateFaceInRegionFromSurface`), a null 3D curve or a null UV curve in a split
//!   `MakeFacesWithCurves` (read through `SmCurveLoops`), curve or orientation counts
//!   shorter than the loop counts (read past the arrays), a failed
//!   `CreateIsoParametricCurve` (a null iso curve is intersected), a failed
//!   `CreateBySplitCurve` (a null curve goes into the loops), a first
//!   `EvaluatePoint` failure in the loop/patch association (an uninitialised point is
//!   dropped), and a `MakeFaceWithCurves` that succeeds without a face (its
//!   `m_bRectangularTrim` is set). A plane fit that fails with
//!   [`SolidError::InvalidInput`] (an input `CreatePlaneWith3DCurves` mishandles
//!   upstream, e.g. the zero-area loop of `FindOuterLoops`, UB-119) returns that error,
//!   not `SM_ERR`.
//! - **Upstream bug kept.** In `MakeFacesWithCurves` the curves of each outer loop group
//!   are counted with `sThisPatchLoops.m_sLoopCounts[ll]` (the patch's loops from the
//!   first) but read from the group's own curve list: for a patch's second group the
//!   counts are those of the first group (`plan/upstream-issues.md` WR-210). Reads past
//!   the group's curves are [`SolidError::InvalidInput`].
//! - **Ignored statuses** stay ignored (`EditParameterization` of the UV trim curves,
//!   `SetTolerance`, `GlobalCurveIntersect`, `InsertIntoLoops`, `UpdateSolutionObjects`,
//!   `AppendLoop`, `FindOuterLoops`, `Copy`, `DeleteEdge`/`DeleteVertex` after
//!   `DeleteFace`, the per-patch `MakeFaceWithCurves`); [`SolidError::Unported`] always
//!   propagates. A failed `FindOuterLoops` leaves no outer loop group (its i/o lists as
//!   the port left them).
//! - **Tolerances.** The loop and iso curves of `MakeFacesWithCurves` belong to no edge,
//!   so `SmTol::GetXSectTol3d(iso, loop)` is twice the system zone tolerance and
//!   `SmTol::GetZoneTol3d(context)` the context's.
//! - **Unported callees** return [`SolidError::Unported`]: `DeleteEdge` / `DeleteVertex`
//!   (`topology.brep_p4`). `DeleteFace` (`topology.brep_p3`), `MakeVertexLoop`
//!   (`topology.brep_p9`), `MakeEdgeInFace` (`topology.brep_p5`), `MakeFaceWithCurves`
//!   (`topology.brep_p7`) and `sm_MarkLoopsToDelete` (`topology.brep_p1`) call their
//!   ports, which may defer further.
//! - **Bounds.** No recursion here (`sm_MarkLoopsToDelete` bounds its own); every loop
//!   runs over input arrays, B-rep lists or the solutions of a finite intersection, whose
//!   splits are bounded by the solutions found.

use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{SolutionArray, SolutionType, SortKey};
use crate::geom::tol::{self, SmallTopoType};
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::face::p1 as qf1;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::Surface;
use crate::topology::brep::p3::{self, DeleteFaceOptions};
use crate::topology::brep::p5::{self, EdgeInFace};
use crate::topology::brep::p7::{self, FaceCurvesInput};
use crate::topology::brep::{p1, p9};
use crate::topology::edge::NotifyEnv;
use crate::topology::face::p2 as face2;
use crate::topology::loopuse;
use crate::topology::topology_traverser::Marks;
use crate::topology::trimming_tools::curve_loops::{
    CurveIntersections, CurveKey, CurveLoops, IsoXSectSegment, LoopObject, StoreDomains,
};
use crate::topology::trimming_tools::find_outer_loops;
use crate::topology::vertex;
use crate::topology::{Brep, EdgeId, FaceId, LoopId, RegionId, ShellId, VertexId};

const OP: &str = "topology::brep::p8";

/// The state the Euler operators of the face builders take: the caller's notification
/// environment, the topology marks of [`p3::delete_face`] and the identity the new
/// faces' surfaces are owned under (see [`p7::make_face_with_curves`]).
pub struct FaceBuildEnv<'a, 'e> {
    /// The notification environment.
    pub env: &'a mut NotifyEnv<'e>,
    /// The topology marks.
    pub marks: &'a mut Marks,
    /// The identity of a new face (`this` of `SmFace`, the owner of its surface).
    pub this_face: &'a dyn Fn(FaceId) -> ObjectRef,
}

/// A face made by [`create_face_in_region_from_surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceInRegion {
    /// `rpNewShell`: the new shell [`p7::make_face_with_curves`] reports, if any.
    pub shell: Option<ShellId>,
    /// `rpNewFace`.
    pub face: FaceId,
}

/// The faces made by [`create_faces_with_curves`] and [`make_faces_with_curves`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewFaces {
    /// `rpNewRegion` (`MakeFacesWithCurves` only): the region split off, if any.
    pub region: Option<RegionId>,
    /// `rpNewShell`: the new shell, if any.
    pub shell: Option<ShellId>,
    /// `rResultingFaces` / `rNewFaces`.
    pub faces: Vec<FaceId>,
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `v[i]`, which upstream reads unchecked.
fn at<T>(v: &[T], i: usize) -> Result<&T> {
    v.get(i)
        .ok_or_else(|| invalid("index past the input arrays"))
}

/// `SmTemporaryChangeValue<SmBoolean>(m_bEditingEnabled, TRUE)` around `f`.
fn with_editing<R>(b: &mut Brep, f: impl FnOnce(&mut Brep) -> Result<R>) -> Result<R> {
    let saved = b.editing_enabled();
    b.set_editing_enabled(true);
    let r = f(b);
    b.set_editing_enabled(saved);
    r
}

/// `SER(SM_ERR)` after a failed call: [`SolidError::Unported`] goes through.
fn as_sm_err(e: SolidError) -> SolidError {
    if e.is_unported() {
        e
    } else {
        SolidError::sm_err(OP)
    }
}

/// `if(SM_SUCCESS != CreatePlaneWith3DCurves(..)) return SM_ERR`: `SM_ERR`, except that
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (the port's marker of an
/// input upstream mishandles, e.g. `FindOuterLoops`' zero-area loop, UB-119) go through.
fn plane_failed(e: SolidError) -> SolidError {
    match e {
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => e,
        other => as_sm_err(other),
    }
}

// ---- B-rep editing callees -------------------------------------------------------------

/// `DeleteEdge(edge)` (default `pOptSurfaceToKeep = NULL`; unit `topology.brep_p4`).
fn delete_edge(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>, e: EdgeId) -> Result<()> {
    super::p4::delete_edge(k, b, &mut super::p4::DeleteEnv::from_notify(env), e, None)
}

/// `DeleteVertex(vertex)` (unit `topology.brep_p4`).
fn delete_vertex(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>, v: VertexId) -> Result<()> {
    super::p4::delete_vertex(k, b, &mut super::p4::DeleteEnv::from_notify(env), v)
}

// ---- CreateFaceInRegionFromSurface ----------------------------------------------------

/// `CreateFaceInRegionFromSurface`: makes a face in `region` (the infinite region for
/// `None`) on `surface` trimmed to `uv_domain` (a sub-domain of the surface's natural
/// domain), bounded by edges on the domain's natural trim curves
/// ([`Surface::create_natural_uv_trim_curves`], counter-clockwise from the `U` minimum
/// side, degenerate curves kept) through [`p7::make_face_with_curves`], and marks the
/// face's trim rectangular. Face containment in the region is not tested.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without surface; `SM_ERR` when `uv_domain` is not contained
/// in the natural domain (within `SM_EFF_ZERO`) or `MakeFaceWithCurves` fails; those of
/// `CreateNaturalUVTrimCurves`; [`SolidError::Unported`] (see the module docs).
pub fn create_face_in_region_from_surface(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<FaceInRegion> {
    // check inputs
    let surface = surface.ok_or_else(|| SolidError::null_pointer(OP))?;

    // Check for region default.
    let region = region.or(b.infinite_region());

    // check input - crUVDomain contained within the pSurface->GetNaturalUVDomain()
    if !uv_domain.is_contained_by(&surface.natural_uv_domain_with_nurb(k)?, EFF_ZERO) {
        return Err(SolidError::sm_err(OP));
    }

    with_editing(b, |b| {
        // get NewFace's Naturally Trimmed OuterLoop
        let natural =
            surface.create_natural_uv_trim_curves(k, uv_domain, SurfParamType::U, false)?;

        // UV curves must be parameterized the same as their Edges, which will be the
        // same as the 3d curves.
        let mut uv_curves: Vec<Curve> = Vec::with_capacity(natural.uv_curves.len());
        for (ii, mut uv) in natural.uv_curves.into_iter().enumerate() {
            let crv_domain = at(&natural.curves_3d, ii)?.natural_interval();
            ignore_status(uv.edit_parameterization(&crv_domain))?;
            uv_curves.push(Curve::from(uv));
        }

        // the old algorithm (sbUseOldAlgorithm is a constexpr TRUE)
        let n = natural.curves_3d.len();
        let input = FaceCurvesInput {
            loops: vec![n],
            curves_3d: Some(natural.curves_3d.into_iter().map(Some).collect()),
            uv_curves: Some(uv_curves),
            orientations: natural.orients,
            loop_points: Vec::new(),
        };
        let made = p7::make_face_with_curves(
            k,
            b,
            region,
            input,
            surface,
            uv_domain,
            OrientType::Same,
            None,
            fenv.this_face,
            fenv.env,
        )
        .map_err(as_sm_err)?;

        // set m_bRectangularTrim = TRUE
        let face = made
            .face
            .ok_or_else(|| invalid("MakeFaceWithCurves made no face"))?;
        b.face_mut(face)?.rectangular_trim = 1;

        Ok(FaceInRegion {
            shell: made.shell,
            face,
        })
    })
}

/// `CreateFaceFromSurface`: [`create_face_in_region_from_surface`] in the infinite
/// region; the new face.
///
/// # Errors
///
/// Those of [`create_face_in_region_from_surface`].
pub fn create_face_from_surface(
    k: &mut Kernel,
    b: &mut Brep,
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<FaceId> {
    let region = b.infinite_region();
    Ok(create_face_in_region_from_surface(k, b, region, surface, uv_domain, fenv)?.face)
}

// ---- CreateFacesFromSurface -----------------------------------------------------------

/// `CreateFacesFromSurface`: the faces of `surface` split at its knots of continuity
/// `continuity` or less (`U` first, then each piece in `V`; knots on a piece's boundary
/// within `SM_EFF_ZERO_SQRT` are skipped), each made by
/// [`create_face_in_region_from_surface`] over its natural domain in the infinite
/// region. `uv_domain` is not used (upstream: `NotUsed`).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without surface; those of `FindKnotsByMaxContinuity`,
/// `SplitAt` and [`create_face_in_region_from_surface`].
pub fn create_faces_from_surface(
    k: &mut Kernel,
    b: &mut Brep,
    surface: Option<Surface>,
    uv_domain: &Extent2d,
    continuity: ContinuityType,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<Vec<FaceId>> {
    let _ = uv_domain; // SM_REF1(crUVDomain)
    let surface = surface.ok_or_else(|| SolidError::null_pointer(OP))?;
    let mut new_faces = Vec::new();

    with_editing(b, |b| {
        // find surface knots where continuity is less than or equal to the given one
        let (u_knots, v_knots) = surface.find_knots_by_max_continuity(k, continuity, false)?;

        let mut splitted: Vec<Surface> = Vec::new();
        if u_knots.is_empty() {
            splitted.push(surface);
        } else {
            let mut sur = surface;
            for &param in &u_knots {
                let dom = sur.natural_uv_domain_with_nurb(k)?;
                let ivl = Extent1d::from_min_max(dom.min().x, dom.max().x);
                if ivl.is_value_on_boundary(param, EFF_ZERO_SQRT).0 {
                    continue;
                }
                let (l, r) = sur.split_at(k, param, SurfParamType::U)?;
                splitted.push(l);
                sur = r;
            }
            splitted.push(sur);
        }

        let mut sub_patches: Vec<Surface> = Vec::new();
        if v_knots.is_empty() {
            sub_patches.append(&mut splitted);
        } else {
            for mut sur in splitted {
                for &param in &v_knots {
                    let dom = sur.natural_uv_domain_with_nurb(k)?;
                    let ivl = Extent1d::from_min_max(dom.min().y, dom.max().y);
                    if ivl.is_value_on_boundary(param, EFF_ZERO_SQRT).0 {
                        continue;
                    }
                    let (l, r) = sur.split_at(k, param, SurfParamType::V)?;
                    sub_patches.push(l);
                    sur = r;
                }
                sub_patches.push(sur);
            }
        }

        // for every subpatch - create face
        for srf in sub_patches {
            let dom = srf.natural_uv_domain_with_nurb(k)?;
            let region = b.infinite_region();
            let made = create_face_in_region_from_surface(k, b, region, Some(srf), &dom, fenv)?;
            new_faces.push(made.face);
        }
        Ok(())
    })?;
    Ok(new_faces)
}

// ---- CreatePlanarFaceWith3DCurves -----------------------------------------------------

/// `CreatePlanarFaceWith3DCurves`: makes one face per outer loop of the coplanar
/// `curves` (any order and orientation) on the plane through them
/// ([`Surface::create_plane_with_3d_curves`] with `tol_3d`), each with its inner
/// loops as holes, through [`p7::make_face_with_curves`] in `region` (the infinite
/// region for `None`); returns the first face (`None` without outer loops).
///
/// # Errors
///
/// `SM_ERR` when the plane fails (open, degenerate or non-coplanar curves); those of
/// `MakeFaceWithCurves`.
pub fn create_planar_face_with_3d_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    curves: &[Curve],
    tol_3d: f64,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<Option<FaceId>> {
    // Check for region default.
    let region = region.or(b.infinite_region());

    // create the plane - returns an error for open, degenerate, or nonCoplanar curves
    let plane = Surface::create_plane_with_3d_curves(k, curves, tol_3d).map_err(plane_failed)?;
    let plane_domain = plane.plane.natural_uv_domain_with_nurb(k)?;

    let mut next_loop = 0usize;
    let mut next_curve = 0usize;
    let mut new_faces: Vec<FaceId> = Vec::new();
    for &this_num_loops in &plane.outer_loop_counts {
        let mut this_loops: Vec<usize> = Vec::new();
        let mut this_curves: Vec<Option<Curve>> = Vec::new();
        let mut this_orients: Vec<OrientType> = Vec::new();
        for _ in 0..this_num_loops {
            let num_curves = *at(&plane.loop_counts, next_loop)?;
            next_loop += 1;
            this_loops.push(num_curves);
            for _ in 0..num_curves {
                let ci = *at(&plane.ordered_curves, next_curve)?;
                // the curve itself goes into the new edge upstream: an exact copy, not
                // the copy constructor (whose knot fix-up would change the curve)
                this_curves.push(Some(at(curves, ci)?.clone_for_evaluation()?));
                this_orients.push(*at(&plane.orients, next_curve)?);
                next_curve += 1;
            }
        }

        let input = FaceCurvesInput {
            loops: this_loops,
            curves_3d: Some(this_curves),
            uv_curves: None,
            orientations: this_orients,
            loop_points: Vec::new(),
        };
        // (one surface for every face upstream: each face gets a copy)
        let made = p7::make_face_with_curves(
            k,
            b,
            region,
            input,
            plane.plane.clone(),
            &plane_domain,
            OrientType::Same,
            None,
            fenv.this_face,
            fenv.env,
        )?;
        // (upstream adds the face pointer as is)
        new_faces.push(
            made.face
                .ok_or_else(|| invalid("MakeFaceWithCurves made no face"))?,
        );
    }
    Ok(new_faces.first().copied())
}

// ---- CreatePlanarFacesWith3DCurves ----------------------------------------------------

/// `CreatePlanarFacesWith3DCurves`: makes faces bounded by the closed loops of the
/// coplanar `curves` (any order and orientation, nested to any level: outer loops
/// surround faces, the loops directly inside them voids, and so on) on the plane
/// through them ([`Surface::create_plane_with_3d_curves`] with the B-rep tolerance),
/// through [`create_faces_with_curves`]. `tol_3d` is not used (upstream: `NotUsed`).
///
/// # Errors
///
/// `SM_ERR` when the plane fails; those of [`create_faces_with_curves`].
pub fn create_planar_faces_with_3d_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    curves: &[Curve],
    tol_3d: f64,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<Vec<FaceId>> {
    let _ = tol_3d; // SM_REF1(d3DToleranceArg)

    // Check for region default.
    let region = region.or(b.infinite_region());
    let tol = b.tolerance();

    // create the plane - fails for non-planar curves, degenerate curves, open loops,
    // spine points
    let plane = Surface::create_plane_with_3d_curves(k, curves, tol).map_err(plane_failed)?;

    let mut ordered: Vec<Option<Curve>> = Vec::with_capacity(plane.ordered_curves.len());
    for &ci in &plane.ordered_curves {
        // (the curves themselves go into the new edges upstream: exact copies)
        ordered.push(Some(at(curves, ci)?.clone_for_evaluation()?));
    }
    let made = create_faces_with_curves(
        k,
        b,
        region,
        &plane.loop_counts,
        Some(ordered),
        &plane.orients,
        Some(plane.plane),
        fenv,
    )?;
    Ok(made.faces)
}

// ---- CreateFacesWithCurves ------------------------------------------------------------

/// `CreateFacesWithCurves`: makes faces on `surface` bounded by loops of 3D curves
/// (`loops` curves per loop, in any order and orientation; no touching loops, no vertex
/// loops). First a face over the surface's natural domain is made
/// ([`create_face_in_region_from_surface`]), then each loop's curves become edges in the
/// face its first point lies in ([`p5::make_edge_in_face`] between vertex loops), each
/// closed loop splitting off a face, and finally the natural face and the faces at odd
/// nesting levels are deleted (`sm_MarkLoopsToDelete`), with the natural boundary's
/// edges and vertices.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without curves or for a null curve; `SM_ERR` when the natural
/// face has more than one loop, a face is split before its loop is closed or not at
/// its end, or a deleted face is not one of the new faces; those of the callees;
/// [`SolidError::InvalidInput`] without surface or for arrays shorter than the loop
/// counts; [`SolidError::Unported`] (see the module docs).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream routine
pub fn create_faces_with_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    loops: &[usize],
    curves_3d: Option<Vec<Option<Curve>>>,
    orientations: &[OrientType],
    surface: Option<Surface>,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<NewFaces> {
    // Check for region default.
    let region = region.or(b.infinite_region());

    with_editing(b, |b| {
        let mut out = NewFaces::default();
        let mut curves_3d = curves_3d.ok_or_else(|| SolidError::null_pointer(OP))?;
        let tol = b.tolerance();

        // First create the big face covering the whole surface.
        // (upstream reads pSurface->GetNaturalUVDomain() before its null check)
        let surface = surface.ok_or_else(|| invalid("no surface (upstream dereferences null)"))?;
        let natural = surface.natural_uv_domain_with_nurb(k)?;
        let big = create_face_in_region_from_surface(k, b, region, Some(surface), &natural, fenv)?;
        out.shell = big.shell;
        let new_face = big.face;

        let mut faces: Vec<Option<FaceId>> = vec![Some(new_face)];

        let face_loops = face2::loops(b, new_face, false, None)?;
        if face_loops.len() != 1 {
            return Err(SolidError::sm_err(OP));
        }
        // The loop we will have to delete
        let big_loop = *at(&face_loops, 0)?;

        let mut curve_total = 0usize;
        for &n_edges in loops {
            let mut v: Option<VertexId> = None; // the last vertex of the current loop
            let mut first_v: Option<VertexId> = None;
            let mut f: Option<FaceId> = None;

            for jj in 0..n_edges {
                let slot = curve_total + jj;
                let curve = curves_3d
                    .get_mut(slot)
                    .ok_or_else(|| invalid("loop counts exceed the curves"))?
                    .take();
                let last_edge = jj + 1 == n_edges;
                let curve = curve.ok_or_else(|| SolidError::null_pointer(OP))?;

                let ivl = curve.natural_interval();
                let (start_param, end_param, orient) =
                    if *at(orientations, slot)? == OrientType::Opposite {
                        (ivl.max(), ivl.min(), OrientType::Opposite)
                    } else {
                        (ivl.min(), ivl.max(), OrientType::Same)
                    };
                let start_pt = curve.evaluate_point(k, start_param)?;
                let end_pt = curve.evaluate_point(k, end_param)?;

                // find the face in which to create a new loop
                let face = if let Some(face) = f {
                    face
                } else {
                    {
                        if v.is_some() {
                            return Err(SolidError::sm_err(OP));
                        }
                        let face = if faces.len() == 1 {
                            new_face
                        } else {
                            let mut found = None;
                            for &f1 in faces.iter().flatten() {
                                let mut pc = PointClassification::<EntityRef>::new(tol);
                                qf1::point_3d_classify(
                                    k,
                                    b,
                                    f1,
                                    start_pt,
                                    tol,
                                    false,
                                    &mut pc,
                                    qf1::Point3dClassifyOptions::default(),
                                )?;
                                if pc.point_class() == PointClassType::Face {
                                    found = Some(f1);
                                    break;
                                }
                            }
                            // if face not found then use first "big" face
                            match found {
                                Some(face) => face,
                                None => faces
                                    .first()
                                    .copied()
                                    .flatten()
                                    .ok_or_else(|| SolidError::null_pointer(OP))?,
                            }
                        };
                        f = Some(face);

                        // Make the first vertex of the loop:
                        let (_loop, nv) = p9::make_vertex_loop(
                            b,
                            face,
                            start_pt,
                            OrientType::Opposite,
                            fenv.env,
                        )?;
                        vertex::set_tolerance(b, nv, tol, true, true)?;
                        v = Some(nv);
                        first_v = Some(nv);
                        face
                    }
                };

                // Make a new vertex if not the last edge (if last edge, take the first
                // vertex of the loop)
                let next_v = if last_edge {
                    first_v
                } else {
                    let (_loop, nv) =
                        p9::make_vertex_loop(b, face, end_pt, OrientType::Opposite, fenv.env)?;
                    vertex::set_tolerance(b, nv, tol, true, true)?;
                    Some(nv)
                };

                // And the real action: make the edge for this curve:
                let mut made = EdgeInFace::default();
                let editing = b.editing_enabled();
                p5::make_edge_in_face(
                    k,
                    b,
                    editing,
                    Some(face),
                    v,
                    next_v,
                    curve,
                    None::<BSplineCurve>,
                    ivl,
                    orient,
                    tol,
                    &mut made,
                    Some(&mut *fenv.env),
                )?;

                // A new face must be created when the loop exhausts:
                if last_edge && made.face.is_none() {
                    return Err(SolidError::sm_err(OP));
                }
                // Register any new face created:
                if let Some(nf) = made.face {
                    // new face must not be created before the loop is exhausted
                    if !last_edge {
                        return Err(SolidError::sm_err(OP));
                    }
                    faces.push(Some(nf));
                }

                // Update the current vertex:
                v = next_v;
            }
            curve_total += n_edges;
        }

        // What remains is deleting the superfluous faces
        let mut loops_to_delete: Vec<LoopId> = Vec::new();
        p1::mark_loops_to_delete(b, &mut loops_to_delete, big_loop, 0)?;

        for &l in &loops_to_delete {
            let face = loopuse::loop_face(b, l)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let loop_edges = loopuse::loop_edges(b, l)?;
            let loop_vertices = loopuse::loop_vertices(b, l)?;

            p3::delete_face(
                k,
                b,
                fenv.env,
                fenv.marks,
                face,
                DeleteFaceOptions {
                    delete_edges_vertices: false,
                    do_region_nesting: true,
                    region_to_keep: None,
                },
            )?;

            let Some(slot) = faces.iter_mut().find(|x| **x == Some(face)) else {
                return Err(SolidError::sm_err(OP));
            };
            *slot = None;

            // For the outermost loop, the edges and vertices need to be deleted too:
            if l == big_loop {
                for e in loop_edges.into_iter().flatten() {
                    ignore_status(delete_edge(k, b, fenv.env, e))?;
                }
                for v in loop_vertices.into_iter().flatten() {
                    ignore_status(delete_vertex(k, b, fenv.env, v))?;
                }
            }
        }

        // Sort out which face remained:
        out.faces = faces.into_iter().flatten().collect();
        Ok(out)
    })
}

// ---- MakeFacesWithCurves --------------------------------------------------------------

/// `MakeFacesWithCurves`: [`p7::make_face_with_curves`] on `surface` (over `uv_domain`,
/// `orientation`) when the surface has no `C0` knots; otherwise the surface is split at
/// its creases (in `U` when it has `U` creases, else in `V`), the loop curves are split
/// where the crease iso curves cross them and the crossing iso curve segments are
/// inserted into the loops, each loop is assigned to the sub-patch containing it, and
/// one face is made per outer loop group of each sub-patch (failures skipped). With
/// UV curves the loops are in UV and the iso curves are UV lines.
///
/// # Errors
///
/// Those of `FindKnotsByMaxContinuity`; in the split case `SM_ERR_INVALID_INPUT`
/// without curves, those of `SplitAt`; else those of `MakeFaceWithCurves`;
/// [`SolidError::InvalidInput`] (see the module docs); [`SolidError::Unported`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream routine
pub fn make_faces_with_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    input: FaceCurvesInput,
    surface: Surface,
    uv_domain: &Extent2d,
    orientation: OrientType,
    fenv: &mut FaceBuildEnv<'_, '_>,
) -> Result<NewFaces> {
    let mut out = NewFaces::default();

    // set input region (NULL = infinite region)
    let region = region.or(b.infinite_region());

    // Check continuity of the surface
    let (u_knots, v_knots) = surface.find_knots_by_max_continuity(k, ContinuityType::C0, false)?;

    // when no discontinuities
    if u_knots.is_empty() && v_knots.is_empty() {
        let made = p7::make_face_with_curves(
            k,
            b,
            region,
            input,
            surface,
            uv_domain,
            orientation,
            None,
            fenv.this_face,
            fenv.env,
        )?;
        out.region = made.region;
        out.shell = made.shell;
        out.faces.extend(made.face);
        return Ok(out);
    }

    // splitting at creases (C0 knots): U splits if any, else V splits
    let srf_dir = if u_knots.is_empty() {
        SurfParamType::V
    } else {
        SurfParamType::U
    };
    let knots_at_split = if u_knots.is_empty() { v_knots } else { u_knots };
    let zone_tol = tol::context_zone_tol_3d(k, Some(&k.context), SmallTopoType::NormalSize);
    let curve_dim: u32 = if input.uv_curves.is_some() { 2 } else { 3 };

    // The curve store of the SmCurveLoops (its curves are keys into it).
    let FaceCurvesInput {
        loops: curve_loop_counts,
        curves_3d,
        uv_curves,
        orientations,
        loop_points,
    } = input;
    let mut store: Vec<Curve> = Vec::new();
    let mut loops = {
        let (uv_keys, keys_3d) = if let Some(uv) = uv_curves {
            let keys: Vec<CurveKey> = (0..uv.len()).map(CurveKey).collect();
            store.extend(uv);
            (Some(keys), None)
        } else if let Some(c3) = curves_3d {
            let mut keys = Vec::new();
            for c in c3 {
                // (a null curve is read through the SmCurveLoops upstream)
                let c = c.ok_or_else(|| invalid("null 3D curve (upstream dereferences null)"))?;
                keys.push(CurveKey(store.len()));
                store.push(c);
            }
            (None, Some(keys))
        } else {
            (None, None)
        };
        CurveLoops::from_loops(
            &curve_loop_counts,
            uv_keys.as_deref(),
            keys_3d.as_deref(),
            &orientations,
        )
    };
    // check state - valid sCurveLoops data structure
    if !loops.is_valid() {
        return Err(SolidError::Upstream {
            operation: OP,
            code: SM_ERR_INVALID_INPUT,
            entity: None,
        });
    }

    // Create iso curves at each discontinuity.
    let mut iso_keys: Vec<CurveKey> = Vec::new();
    for &knot in &knots_at_split {
        let iso = if curve_dim == 2 {
            let du = uv_domain.u_interval();
            let dv = uv_domain.v_interval();
            let (lo, hi) = if srf_dir == SurfParamType::U {
                (
                    Point3::new(knot, dv.min(), 0.0),
                    Point3::new(knot, dv.max(), 0.0),
                )
            } else {
                (
                    Point3::new(du.min(), knot, 0.0),
                    Point3::new(du.max(), knot, 0.0),
                )
            };
            Curve::from(Line::from_points(lo, hi, 2))
        } else {
            // (status ignored upstream: a failed curve leaves a null pointer that is
            // intersected next)
            ignored(surface.create_iso_parametric_curve(k, srf_dir, knot, 0.0, None, false))?
                .ok_or_else(|| invalid("iso curve failed (upstream dereferences null)"))?
                .curve
        };
        iso_keys.push(CurveKey(store.len()));
        store.push(iso);
    }

    // Intersect each iso curve with each input Loop curve. (The list is linked
    // backwards upstream: the last iso curve heads it.)
    let xsect_tol = tol::xsect_tol_3d_from_zones(
        k,
        tol::zone_tol_3d(k, SmallTopoType::NormalSize),
        tol::zone_tol_3d(k, SmallTopoType::NormalSize),
    );
    let mut crv_ints: Vec<CurveIntersections> = Vec::new();
    for (ii, &iso_key) in iso_keys.iter().enumerate() {
        let knot = *at(&knots_at_split, ii)?;
        let mut ci = CurveIntersections::new(Some(iso_key), srf_dir, knot);
        let iso = store.get(iso_key.0).ok_or_else(|| invalid("curve store"))?;
        let iso_domain = iso.natural_interval();
        let mut loop_start = 0usize;
        for &count in &curve_loop_counts {
            for kk in loop_start..loop_start + count {
                let loop_key = *at(&loops.curves, kk)?;
                let loop_curve = store
                    .get(loop_key.0)
                    .ok_or_else(|| invalid("curve store"))?;
                let crv_domain = loop_curve.natural_interval();
                // Intersect the curves (status ignored upstream).
                let mut sols = ignored(iso.global_curve_intersect::<LoopObject>(
                    k,
                    &iso_domain,
                    loop_curve,
                    &crv_domain,
                    xsect_tol,
                    &CurveIntersectOptions::default(),
                ))?
                .unwrap_or_default();
                for ll in 0..sols.len() {
                    if let Some(s) = sols.get_mut(ll) {
                        s.objects[0] = Some(LoopObject::Curve(iso_key));
                        s.objects[1] = Some(LoopObject::Curve(loop_key));
                    }
                }
                ci.solutions.append(&sols);
            }
            loop_start += count;
        }
        crv_ints.push(ci);
    }
    crv_ints.reverse();

    // For each intersection inside a loop curve: split the loop curve there.
    for ci_index in 0..crv_ints.len() {
        let mut jj = 0usize;
        while jj < crv_ints.get(ci_index).map_or(0, |c| c.solutions.len()) {
            let Some(sol) = crv_ints
                .get(ci_index)
                .and_then(|c| c.solutions.get(jj))
                .copied()
            else {
                break;
            };
            jj += 1;
            // Skip unexpected NULL pLoopCrvs (SE(SM_ERR): status only)
            let Some(LoopObject::Curve(loop_key)) = sol.objects[1] else {
                continue;
            };
            let loop_crv = store
                .get(loop_key.0)
                .ok_or_else(|| invalid("curve store"))?;
            let loop_ivl = loop_crv.natural_interval();
            let param = sol.start.parameters[1];
            let tol_1d = map_curve_tol_to_1d(k, loop_crv, zone_tol, param)?;

            // when the parameter is strictly inside the loop curve: split it
            if loop_ivl.contains_value(param, -tol_1d) {
                let new_curve = {
                    let c = store
                        .get_mut(loop_key.0)
                        .ok_or_else(|| invalid("curve store"))?;
                    // (status ignored upstream: a null new curve then goes into the loops)
                    ignored(c.create_by_split_curve(k, param))?
                        .ok_or_else(|| invalid("split failed (upstream inserts null)"))?
                };
                let new_key = CurveKey(store.len());
                store.push(new_curve);

                // insert the split LoopCurve children into the loops (status ignored)
                ignore_status(loops.insert_into_loops(new_key, loop_key))?;

                // add new Solution for the new curve to SolArray
                let mut new_sol = sol;
                new_sol.objects[1] = Some(LoopObject::Curve(new_key));
                if let Some(c) = crv_ints.get_mut(ci_index) {
                    c.solutions.insert_at(jj, &new_sol, 1);
                }

                // other intersections on the loop curve may now be on the new part
                ignore_status(loops.update_solution_objects(
                    k,
                    &store,
                    &mut crv_ints,
                    loop_key,
                    new_key,
                    zone_tol,
                ))?;
            }
        }
    }

    // For each iso curve, pair its sorted intersections into the iso segments that
    // split the loops.
    for ci_index in 0..crv_ints.len() {
        let Some(ci) = crv_ints.get(ci_index).cloned() else {
            break;
        };
        let mut new_sols: SolutionArray<LoopObject> = SolutionArray::new();
        for ii in 0..ci.solutions.len() {
            if let Some(s) = ci.solutions.get(ii) {
                // (IsEqual reads the natural intervals of the solutions' curves)
                ignore_status(new_sols.add_sorted_solution_in(
                    s,
                    SortKey::ByFirstParameter,
                    &StoreDomains(&store),
                ))?;
            }
        }
        let iso_key = ci
            .iso_curve
            .ok_or_else(|| invalid("intersections without iso curve"))?;
        let mut seg = IsoXSectSegment::new(iso_key);
        let mut in_segment = false;

        let n = new_sols.len();
        let mut ii = 0usize;
        while ii < n {
            // check state - ii index is in sNewSolArray bounds
            if ii + 1 >= n {
                break;
            }
            let (Some(sol1), Some(sol2)) =
                (new_sols.get(ii).copied(), new_sols.get(ii + 1).copied())
            else {
                break;
            };
            // processing two consecutive intersections
            ii += 1;
            'pair: {
                // skip Range solutions
                if sol1.solution_type == SolutionType::RangeOfValues
                    || sol2.solution_type == SolutionType::RangeOfValues
                {
                    break 'pair;
                }
                let iso_param1 = sol1.start.parameters[0];
                let crv_param1 = sol1.start.parameters[1];
                let crv_param2 = sol2.start.parameters[1];

                // skip tangent intersections
                let Some(LoopObject::Curve(lc1)) = sol1.objects[1] else {
                    break 'pair;
                };
                let Some(LoopObject::Curve(lc2)) = sol2.objects[1] else {
                    break 'pair;
                };
                if loops.is_same_side_intersection(
                    k, &store, lc1, crv_param1, &ci, &surface, uv_domain, zone_tol,
                )? {
                    break 'pair;
                }
                if loops.is_same_side_intersection(
                    k, &store, lc2, crv_param2, &ci, &surface, uv_domain, zone_tol,
                )? {
                    break 'pair;
                }

                // Get CurveLoop's indices for the LoopCrvs
                let Some(indx1) = loops.curves.iter().position(|&c| c == lc1) else {
                    break 'pair;
                };
                let Some(indx2) = loops.curves.iter().position(|&c| c == lc2) else {
                    break 'pair;
                };

                if in_segment {
                    // Exiting a usable segment of the iso curve
                    let (enter, leave) = match loops.find_entering_and_leaving(indx1, indx2) {
                        Ok(x) => x,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => break 'pair,
                    };
                    seg.end_enter = enter;
                    seg.end_leave = leave;
                    if iso_param1 > seg.interval.min() {
                        ignore_status(seg.interval.set_max(iso_param1))?;
                        seg.orient = OrientType::Same;
                    } else {
                        ignore_status(seg.interval.set_min(iso_param1))?;
                        seg.orient = OrientType::Opposite;
                    }
                    // the segment is complete - insert it into the loops
                    ignore_status(loops.insert_iso_xsect_segment(k, &mut store, &mut seg))?;
                    in_segment = false;
                } else {
                    // Entering a usable segment of the iso curve
                    let (enter, leave) = match loops.find_entering_and_leaving(indx1, indx2) {
                        Ok(x) => x,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => break 'pair,
                    };
                    seg.start_enter = enter;
                    seg.start_leave = leave;
                    ignore_status(seg.interval.set_min_max(iso_param1, iso_param1))?;
                    in_segment = true;
                }
            }
            ii += 1;
        }
    }

    // Split surface into sub-patches at its creases.
    let mut sub_patches: Vec<Surface> = Vec::new();
    let mut to_split: Option<Surface> = None;
    for &param in &knots_at_split {
        let (l, r) = to_split
            .as_ref()
            .unwrap_or(&surface)
            .split_at(k, param, srf_dir)?;
        // save all Left split children; continue splitting the right one
        sub_patches.push(l);
        to_split = Some(r);
    }
    // Add in the last right child.
    sub_patches.extend(to_split);

    // Associate each curve loop with a surface patch.
    let num_sub_patches = sub_patches.len();
    let num_loops = loops.loop_counts.len();
    let unassigned = num_sub_patches + 2;
    let mut which_sub_patch: Vec<usize> = vec![unassigned; num_loops];
    let dir_uv = if srf_dir == SurfParamType::V {
        Vec2::new(0.0, 1.0)
    } else {
        Vec2::new(1.0, 0.0)
    };
    let mut crv_pt: Option<Point3> = None;
    let mut loop_start = 0usize;
    for ii in 0..num_loops {
        let num_loop_curves = *at(&loops.loop_counts, ii)?;
        let mut found_patch = false;
        // the seam curve was put in as the first loop curve
        let start_idx = if num_loop_curves > 1 {
            loop_start + 1
        } else {
            loop_start
        };
        for jj in start_idx..loop_start + num_loop_curves {
            let key = *at(&loops.curves, jj)?;
            let loop_curve = store.get(key.0).ok_or_else(|| invalid("curve store"))?;
            let crv_domain = loop_curve.natural_interval();
            // near MidPt evaluation (status ignored: the previous point is kept)
            if let Some(p) = ignored(loop_curve.evaluate_point(k, crv_domain.evaluate(0.48)))? {
                crv_pt = Some(p);
            }
            let pt = crv_pt
                .ok_or_else(|| invalid("no curve point (upstream reads it uninitialised)"))?;
            let mut pt_uv = Point2::new(pt.x, pt.y);

            for (kk, sub) in sub_patches.iter().enumerate() {
                let sub_domain = sub.natural_uv_domain_with_nurb(k)?;
                if curve_dim == 3 {
                    let r = ignored(sub.drop_point::<LoopObject>(
                        k,
                        pt,
                        &sub_domain,
                        &DropPointOptions {
                            operation: SolverOperation::Intersect,
                            ..DropPointOptions::default()
                        },
                    ))?;
                    let Some(d) = r else { continue };
                    pt_uv = d.result_uv;
                    if !d.success || d.gap > zone_tol {
                        continue;
                    }
                }
                let param_domain = if srf_dir == SurfParamType::U {
                    sub_domain.u_interval()
                } else {
                    sub_domain.v_interval()
                };
                let param = if srf_dir == SurfParamType::U {
                    pt_uv.x
                } else {
                    pt_uv.y
                };
                let mut param_tol = crate::surface::surface::p1::map_to_2d_dir(
                    k, zone_tol, pt_uv, dir_uv, &surface,
                )?;
                // Sanity check: limit tiny derivative's huge tolerances
                let tol_lim = param_domain.length() / 100.0;
                if param_tol < tol_lim {
                    param_tol = tol_lim;
                }
                if param_domain.contains_value(param, -param_tol) {
                    found_patch = true;
                    if let Some(w) = which_sub_patch.get_mut(ii) {
                        *w = kk;
                    }
                    break;
                }
            }
            if found_patch {
                break;
            }
        }
        loop_start += num_loop_curves;
    }

    // For each sub-patch: collect its loops, find its outer loop groups and make a face
    // per group.
    let mut surface_uses: Vec<usize> = vec![0; num_sub_patches];
    let mut loop_points = loop_points; // passed in on the first call only
    for (ii, sub_patch) in sub_patches.iter().enumerate() {
        let mut patch_loops = CurveLoops::new();
        for (jj, &w) in which_sub_patch.iter().enumerate() {
            if w == ii {
                ignore_status(patch_loops.append_loop(&loops, jj))?;
            }
        }
        if patch_loops.loop_counts.is_empty() {
            continue;
        }

        // FindOuterLoops() needs a surface normal.
        let mut norm = Vec3::new(0.0, 0.0, 1.0);
        if curve_dim == 3 {
            let dom = sub_patch.natural_uv_domain_with_nurb(k)?;
            match ignored(sub_patch.evaluate_normal(k, dom.mid(), Side::Right, Side::Right))? {
                Some(nv) => norm = nv,
                None => continue,
            }
        }

        let mut ordered: Vec<usize> = patch_loops.curves.iter().map(|c| c.0).collect();
        let outer_loop_counts = ignored(find_outer_loops(
            k,
            &store,
            &mut ordered,
            &mut patch_loops.loop_counts,
            &mut patch_loops.orients,
            &mut norm,
            zone_tol,
        ))?
        .unwrap_or_default();
        patch_loops.curves = ordered.into_iter().map(CurveKey).collect();

        // Create a Face for each outer loop group.
        let mut group = CurveLoops::new();
        let mut group_start = 0usize;
        for &num_in_group in &outer_loop_counts {
            group.reset();
            for kk in 0..num_in_group {
                ignore_status(group.append_loop(&patch_loops, group_start + kk))?;
            }
            group_start += num_in_group;

            // (upstream counts with the patch's loop counts: module docs)
            let mut single_loops: Vec<usize> = Vec::new();
            let mut single_curves: Vec<Curve> = Vec::new();
            let mut single_orients: Vec<OrientType> = Vec::new();
            let mut ls = 0usize;
            for ll in 0..num_in_group {
                let num_loop_curves = *at(&patch_loops.loop_counts, ll)?;
                single_loops.push(num_loop_curves);
                for mm in ls..ls + num_loop_curves {
                    let key = *at(&group.curves, mm)?;
                    single_curves.push(
                        store
                            .get(key.0)
                            .ok_or_else(|| invalid("curve store"))?
                            .clone(),
                    );
                    single_orients.push(*at(&group.orients, mm)?);
                }
                ls += num_loop_curves;
            }

            // copy sub patches that have already been used
            let src_to_use = sub_patch.clone();
            let sub_domain = src_to_use.natural_uv_domain_with_nurb(k)?;
            let face_input = FaceCurvesInput {
                loops: single_loops,
                curves_3d: (curve_dim == 3)
                    .then(|| single_curves.iter().cloned().map(Some).collect()),
                uv_curves: (curve_dim == 2).then_some(single_curves),
                orientations: single_orients,
                loop_points: loop_points.clone(),
            };
            let r = p7::make_face_with_curves(
                k,
                b,
                region,
                face_input,
                src_to_use,
                &sub_domain,
                orientation,
                None,
                fenv.this_face,
                fenv.env,
            );
            match r {
                Ok(made) => {
                    if let Some(face) = made.face {
                        if made.region.is_some() {
                            out.region = made.region;
                        }
                        if made.shell.is_some() {
                            out.shell = made.shell;
                        }
                        out.faces.push(face);
                        if let Some(u) = surface_uses.get_mut(ii) {
                            *u += 1;
                        }
                    }
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {}
            }

            // sLoopPoints only used on the 1st pass
            loop_points.clear();
        }
    }
    let _ = surface_uses; // (unused sub-patches are dropped)
    Ok(out)
}

/// `SmTol::MapTo1d(tol, param, curve)`: `tol` over the curve's first derivative at
/// `param` (its `Evaluate` status ignored: a zero derivative then).
fn map_curve_tol_to_1d(k: &Kernel, curve: &Curve, tol_3d: f64, param: f64) -> Result<f64> {
    let deriv = ignored(curve.evaluate(k, param, 1, true, true))?
        .and_then(|d| d.get(1).copied())
        .unwrap_or(Vec3::new(0.0, 0.0, 0.0));
    Ok(tol::map_to_1d(tol_3d, deriv, &curve.natural_interval()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{SM_ERR, SM_ERR_NULL_POINTER};
    use crate::geom::attribute::AttributeStore;
    use crate::math::Point2;
    use crate::surface::plane::Plane;

    fn code(e: &SolidError) -> Option<i32> {
        match e {
            SolidError::Upstream { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Runs `f` on a new B-rep (`new SmBrep()`) with an arena-only notification store.
    fn with_brep<R>(
        f: impl FnOnce(&mut Kernel, &mut Brep, &mut FaceBuildEnv<'_, '_>) -> R,
    ) -> (R, Brep) {
        let mut k = Kernel::default();
        let mut b = p3::new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &loopuse::notify_ref,
        };
        let mut marks = Marks::default();
        let this_face = |f: FaceId| loopuse::notify_ref(EntityRef::Face(f));
        let mut fenv = FaceBuildEnv {
            env: &mut env,
            marks: &mut marks,
            this_face: &this_face,
        };
        let r = f(&mut k, &mut b, &mut fenv);
        (r, b)
    }

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ))
    }

    fn unit_box() -> Extent2d {
        Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))
    }

    fn line(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(
            Line::create_line_segment(
                3,
                Point3::new(a[0], a[1], a[2]),
                Point3::new(b[0], b[1], b[2]),
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn null_surfaces_and_curves_are_null_pointer_statuses() {
        let (r, _) = with_brep(|k, b, fenv| {
            create_face_in_region_from_surface(k, b, None, None, &unit_box(), fenv)
        });
        assert_eq!(code(&r.unwrap_err()), Some(SM_ERR_NULL_POINTER));
        let (r, _) = with_brep(|k, b, fenv| {
            create_faces_from_surface(k, b, None, &unit_box(), ContinuityType::C0, fenv)
        });
        assert_eq!(code(&r.unwrap_err()), Some(SM_ERR_NULL_POINTER));
        let (r, _) = with_brep(|k, b, fenv| {
            create_faces_with_curves(k, b, None, &[4], None, &[], Some(plane()), fenv)
        });
        assert_eq!(code(&r.unwrap_err()), Some(SM_ERR_NULL_POINTER));
    }

    #[test]
    fn domain_outside_the_natural_one_is_sm_err_and_editing_is_untouched() {
        // a bilinear patch over [0, 1]^2; [-0.5, 1] x [0, 1] is not contained
        let bs = crate::nurbs::b_spline_surface::BSplineSurface::create_bilinear_surface(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .unwrap();
        let dom = Extent2d::from_points(Point2::new(-0.5, 0.0), Point2::new(1.0, 1.0));
        let (r, b) = with_brep(|k, b, fenv| {
            create_face_in_region_from_surface(k, b, None, Some(Surface::from(bs)), &dom, fenv)
        });
        assert_eq!(code(&r.unwrap_err()), Some(SM_ERR));
        assert!(!b.editing_enabled());
    }

    #[test]
    fn open_or_empty_curve_sets_make_no_plane() {
        let open = vec![
            line([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            line([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
        ];
        for curves in [open, Vec::new()] {
            let (r, _) = with_brep(|k, b, fenv| {
                create_planar_face_with_3d_curves(k, b, None, &curves, 1e-6, fenv)
            });
            assert_eq!(code(&r.unwrap_err()), Some(SM_ERR));
            let (r, _) = with_brep(|k, b, fenv| {
                create_planar_faces_with_3d_curves(k, b, None, &curves, 1e-6, fenv)
            });
            assert_eq!(code(&r.unwrap_err()), Some(SM_ERR));
        }
    }

    #[test]
    fn a_square_reaches_the_face_construction_and_restores_editing() {
        let square = vec![
            line([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            line([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
            line([1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
            line([0.0, 1.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        // the wire edges are made; the face itself stops in MakeFaceTopology
        let (r, b) = with_brep(|k, b, fenv| {
            create_planar_face_with_3d_curves(k, b, None, &square, 1e-6, fenv)
        });
        match r {
            Ok(face) => assert!(face.is_some()),
            Err(e) => assert!(e.is_unported(), "{e}"),
        }
        assert!(!b.editing_enabled());
        let (r, b) = with_brep(|k, b, fenv| {
            create_face_in_region_from_surface(k, b, None, Some(plane()), &unit_box(), fenv)
        });
        match r {
            Ok(m) => assert_eq!(b.face(m.face).unwrap().rectangular_trim, 1),
            Err(e) => assert!(e.is_unported(), "{e}"),
        }
        assert!(!b.editing_enabled());
    }
}

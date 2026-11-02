//! Tools for trimmed surfaces (`SmTrimmingTools`): ordering curves into planar loops and
//! finding the outer loops, trimming a surface with model-space curves, checking and
//! repairing the trim loops of a face (UV gaps, backwards seam curves, loop order and
//! orientation), and the UV iso-curve tests these use. The loop helper classes
//! (`SmCurveLoops`, `SmIsoXSectSegment`, `SmCurveIntersections`) are in [`curve_loops`].
//!
//! Upstream: `source/SMLib/src/SmTrimmingTools.cpp`, `source/SMLib/inc/SmTrimmingTools.h`.
//!
//! | Rust                                         | Upstream                                                  |
//! |----------------------------------------------|-----------------------------------------------------------|
//! | [`TYPE`], [`class_type`]                     | `SmTrimmingTools_TYPE`, `SmTrimmingTools::GetClassType`   |
//! | [`trim_surface_with_model_space_curves`], [`TrimInput`] | `SmTrimmingTools::TrimSurfaceWithModelSpaceCurves` (region overload) |
//! | [`loop_uv_curves_present`], [`face_uv_curves_present`] | `SmTrimmingTools::AreUVCurvesPresent` (loop, face) |
//! | [`create_uv_trim_curves`]                    | `SmTrimmingTools::CreateUVTrimCurves` (face)              |
//! | [`clear_uv_trim_curves`]                     | `SmTrimmingTools::ClearUVTrimCurves` (face)               |
//! | [`split_at_seams`]                           | `SmTrimmingTools::SplitAtSeams` (face)                    |
//! | [`fix_loop_tiny_parameter_space_gaps`], [`fix_face_tiny_parameter_space_gaps`] | `SmTrimmingTools::FixTrimLoopTinyParameterSpaceGaps` (loop, face) |
//! | [`ensure_last_edgeuse_uv_correct`]           | `SmTrimmingTools::EnsureLastEdgeuseUVCorrect`             |
//! | [`fix_backwards_outer_loop_on_seam`]         | `SmTrimmingTools::FixBackwardsOuterLoopOnSeam`            |
//! | [`fix_loop_orientation`], [`fix_face_loop_orientations`] | `SmTrimmingTools::FixTrimLoopOrientation` (loop, face) |
//! | [`fix_trim_loop_order`]                      | `SmTrimmingTools::FixTrimLoopOrder`                       |
//! | [`order_curves_into_loops`], [`OrderedLoops`] | `SmTrimmingTools::OrderCurvesIntoLoops`                  |
//! | [`find_outer_loops`]                         | `SmTrimmingTools::FindOuterLoops`                         |
//! | [`check_face`], [`check_faces`]              | `SmTrimmingTools::CheckFace`, `CheckFaces`                |
//! | [`is_loop_uv_gap_within_tolerance`]          | `sm_IsLoopUVGapWithinTolerance` (static)                  |
//! | [`check_loop`]                               | `SmTrimmingTools::CheckLoop`                              |
//! | [`edge_uv_extremes`], [`EdgeUvExtremes`], [`edge_uv_ends`] | `SmTrimmingTools::GetEdgeUVExtremes` (with and without weights) |
//! | [`surface_continuity_check`]                 | `SmTrimmingTools::SurfaceContinuityCheck`                 |
//! | [`mirror_curve`]                             | `SmTrimmingTools::MirrorCurve`                            |
//! | [`is_iso_curve_at`]                          | `SmTrimmingTools::IsIsoCurve(curve, side, low, tol)`      |
//! | [`is_iso_curve`]                             | `SmTrimmingTools::IsIsoCurve(curve, &side, &param)`       |
//! | [`is_iso_curve_on_seam`]                     | `SmTrimmingTools::IsIsoCurveOnSeam`                       |
//! | [`min`]                                      | `SmTrimmingTools::Min`                                    |
//!
//! Deviations:
//! - **Object model.** Faces, loops and edge uses are ids into a [`Brep`]; the static
//!   methods are free functions taking the B-rep (and the [`Kernel`] where tolerances or
//!   solves are read). Out-params are return values. A UV trim curve is the
//!   [`Curve`] an edge use (or its mate, `GetUVTrimCurve`) holds; edits go to that slot.
//!   A `SmBSplineCurve` method on it runs on the B-spline kind, or on the B-spline part
//!   of an `SmLine`, `SmEllipse`, `SmHyperbola` or `SmParabola` (all `SmBSplineCurve`s;
//!   a conic whose `MakeNurb` failed has the null NURB upstream dereferences:
//!   [`SolidError::InvalidInput`]).
//! - **Null pointers.** `SM_ASSERT`s are compiled out of the reference build. Where
//!   upstream dereferences a null pointer (a loop without loop use, a face without
//!   surface or loops, an edge use whose UV trim curve or vertex use is missing where it
//!   is used, ...) the port returns [`SolidError::InvalidInput`]. `SER` / `NER` returns
//!   are [`SolidError::Upstream`] with the upstream code (`SM_ERR`, `SM_ERR_NULL_POINTER`,
//!   `SM_ERR_INVALID_INPUT` for a failed `Unitize`).
//! - **Projected seam endpoints (WR-440, #530), opt-in.** With
//!   `Kernel::fixes.check_loop_projected_seam_ends` (off by default: upstream's raw UV
//!   jump), `CheckLoop` lifts only an end obtained from vertex projection to its
//!   nearest period in a closed direction. Exact UV trim jumps remain errors, and the
//!   physical gap uses the original parameters.
//! - **Assert reports.** `pAList` is an optional [`AssertArray`]. The release build tests
//!   every predicate (`eTestLevel < SM_LEVEL_0` is never true), so `eTestLevel` and
//!   `pTestRequests` (unused) are dropped. A face or loop is named in a report by
//!   [`topo_key`].
//! - **Consumed inputs.** `TrimSurfaceWithModelSpaceCurves` takes its curves and surface
//!   by value (upstream deletes them on early errors and hands them to the B-rep
//!   otherwise). The region overload is ported; the `SmBrep*&` overload is not part of
//!   this unit.
//! - **`OrderCurvesIntoLoops`** takes the curves as a slice (no null entries: upstream
//!   skips a null curve when collecting end points, which shifts every later curve's
//!   end points, then dereferences it in the planarity test). Its outputs name curves by
//!   index into the slice. A loop found open or compound (upstream's tentative
//!   `sRet = SM_ERR`) and a curve off the plane (`return SM_ERR` after all outputs are
//!   set) are reported in [`OrderedLoops`] rather than as an error, so the outputs
//!   upstream sets stay observable; [`OrderedLoops::status`] gives upstream's status.
//! - **`FindOuterLoops`** reorders index lists; the plane normal is `&mut` because the
//!   point sequences unitize it in place (`SmPointSet3d` with `pOptNormal`). A failing
//!   `Tessellate`, whose status upstream ignores, is returned.
//! - **`IsIsoCurve(curve, side, low, tol)`** returns `SER(GetCanonical)`'s nonzero
//!   status as an `SmBoolean`: a curve without NURB counts as an iso curve, as upstream
//!   (`plan/upstream-issues.md` WR-97). `MirrorCurve` divides rational control points by
//!   their weights twice, as upstream (WR-98).
//! - **`FindOuterLoops`** with a loop of zero area reads past its arrays upstream
//!   (UB-119): [`SolidError::InvalidInput`]. A null curve of `OrderCurvesIntoLoops`
//!   (UB-120) cannot be passed.
//! - **`CheckLoop`** takes the surface separately, as upstream; `CheckFace` passes a
//!   copy of the face's surface (the edge use UV extremes need the B-rep mutably).
//!   With the opt-in `Kernel::fixes.check_loop_vertex_tolerance` (WR-381, #476) its gap
//!   limit also honours a shared vertex's zone tolerance; upstream (the default) uses only
//!   the edge intersection tolerance and rejects consistent widened vertices.
//! - **Callees.** `SmBrep::MakeFaceWithCurves` (`topology.brep_p7`) and
//!   `SmBrep::MakeFacesWithCurves` (`topology.brep_p8`) call their ports. Unported callees
//!   return [`SolidError::Unported`]: `SmFace::FlipFaceOrientation` and `SmFace::SplitAtSeam` (`topology.face_p1`),
//!   `SmFace::GetEdges` and `SmFace::HasSeamProblem` (`topology.face_p4`).
//! - The other members of the class (`FindLoopCurveOrientations`,
//!   `ComputeLoopOrientation`, `CopyNonDegenerate`, `OrientUVLoops`,
//!   `ApproximateLengthThroughSurface`, `IsCurveOnSurfaceDegenerate`,
//!   `CheckLoopForMiniHourglass`, `CheckTessellation`, `CheckFaceUVSeamJumps`,
//!   `RecreateUVTrimCurves`, `RedropUVTrimCurves`, the `SmBrep` overloads,
//!   `TrimSurfaceWithParameterSpaceCurves`, `IsIsoCurve(curve, surface, ...)`) are not
//!   reached by in-scope code (`harness/callgraph`) and are not ported.

pub mod curve_loops;

use std::borrow::Cow;

use crate::curve::curve::p2::CurveKind;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::assert_array::{AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::point_set::{PointContainment, PointSequence};
use crate::geom::tol::{self, SmallTopoType, ZONE_TOL_3D};
use crate::geom::tools::find_first_point_within_tol_to_pnt;
use crate::geom::vector2d::Point2;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve::{BSplineCurve, CanonicalData};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface::p2::IsoCurveOptions;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as fp2;
use crate::topology::loopuse as lu1;
use crate::topology::vertexuse as vu1;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, LoopId, RegionId};

const OP: &str = "topology::trimming_tools";

/// `SmTrimmingTools_TYPE` (`CONT_BASE_TYPE + 225`).
pub const TYPE: i64 = 12225;

/// `SmFace_TYPE` (`TOPO_BASE_TYPE + 2`), the owner type of a face report.
const FACE_TYPE: i64 = 16002;
/// `SmLoop_TYPE` (`TOPO_BASE_TYPE + 3`), the owner type of a loop report.
const LOOP_TYPE: i64 = 16003;

/// Upstream `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

/// `SM_LIST_2`: the `CheckFace` label table.
const LIST_CHECK_FACE: u64 = 2;
/// `SM_LIST_3`: the `CheckLoop` label table.
const LIST_CHECK_LOOP: u64 = 3;

/// `SmTrimmingTools::GetClassType` (static): [`TYPE`].
#[must_use]
pub const fn class_type() -> i64 {
    TYPE
}

/// `SmTrimmingTools::Min`: `a` when `a < b`, else `b`.
#[must_use]
pub fn min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// An upstream status other than `SM_ERR` / `SM_ERR_NULL_POINTER`.
pub(crate) const fn status(code: i32) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code,
        entity: None,
    }
}

/// The identity a face or loop has in an assert report (upstream compares pointers):
/// its `SmTopoTypes.h` code in the high bits and its arena index in the low ones.
#[must_use]
pub fn topo_key(e: EntityRef) -> ObjectKey {
    let (ty, index) = match e {
        EntityRef::Face(f) => (FACE_TYPE, f.index()),
        EntityRef::Loop(l) => (LOOP_TYPE, l.index()),
        other => (0, other_index(other)),
    };
    ObjectKey(((ty as u64) << 32) | u64::from(index))
}

fn other_index(e: EntityRef) -> u32 {
    match e {
        EntityRef::Edge(x) => x.index(),
        EntityRef::Edgeuse(x) => x.index(),
        EntityRef::Vertex(x) => x.index(),
        _ => 0,
    }
}

/// The `SM_ASSERT_OBJ_VALUE_REPORT` / `SM_ASSERT_OBJ_BOOLEAN_REPORT` macros (release
/// build) with `this` = `owner` (a face or loop of type `owner_type`): `true` when
/// `passed`; otherwise, with a list, a report for test `test_index` of table
/// `list_index` is added unless an equal one is there. `tol` / `val` are
/// [`UNDEF_DOUBLE`] for a boolean report.
#[allow(clippy::too_many_arguments)]
fn report(
    alist: &mut Option<&mut AssertArray>,
    owner: EntityRef,
    owner_type: i64,
    owner_string: &'static str,
    list_index: u64,
    test_index: u64,
    passed: bool,
    tol: f64,
    val: f64,
) -> Result<bool> {
    if passed {
        if let Some(l) = alist.as_deref_mut() {
            l.set_last_add(false);
        }
        return Ok(true);
    }
    if let Some(l) = alist.as_deref_mut() {
        let key = topo_key(owner);
        let r = AssertReport::new(ReportSpec {
            reporter: Some(key),
            test_index,
            test_class: TYPE,
            owner: Some(key),
            owner_type,
            owner_type_string: Some(owner_string),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: if tol == UNDEF_DOUBLE {
                tol
            } else {
                tol::tol3d(tol)
            },
            value: val,
            list_index,
            message: None,
        })?;
        l.add_unique_delete_duplicate(r);
    }
    Ok(false)
}

// ---- unported callees -------------------------------------------------------------------

/// `SmBrep::SetTolerance(d3DTolerance)` (the double becomes an `SmZoneTol3d`; default
/// arguments).
fn brep_set_tolerance(k: &Kernel, b: &mut Brep, tol: f64) -> Result<()> {
    crate::topology::brep::p2::set_tolerance(k, b, tol::zone_tol3d(k, tol), true, true)
}

/// `SmBrep::MakeFaceWithCurves(region, loops, 3d curves, NULL, ...)` (unit
/// `topology.brep_p7`). The new face owns the surface under the caller's face identity.
fn make_face_with_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: RegionId,
    input: TrimInput,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<FaceId> {
    let TrimInput {
        loop_counts,
        curves,
        orientations,
        loop_points,
        surface,
        uv_domain,
        surface_orientation,
    } = input;
    let curves_input = crate::topology::brep::p7::FaceCurvesInput {
        loops: loop_counts,
        curves_3d: Some(curves.into_iter().map(Some).collect()),
        uv_curves: None,
        orientations,
        loop_points,
    };
    let key_of = env.key_of;
    let face_ref = |f: FaceId| key_of(EntityRef::Face(f));
    let made = crate::topology::brep::p7::make_face_with_curves(
        k,
        b,
        Some(region),
        curves_input,
        surface,
        &uv_domain,
        surface_orientation,
        None,
        &face_ref,
        env,
    )?;
    // (upstream adds the new face pointer as is)
    made.face
        .ok_or_else(|| SolidError::invalid_input(OP, "MakeFaceWithCurves made no face"))
}

/// `SmBrep::MakeFacesWithCurves(region, loops, 3d curves, NULL, ...)` (unit
/// `topology.brep_p8`). Each new face owns its surface under the caller's face identity,
/// as [`make_face_with_curves`].
fn make_faces_with_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: RegionId,
    input: TrimInput,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<Vec<FaceId>> {
    let TrimInput {
        loop_counts,
        curves,
        orientations,
        loop_points,
        surface,
        uv_domain,
        surface_orientation,
    } = input;
    let curves_input = crate::topology::brep::p7::FaceCurvesInput {
        loops: loop_counts,
        curves_3d: Some(curves.into_iter().map(Some).collect()),
        uv_curves: None,
        orientations,
        loop_points,
    };
    let key_of = env.key_of;
    let face_ref = |f: FaceId| key_of(EntityRef::Face(f));
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env,
        marks: &mut marks,
        this_face: &face_ref,
    };
    let made = crate::topology::brep::p8::make_faces_with_curves(
        k,
        b,
        Some(region),
        curves_input,
        surface,
        &uv_domain,
        surface_orientation,
        &mut fenv,
    )?;
    Ok(made.faces)
}

/// `SmFace::CreateUVTrimCurves(FALSE, NULL, NULL, ...)`.
fn face_create_uv_trim_curves(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    crate::topology::face::p3::create_uv_trim_curves(k, b, f, false, None).map(|_| ())
}

/// `SmFace::ComputeLoopOrientation(loop, orientation)` (default arguments: no UV
/// bounds, `bDo3DLoopComputation = FALSE`).
fn face_compute_loop_orientation(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    l: LoopId,
) -> Result<OrientType> {
    crate::topology::face::p3::compute_loop_orientation(k, b, f, l, None, false)
}

/// `SmFace::FlipFaceOrientation` (unit `topology.face_p1`).
fn face_flip_face_orientation(b: &mut Brep, f: FaceId) -> Result<()> {
    crate::topology::face::p1::flip_face_orientation(b, f)
}

/// `SmFace::GetEdges` (unit `topology.face_p4`).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

use crate::topology::brep::p1::HealLoopHandle;

/// `SmFace::HasSeamProblem(rCrvClassU, rCrvClassV, eCrossed, eMissing, eNearMiss)` (unit
/// `topology.face_p4`).
fn face_has_seam_problem(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
) -> Result<(
    bool,
    crate::intersect::curve_class::p1::CurveClassification<HealLoopHandle>,
    crate::intersect::curve_class::p1::CurveClassification<HealLoopHandle>,
    crate::topology::face::p1::SeamProblem,
)> {
    let check = crate::topology::face::p4::has_seam_problem_in_brep_mut(
        k,
        b,
        f,
        crate::topology::face::p4::HasSeamProblemOptions::default(),
    )?;
    Ok((
        check.has_problem,
        check.class_u,
        check.class_v,
        check.problem,
    ))
}

/// `SmFace::HasSeamProblem` and, when it reports one, `SmFace::SplitAtSeam` (unit
/// `topology.face_p1`) appending the new faces.
fn face_split_at_seam_problem(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<Vec<FaceId>> {
    let (has, mut cu, mut cv, problem) = face_has_seam_problem(k, b, f)?;
    if !has {
        return Ok(Vec::new());
    }
    let split = crate::topology::face::p1::split_at_seam(
        k,
        b,
        f,
        &mut cu,
        &mut cv,
        problem,
        None,
        &|o: HealLoopHandle| match o {
            HealLoopHandle::Topo(EntityRef::Vertex(v)) => Some(EntityRef::Vertex(v)),
            _ => None,
        },
    )?;
    Ok(split.faces)
}

// ---- trimming a surface -----------------------------------------------------------------

/// The trimming data of [`trim_surface_with_model_space_curves`] (upstream's
/// `crCurveLoops` .. `eSurfaceOrientation` arguments).
#[derive(Debug)]
pub struct TrimInput {
    /// `crCurveLoops`: the number of curves in each loop (first loop outer).
    pub loop_counts: Vec<usize>,
    /// `cp3DCurves`: the curves ordered by loop, consumed.
    pub curves: Vec<Curve>,
    /// `crCurveOrientations`: each curve's orientation in its loop.
    pub orientations: Vec<OrientType>,
    /// `crLoopPoints`: points inside the outer loop that become vertex loops.
    pub loop_points: Vec<Point3>,
    /// `pSurface`: the surface to trim, consumed.
    pub surface: Surface,
    /// `crUVDomain`: the domain of the surface the face uses.
    pub uv_domain: Extent2d,
    /// `eSurfaceOrientation`: the face's orientation relative to the surface normal.
    pub surface_orientation: OrientType,
}

/// Model-space trim data, with the original surface retained separately.
#[derive(Debug)]
pub(crate) struct TrimCurvesInput {
    pub(crate) loop_counts: Vec<usize>,
    pub(crate) curves: Vec<Curve>,
    pub(crate) orientations: Vec<OrientType>,
    pub(crate) loop_points: Vec<Point3>,
    pub(crate) uv_domain: Extent2d,
    pub(crate) surface_orientation: OrientType,
}

/// Original surface location after a consuming trim attempt, including errors.
#[derive(Debug)]
pub(crate) enum TrimSurfaceLocation {
    Owned(Box<Surface>),
    Installed(FaceId),
    Destroyed,
}

/// The native trim tool destroys inputs on early errors, but hands ownership to
/// the face builder after continuity validation. No surface copy is made here.
#[derive(Debug)]
pub(crate) struct TrimSurfaceTransfer {
    transfer: Option<crate::topology::brep::p6::FaceSurfaceTransfer>,
}

impl TrimSurfaceTransfer {
    pub(crate) const fn new(surface: Surface) -> Self {
        Self {
            transfer: Some(crate::topology::brep::p6::FaceSurfaceTransfer::new(surface)),
        }
    }

    pub(crate) fn into_location(self) -> Result<TrimSurfaceLocation> {
        use crate::topology::brep::p6::FaceSurfaceLocation;
        match self.transfer {
            None => Ok(TrimSurfaceLocation::Destroyed),
            Some(transfer) => match transfer.into_location()? {
                FaceSurfaceLocation::Owned(surface) => Ok(TrimSurfaceLocation::Owned(surface)),
                FaceSurfaceLocation::Installed(face) => Ok(TrimSurfaceLocation::Installed(face)),
            },
        }
    }
}

enum TrimSurfaceBackend<'a> {
    Legacy(&'a mut Option<Surface>),
    Retained(&'a mut TrimSurfaceTransfer),
}

impl TrimSurfaceBackend<'_> {
    fn surface(&self) -> Result<&Surface> {
        match self {
            Self::Legacy(surface) => surface.as_ref().ok_or_else(|| SolidError::null_pointer(OP)),
            Self::Retained(transfer) => transfer
                .transfer
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP))?
                .owned_surface(),
        }
    }

    fn destroy_early(&mut self) {
        if let Self::Retained(transfer) = self {
            transfer.transfer.take();
        }
    }
}

/// Consuming ORIGINAL-surface successor for the live cliff caller. The G1 path
/// retains the actual installed face even when later UV repair/checking fails.
/// Uses the native cliff flags: first loop outer, loops not pre-oriented, no seam split.
/// Non-G1 inputs therefore fail the native early check and are destroyed.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) fn trim_surface_with_model_space_curves_transfer(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    tol_3d: f64,
    input: TrimCurvesInput,
    transfer: &mut TrimSurfaceTransfer,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<Vec<FaceId>> {
    trim_surface_impl(
        k,
        b,
        region,
        tol_3d,
        input,
        TrimSurfaceBackend::Retained(transfer),
        true,
        false,
        false,
        env,
    )
}

/// `TrimSurfaceWithModelSpaceCurves` (region overload): trims `input.surface` with the
/// model-space curve loops of `input` into one face (a G1 surface) or several (split at
/// its creases) in `region`, splits them at the surface seams when `may_cross_seam`,
/// repairs their trim loops (tiny UV gaps; loop order when `oriented_correctly`, else
/// backwards seam curves and loop orientations) and checks them with
/// [`check_faces`]. Returns the new faces. `first_is_outer` is only asserted upstream.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` without region or when the region is not in the B-rep; `SM_ERR`
/// for a surface that is not G1 when the loops are not oriented correctly; those of the
/// callees; [`SolidError::Unported`] (`SmBrep::SetTolerance` and the face builders).
#[allow(clippy::fn_params_excessive_bools)]
pub fn trim_surface_with_model_space_curves(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    tol_3d: f64,
    input: TrimInput,
    first_is_outer: bool,
    oriented_correctly: bool,
    may_cross_seam: bool,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<Vec<FaceId>> {
    let TrimInput {
        loop_counts,
        curves,
        orientations,
        loop_points,
        surface,
        uv_domain,
        surface_orientation,
    } = input;
    let mut surface = Some(surface);
    trim_surface_impl(
        k,
        b,
        region,
        tol_3d,
        TrimCurvesInput {
            loop_counts,
            curves,
            orientations,
            loop_points,
            uv_domain,
            surface_orientation,
        },
        TrimSurfaceBackend::Legacy(&mut surface),
        first_is_outer,
        oriented_correctly,
        may_cross_seam,
        env,
    )
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn trim_surface_impl(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    tol_3d: f64,
    input: TrimCurvesInput,
    mut surface: TrimSurfaceBackend<'_>,
    first_is_outer: bool,
    oriented_correctly: bool,
    may_cross_seam: bool,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<Vec<FaceId>> {
    // Native SmObjDelete/SmObjsDelete remain active through these early checks.
    let early = (|| {
        let region = region.ok_or(SolidError::null_pointer(OP))?;
        if !b.region(region)?.in_brep {
            return Err(SolidError::null_pointer(OP));
        }
        let _ = first_is_outer; // SM_ASSERT only
        brep_set_tolerance(k, b, tol_3d)?;
        let g1_surface = surface_continuity_check(k, surface.surface()?)?;
        if !g1_surface && !oriented_correctly {
            return Err(SolidError::sm_err(OP));
        }
        Ok((region, g1_surface))
    })();
    let (region, g1_surface) = match early {
        Ok(result) => result,
        Err(error) => {
            drop(input);
            surface.destroy_early();
            return Err(error);
        }
    };
    // Native clears the temporary cleanup immediately before MakeFace(s).
    let mut new_faces = match surface {
        TrimSurfaceBackend::Legacy(surface) => {
            let surface = surface.take().ok_or_else(|| SolidError::null_pointer(OP))?;
            let input = TrimInput {
                loop_counts: input.loop_counts,
                curves: input.curves,
                orientations: input.orientations,
                loop_points: input.loop_points,
                surface,
                uv_domain: input.uv_domain,
                surface_orientation: input.surface_orientation,
            };
            if g1_surface {
                vec![make_face_with_curves(k, b, region, input, env)?]
            } else {
                let faces = make_faces_with_curves(k, b, region, input, env)?;
                for &f in &faces {
                    clear_uv_trim_curves(b, f)?;
                }
                faces
            }
        }
        TrimSurfaceBackend::Retained(transfer) => {
            // Only the fixed cliff entry uses this backend. Its early native
            // !G1 && !oriented check already rejected non-G1 surfaces.
            debug_assert!(g1_surface);
            let key_of = env.key_of;
            let mut made = crate::topology::brep::p7::NewFace::default();
            crate::topology::brep::p7::make_face_with_curves_transfer(
                k,
                b,
                Some(region),
                crate::topology::brep::p7::FaceCurvesInput {
                    loops: input.loop_counts,
                    curves_3d: Some(input.curves.into_iter().map(Some).collect()),
                    uv_curves: None,
                    orientations: input.orientations,
                    loop_points: input.loop_points,
                },
                transfer
                    .transfer
                    .as_mut()
                    .ok_or_else(|| SolidError::null_pointer(OP))?,
                &input.uv_domain,
                input.surface_orientation,
                None,
                &|face| key_of(EntityRef::Face(face)),
                env,
                &mut made,
            )?;
            vec![made
                .face
                .ok_or_else(|| SolidError::invalid_input(OP, "MakeFaceWithCurves made no face"))?]
        }
    };

    // when warned - split all new faces at seam boundaries
    if may_cross_seam {
        let mut split_faces = Vec::new();
        for ii in 0..new_faces.len() {
            let f = *new_faces.get(ii).ok_or_else(|| null("face index"))?;
            let this_split = split_at_seams(k, b, f)?;
            split_faces.extend(this_split);
        }
        new_faces.extend(split_faces);
    }

    // for every new face - fix up known common problems
    for &f in &new_faces {
        if !face_uv_curves_present(b, f)? {
            create_uv_trim_curves(k, b, f)?;
        }
        if oriented_correctly {
            fix_face_tiny_parameter_space_gaps(k, b, f, ZONE_TOL_3D / 10.0)?;
            fix_trim_loop_order(k, b, f)?;
        } else {
            fix_backwards_outer_loop_on_seam(k, b, f)?;
            fix_face_tiny_parameter_space_gaps(k, b, f, ZONE_TOL_3D / 10.0)?;
            fix_face_loop_orientations(k, b, f)?;
        }
    }

    // all done
    check_faces(k, b, &new_faces, tol_3d, None)?;
    Ok(new_faces)
}

// ---- UV trim curve presence -------------------------------------------------------------

/// `AreUVCurvesPresent(SmLoop*)`: whether every edge use of the loop's (first) loop use
/// has a UV trim curve (its own or its mate's).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use; [`SolidError::NotFound`]
/// for a dangling id.
pub fn loop_uv_curves_present(b: &Brep, l: LoopId) -> Result<bool> {
    let (lu, _) = lu1::loop_loopuses(b, l)?;
    for eu in lu1::edgeuses(b, lu, None, None)? {
        if eu1::uv_trim_curve(b, eu)?.0.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `AreUVCurvesPresent(SmFace*)`: [`loop_uv_curves_present`] for every loop of the face.
///
/// # Errors
///
/// Those of [`fp2::loops`] and [`loop_uv_curves_present`].
pub fn face_uv_curves_present(b: &Brep, f: FaceId) -> Result<bool> {
    for l in fp2::loops(b, f, false, None)? {
        if !loop_uv_curves_present(b, l)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `CreateUVTrimCurves(SmFace*)`: `SmFace::CreateUVTrimCurves` without tolerance
/// adjustment.
///
/// # Errors
///
/// Those of [`crate::topology::face::p3::create_uv_trim_curves`].
pub fn create_uv_trim_curves(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    face_create_uv_trim_curves(k, b, f)
}

/// `ClearUVTrimCurves(SmFace*)`: deletes the UV trim curve of every edge use of every
/// edge of the face.
///
/// # Errors
///
/// Those of [`eu1::set_uv_trim_curve`] and `SmFace::GetEdges` (`topology::face::p4`).
pub fn clear_uv_trim_curves(b: &mut Brep, f: FaceId) -> Result<()> {
    for e in face_edges(b, f)? {
        for eu in crate::topology::edge::edgeuses(b, e, None, None)? {
            // TRUE = delete existing UVTrimCurve
            eu1::set_uv_trim_curve(b, eu, eu1::UvTrimCurveArg::Null, 0.0, true, None)?;
        }
    }
    Ok(())
}

/// `SplitAtSeams(SmFace*)`: when the face has a seam problem, splits it at the seam;
/// returns the new faces.
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmFace::HasSeamProblem`, `topology.face_p4`;
/// `SmFace::SplitAtSeam`, `topology.face_p1`).
pub fn split_at_seams(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<Vec<FaceId>> {
    face_split_at_seam_problem(k, b, f)
}

// ---- UV gap repair ----------------------------------------------------------------------

/// The UV trim curve slot an edge use reads (`GetUVTrimCurve`: its own or its mate's).
fn uv_slot(b: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    let (c, slot) = eu1::uv_trim_curve(b, eu)?;
    Ok(c.and(slot))
}

fn held_curve(b: &Brep, holder: EdgeuseId) -> Result<&Curve> {
    b.edgeuse(holder)?
        .uv_trim_curve
        .as_ref()
        .ok_or_else(|| null("edge use without UV trim curve"))
}

fn held_curve_mut(b: &mut Brep, holder: EdgeuseId) -> Result<&mut Curve> {
    b.edgeuse_mut(holder)?
        .uv_trim_curve
        .as_mut()
        .ok_or_else(|| null("edge use without UV trim curve"))
}

/// The `SmBSplineCurve` of a UV trim curve: the B-spline, or a conic's or line's B-spline part
/// (made on a copy when missing).
fn bspline_of(c: &Curve) -> Result<Cow<'_, BSplineCurve>> {
    match c.kind() {
        CurveKind::BSpline(b) => Ok(Cow::Borrowed(b)),
        CurveKind::Line(l) => l.base_with_nurb(),
        // the other SmBSplineCurve kinds (their constructors make the NURB)
        CurveKind::Ellipse(e) => conic_base(e.base()),
        CurveKind::Hyperbola(h) => conic_base(h.base()),
        CurveKind::Parabola(p) => conic_base(p.base()),
        _ => Err(null("UV trim curve is not an SmBSplineCurve")),
    }
}

/// The B-spline part of a conic UV trim curve. A conic's constructor makes its NURB; one
/// without (its `MakeNurb` failed) is the null `GetOrCreateGwNurbPointer` upstream
/// dereferences.
fn conic_base(base: &BSplineCurve) -> Result<Cow<'_, BSplineCurve>> {
    if base.gw_nurb().is_none() {
        return Err(null("conic UV trim curve without NURB"));
    }
    Ok(Cow::Borrowed(base))
}

/// `SmBSplineCurve::EditEndPoint(point, bEditStart, pdWeight)` of a UV trim curve.
fn edit_end_point(
    c: &mut Curve,
    point: Point3,
    edit_start: bool,
    weight: Option<f64>,
) -> Result<()> {
    match c.kind_mut() {
        CurveKind::BSpline(b) => b.edit_end_point(point, edit_start, weight),
        CurveKind::Line(l) => {
            // MakeNurb (the SmLine override) when missing, then RefreshAnalytics
            if l.base.gw_nurb().is_none() {
                let _ = l.make_nurb();
            }
            l.base.edit_end_point(point, edit_start, weight)?;
            l.refresh_analytics()
        }
        // SmBSplineCurve::EditEndPoint on the base (RefreshAnalytics is the base no-op)
        CurveKind::Ellipse(e) => conic_edit_end_point(e.base_mut(), point, edit_start, weight),
        CurveKind::Hyperbola(h) => conic_edit_end_point(h.base_mut(), point, edit_start, weight),
        CurveKind::Parabola(p) => conic_edit_end_point(p.base_mut(), point, edit_start, weight),
        _ => Err(null("EditEndPoint: UV trim curve is not an SmBSplineCurve")),
    }
}

/// `SmBSplineCurve::EditEndPoint` on a conic's B-spline part (see [`conic_base`]).
fn conic_edit_end_point(
    base: &mut BSplineCurve,
    point: Point3,
    edit_start: bool,
    weight: Option<f64>,
) -> Result<()> {
    conic_base(base)?;
    base.edit_end_point(point, edit_start, weight)
}

/// [`edge_uv_extremes`]: the UV start and end of an edge use and, when they come from
/// its UV trim curve, the curve's end weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeUvExtremes {
    /// `rStart`: the edge use's start in UV.
    pub start: Point2,
    /// `rdStartWeight`; `None` when computed from the vertex uses (upstream leaves it
    /// unset).
    pub start_weight: Option<f64>,
    /// `rEnd`: the edge use's end in UV.
    pub end: Point2,
    /// `rdEndWeight`; `None` as [`EdgeUvExtremes::start_weight`].
    pub end_weight: Option<f64>,
}

/// `GetEdgeUVExtremes(pEdgeuse, rStart, rdStartWeight, rEnd, rdEndWeight)`: the natural
/// ends of the edge use's UV trim curve (swapped, with their weights, for an
/// [`OrientType::Opposite`] edge use); without UV trim curve, the UV points of the edge
/// use's and its mate's vertex uses (`ComputeUVPoint(uv, FALSE)`, never swapped).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a missing vertex use or mate; those of
/// [`vu1::compute_uv_point`] and of the curve's ends.
pub fn edge_uv_extremes(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<EdgeUvExtremes> {
    // get underlying edgeuse->UVTrimCurve
    let ends = match eu1::uv_trim_curve(b, eu)?.0 {
        Some(c) => Some(c.ends(k)?),
        None => None,
    };
    let Some(ends) = ends else {
        // Don't try to create a uv curve, the topology might not be
        // in good shape (as during read-in).  [B513]
        let vu1_id = eu1::vertexuse(b, eu)?.ok_or_else(|| null("edge use without vertex use"))?;
        let uv1 = vu1::compute_uv_point(k, b, vu1_id, false)?;
        let mate = eu1::mate(b, eu)?.ok_or_else(|| null("edge use without mate"))?;
        let vu2_id = eu1::vertexuse(b, mate)?.ok_or_else(|| null("mate without vertex use"))?;
        let uv2 = vu1::compute_uv_point(k, b, vu2_id, false)?;
        return Ok(EdgeUvExtremes {
            start: uv1,
            start_weight: None,
            end: uv2,
            end_weight: None,
        });
    };

    // get UVTrimCurve natural start/end positions and weights
    let mut start = Point2::new(ends.start.x, ends.start.y);
    let mut end = Point2::new(ends.end.x, ends.end.y);
    let mut start_weight = ends.start_weight;
    let mut end_weight = ends.end_weight;

    // swap ends when Orientation is reversed
    if eu1::orientation(b, eu)? == OrientType::Opposite {
        core::mem::swap(&mut start, &mut end);
        core::mem::swap(&mut start_weight, &mut end_weight);
    }
    Ok(EdgeUvExtremes {
        start,
        start_weight: Some(start_weight),
        end,
        end_weight: Some(end_weight),
    })
}

/// `GetEdgeUVExtremes(pEdgeuse, rStart, rEnd)`: [`edge_uv_extremes`] without weights.
///
/// # Errors
///
/// Those of [`edge_uv_extremes`].
pub fn edge_uv_ends(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<(Point2, Point2)> {
    let x = edge_uv_extremes(k, b, eu)?;
    Ok((x.start, x.end))
}

/// The edge uses of a loop's (first) loop use.
fn loop_edgeuses(b: &Brep, l: LoopId) -> Result<Vec<EdgeuseId>> {
    let (lu, _) = lu1::loop_loopuses(b, l)?;
    lu1::edgeuses(b, lu, None, None)
}

fn at<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i).copied().ok_or_else(|| null("index out of range"))
}

/// `FixTrimLoopTinyParameterSpaceGaps(SmLoop*)`: moves the UV ends of consecutive edge
/// uses of the loop onto their mid point where they are less than `2 * tol_2d` apart.
/// Returns whether a curve was edited.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use or a gap to close at an edge
/// use without UV trim curve; those of [`edge_uv_extremes`] and of the edits.
pub fn fix_loop_tiny_parameter_space_gaps(
    k: &Kernel,
    b: &mut Brep,
    l: LoopId,
    tol_2d: f64,
) -> Result<bool> {
    // init output
    let mut repairs_made = false;
    let eus = loop_edgeuses(b, l)?;
    let n = eus.len();

    // for every upward edgeuse
    for ii in 0..n {
        let i0 = if ii == 0 { n - 1 } else { ii - 1 };
        let curr = at(&eus, ii)?;
        let prev = at(&eus, i0)?;

        let p = edge_uv_extremes(k, b, prev)?;
        let c = edge_uv_extremes(k, b, curr)?;

        // Only fix them if they are fairly close to being right anyway.
        let gap = p.end.distance(c.start);
        if gap < 2.0 * tol_2d {
            let target = Vec3::from((p.end + c.start) / 2.0);

            // First edit the previous curve.  We want the end, so we look at the start
            // only if the edge's orientation is reversed.
            let edit_start = eu1::orientation(b, prev)? == OrientType::Opposite;
            let slot =
                uv_slot(b, prev)?.ok_or_else(|| null("previous edge use without UV trim curve"))?;
            let curve = held_curve_mut(b, slot)?;
            if curve.is_rational()? {
                let w = p.end_weight.ok_or_else(|| null("weight"))?;
                edit_end_point(curve, target * w, edit_start, Some(w))?;
            } else {
                edit_end_point(curve, target, edit_start, None)?;
            }

            // Then edit the current curve.
            let edit_start = eu1::orientation(b, curr)? == OrientType::Same;
            let slot =
                uv_slot(b, curr)?.ok_or_else(|| null("current edge use without UV trim curve"))?;
            let curve = held_curve_mut(b, slot)?;
            if curve.is_rational()? {
                let w = c.start_weight.ok_or_else(|| null("weight"))?;
                edit_end_point(curve, target * w, edit_start, Some(w))?;
            } else {
                edit_end_point(curve, target, edit_start, None)?;
            }
            repairs_made = true;
        }
    }
    Ok(repairs_made)
}

/// `FixTrimLoopTinyParameterSpaceGaps(SmFace*)`: [`fix_loop_tiny_parameter_space_gaps`]
/// for every loop of the face; whether any curve was edited.
///
/// # Errors
///
/// Those of [`fp2::loops`] and [`fix_loop_tiny_parameter_space_gaps`].
pub fn fix_face_tiny_parameter_space_gaps(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    tol_2d: f64,
) -> Result<bool> {
    let mut repairs_made = false;
    for l in fp2::loops(b, f, false, None)? {
        if fix_loop_tiny_parameter_space_gaps(k, b, l, tol_2d)? {
            repairs_made = true;
        }
    }
    Ok(repairs_made)
}

/// `EnsureLastEdgeuseUVCorrect`: when some UV trim curve of `edgeuses` is an iso curve on
/// a seam of `surface` and another is not, rotates the list so that the first edge use
/// after the last such other one is first. Returns `rbAllEdgeusesCorrect`: `true` when
/// no curve is on a seam or all are (nothing rotated).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without UV trim curve; those of
/// [`is_iso_curve_on_seam`].
pub fn ensure_last_edgeuse_uv_correct(
    k: &Kernel,
    b: &Brep,
    surface: &Surface,
    edgeuses: &mut [EdgeuseId],
) -> Result<bool> {
    // Try to find an edge which is not an isocurve on a seam.
    let n = edgeuses.len();
    let mut dropped_correctly = n;
    let mut iso_curve_on_seam_present = false;
    for (ii, &eu) in edgeuses.iter().enumerate() {
        let edge = eu1::uv_trim_curve(b, eu)?
            .0
            .ok_or_else(|| null("edge use without UV trim curve"))?;
        if is_iso_curve_on_seam(k, surface, edge)? {
            iso_curve_on_seam_present = true;
        } else {
            dropped_correctly = ii;
        }
        if iso_curve_on_seam_present && dropped_correctly < n {
            break;
        }
    }

    // No iso curve on seam, nothing to worry about!
    if !iso_curve_on_seam_present {
        return Ok(true);
    }

    // All curves are iso on seam, so the orientation is going to be consistent.
    if dropped_correctly == n {
        return Ok(true);
    }

    // RotateArray
    edgeuses.rotate_left(if dropped_correctly + 1 < n {
        dropped_correctly + 1
    } else {
        0
    });
    Ok(false)
}

/// One parameter direction of [`fix_backwards_outer_loop_on_seam`]: `closed[jj]`,
/// `low[jj]`, `high[jj]`, `tolerance[jj]`.
struct SeamDir {
    closed: bool,
    low: f64,
    high: f64,
    tolerance: f64,
}

/// `FixBackwardsOuterLoopOnSeam`: on a closed surface, mirrors across the seam each
/// iso-curve UV trim curve of the face's first (outer) loop that lies on the wrong side
/// of the seam (its start a period away from the previous edge use's end). Returns
/// whether a curve was mirrored.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface or loops, a loop without loop
/// use, or a curve to mirror that is missing; those of the callees.
pub fn fix_backwards_outer_loop_on_seam(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    let mut repair_made = false;
    let surface = b
        .face(f)?
        .surface
        .clone()
        .ok_or_else(|| null("face without surface"))?;
    let bounds = surface.natural_uv_domain_with_nurb(k)?;
    let closed = [
        surface
            .is_closed(k, &bounds, SurfParamType::U, None, false)?
            .0,
        surface
            .is_closed(k, &bounds, SurfParamType::V, None, false)?
            .0,
    ];
    let [closed_u, closed_v] = closed;
    if !closed_u && !closed_v {
        return Ok(false);
    }
    let (low, high) = (bounds.min(), bounds.max());

    // This is the tolerance that tells us something is wrong.
    let big_tolerance = min(high.x - low.x, high.y - low.y) / 2.0;

    // These are the tolerances we use to check to see if a correction makes sense,
    // with each direction's closure and (swappable) bounds.
    let mut dirs = [
        SeamDir {
            closed: closed[0],
            low: low.x,
            high: high.x,
            tolerance: (high.x - low.x) / 1000.0,
        },
        SeamDir {
            closed: closed[1],
            low: low.y,
            high: high.y,
            tolerance: (high.y - low.y) / 1000.0,
        },
    ];

    // Get the edgeuses on the first (ie outer) loop.
    let loops = fp2::loops(b, f, false, None)?;
    let first = *loops.first().ok_or_else(|| null("face without loops"))?;
    let mut eus = loop_edgeuses(b, first)?;

    // First we try to find an edge which is not an isocurve on a seam.
    if ensure_last_edgeuse_uv_correct(k, b, &surface, &mut eus)? {
        return Ok(false);
    }

    // Now look for gaps caused by incorrect seam curves.
    let n = eus.len();
    for ii in 0..n {
        let previous = if ii == 0 { n - 1 } else { ii - 1 };
        let (_, previous_end) = edge_uv_ends(k, b, at(&eus, previous)?)?;
        let (current_start, current_end) = edge_uv_ends(k, b, at(&eus, ii)?)?;
        let cs = [current_start.x, current_start.y];
        let ce = [current_end.x, current_end.y];

        if previous_end.distance(current_start) > big_tolerance {
            for (jj, d) in dirs.iter_mut().enumerate() {
                let c = cs.get(jj).copied().unwrap_or(0.0);
                let c_end = ce.get(jj).copied().unwrap_or(0.0);
                let tol_j = d.tolerance;
                if d.closed && (c - c_end).abs() < tol_j {
                    if (c - d.high).abs() < tol_j {
                        core::mem::swap(&mut d.low, &mut d.high);
                    }

                    if (c - d.low).abs() < tol_j {
                        let alt_start = if jj == 0 {
                            Point2::new(d.high, current_start.y)
                        } else {
                            Point2::new(current_start.x, d.high)
                        };
                        if previous_end.distance(alt_start) < tol_j {
                            let side = if jj == 0 {
                                SurfParamType::U
                            } else {
                                SurfParamType::V
                            };
                            let slot = uv_slot(b, at(&eus, ii)?)?
                                .ok_or_else(|| null("edge use without UV trim curve"))?;
                            if is_iso_curve_at(held_curve(b, slot)?, side, d.low, tol_j)? {
                                mirror_curve(
                                    k,
                                    held_curve_mut(b, slot)?,
                                    side,
                                    d.low,
                                    d.high,
                                    tol_j,
                                )?;
                                repair_made = true;
                            }
                        }
                    }

                    if d.high < d.low {
                        core::mem::swap(&mut d.low, &mut d.high);
                    }
                }
            }
        }
    }
    Ok(repair_made)
}

// ---- loop orientation and order ---------------------------------------------------------

/// `FixTrimLoopOrientation(pFace, pLoop, bOuterLoop, ..)`: when the loop's orientation
/// is known and not the one wanted (same for the outer loop, opposite for an inner one),
/// flips the face (outer loop) or the loop. Returns whether it reoriented.
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmFace::FlipFaceOrientation`, `topology.face_p1`); those
/// of `SmFace::ComputeLoopOrientation` and
/// [`lu1::flip_loop_orientation`].
pub fn fix_loop_orientation(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    l: LoopId,
    outer_loop: bool,
) -> Result<bool> {
    // Determine if the orientation is correct
    let desired = if outer_loop {
        OrientType::Same
    } else {
        OrientType::Opposite
    };
    let loop_orientation = face_compute_loop_orientation(k, b, f, l)?;
    if loop_orientation != desired && loop_orientation != OrientType::Unknown {
        if outer_loop {
            face_flip_face_orientation(b, f)?;
        } else {
            lu1::flip_loop_orientation(b, l)?;
        }
        return Ok(true);
    }
    Ok(false)
}

/// `FixTrimLoopOrientation(pFace, &count)`: [`fix_loop_orientation`] for every loop of
/// the face, the first as outer loop; the number reoriented.
///
/// # Errors
///
/// Those of [`fp2::loops`] and [`fix_loop_orientation`].
pub fn fix_face_loop_orientations(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<usize> {
    let mut reoriented = 0;
    let loops = fp2::loops(b, f, false, None)?;
    for (ii, &l) in loops.iter().enumerate() {
        if fix_loop_orientation(k, b, f, l, ii == 0)? {
            reoriented += 1;
        }
    }
    Ok(reoriented)
}

/// [`fix_loop_orientation`] whose status upstream ignores: its `rbLoopReoriented`,
/// `false` when it failed (set before the failing call); an unported callee still
/// propagates.
fn fix_loop_orientation_ignored(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    l: LoopId,
    outer_loop: bool,
) -> Result<bool> {
    match fix_loop_orientation(k, b, f, l, outer_loop) {
        Ok(r) => Ok(r),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
    }
}

/// `SmOwningTopology::Remove` then `PreInsert` of a loop use in its face use's list:
/// moves it to the head.
fn move_loopuse_first(
    b: &mut Brep,
    fu: crate::topology::FaceuseId,
    lu: crate::topology::LoopuseId,
) -> Result<()> {
    let list = &mut b.faceuse_mut(fu)?.loopuses;
    let pos = list
        .iter()
        .position(|&x| x == lu)
        .ok_or(SolidError::sm_err(OP))?;
    list.remove(pos);
    list.insert(0, lu);
    Ok(())
}

/// `FixTrimLoopOrder`: takes the loop whose UV trim curves have the largest bounding box
/// (by squared diagonal) as the outer loop, orients it as outer and the others as inner
/// when their orientation shows the opposite, moves its loop uses to the head of their
/// face uses' lists and labels the loop uses (outer: same, others: opposite). Returns
/// `rbLoopsReordered` as upstream leaves it (the last orientation fix's flag when one
/// ran, else whether the outer loop was not first).
///
/// # Errors
///
/// `SM_ERR` for a face without loops; [`SolidError::InvalidInput`] for an edge without
/// UV trim curve on the face's surface (upstream dereferences it); `SM_ERR_NULL_POINTER`
/// for a loop use without face use; those of the callees
/// (`SmFace::ComputeLoopOrientation`, `topology.face_p3`).
pub fn fix_trim_loop_order(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    let mut loops_reordered = false;

    // Get the loops
    let loops = fp2::loops(b, f, false, None)?;
    if loops.is_empty() {
        return Err(SolidError::sm_err(OP)); // no loops at all
    }

    // Find the outer loop
    let mut outer_index: Option<usize> = None;
    let mut size_max = -1.0;
    let mut inners: Vec<usize> = Vec::new();
    for (ii, &l) in loops.iter().enumerate() {
        // find the 2d size of the loop and save the largest size
        let mut uv_box = Extent3d::new();
        for (jj, e) in lu1::loop_edges(b, l)?.into_iter().enumerate() {
            let e = e.ok_or_else(|| null("loop edge use without edge"))?;
            let holder = crate::topology::edge::uv_trim_curve_of_surface(k, b, e, Some(f))?
                .ok_or_else(|| null("edge without UV trim curve on the face's surface"))?;
            let crv = eu1::uv_trim_curve(b, holder)?
                .0
                .ok_or_else(|| null("edge without UV trim curve on the face's surface"))?;

            // get curve's UV bounding box
            let crv_box = crv
                .calculate_bounding_box(
                    k,
                    &crv.natural_interval(),
                    BoundingBoxRequest {
                        normal: true,
                        pseudo: false,
                        polar: false,
                    },
                    true,
                )?
                .normal
                .unwrap_or_default();

            // get union of all curve bounding boxes
            uv_box = if jj == 0 {
                crv_box
            } else {
                crv_box.union(&uv_box)
            };
        }

        let size = uv_box.size().length_squared();
        match outer_index {
            None => {
                // first "outer' candidate, save size and index
                size_max = size;
                outer_index = Some(ii);
            }
            Some(prev) if size > size_max => {
                // save the new best, put previous on inners list
                size_max = size;
                inners.push(prev);
                outer_index = Some(ii);
            }
            Some(_) => inners.push(ii), // not big enough ..add this to inners list
        }
    }

    // set direction of the outer loop
    let outer_index = outer_index.ok_or_else(|| null("no outer loop"))?;
    let outer_loop = at(&loops, outer_index)?;

    // if outer is not the first
    if outer_index != 0 {
        loops_reordered = true;
    }

    if face_compute_loop_orientation(k, b, f, outer_loop)? == OrientType::Opposite {
        loops_reordered = fix_loop_orientation_ignored(k, b, f, outer_loop, true)?;
    }

    // set direction of the inner loops
    for &kk in &inners {
        let inner_loop = at(&loops, kk)?;
        if face_compute_loop_orientation(k, b, f, inner_loop)? == OrientType::Same {
            loops_reordered = fix_loop_orientation_ignored(k, b, f, inner_loop, false)?;
        }
    }

    // Move outer to the first position
    let (lu_a, lu_b) = lu1::loop_loopuses(b, outer_loop)?;
    let lu_b = lu_b.ok_or_else(|| null("loop without second loop use"))?;
    let fu_a = b
        .loopuse(lu_a)?
        .faceuse
        .ok_or(SolidError::null_pointer(OP))?;
    let fu_b = b
        .loopuse(lu_b)?
        .faceuse
        .ok_or(SolidError::null_pointer(OP))?;
    move_loopuse_first(b, fu_a, lu_a)?;
    move_loopuse_first(b, fu_b, lu_b)?;

    // First set them all to inner loops
    for &l in &loops {
        let (x, y) = lu1::loop_loopuses(b, l)?;
        let y = y.ok_or_else(|| null("loop without second loop use"))?;
        lu1::set_orientation(b, x, OrientType::Opposite)?;
        lu1::set_orientation(b, y, OrientType::Opposite)?;
    }

    // Then set the outer loop
    lu1::set_orientation(b, lu_a, OrientType::Same)?;
    lu1::set_orientation(b, lu_b, OrientType::Same)?;
    Ok(loops_reordered)
}

// ---- planar loops of 3D curves ------------------------------------------------------------

/// [`order_curves_into_loops`]' outputs.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedLoops {
    /// `rsOrderedCurves`: indices of the input curves in loop order.
    pub ordered: Vec<usize>,
    /// `rsLoopCounts`: the number of curves in each loop.
    pub loop_counts: Vec<usize>,
    /// `rsOrients`: each ordered curve's orientation in its loop.
    pub orients: Vec<OrientType>,
    /// `rsPlanePoint`: the average of the loop points.
    pub plane_point: Point3,
    /// `rsPlaneNormal`: the unit plane normal.
    pub plane_normal: Vec3,
    /// `rsCurvesBBox`: the bounding box of all curves.
    pub curves_box: Extent3d,
    /// `false` when a loop was open or compound, ended away from its first curve's
    /// start point, or a single curve was not closed (upstream's tentative
    /// `sRet = SM_ERR`).
    pub closed: bool,
    /// `false` when a curve was found off the plane (upstream returns `SM_ERR` there).
    pub on_plane: bool,
}

impl OrderedLoops {
    /// Upstream's return status: `SM_ERR` unless [`OrderedLoops::closed`] and
    /// [`OrderedLoops::on_plane`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` as described.
    pub const fn status(&self) -> Result<()> {
        if self.closed && self.on_plane {
            Ok(())
        } else {
            Err(SolidError::sm_err(OP))
        }
    }
}

fn eval_point(k: &Kernel, c: &Curve, t: f64) -> Result<Point3> {
    c.evaluate_point(k, t)
}

/// `OrderCurvesIntoLoops`: orders `curves` into closed loops by matching their end
/// points (within `max(tol_3d, radius/1000)` of the curves' bounding sphere), computes the
/// plane through all loops (average loop point, unit normal from the loops' polygons)
/// and tests every curve against it (within `1000 * tol_3d`).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` when a loop or the summed normal does not unitize; those of
/// the curve evaluations, bounding boxes, the point search and the plane test.
#[allow(clippy::too_many_lines)]
pub fn order_curves_into_loops(k: &Kernel, curves: &[Curve], tol_3d: f64) -> Result<OrderedLoops> {
    // init outputs
    let mut out = OrderedLoops {
        ordered: Vec::new(),
        loop_counts: Vec::new(),
        orients: Vec::new(),
        plane_point: Point3::new(0.0, 0.0, 0.0),
        plane_normal: Vec3::new(0.0, 0.0, 0.0),
        curves_box: Extent3d::new(),
        closed: true,
        on_plane: true,
    };

    let n_curves = curves.len();
    let mut vertex_points: Vec<Point3> = Vec::new();
    let mut seen = vec![false; n_curves];

    // for every input curve - build sVertexPoints list and rsCurveBBox
    for (ii, c) in curves.iter().enumerate() {
        let iv = c.natural_interval();

        // Degenerate curves are only signalled (SE): the status is ignored
        crate::error::ignore_status(c.is_degenerate(k, crate::math::EFF_ZERO, None))?;

        // curve start/end points
        let start = eval_point(k, c, iv.min())?;
        let end = eval_point(k, c, iv.max())?;

        // when endPoints are within tol - degenerate check (signalled only)
        if start.distance(end) < 2.0 * tol_3d {
            crate::error::ignore_status(c.approximate_length(k, &iv, 5, false))?;
        }

        // build sVertexPoints list
        vertex_points.push(start);
        vertex_points.push(end);

        // accumulate bounding box for all curves
        let ext = c
            .calculate_bounding_box(
                k,
                &iv,
                BoundingBoxRequest {
                    normal: true,
                    pseudo: false,
                    polar: false,
                },
                true,
            )?
            .normal
            .unwrap_or_default();
        out.curves_box = if ii == 0 {
            ext
        } else {
            ext.union(&out.curves_box)
        };
    }

    // curve bounding box size scale
    let face_radius = out.curves_box.sphere_bound().radius;
    let feature_size = face_radius / 1000.0;

    let mut plane_base_count: usize = 0;

    // start-next-loop while block: find a curve not yet used in a loop
    while let Some(first) = seen.iter().position(|&s| !s) {
        let mut curve_index = first;
        let mut loop_points: Vec<Point3> = Vec::new();
        let mut orient = OrientType::Same;
        let mut loop_edges: usize = 0;
        let first_curve_index = curve_index;

        // build-this-loop while block
        loop {
            let c = curves.get(curve_index).ok_or_else(|| null("curve index"))?;
            out.orients.push(orient);
            out.ordered.push(curve_index);
            loop_edges += 1;
            if let Some(s) = seen.get_mut(curve_index) {
                *s = true;
            }

            let start = at(&vertex_points, 2 * curve_index)?;
            let end = at(&vertex_points, 2 * curve_index + 1)?;

            let ivl = c.natural_interval();
            let end_pt_gap = start.distance(end);
            let single_curve_loop = end_pt_gap < tol_3d;

            // first loop curve is not closed
            if n_curves == 1 && !single_curve_loop {
                out.closed = false;
            }

            // Get 3 distinct points for the plane computation
            let (pt1, pt2, pt3) = if single_curve_loop {
                (
                    start,
                    eval_point(k, c, ivl.evaluate(0.3))?,
                    eval_point(k, c, ivl.evaluate(0.6))?,
                )
            } else {
                (start, eval_point(k, c, ivl.evaluate(0.5))?, end)
            };

            // Registering the 3 loop points in the correct order
            if orient == OrientType::Same {
                loop_points.extend([pt1, pt2, pt2, pt3]);
                if single_curve_loop {
                    loop_points.extend([pt3, pt1]);
                }
            } else {
                loop_points.extend([pt3, pt2, pt2, pt1]);
                if single_curve_loop {
                    loop_points.extend([pt1, pt3]);
                }
            }

            // find unique matching curve-end point within tol.
            let target = if orient == OrientType::Same {
                2 * curve_index + 1
            } else {
                2 * curve_index
            };
            let m = find_first_point_within_tol_to_pnt(
                &vertex_points,
                target,
                if tol_3d > feature_size {
                    tol_3d
                } else {
                    feature_size
                },
            )?;

            // abnormal exit: open loop (no matches) or compound loop
            if !m.found {
                out.closed = false;
                break;
            }

            // Matching Curve index and seen status
            let next_index = if m.index % 2 == 0 {
                m.index / 2
            } else {
                (m.index - 1) / 2
            };

            // expected exit: closed loop
            if at(&seen, next_index)? {
                // a closed loop ends at its first curve's start point; ending on another
                // seen curve (e.g. the start of a closed curve in the chain) leaves it open
                let loop_end_pt = if orient == OrientType::Same {
                    end
                } else {
                    start
                };
                let loop_tol = if tol_3d > feature_size {
                    tol_3d
                } else {
                    feature_size
                };
                if loop_end_pt.distance(at(&vertex_points, 2 * first_curve_index)?) >= loop_tol {
                    out.closed = false;
                }
                break;
            }

            // Preparing for next iter
            orient = if m.index % 2 == 0 {
                OrientType::Same
            } else {
                OrientType::Opposite
            };
            curve_index = next_index;
        }

        // set output
        out.loop_counts.push(loop_edges);

        // for every current-loop point pair - gather loop point sum and the normal sum
        let mut loop_normal = Vec3::new(0.0, 0.0, 0.0);
        let mut jj = 0;
        while jj < loop_points.len() {
            let p1 = at(&loop_points, jj)?;
            let p2 = at(&loop_points, jj + 1)?;
            loop_normal = Vec3::new(
                loop_normal.x + (p1.y - p2.y) * (p1.z + p2.z),
                loop_normal.y + (p1.z - p2.z) * (p1.x + p2.x),
                loop_normal.z + (p1.x - p2.x) * (p1.y + p2.y),
            );
            out.plane_point += p1;
            plane_base_count += 1;
            jj += 2;
        }
        let loop_normal = loop_normal
            .unitize()
            .map_err(|_| status(SM_ERR_INVALID_INPUT))?;

        // Update the plane data for all loops
        if out.loop_counts.len() == 1 {
            out.plane_normal = loop_normal;
        } else {
            let discr = out.plane_normal.dot(loop_normal);
            out.plane_normal = if discr < 0.0 {
                (out.plane_normal - loop_normal) * 0.5
            } else {
                (out.plane_normal + loop_normal) * 0.5
            };
        }
    }

    // let the planeBase = Avg(All Plane Points)
    // operator/(v, d): plain division (`/=` guards tiny divisors)
    let count = plane_base_count as f64;
    out.plane_point = Vec3::new(
        out.plane_point.x / count,
        out.plane_point.y / count,
        out.plane_point.z / count,
    );

    // Now we can create the plane from normal, basepoint, extents
    out.plane_normal = out
        .plane_normal
        .unitize()
        .map_err(|_| status(SM_ERR_INVALID_INPUT))?;

    // test every curve to see if it's on the plane
    for c in curves {
        let r = c.is_on_plane(k, out.plane_point, out.plane_normal, tol_3d * 1000.0, None)?;
        if !r.on_plane {
            out.on_plane = false;
            return Ok(out);
        }
    }
    Ok(out)
}

/// `FindOuterLoops`: classifies the loops of `curves` / `loop_counts` / `orients` (as
/// [`order_curves_into_loops`] gives them; `curves` are indices into `all_curves`) on the
/// plane with normal `plane_normal`: repeatedly the largest unclassified loop (by the
/// area of its tessellation) becomes an outer loop, oriented counter-clockwise, followed
/// by the unclassified loops inside it, oriented clockwise. Rewrites the three lists in
/// that order and returns the number of loops in each outer loop group.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for counts that do not match the lists or an index out of
/// range; those of the tessellations, areas and containment tests.
#[allow(clippy::too_many_lines)]
pub fn find_outer_loops(
    k: &Kernel,
    all_curves: &[Curve],
    curves: &mut Vec<usize>,
    loop_counts: &mut [usize],
    orients: &mut Vec<OrientType>,
    plane_normal: &mut Vec3,
    tol_3d: f64,
) -> Result<Vec<usize>> {
    // init outputs
    let mut outer_loop_counts = Vec::new();
    if loop_counts.is_empty() {
        return Ok(outer_loop_counts);
    }

    let num_curves = curves.len();
    let num_loops = loop_counts.len();

    let mut out_curves: Vec<usize> = Vec::new();
    let mut out_loops: Vec<usize> = Vec::new();
    let mut out_orients: Vec<OrientType> = Vec::new();
    let mut out_outer_loops: Vec<usize> = Vec::new();
    let mut in_loops: Vec<usize> = Vec::new();

    // set sInLoops[i] = Input loop[i] FirstCrv index
    let mut loop_index: usize = 0;
    for &c in loop_counts.iter() {
        in_loops.push(loop_index);
        loop_index = loop_index.wrapping_add(c);
    }

    // Tessellate parameters
    let ch = 0.0;
    let ang = 15.0;
    let min_segs = 2;

    let mut loop_areas = vec![0.0; num_loops];
    let mut loop_orients = vec![OrientType::Same; num_loops];
    let mut pt_seqs: Vec<PointSequence> = Vec::new();

    let mut next_loop_start: usize = 0;
    // for every loop - build sLoopAreas, sLoopOrients, sPtSeqs
    for ii in 0..num_loops {
        let mut loop_tess_pts: Vec<Point3> = Vec::new();
        let this_loop_start = next_loop_start;
        next_loop_start = next_loop_start.wrapping_add(at(loop_counts, ii)?);

        for jj in this_loop_start..next_loop_start {
            let crv = all_curves
                .get(at(curves, jj)?)
                .ok_or_else(|| null("curve index"))?;
            let tess = crv.tessellate(k, &crv.natural_interval(), ch, ang, min_segs)?;
            let mut pts = tess.points;
            if at(orients, jj)? == OrientType::Opposite {
                pts.reverse();
            }
            loop_tess_pts.extend(pts);
        }

        let mut seq = PointSequence::new(loop_tess_pts, tol_3d, true, Some(plane_normal));
        let area = seq.area()?;
        pt_seqs.push(seq);
        if let Some(o) = loop_orients.get_mut(ii) {
            *o = if area > 0.0 {
                OrientType::Same
            } else {
                OrientType::Opposite
            };
        }
        if let Some(a) = loop_areas.get_mut(ii) {
            *a = area.abs();
        }
    }

    let flip = |o: OrientType| {
        if o == OrientType::Same {
            OrientType::Opposite
        } else {
            OrientType::Same
        }
    };

    // while loops remain to be classified
    loop {
        let mut max_area = -1.0;
        let mut largest: usize = 0;

        // Find largest unprocessed loop - it's the next OuterLoop
        for (ii, &a) in loop_areas.iter().enumerate() {
            if a <= 0.0 {
                continue;
            }
            if a > max_area {
                max_area = a;
                largest = ii;
            }
        }

        // exit case - no unprocessed loops
        if max_area < 0.0 {
            break;
        }

        // get largest loop's start and end Curve indices
        let this_start = at(&in_loops, largest)?;
        let next_start = if largest + 1 >= num_loops {
            num_curves
        } else {
            at(&in_loops, largest + 1)?
        };

        // remember to flip CW outer loops (outerloops are stored CCW)
        let do_flip = at(&loop_orients, largest)? == OrientType::Opposite;
        for ii in this_start..next_start {
            let index = if do_flip {
                (next_start - 1) - (ii - this_start)
            } else {
                ii
            };
            let o = at(orients, index)?;
            out_curves.push(at(curves, index)?);
            out_orients.push(if do_flip { flip(o) } else { o });
        }
        out_loops.push(out_curves.len());

        // Mark Loop as processed (set its area to -1)
        if let Some(a) = loop_areas.get_mut(largest) {
            *a *= -1.0;
        }

        // for every inloop - classify Loop against current OuterLoop
        for ii in 0..num_loops {
            if ii == largest || at(&loop_areas, ii)? < 0.0 {
                continue;
            }

            let mut in_count: i32 = 0;
            let pts: Vec<Point3> = pt_seqs
                .get(ii)
                .ok_or_else(|| null("point sequence"))?
                .points()
                .to_vec();
            let num_pts = pts.len();
            for &p in &pts {
                let contain = pt_seqs
                    .get_mut(largest)
                    .ok_or_else(|| null("point sequence"))?
                    .point_containment(p)?;
                in_count += match contain {
                    PointContainment::Inside => 1,
                    PointContainment::Outside => -1,
                    _ => 0,
                };
                if in_count >= 2 || in_count <= -2 {
                    break;
                }
            }

            // classify the loop from the point classifications - allow for small sets
            let contained = in_count >= 2 || (in_count > 0 && num_pts < 3);
            if !contained {
                continue;
            }

            let this_start = at(&in_loops, ii)?;
            let next_start = if ii + 1 >= num_loops {
                num_curves
            } else {
                at(&in_loops, ii + 1)?
            };

            // Inner loops are output with 'Opposite' CW orientation
            let do_flip = at(&loop_orients, ii)? == OrientType::Same;
            for jj in this_start..next_start {
                let index = if do_flip {
                    (next_start - 1) - (jj - this_start)
                } else {
                    jj
                };
                let o = at(orients, index)?;
                out_curves.push(at(curves, index)?);
                out_orients.push(if do_flip { flip(o) } else { o });
            }
            out_loops.push(out_curves.len());

            // mark current loop as processed
            if let Some(a) = loop_areas.get_mut(ii) {
                *a *= -1.0;
            }
        }

        // set sOutOuterLoops[i] = NextOuterLoop StartIndex in sOutLoops array
        out_outer_loops.push(out_loops.len());
    }

    // set output
    *curves = out_curves;
    *orients = out_orients;

    // all loops list - from start indices back to curve counts per loop
    let first = at(&out_loops, 0)?;
    if let Some(c) = loop_counts.first_mut() {
        *c = first;
    }
    for ii in 1..num_loops {
        let v = at(&out_loops, ii)?.wrapping_sub(at(&out_loops, ii - 1)?);
        if let Some(c) = loop_counts.get_mut(ii) {
            *c = v;
        }
    }

    // OuterLoops list - back to Loop counts per OuterLoop
    if let Some(&f0) = out_outer_loops.first() {
        outer_loop_counts.push(f0);
    }
    for ii in 1..out_outer_loops.len() {
        outer_loop_counts.push(at(&out_outer_loops, ii)? - at(&out_outer_loops, ii - 1)?);
    }
    Ok(outer_loop_counts)
}

// ---- checks -------------------------------------------------------------------------------

/// `CheckFace`: `SM_ERR` (with a report) for a face without loops; nothing to check
/// when some UV trim curve is missing; otherwise [`check_loop`] (unless
/// `skip_check_loop`) and the loop orientation test (outer loop first, the others
/// inner, none of zero area) for every loop.
///
/// # Errors
///
/// `SM_ERR` as described; [`SolidError::InvalidInput`] for a face without surface (with
/// a loop to check); those of `SmFace::ComputeLoopOrientation` (`topology.face_p3`) and
/// [`check_loop`].
pub fn check_face(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    tol_3d: f64,
    mut alist: Option<&mut AssertArray>,
    skip_check_loop: bool,
) -> Result<()> {
    let face = EntityRef::Face(f);
    let loops = fp2::loops(b, f, false, None)?;

    // error - missing loops
    if loops.is_empty() {
        report(
            &mut alist,
            face,
            FACE_TYPE,
            "SmFace",
            LIST_CHECK_FACE,
            0,
            false,
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;
        return Err(SolidError::sm_err(OP)); // Loops are missing
    }

    // it's not a problem when an edge does not have a stored UVTrimCurve
    if !face_uv_curves_present(b, f)? {
        return Ok(());
    }

    for (ii, &l) in loops.iter().enumerate() {
        if !skip_check_loop {
            let surface = b
                .face(f)?
                .surface
                .clone()
                .ok_or_else(|| null("face without surface"))?;
            check_loop(k, b, &surface, l, tol_3d, alist.as_deref_mut())?;
        }

        // Check the loop orientation results.
        let lo = face_compute_loop_orientation(k, b, f, l);
        let lo = match lo {
            Err(e) if e.is_unported() => return Err(e),
            other => other,
        };
        report(
            &mut alist,
            face,
            FACE_TYPE,
            "SmFace",
            LIST_CHECK_FACE,
            2,
            lo.is_ok(),
            tol_3d,
            UNDEF_DOUBLE,
        )?;
        let lo = lo?;

        report(
            &mut alist,
            face,
            FACE_TYPE,
            "SmFace",
            LIST_CHECK_FACE,
            3,
            (ii == 0 && lo == OrientType::Same) || (ii != 0 && lo == OrientType::Opposite),
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;

        // signal bad loop errors: we want 1st Loop Outer, rest Inner
        if (ii == 0 && lo == OrientType::Opposite) || (ii != 0 && lo == OrientType::Same) {
            return Err(SolidError::sm_err(OP));
        }

        report(
            &mut alist,
            face,
            FACE_TYPE,
            "SmFace",
            LIST_CHECK_FACE,
            4,
            lo != OrientType::Unknown,
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;

        // Also check for zero-area loop.
        if lo == OrientType::Unknown {
            return Err(SolidError::sm_err(OP));
        }
    }
    Ok(())
}

/// `CheckFaces`: [`check_face`] (with loop checks) for every face.
///
/// # Errors
///
/// Those of [`check_face`].
pub fn check_faces(
    k: &Kernel,
    b: &mut Brep,
    faces: &[FaceId],
    tol_3d: f64,
    mut alist: Option<&mut AssertArray>,
) -> Result<()> {
    for &f in faces {
        check_face(k, b, f, tol_3d, alist.as_deref_mut(), false)?;
    }
    Ok(())
}

/// `sm_IsLoopUVGapWithinTolerance`: whether the UV gap from `start_uv` to `end_uv` on a
/// B-spline surface (any `SmBSplineSurface` kind) is certified closed: the gap's UV
/// rectangle lies in the natural domain and every control point of its exact sub-patch
/// (or, for a zero-width rectangle, of the iso-parametric curve over the gap) has a
/// positive weight and lies within `xsect_tol_3d` of both `start_point` and
/// `end_point`. Failures of the extraction give `false`.
///
/// # Errors
///
/// [`SolidError::Unported`] from the extraction.
pub fn is_loop_uv_gap_within_tolerance(
    k: &Kernel,
    surface: &Surface,
    start_uv: Point2,
    end_uv: Point2,
    start_point: Point3,
    end_point: Point3,
    xsect_tol_3d: f64,
) -> Result<bool> {
    // the extractions run on GetOrCreateGwNurbPointer: a plane makes its NURB (a failure
    // is their NER, which certifies nothing)
    let bss: Cow<'_, BSplineSurface> = match surface.kind_with_nurb(k)? {
        SurfaceKind::BSpline(b) => Cow::Borrowed(b),
        SurfaceKind::Plane(p) => match p.base_with_nurb() {
            Ok(b) => b,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(false),
        },
        SurfaceKind::SurfOfRevolution(s) => Cow::Borrowed(s.base()),
        SurfaceKind::SurfOfExtrusion(s) => Cow::Borrowed(s.base()),
        SurfaceKind::Cone(s) => Cow::Borrowed(s.rev().base()),
        SurfaceKind::Sphere(s) => Cow::Borrowed(s.base().base()),
        SurfaceKind::Torus(s) => Cow::Borrowed(s.base().base()),
        // (dynamic_cast<const SmBSplineSurface*> fails: FALSE)
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::OffsetSurface(_) => return Ok(false),
    };

    let mut gap = Extent2d::new();
    gap.add_point(start_uv);
    gap.add_point(end_uv);

    // Only certify gaps inside the surface's natural domain.
    let nat = surface.natural_uv_domain_with_nurb(k)?;
    if gap.min().x < nat.min().x
        || gap.min().y < nat.min().y
        || gap.max().x > nat.max().x
        || gap.max().y > nat.max().y
    {
        return Ok(false);
    }

    let (points, weights) = if gap.x_length() == 0.0 || gap.y_length() == 0.0 {
        // A zero-width rectangle is an isoparametric curve, not a surface patch.
        let dir = if gap.x_length() == 0.0 {
            SurfParamType::U
        } else {
            SurfParamType::V
        };
        let param = if dir == SurfParamType::U {
            start_uv.x
        } else {
            start_uv.y
        };
        let gap_ivl = if dir == SurfParamType::U {
            gap.v_interval()
        } else {
            gap.u_interval()
        };
        if gap_ivl.length() <= 0.0 {
            return Ok(false);
        }
        let iso = match bss.create_iso_parametric_curve(
            k,
            dir,
            param,
            IsoCurveOptions {
                domain: None,
                uv_iso_curve: false,
            },
        ) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(false),
        };
        let mut curve = iso.curve;
        let mut trim = gap_ivl;
        match curve.trim(k, &mut trim, TrimMode::Notify) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(false),
        }
        // Trimming can snap to knots; the curve must still cover the whole gap.
        let civl = curve.natural_interval();
        if civl.min() > gap_ivl.min() || civl.max() < gap_ivl.max() {
            return Ok(false);
        }
        match curve.control_polygon() {
            Ok(x) => x,
            Err(_) => return Ok(false),
        }
    } else {
        let patch = match bss.copy_sub_patch(&gap) {
            Ok(p) => p,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(false),
        };

        // CopySubPatch can snap to knots. A shrunken domain cannot certify this gap.
        let pd = patch.natural_uv_domain();
        if pd.min().x > gap.min().x
            || pd.min().y > gap.min().y
            || pd.max().x < gap.max().x
            || pd.max().y < gap.max().y
        {
            return Ok(false);
        }
        match patch.control_point_net() {
            Ok(net) => (net.points, net.weights),
            Err(_) => return Ok(false),
        }
    };
    if points.is_empty() {
        return Ok(false);
    }

    // Empty weights mean a nonrational curve or patch, with implicit unit weights.
    let rational = !weights.is_empty();
    for (i, p) in points.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(0.0);
        #[allow(clippy::neg_cmp_op_on_partial_ord)] // upstream's NaN-safe tests
        if (rational && !(w > 0.0))
            || !(p.distance(start_point) <= xsect_tol_3d)
            || !(p.distance(end_point) <= xsect_tol_3d)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `SmTol::GetZoneTol3d(SmEdge*)`: the edge's tolerance, the system one for no edge.
fn edge_zone_tol(k: &Kernel, b: &Brep, e: Option<EdgeId>) -> Result<f64> {
    Ok(match e {
        Some(e) => tol::zone_tol3d(k, crate::topology::edge::tolerance(b, e)?),
        None => tol::zone_tol_3d(k, SmallTopoType::NormalSize),
    })
}

// Only a genuinely shared topological vertex can enlarge the edge/edge limit, and only
// with the opt-in `Kernel::fixes.check_loop_vertex_tolerance` (WR-381, #476).
// Use the previous edge use's mate: its vertex use is the previous end regardless
// of either edge use's orientation (including closed edges and reversed loops).
fn loop_junction_tolerance(
    k: &Kernel,
    b: &Brep,
    current: EdgeuseId,
    previous: EdgeuseId,
) -> Result<f64> {
    let edge_limit = tol::xsect_tol_3d_from_zones(
        k,
        edge_zone_tol(k, b, eu1::edge(b, current)?)?,
        edge_zone_tol(k, b, eu1::edge(b, previous)?)?,
    );
    if !k.fixes.check_loop_vertex_tolerance {
        return Ok(edge_limit);
    }
    let Some(start_use) = eu1::vertexuse(b, current)? else {
        return Ok(edge_limit);
    };
    let Some(end) = eu1::mate(b, previous)? else {
        return Ok(edge_limit);
    };
    let Some(end_use) = eu1::vertexuse(b, end)? else {
        return Ok(edge_limit);
    };
    let start_vertex = b.vertexuse(start_use)?.vertex;
    let end_vertex = b.vertexuse(end_use)?.vertex;
    if let Some(vertex) = start_vertex.filter(|v| Some(*v) == end_vertex) {
        return Ok(edge_limit.max(tol::zone_tol3d(k, b.vertex(vertex)?.tolerance)));
    }
    Ok(edge_limit)
}

// A projected endpoint is known only modulo the natural period. Exact UV trims
// keep their chosen sheet. This lift is only for the UV gap test: surface points
// are evaluated at the original parameters so a real 3D gap cannot be hidden by
// a B-spline clamping an out-of-domain periodic representative.
fn nearest_period(value: f64, target: f64, period: f64) -> f64 {
    if !value.is_finite() || !target.is_finite() || !period.is_finite() || period <= 0.0 {
        return value;
    }
    let lifted = value + ((target - value) / period).round() * period;
    if lifted.is_finite() {
        lifted
    } else {
        value
    }
}

/// `CheckLoop`: for each pair of consecutive edge uses of the loop whose UV ends differ
/// (by more than `1e-10`), the surface points must be within the edges'
/// intersection tolerance (with `Kernel::fixes.check_loop_vertex_tolerance`, the larger of
/// it and their shared vertex's zone tolerance; else closure 3, signalled only) and the UV jump must stay under a tenth of
/// the natural domain in each direction, unless both ends are singular in that
/// direction or the gap is certified by [`is_loop_uv_gap_within_tolerance`] (not across
/// a seam). A large UV jump gives closure 1 when an end is singular, else `SM_ERR`.
/// With `Kernel::fixes.check_loop_projected_seam_ends` (WR-440), projected ends without
/// UV trims are compared on the nearest natural period in closed directions; exact trim
/// ends retain their sheet. The 3D gap is always measured at the original parameters.
/// Returns the closure: 0 closed, 1 closed except at singularities, 3 not closed in 3D.
///
/// # Errors
///
/// `SM_ERR` for a large UV gap without singularity; [`SolidError::InvalidInput`] for a
/// loop without loop use; those of [`edge_uv_ends`], the evaluations and the
/// singularity tests.
pub fn check_loop(
    k: &Kernel,
    b: &mut Brep,
    surface: &Surface,
    l: LoopId,
    tol_3d: f64,
    mut alist: Option<&mut AssertArray>,
) -> Result<u32> {
    let lp = EntityRef::Loop(l);
    let mut closure = 0;
    let eus = loop_edgeuses(b, l)?;
    let n = eus.len();

    for ii in 0..n {
        let cur = at(&eus, ii)?;
        let current_uv = edge_uv_extremes(k, b, cur)?;
        let current_start = current_uv.start;
        let previous = if ii == 0 { n - 1 } else { ii - 1 };
        let prev = at(&eus, previous)?;
        let previous_uv = edge_uv_extremes(k, b, prev)?;
        let previous_end = previous_uv.end;

        // When UVPoints vary by more than 1e-10
        if previous_end.distance(current_start) > 1e-10 {
            let previous_end_3d = surface.evaluate_point(k, previous_end)?;
            let current_start_3d = surface.evaluate_point(k, current_start)?;
            let end_start_dist = previous_end_3d.distance(current_start_3d);

            // A widened shared vertex covers gaps that the two edge zones alone
            // need not cover (#476). Retain the existing edge intersection limit.
            let xsect = loop_junction_tolerance(k, b, cur, prev)?;
            report(
                &mut alist,
                lp,
                LOOP_TYPE,
                "SmLoop",
                LIST_CHECK_LOOP,
                0,
                end_start_dist <= xsect,
                xsect,
                end_start_dist,
            )?;
            if end_start_dist > xsect {
                closure = 3; // SE_MSG: signalled, processing continues
            }

            let dom = surface.natural_uv_domain_with_nurb(k)?;
            let mut gap_start = current_start;
            let mut gap_end = previous_end;
            // WR-440 (#530), opt-in: upstream compares the raw representatives.
            if k.fixes.check_loop_projected_seam_ends
                && (current_uv.start_weight.is_none() || previous_uv.end_weight.is_none())
            {
                for (start, end, period, direction) in [
                    (
                        &mut gap_start.x,
                        &mut gap_end.x,
                        dom.x_length(),
                        SurfParamType::U,
                    ),
                    (
                        &mut gap_start.y,
                        &mut gap_end.y,
                        dom.y_length(),
                        SurfParamType::V,
                    ),
                ] {
                    if period.is_finite()
                        && period > 0.0
                        && (*start - *end).abs() > period * 0.10
                        && surface.is_closed(k, &dom, direction, None, false)?.0
                    {
                        if current_uv.start_weight.is_none() {
                            *start = nearest_period(*start, *end, period);
                        } else {
                            *end = nearest_period(*end, *start, period);
                        }
                    }
                }
            }
            let sing1 = surface
                .is_singularity(k, previous_end, tol_3d, false)?
                .direction;
            let sing2 = surface
                .is_singularity(k, current_start, tol_3d, false)?
                .direction;
            let large_u = (gap_start.x - gap_end.x).abs() > dom.x_length() * 0.10;
            let large_v = (gap_start.y - gap_end.y).abs() > dom.y_length() * 0.10;
            let is_u = |d: SurfParamType| d == SurfParamType::U || d == SurfParamType::Both;
            let is_v = |d: SurfParamType| d == SurfParamType::V || d == SurfParamType::Both;
            let singular_u = is_u(sing1) && is_u(sing2);
            let singular_v = is_v(sing1) && is_v(sing2);
            let mut bounded = false;
            if end_start_dist <= xsect
                && ((large_u && !singular_u) || (large_v && !singular_v))
                && !(large_u
                    && !singular_u
                    && surface.is_closed(k, &dom, SurfParamType::U, None, false)?.0)
                && !(large_v
                    && !singular_v
                    && surface.is_closed(k, &dom, SurfParamType::V, None, false)?.0)
            {
                bounded = is_loop_uv_gap_within_tolerance(
                    k,
                    surface,
                    previous_end,
                    current_start,
                    previous_end_3d,
                    current_start_3d,
                    xsect,
                )?;
            }
            report(
                &mut alist,
                lp,
                LOOP_TYPE,
                "SmLoop",
                LIST_CHECK_LOOP,
                1,
                !large_u || singular_u || bounded,
                dom.x_length() * 0.10,
                (gap_start.x - gap_end.x).abs(),
            )?;
            report(
                &mut alist,
                lp,
                LOOP_TYPE,
                "SmLoop",
                LIST_CHECK_LOOP,
                2,
                !large_v || singular_v || bounded,
                dom.y_length() * 0.10,
                (gap_start.y - gap_end.y).abs(),
            )?;

            if (large_u || large_v) && !bounded {
                closure = 2;
                if sing1 != SurfParamType::Neither || sing2 != SurfParamType::Neither {
                    closure = 1;
                }
                if closure >= 2 {
                    return Err(SolidError::sm_err(OP));
                }
            }
        }
    }
    Ok(closure)
}

// ---- surface and iso-curve tests ----------------------------------------------------------

/// `SurfaceContinuityCheck`: whether the surface is at least G1 in both directions
/// (neither minimum interior continuity is C0).
///
/// # Errors
///
/// `SM_ERR` for a discontinuous surface; those of `CalculateContinuities`.
pub fn surface_continuity_check(k: &Kernel, surface: &Surface) -> Result<bool> {
    let (cu, _) = surface.calculate_continuities(k, SurfParamType::U)?;
    let (cv, _) = surface.calculate_continuities(k, SurfParamType::V)?;
    if cu == ContinuityType::Discontinuous || cv == ContinuityType::Discontinuous {
        return Err(SolidError::sm_err(OP));
    }
    Ok(!(cu == ContinuityType::C0 || cv == ContinuityType::C0))
}

/// `MirrorCurve`: reflects the UV curve's `side` coordinate (u for
/// [`SurfParamType::U`]) about the middle of `[low, high]` (`high - (x - low)`), then
/// divides each control point by its weight again (as upstream does with the Euclidean
/// points `GetCanonical` returns) and makes the curve 2D. `tolerance` is unused.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a curve that is not an `SmBSplineCurve`; those of
/// `GetCanonical`, `SetCanonical` and `ConvertTo2D`.
pub fn mirror_curve(
    k: &Kernel,
    curve: &mut Curve,
    side: SurfParamType,
    low: f64,
    high: f64,
    tolerance: f64,
) -> Result<()> {
    let _ = tolerance; // NotUsed
    let coord = usize::from(side != SurfParamType::U);

    // Get the curve's control point info
    let canon = bspline_of(curve)?.canonical()?;
    let mut cps = canon.control_points;
    for (ii, p) in cps.iter_mut().enumerate() {
        let v = p.component(coord);
        if let Ok(c) = p.component_mut(coord) {
            *c = high - (v - low);
        }
        if let Some(&w) = canon.weights.get(ii) {
            // operator/(v, d): plain division (`/=` guards tiny divisors)
            *p = Vec3::new(p.x / w, p.y / w, p.z / w);
        }
    }
    let data = CanonicalData {
        dim: canon.dim,
        degree: canon.degree,
        control_points: &cps,
        form: canon.form,
        knot_multiplicities: &canon.knot_multiplicities,
        knots: &canon.knots,
        knot_type: canon.knot_type,
        weights: if canon.weights.is_empty() {
            None
        } else {
            Some(&canon.weights)
        },
        trim: None,
    };
    match curve.kind_mut() {
        CurveKind::BSpline(bc) => bc.set_canonical(&data)?,
        CurveKind::Line(l) => {
            if l.base.gw_nurb().is_none() {
                let _ = l.make_nurb();
            }
            l.base.set_canonical(&data)?;
        }
        // SmBSplineCurve::SetCanonical on the base
        CurveKind::Ellipse(e) => e.base_mut().set_canonical(&data)?,
        CurveKind::Hyperbola(h) => h.base_mut().set_canonical(&data)?,
        CurveKind::Parabola(p) => p.base_mut().set_canonical(&data)?,
        _ => return Err(null("MirrorCurve: not an SmBSplineCurve")),
    }
    curve.convert_to_2d(k)
}

/// `IsIsoCurve(cpUVCurve, eSide, dLow, d2DTolerance)`: whether every control point's
/// `side` coordinate is within `tol_2d` of `low`. A curve whose `GetCanonical` fails is
/// an iso curve (upstream returns the error status as `TRUE`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a curve that is not an `SmBSplineCurve`;
/// [`SolidError::Unported`].
pub fn is_iso_curve_at(curve: &Curve, side: SurfParamType, low: f64, tol_2d: f64) -> Result<bool> {
    let canon = match bspline_of(curve).and_then(|c| c.canonical()) {
        Ok(c) => c,
        Err(e)
            if e.is_unported()
                || matches!(
                    e,
                    SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
                ) =>
        {
            return Err(e)
        }
        Err(_) => return Ok(true),
    };
    let coord = usize::from(side != SurfParamType::U);
    for p in &canon.control_points {
        if (p.component(coord) - low).abs() > tol_2d {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `IsIsoCurve(cpUVCurve, rSide, rdParameter)`: for u then v, when the curve's end
/// points agree in that coordinate within `SM_ZONE_TOL_3D / 10`, whether the curve is an
/// iso curve at their mean ([`is_iso_curve_at`]); the side and parameter of the first
/// that is.
///
/// # Errors
///
/// Those of the curve's ends and [`is_iso_curve_at`].
pub fn is_iso_curve(k: &Kernel, curve: &Curve) -> Result<Option<(SurfParamType, f64)>> {
    let tolerance = ZONE_TOL_3D / 10.0;
    let ends = curve.ends(k)?;
    for ii in 0..2 {
        let (s, e) = (ends.start.component(ii), ends.end.component(ii));
        if (s - e).abs() < tolerance {
            let side = if ii == 0 {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
            let param = (s + e) / 2.0;
            if is_iso_curve_at(curve, side, param, tolerance)? {
                return Ok(Some((side, param)));
            }
        }
    }
    Ok(None)
}

/// `IsIsoCurveOnSeam`: whether the UV curve is an iso curve ([`is_iso_curve`]) on the
/// boundary (within a thousandth of the range) of a direction in which the surface is
/// closed.
///
/// # Errors
///
/// Those of [`is_iso_curve`] and the closure test.
pub fn is_iso_curve_on_seam(k: &Kernel, surface: &Surface, curve: &Curve) -> Result<bool> {
    let Some((side, param)) = is_iso_curve(k, curve)? else {
        return Ok(false);
    };
    let bounds = surface.natural_uv_domain_with_nurb(k)?;
    let closed = surface.is_closed(k, &bounds, side, None, false)?.0;
    let (lo, hi) = if side == SurfParamType::U {
        (bounds.min().x, bounds.max().x)
    } else {
        (bounds.min().y, bounds.max().y)
    };
    let range = Extent1d::from_raw(lo, hi);
    let tolerance = range.length() / 1000.0;
    Ok(closed
        && ((param - range.min()).abs() < tolerance || (param - range.max()).abs() < tolerance))
}

/// `SM_ERR_INVALID_INPUT` as an error.
pub(crate) const fn invalid_input_status() -> SolidError {
    status(SM_ERR_INVALID_INPUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::{CPoint, NOW, NOZ};

    #[test]
    fn original_g1_trim_retains_surface_identity_and_issued_face_owner() {
        use crate::curve::line::Line;
        use crate::geom::attribute::AttributeStore;
        use crate::surface::plane::Plane;
        use crate::topology::brep::{p1, p3};
        use crate::topology::edge::NotifyEnv;

        let mut k = Kernel::default();
        let mut b = p3::new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let serial = p3::notify_serial(&mut k.context, &mut b).unwrap();
        let key_of = |entity| p3::topology_ref(serial, entity);
        let mut records = crate::topology::topology::NotifyRecords::new();
        p3::seed_records(&mut records, &b, &key_of);
        let mut attributes = AttributeStore::default();
        let mut surface = Surface::new(SurfaceKind::Plane(Plane::from_normal(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
        )));
        let original_key = k
            .context
            .geometry_key(surface.notify_identity_slot())
            .unwrap();
        let uv = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];
        let points = uv.map(|p| surface.evaluate_point(&k, p).unwrap());
        let interval = Extent1d::from_min_max(0.0, 1.0);
        let curves = (0..4)
            .map(|i| {
                Curve::from(
                    Line::create_line_segment(3, points[i], points[(i + 1) % 4], Some(&interval))
                        .unwrap(),
                )
            })
            .collect();
        let input = TrimInput {
            loop_counts: vec![4],
            curves,
            orientations: vec![OrientType::Same; 4],
            loop_points: Vec::new(),
            surface,
            uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            surface_orientation: OrientType::Same,
        };
        let region = b.infinite_region();
        // This is the original public trim entry, not a constructor control. No
        // callbacks are requested; keys still come from the actual kernel context.
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        let made = trim_surface_with_model_space_curves(
            &mut k, &mut b, region, 1.0e-5, input, true, false, false, &mut env,
        )
        .unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(p1::vertices(&b, None).unwrap().len(), 4);
        assert_eq!(p1::edges(&b, None).unwrap().len(), 4);
        assert_eq!(p1::faces(&b, None).unwrap(), made);
        assert_eq!(b.tolerance(), 1.0e-5);
        let face = made[0];
        let installed = b.face(face).unwrap().surface.as_ref().unwrap();
        assert_eq!(
            (installed.notify_key(), installed.owner()),
            (Some(original_key), Some(key_of(EntityRef::Face(face)))),
            "the original surface must remain the face's surface under the caller's issued owner"
        );
    }

    #[test]
    fn retained_original_trim_records_consumption_and_installation() {
        use crate::curve::line::Line;
        use crate::geom::attribute::AttributeStore;
        use crate::surface::plane::Plane;
        use crate::topology::brep::{p1, p3};
        use crate::topology::edge::NotifyEnv;

        // 0 success; 1 native early null region; 2 native early non-G1;
        // 3 face-builder validation failure after its ownership handoff;
        // 4 a real degenerate curve makes an edgeless face, then repair fails.
        for mode in 0..5 {
            let mut k = Kernel::default();
            let mut b = p3::new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
            let serial = p3::notify_serial(&mut k.context, &mut b).unwrap();
            let key_of = |entity| p3::topology_ref(serial, entity);
            let mut records = crate::topology::topology::NotifyRecords::new();
            p3::seed_records(&mut records, &b, &key_of);
            let mut attributes = AttributeStore::default();
            let mut surface = if mode == 2 {
                let net = (0..3)
                    .map(|i| {
                        (0..2)
                            .map(|j| {
                                CPoint::new(
                                    0.5 * f64::from(i),
                                    f64::from(j),
                                    if i == 1 { 0.25 } else { 0.0 },
                                    1.0,
                                )
                            })
                            .collect()
                    })
                    .collect();
                let nurb = crate::nurbs::srf_basic::Surface::new(
                    net,
                    1,
                    1,
                    vec![0.0, 0.0, 0.5, 1.0, 1.0],
                    vec![0.0, 0.0, 1.0, 1.0],
                )
                .unwrap();
                Surface::from(BSplineSurface::from_nurb(nurb, false))
            } else {
                Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)))
            };
            let original_key = k
                .context
                .geometry_key(surface.notify_identity_slot())
                .unwrap();
            let uv = [
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(0.0, 1.0),
            ];
            let points = uv.map(|p| surface.evaluate_point(&k, p).unwrap());
            let interval = Extent1d::from_min_max(0.0, 1.0);
            let curves = (0..4)
                .map(|i| {
                    Curve::from(
                        Line::create_line_segment(
                            3,
                            points[i],
                            points[(i + 1) % 4],
                            Some(&interval),
                        )
                        .unwrap(),
                    )
                })
                .collect();
            let curves = if mode == 4 {
                let point = CPoint::new(0.0, 0.0, 0.0, NOW);
                let curve = NlCurve::new(vec![point, point], 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
                vec![Curve::from(BSplineCurve::from_nurb(curve, 3, false))]
            } else {
                curves
            };
            let input = TrimCurvesInput {
                loop_counts: if mode == 3 {
                    Vec::new()
                } else if mode == 4 {
                    vec![1]
                } else {
                    vec![4]
                },
                curves,
                orientations: vec![OrientType::Same; 4],
                loop_points: Vec::new(),
                uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                surface_orientation: OrientType::Same,
            };
            let mut transfer = TrimSurfaceTransfer::new(surface);
            let region = if mode == 1 { None } else { b.infinite_region() };
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &key_of,
            };
            let result = trim_surface_with_model_space_curves_transfer(
                &mut k,
                &mut b,
                region,
                1e-5,
                input,
                &mut transfer,
                &mut env,
            );
            match transfer.into_location().unwrap() {
                TrimSurfaceLocation::Installed(face) => {
                    if mode == 4 {
                        assert!(matches!(
                            result,
                            Err(SolidError::Upstream {
                                code: crate::error::SM_ERR,
                                ..
                            })
                        ));
                        assert_eq!(p1::faces(&b, None).unwrap(), vec![face]);
                        assert!(b.face(face).unwrap().loops.is_empty());
                    } else {
                        assert_eq!(mode, 0);
                        assert_eq!(result.unwrap(), vec![face]);
                        assert_eq!(p1::vertices(&b, None).unwrap().len(), 4);
                        assert_eq!(p1::edges(&b, None).unwrap().len(), 4);
                    }
                    let installed = b.face(face).unwrap().surface.as_ref().unwrap();
                    assert_eq!(installed.notify_key(), Some(original_key));
                    assert_eq!(installed.owner(), Some(key_of(EntityRef::Face(face))));
                    assert_eq!(
                        b.geometry_owner_keys.get(&EntityRef::Face(face)),
                        Some(&key_of(EntityRef::Face(face)))
                    );
                }
                TrimSurfaceLocation::Destroyed => {
                    assert!(mode == 1 || mode == 2);
                    let code = if mode == 1 {
                        crate::error::SM_ERR_NULL_POINTER
                    } else {
                        crate::error::SM_ERR
                    };
                    assert!(
                        matches!(result, Err(SolidError::Upstream {code: got, ..}) if got==code)
                    );
                    assert!(p1::faces(&b, None).unwrap().is_empty());
                }
                TrimSurfaceLocation::Owned(surface) => {
                    assert_eq!(mode, 3);
                    assert!(result.is_err());
                    assert_eq!(surface.notify_key(), Some(original_key));
                    assert_eq!(surface.owner(), None);
                }
            }
        }
    }

    fn uv_line(a: [f64; 2], b: [f64; 2]) -> Curve {
        let pw = vec![
            CPoint::new(a[0], a[1], NOZ, NOW),
            CPoint::new(b[0], b[1], NOZ, NOW),
        ];
        let nl = NlCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(nl, 2, false))
    }

    fn line3(a: [f64; 3], b: [f64; 3]) -> Curve {
        let pw = vec![
            CPoint::new(a[0], a[1], a[2], NOW),
            CPoint::new(b[0], b[1], b[2], NOW),
        ];
        let nl = NlCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(nl, 3, false))
    }

    // A closed topological edge with a small geometric trim gap at its vertex.
    fn vertex_gap_loop(gap: f64, vertex_tol: f64, same_vertex: bool) -> (Brep, LoopId) {
        use crate::topology::{
            Edge, Edgeuse, EdgeuseOwner, Loop, Loopuse, LoopuseStart, Vertex, Vertexuse,
            VertexuseId,
        };
        let mut b = Brep::new();
        let v = b
            .add_vertex_record(Vertex {
                point: Vec3::ZERO,
                tolerance: vertex_tol,
                ..Vertex::default()
            })
            .unwrap();
        let other = if same_vertex {
            v
        } else {
            b.add_vertex_record(Vertex {
                point: Vec3::new(gap, 0.0, 0.0),
                tolerance: vertex_tol,
                ..Vertex::default()
            })
            .unwrap()
        };
        let e = b
            .add_edge_record(Edge {
                vertices: [v, other],
                curve: None,
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 5.8e-6,
                edgeuses: vec![EdgeuseId::from_index(0), EdgeuseId::from_index(1)],
            })
            .unwrap();
        let l = b
            .add_loop_record(Loop {
                face: FaceId::from_index(0),
                edges: Vec::new(),
                loopuse: Some(crate::topology::LoopuseId::from_index(0)),
            })
            .unwrap();
        let lu = b
            .add_loopuse_record(Loopuse {
                loop_: Some(l),
                start: LoopuseStart::Edgeuse(Some(EdgeuseId::from_index(0))),
                ..Loopuse::default()
            })
            .unwrap();
        for (i, vertex) in [v, other].into_iter().enumerate() {
            let eu = EdgeuseId::from_index(i as u32);
            b.add_vertexuse_record(Vertexuse {
                vertex: Some(vertex),
                ..Vertexuse::default()
            })
            .unwrap();
            b.add_edgeuse_record(Edgeuse {
                edge: Some(e),
                owner: EdgeuseOwner::Loopuse(Some(lu)),
                ccw: Some(eu),
                cw: Some(eu),
                vertexuse: Some(VertexuseId::from_index(i as u32)),
                orientation: if i == 0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                uv_trim_curve: (i == 0).then(|| uv_line([0.0, 0.0], [gap, 0.0])),
                ..Edgeuse::default()
            })
            .unwrap();
        }
        (b, l)
    }

    fn fixed_kernel() -> Kernel {
        let mut k = Kernel::default();
        k.fixes.check_loop_vertex_tolerance = true;
        k
    }

    #[test]
    fn issue_476_loop_gap_uses_shared_vertex_tolerance() {
        let k = fixed_kernel();
        let surface = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        for (gap, vertex_tol, same_vertex, expected) in [
            (1.3e-5, 2.6e-5, true, 0),
            (2.6e-5, 2.6e-5, true, 0),
            (2.61e-5, 2.6e-5, true, 3),
            (1.3e-5, 1e-6, true, 3),
            (1e-5, 1e-6, true, 0), // Existing edge intersection tolerance remains valid.
            (1.3e-5, 2.6e-5, false, 3), // Unrelated vertices cannot bridge the gap.
        ] {
            for start in 0..2 {
                let (mut b, l) = vertex_gap_loop(gap, vertex_tol, same_vertex);
                b.loopuse_mut(crate::topology::LoopuseId::from_index(0))
                    .unwrap()
                    .start =
                    crate::topology::LoopuseStart::Edgeuse(Some(EdgeuseId::from_index(start)));
                assert_eq!(
                    check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(),
                    expected,
                    "gap={gap}, vertex_tol={vertex_tol}, shared={same_vertex}, start={start}"
                );
            }
        }
    }

    #[test]
    fn issue_476_default_keeps_the_edge_only_gap_limit() {
        // Default (SMLib): a gap the widened shared vertex would cover is closure 3.
        let k = Kernel::default();
        assert!(!k.fixes.check_loop_vertex_tolerance);
        let surface = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        let (mut b, l) = vertex_gap_loop(1.3e-5, 2.6e-5, true);
        assert_eq!(check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(), 3);
        // The same loop with the field on is accepted.
        let (mut b, l) = vertex_gap_loop(1.3e-5, 2.6e-5, true);
        assert_eq!(
            check_loop(&fixed_kernel(), &mut b, &surface, l, 1e-5, None).unwrap(),
            0
        );
    }

    #[test]
    fn issue_476_loop_junction_uses_previous_end_not_previous_start() {
        use crate::topology::{Edge, Edgeuse, VertexId, Vertexuse, VertexuseId};
        let k = fixed_kernel();
        let (mut b, l) = vertex_gap_loop(1.3e-5, 2.6e-5, false);
        // The first edge leaves a tight vertex; its far end joins the second edge
        // at the widened vertex. Looking at the previous start gives the wrong limit.
        b.vertex_mut(VertexId::from_index(0)).unwrap().tolerance = 1e-6;
        b.edgeuse_mut(EdgeuseId::from_index(0))
            .unwrap()
            .uv_trim_curve = Some(uv_line([0.0, 0.0], [0.5, 0.0]));
        let e = b
            .add_edge_record(Edge {
                vertices: [VertexId::from_index(1), VertexId::from_index(0)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 5.8e-6,
                curve: None,
                edgeuses: vec![EdgeuseId::from_index(2), EdgeuseId::from_index(3)],
            })
            .unwrap();
        for (i, vertex) in [(2, 1), (3, 0)] {
            let vu = b
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(VertexId::from_index(vertex)),
                    ..Vertexuse::default()
                })
                .unwrap();
            let mut eu: Edgeuse = b.edgeuse(EdgeuseId::from_index(i - 2)).unwrap().clone();
            eu.edge = Some(e);
            eu.vertexuse = Some(vu);
            eu.uv_trim_curve = (i == 2).then(|| uv_line([0.5 + 1.3e-5, 0.0], [0.0, 0.0]));
            b.add_edgeuse_record(eu).unwrap();
        }
        for (a, c) in [(0, 2), (2, 0), (1, 3), (3, 1)] {
            let eu = b.edgeuse_mut(EdgeuseId::from_index(a)).unwrap();
            eu.ccw = Some(EdgeuseId::from_index(c));
            eu.cw = eu.ccw;
        }
        let surface = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        for start in 0..4 {
            b.loopuse_mut(crate::topology::LoopuseId::from_index(0))
                .unwrap()
                .start = crate::topology::LoopuseStart::Edgeuse(Some(EdgeuseId::from_index(start)));
            assert_eq!(check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(), 0);
        }
        // Missing endpoint topology must fall back to the edges, not another vertex.
        b.vertexuse_mut(VertexuseId::from_index(2)).unwrap().vertex = None;
        assert_eq!(check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(), 3);
    }

    #[test]
    fn issue_530_projected_rotational_sweep_loops_cross_periodic_seams() {
        for (start, end) in [
            ([2.0, 0.0, 0.0], [2.0, 0.0, 3.0]),
            ([2.0, 0.0, 0.0], [1.0, 0.0, 3.0]),
            ([1.0, 0.0, 0.0], [2.0, 0.0, 3.0]),
        ] {
            let mut k = Kernel::default();
            k.fixes.check_loop_projected_seam_ends = true;
            let mut b =
                crate::topology::brep::p3::new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT)
                    .unwrap();
            crate::primitive::primitive_creation::p1::analytic_rotational_sweep(
                &mut k,
                &mut b,
                &line3(start, end),
                Vec3::ZERO,
                Vec3::Z,
            )
            .unwrap();
            let loops: Vec<_> = b.loops().iter().map(|(id, _)| id).collect();
            assert!(!loops.is_empty());
            let mut mixed_seam_off = None;
            for l in loops {
                let eus = loop_edgeuses(&b, l).unwrap();
                assert!(!eus.is_empty());
                assert!(eus
                    .iter()
                    .all(|&eu| eu1::uv_trim_curve(&b, eu).unwrap().0.is_none()));
                let surface = b
                    .face(b.loop_(l).unwrap().face)
                    .unwrap()
                    .surface
                    .clone()
                    .unwrap();
                assert_eq!(
                    check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(),
                    0,
                    "profile {start:?} -> {end:?}"
                );
                // One seam retains its exact curve on the far side, while its
                // neighbours' vertex projections choose the near side.
                for eu in eus {
                    let (mut a, mut z) = edge_uv_ends(&k, &mut b, eu).unwrap();
                    if a.y == z.y {
                        continue;
                    }
                    a.x = surface.natural_uv_domain_if_built().max().x;
                    z.x = a.x;
                    if eu1::orientation(&b, eu).unwrap() == OrientType::Opposite {
                        core::mem::swap(&mut a, &mut z);
                    }
                    b.edgeuse_mut(eu).unwrap().uv_trim_curve =
                        Some(uv_line([a.x, a.y], [z.x, z.y]));
                    assert_eq!(
                        check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(),
                        0,
                        "mixed seam {eu:?}, profile {start:?} -> {end:?}"
                    );
                    // Off (the default), the raw one-period jump is upstream's.
                    if mixed_seam_off.is_none() {
                        let mut off = k.fixes;
                        off.check_loop_projected_seam_ends = false;
                        let on = core::mem::replace(&mut k.fixes, off);
                        mixed_seam_off = Some(check_loop(&k, &mut b, &surface, l, 1e-5, None));
                        k.fixes = on;
                    }
                    // A short physical gap must still fail after lifting the U
                    // representative (including at a clamped surface boundary).
                    a.y += 0.001;
                    z.y += 0.001;
                    b.edgeuse_mut(eu).unwrap().uv_trim_curve =
                        Some(uv_line([a.x, a.y], [z.x, z.y]));
                    let mut reports = AssertArray::new();
                    assert_eq!(
                        check_loop(&k, &mut b, &surface, l, 1e-5, Some(&mut reports)).unwrap(),
                        3
                    );
                    assert!(!reports.is_empty());
                    b.edgeuse_mut(eu).unwrap().uv_trim_curve = None;
                }
                // An exact UV polygon with one deliberately misplaced seam must
                // retain its failure, even though both seam sides map to the same
                // points in 3D. No endpoint here comes from projection.
                let eus = loop_edgeuses(&b, l).unwrap();
                let d = surface.natural_uv_domain_if_built();
                let (u0, u1, v0, v1) = (d.min().x, d.max().x, d.min().y, d.max().y);
                let corners = [
                    ([u0, v1], [u0, v0]),
                    ([u0, v0], [u1, v0]),
                    ([u1, v0], [u1, v1]),
                    ([u1, v1], [u0, v1]),
                ];
                assert_eq!(eus.len(), corners.len());
                for (&eu, (mut a, mut z)) in eus.iter().zip(corners) {
                    if eu1::orientation(&b, eu).unwrap() == OrientType::Opposite {
                        core::mem::swap(&mut a, &mut z);
                    }
                    b.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(uv_line(a, z));
                }
                assert_eq!(check_loop(&k, &mut b, &surface, l, 1e-5, None).unwrap(), 0);
                let eu = eus[0];
                b.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(uv_line([u1, v1], [u1, v0]));
                assert!(check_loop(&k, &mut b, &surface, l, 1e-5, None).is_err());
            }
            // With the fix off the cylinder's mixed seam is upstream's SM_ERR (1001).
            let off = mixed_seam_off.expect("a mixed seam was checked");
            if start[0] == end[0] {
                assert!(off.is_err(), "profile {start:?} -> {end:?}: {off:?}");
            }
        }
    }

    // Both opt-in CheckLoop fixes on one junction: a seam edge use keeps its exact UV
    // trim on the far sheet (WR-440 lifts its projected neighbours) and is displaced in
    // V by a 3D gap that only the widened shared vertices cover (WR-381). Each field
    // alone leaves the loop failing; both together close it.
    #[test]
    fn issue_476_and_530_fixes_combine_on_a_mixed_seam_gap() {
        use crate::topology::VertexId;
        let mut k = Kernel::default();
        let mut b = crate::topology::brep::p3::new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT)
            .unwrap();
        crate::primitive::primitive_creation::p1::analytic_rotational_sweep(
            &mut k,
            &mut b,
            &line3([2.0, 0.0, 0.0], [2.0, 0.0, 3.0]),
            Vec3::ZERO,
            Vec3::Z,
        )
        .unwrap();
        for i in 0..b.vertices().len() {
            b.vertex_mut(VertexId::from_index(i as u32))
                .unwrap()
                .tolerance = 0.01;
        }
        let loops: Vec<_> = b.loops().iter().map(|(id, _)| id).collect();
        let mut checked = 0;
        for l in loops {
            let surface = b
                .face(b.loop_(l).unwrap().face)
                .unwrap()
                .surface
                .clone()
                .unwrap();
            let Some(eu) = loop_edgeuses(&b, l).unwrap().into_iter().find(|&eu| {
                let (a, z) = edge_uv_ends(&k, &mut b, eu).unwrap();
                a.y != z.y
            }) else {
                continue;
            };
            let (mut a, mut z) = edge_uv_ends(&k, &mut b, eu).unwrap();
            a.x = surface.natural_uv_domain_if_built().max().x;
            z.x = a.x;
            if eu1::orientation(&b, eu).unwrap() == OrientType::Opposite {
                core::mem::swap(&mut a, &mut z);
            }
            a.y += 0.001;
            z.y += 0.001;
            b.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(uv_line([a.x, a.y], [z.x, z.y]));
            let mut run = |vertex: bool, seam: bool| {
                k.fixes.check_loop_vertex_tolerance = vertex;
                k.fixes.check_loop_projected_seam_ends = seam;
                check_loop(&k, &mut b, &surface, l, 1e-5, None)
            };
            assert_eq!(run(true, true).unwrap(), 0, "both fixes");
            assert_eq!(run(false, true).unwrap(), 3, "seam fix only: 3D gap");
            assert!(run(true, false).is_err(), "vertex fix only: raw seam jump");
            assert_ne!(run(false, false).ok(), Some(0), "SMLib");
            checked += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn min_and_type() {
        assert_eq!(min(1.0, 2.0), 1.0);
        assert_eq!(min(2.0, 1.0), 1.0);
        assert_eq!(class_type(), 12225);
    }

    #[test]
    fn iso_curves() {
        let k = &Kernel::default();
        let c = uv_line([0.25, 0.0], [0.25, 1.0]);
        assert_eq!(is_iso_curve(k, &c).unwrap(), Some((SurfParamType::U, 0.25)));
        assert!(is_iso_curve_at(&c, SurfParamType::U, 0.25, 1e-6).unwrap());
        assert!(!is_iso_curve_at(&c, SurfParamType::V, 0.25, 1e-6).unwrap());
        let d = uv_line([0.0, 0.0], [1.0, 1.0]);
        assert_eq!(is_iso_curve(k, &d).unwrap(), None);
    }

    #[test]
    fn mirror_reflects_coordinate() {
        let k = Kernel::default();
        let mut c = uv_line([0.0, 0.1], [0.0, 0.9]);
        mirror_curve(&k, &mut c, SurfParamType::U, 0.0, 1.0, 0.0).unwrap();
        let e = c.ends(&k).unwrap();
        assert!((e.start.x - 1.0).abs() < 1e-15);
        assert!((e.end.y - 0.9).abs() < 1e-15);
    }

    #[test]
    fn square_orders_into_one_loop() {
        let k = Kernel::default();
        let cs = vec![
            line3([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            line3([1.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
            line3([1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
            line3([0.0, 1.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        let r = order_curves_into_loops(&k, &cs, 1e-6).unwrap();
        assert!(r.status().is_ok());
        assert_eq!(r.loop_counts, vec![4]);
        assert_eq!(r.ordered, vec![0, 1, 2, 3]);
        assert_eq!(
            r.orients,
            vec![
                OrientType::Same,
                OrientType::Opposite,
                OrientType::Same,
                OrientType::Same
            ]
        );
        assert!((r.plane_normal.z.abs() - 1.0).abs() < 1e-12);

        let mut ordered = r.ordered.clone();
        let mut counts = r.loop_counts.clone();
        let mut orients = r.orients.clone();
        let mut n = r.plane_normal;
        let outer = find_outer_loops(
            &k,
            &cs,
            &mut ordered,
            &mut counts,
            &mut orients,
            &mut n,
            1e-6,
        )
        .unwrap();
        assert_eq!(outer, vec![1]);
        assert_eq!(counts, vec![4]);
    }

    #[test]
    fn open_curves_are_reported() {
        let k = Kernel::default();
        let cs = vec![line3([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])];
        let r = order_curves_into_loops(&k, &cs, 1e-6);
        // a single open line: its loop normal does not unitize
        assert!(r.is_err() || r.unwrap().status().is_err());
    }
}

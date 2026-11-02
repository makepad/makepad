//! Calls from the `AssertValid` walk into checks other work packages port.
//!
//! Upstream: `source/SMLib/src/SmEdgeuse.cpp`, `SmBSplineCurve.cpp`, `SmLoopuse.cpp`,
//! `SmVertex.cpp`, `SmShell.cpp`, `SmFace.cpp`, `SmEdge.cpp`, `SmSurface.cpp` (and the
//! surface classes), `SmOffsetCurve.cpp`, `SmProjectedCurve.cpp`, `SmHermiteCurve.cpp`,
//! `SmCrvOnSurf.cpp`, `SmCrvInVolume.cpp`.
//!
//! | Rust (private seam)          | Upstream                                  |
//! |------------------------------|-------------------------------------------|
//! | `has_dog_leg_near_pole`      | `SmEdgeuse::HasDogLegNearPole`            |
//! | `assert_no_periodic_jump`    | `SmBSplineCurve::AssertNoPeriodicJump`    |
//! | `is_loopuse_side_in_surface` | `SmLoopuse::IsLoopuseSideInSurface`       |
//! | `vertex_check_pointers`      | `SmVertex::CheckPointers`                 |
//! | `is_inner_shell_of`          | `SmShell::IsInnerShellOf`                 |
//! | `assert_test_face37`         | `sm_AssertTestFace37`                     |
//! | `surface_assert_valid`       | `SmSurface::AssertValid` (virtual; wired) |
//! | `offset_curve_assert_valid`  | `SmOffsetCurve::AssertValid`              |
//! | `projected_curve_assert_valid` | `SmProjectedCurve::AssertValid`         |
//! | `hermite_curve_assert_valid` | `SmHermiteCurve::AssertValid`             |
//! | `crv_on_surf_assert_valid`   | `SmCrvOnSurf::AssertValid`                |
//! | `crv_in_volume_assert_valid` | `SmCrvInVolume::AssertValid`              |
//! | `assert_test_face32`         | `sm_AssertTestFace32`                     |
//! | `assert_test_face33`         | `sm_AssertTestFace33`                     |
//! | `assert_test_face38`         | `sm_AssertTestFace38`                     |
//! | `assert_test_edge13`         | `sm_AssertTestEdge13`                     |
//! | `assert_test_vertex1`        | `sm_AssertTestVertex1`                    |
//!
//! Deviations: one function per upstream callee, so each package replaces exactly one
//! body. Until a callee is ported its seam returns [`SolidError::Unported`]; a
//! difftest case reaching it is deferred, never passed. Upstream evaluates the level-2
//! seams (Face 32/33/38, Edge 13) only at `SM_LEVEL_2`; `sm_AssertTestVertex1` runs at
//! every level (only its report is level 2).
//!
//! Signatures are the contract with the packages porting the callees: keep them, and
//! replace a body with the call into the ported function.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::assert_array::{AssertArray, ObjectKey};
use crate::geom::extent2d::Extent2d;
use crate::kernel::Kernel;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, LoopuseId, ShellId, VertexId};

fn unported(upstream: &'static str, unit: &'static str) -> SolidError {
    SolidError::Unported { upstream, unit }
}

// ---- level-0 helpers (WP2, wired: #1472) ------------------------------------------------------------

/// `SmEdgeuse::HasDogLegNearPole()` of `eu`: `true` when the UV trim's control polygon
/// turns to follow a singular surface boundary (Edgeuse test 20 fails on `true`).
pub(super) fn has_dog_leg_near_pole(k: &mut Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<bool> {
    crate::topology::edgeuse::p2::has_dog_leg_near_pole(k, b, eu).map(|d| d.has_dog_leg)
}

/// `m_pUVTrimCurve->AssertNoPeriodicJump(bClosedU, bClosedV, sUVDomain)`: `true` when
/// the UV trim `curve` of an edge use does not jump across a periodic boundary of the
/// surface domain `domain` (closed in U / V as given). Edgeuse test 7.
pub(super) fn assert_no_periodic_jump(
    _k: &Kernel,
    curve: &Curve,
    closed_u: bool,
    closed_v: bool,
    domain: &Extent2d,
) -> Result<bool> {
    crate::nurbs::b_spline_curve::p3::assert_no_periodic_jump_of(curve, closed_u, closed_v, domain)
}

/// `SmLoopuse::IsLoopuseSideInSurface()` of the loop's primary loop use `lu`. Loop
/// test 2.
pub(super) fn is_loopuse_side_in_surface(
    k: &mut Kernel,
    b: &mut Brep,
    lu: LoopuseId,
) -> Result<bool> {
    crate::topology::loopuse::is_loopuse_side_in_surface(k, b, lu, None, None).map(|r| r.in_surface)
}

/// `SmVertex::CheckPointers()` of `v`. Vertex test 2.
pub(super) fn vertex_check_pointers(_k: &mut Kernel, b: &mut Brep, v: VertexId) -> Result<bool> {
    crate::topology::vertex::check_pointers(b, v)
}

/// `pInnerShell->IsInnerShellOf(pOuterShell)` (`== TRUE`). Brep test 12, evaluated only
/// for the inner shells of a region other than the infinite one.
pub(super) fn is_inner_shell_of(
    k: &mut Kernel,
    b: &mut Brep,
    inner: ShellId,
    outer: ShellId,
) -> Result<bool> {
    use crate::topology::shell;
    shell::is_inner_shell_of(k, b, inner, Some(outer)).map(|r| r == shell::SM_TRUE)
}

/// `sm_AssertTestFace37(this)` (`== TRUE`): the face's inner loops nest inside the
/// outer loop and outside each other (with `sm_GetLoopSampleUVPoint`). Face test 37.
pub(super) fn assert_test_face37(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    crate::topology::face::p4::assert_test_face37(k, b, f)
}

// ---- geometry checks (WP3a / WP3b) ---------------------------------------------------

/// `m_pSurface->AssertValid(pAList, eTestLevel, SM_NO_WALK)` for the surface of face
/// `f` (virtual: the surface's class check, with its base classes), reporting into
/// `list` about the surface object `key`. `level` is `SM_LEVEL_0`..`SM_LEVEL_2` (the
/// call is skipped at `SM_LEVEL_GIVEN`). Called only when the face has a surface.
pub(super) fn surface_assert_valid(
    k: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    list: Option<&mut AssertArray>,
    level: u8,
    key: ObjectKey,
) -> Result<bool> {
    let surface = b.face(f)?.surface.as_ref().ok_or(SolidError::null_pointer(
        "topology::brep::validation::assert_valid",
    ))?;
    surface.assert_valid(k, list, level, key)
}

/// `SmOffsetCurve::AssertValid` of `curve` (an offset curve), reporting about `key`.
pub(super) fn offset_curve_assert_valid(
    _k: &Kernel,
    _curve: &mut Curve,
    _list: Option<&mut AssertArray>,
    _level: u8,
    _key: ObjectKey,
) -> Result<bool> {
    // UNPORTED(curve.offset_curve): SmOffsetCurve::AssertValid
    Err(unported("SmOffsetCurve::AssertValid", "curve.offset_curve"))
}

/// `SmProjectedCurve::AssertValid` of `curve` (a projected curve), reporting about `key`.
pub(super) fn projected_curve_assert_valid(
    _k: &Kernel,
    _curve: &mut Curve,
    _list: Option<&mut AssertArray>,
    _level: u8,
    _key: ObjectKey,
) -> Result<bool> {
    // UNPORTED(curve.projected_curve): SmProjectedCurve::AssertValid
    Err(unported(
        "SmProjectedCurve::AssertValid",
        "curve.projected_curve",
    ))
}

/// `SmHermiteCurve::AssertValid` of `curve` (a Hermite curve), reporting about `key`.
pub(super) fn hermite_curve_assert_valid(
    _k: &Kernel,
    _curve: &mut Curve,
    _list: Option<&mut AssertArray>,
    _level: u8,
    _key: ObjectKey,
) -> Result<bool> {
    // UNPORTED(curve.hermite_curve): SmHermiteCurve::AssertValid
    Err(unported(
        "SmHermiteCurve::AssertValid",
        "curve.hermite_curve",
    ))
}

/// `SmCrvOnSurf::AssertValid` of `curve` (a curve on surface or a tangent field, which
/// inherits it), reporting about `key`.
pub(super) fn crv_on_surf_assert_valid(
    _k: &Kernel,
    _curve: &mut Curve,
    _list: Option<&mut AssertArray>,
    _level: u8,
    _key: ObjectKey,
) -> Result<bool> {
    // UNPORTED(curve.crv_on_surf): SmCrvOnSurf::AssertValid
    Err(unported("SmCrvOnSurf::AssertValid", "curve.crv_on_surf"))
}

/// `SmCrvInVolume::AssertValid` of `curve` (a curve in volume), reporting about `key`.
pub(super) fn crv_in_volume_assert_valid(
    _k: &Kernel,
    _curve: &mut Curve,
    _list: Option<&mut AssertArray>,
    _level: u8,
    _key: ObjectKey,
) -> Result<bool> {
    // UNPORTED(curve.crv_in_volume): SmCrvInVolume::AssertValid
    Err(unported(
        "SmCrvInVolume::AssertValid",
        "curve.crv_in_volume",
    ))
}

// ---- level-2 tests (WP4) --------------------------------------------------------------

/// `sm_AssertTestFace32(this)` (`== TRUE`): no coincident vertices
/// (`HasCoincidentVertices`). Face test 32, evaluated only at `SM_LEVEL_2` and above.
pub(super) fn assert_test_face32(_k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    crate::topology::face::assert_tests::assert_test_face32(b, f)
}

/// `sm_AssertTestFace33(this)` (`== TRUE`): no coincident edges (`HasCoincidentEdges`).
/// Face test 33, evaluated only at `SM_LEVEL_2` and above.
pub(super) fn assert_test_face33(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    crate::topology::face::assert_tests::assert_test_face33(k, b, f)
}

/// `sm_AssertTestFace38(this)` (`== TRUE`): the face loops are disjoint in UV
/// (`sm_DoLoopsIntersectInUV`). Face test 38, evaluated only at `SM_LEVEL_2` and above
/// and only for a face with a surface.
pub(super) fn assert_test_face38(k: &mut Kernel, b: &mut Brep, f: FaceId) -> Result<bool> {
    crate::topology::face::assert_tests::assert_test_face38(k, b, f)
}

/// `sm_AssertTestEdge13(this, &dMaxGap)`: `(passed, max_gap)`, the edge curve within
/// `SmTol::GetXSectTol3d(this, this)` of every face surface (`IsOnFaceEdgeForm`).
/// Edge test 13, evaluated only at `SM_LEVEL_2` and above; `max_gap` is the report
/// value (upstream initialises it to `0.0`).
pub(super) fn assert_test_edge13(k: &mut Kernel, b: &mut Brep, e: EdgeId) -> Result<(bool, f64)> {
    crate::topology::edge::edge_form::assert_test_edge13(k, b, e)
}

/// `sm_AssertTestVertex1(this, &dMaxGap3d, &pMaxGapFace)`: `(passed, max_gap_3d)`, the
/// vertex within the cross-section tolerance of its faces and on their corner
/// (`IsWithinXSectTol3dOfFaces`, `IsOnFaceCorner`). Called at every level; its report
/// (Vertex test 1) is level 2.
pub(super) fn assert_test_vertex1(
    k: &mut Kernel,
    b: &mut Brep,
    v: VertexId,
) -> Result<(bool, f64)> {
    let (passed, max_gap_3d, _) = crate::topology::vertex::corner::assert_test_vertex1(k, b, v)?;
    Ok((passed, max_gap_3d))
}

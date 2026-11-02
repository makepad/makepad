//! `SmSurface`, part 1: the STEP surface wrapper [`StepSurface`] (`SmSTEPSurface`), the
//! surface caches (`CacheMakeOrValidate`, the base `CalculateBoundingBox`,
//! `GetDomainWithContinuity`), the owner and face queries, the evaluation wrappers
//! (`Evaluate1stDerivatives`, `Evaluate2ndDerivatives`, `EvaluateSTEP`, `EvaluateSimple`,
//! directional derivatives), the degeneracy, periodicity and boundary tests, the
//! constructors of the file-local solver and evaluator classes of `SmSurface.cpp`, and a
//! few standalone helpers (`smsurf_EvaluateNormalSection`, `sm_CrossedSeamOrPole`, the
//! tree node validity tests).
//!
//! Deviations: compound evaluation copies carry a depth counter checked by Surface against
//! [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING). The caches
//! hold an exact copy of their surface (`clone_for_evaluation`) where upstream holds a
//! pointer to it: `Clone` is the copy constructor, whose second knot fixup changed a
//! knot vector the first fixup had left non-monotone (PR #147 round 4). Unlike `Clone`
//! the copy keeps the surface's owner, so the cache sees the owning face as upstream's
//! pointer does.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`,
//! `source/SMLib/src/SmSTEPSurface.cpp`, `source/SMLib/inc/SmSTEPSurface.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`StepSurface`]                                   | `SmSTEPSurface` (constructors, destructor, `GetNaturalUVDomain`, `operator==`, `EvaluatePoint`, `Evaluate`, `GetDomainWithContinuity`, `GetKnots`, `ReadFromDB`, `IsKindOf`, `SetContext`, `GetType`, `GetTypeString`, `GetClassString`) |
//! | [`Surface::new`] (part 2), `Clone`                | `SmSurface::SmSurface` (both)                    |
//! | [`Surface::approx_eq`], [`Surface::base_eq`]      | `operator==` (virtual), `SmSurface::operator==`  |
//! | [`Surface::set_owner`], [`Surface::face`]         | `SetOwner`, `GetFace`                            |
//! | [`Surface::calculate_bounding_box`], [`SurfaceBoxes`] | `CalculateBoundingBox` (base body)           |
//! | [`Surface::approximate_size`]                     | `ApproximateSize`                                |
//! | [`validate_surface_cache`], [`make_surface_cache`], [`make_trim_srf_cache`], [`cache_tolerances`] | `CacheMakeOrValidate` |
//! | [`make_trim_srf_cache_with_face`], [`get_surface_cache_with_face`] | `CacheMakeOrValidate(SM_OC_TRIMSRF)`, `smsurf_GetSurfaceCache` of a face's surface |
//! | [`cache_tree_built_on_get`], [`continuity_surface_cache`], [`note_surface_cache`] | the tree state of the cache `smsurf_GetSurfaceCache` hands out (`m_pCacheObj` reused or made) |
//! | [`Surface::compute_param_direction`]              | `ComputeParamDirection`                          |
//! | [`Surface::evaluate_directional_derivs`]          | `EvaluateDirectionalDerivs`                      |
//! | [`Surface::create_offset_surface`]                | `CreateOffsetSurface`                            |
//! | [`Surface::create_silhouette_curves`]             | `CreateSilhouetteCurves`                         |
//! | [`Surface::evaluate_1st_derivatives`], [`Surface::evaluate_2nd_derivatives`] | `Evaluate1stDerivatives`, `Evaluate2ndDerivatives` |
//! | [`Surface::evaluate_step`], [`Surface::evaluate_step_point`], [`Surface::evaluate_simple`] | `EvaluateSTEP`, `EvaluateSTEPPoint`, `EvaluateSimple` |
//! | [`Surface::domain_with_continuity`], [`Surface::domain_with_continuity_from_cache`] | `GetDomainWithContinuity`                        |
//! | [`Surface::convert_domain_from_step_to_nurbs`], [`Surface::convert_domain_from_nurbs_to_step`] | `ConvertDomainFromSTEPToNURBS`, `ConvertDomainFromNURBSToSTEP` |
//! | [`Surface::is_periodic`]                          | `IsPeriodic`                                     |
//! | [`Surface::is_degenerate_point`], [`base_is_degenerate_point`] | `IsDegeneratePoint`                 |
//! | [`Surface::is_degenerate_curve`], [`base_is_degenerate_curve`] | `IsDegenerateCurve`                 |
//! | [`Surface::is_flat_corner`], [`Surface::is_on_boundary`], [`Surface::is_in_domain`], [`Surface::is_analytic`] | `IsFlatCorner`, `IsOnBoundary`, `IsInDomain`, `IsAnalytic` |
//! | [`find_uv_curve_ivl_in_domain`]                   | `FindUVCurveIvlInDomain` (static)                |
//! | [`Surface::snap_to_knots`]                        | `SnapToKnots`                                    |
//! | [`evaluate_normal_section`]                       | `smsurf_EvaluateNormalSection`                   |
//! | [`crossed_seam_or_pole`]                          | `sm_CrossedSeamOrPole` (static)                  |
//! | [`surface_to_evaluate`], [`is_point_valid`], [`is_node_valid`] | `sm_GetSurfaceToEvaluate`, `sm_IsPointValid`, `sm_IsNodeValid` (static) |
//! | [`zone_tol_3d`], [`map_to_2d_dir`], [`map_to_2d`], [`scaled_zero_of_surface`] | `SmTol::GetZoneTol3d(const SmObject*)` for surfaces, `SmTol::MapTo2d` (surface overloads), `SmTol::GetScaledZero(const SmSurface&)` (co-ports) |
//! | [`map_to_2d_on_curve`]                            | `SmTol::MapTo2d(const SmTol3d&, double, const SmCurve&, const SmSurface&)` (co-port, first ported privately by `intersect.curve_class_p1`) |
//! | [`SurfaceEval::fails_eval`], [`SurfaceEval::pos`], ... | `SmSurfaceEval::FailsEval`, `Pos`, `Su`, `Sv`, `Suu`, `Suv`, `Svv` |
//! | [`DropCurveFail::copy_of`]                        | `SmDropCurveFail` copy constructor               |
//! | [`IsoCurveLengthIntegrand::evaluate`], [`AreaIntegrand`] | `SmIsoCurveLengthIFEO::Evaluate`, `SmAreaIFEO::Evaluate` |
//! | [`FindSingularParamWithGivenTangentDir`]          | `SmFindSingularParamWithGivenTangentDirEFO` (constructor, `Evaluate`) |
//! | [`evaluate_1st_derivatives`], [`evaluate_2nd_derivatives`], [`SecondDerivatives`], [`evaluate_directional_derivs`] | `Evaluate1stDerivatives`, `Evaluate2ndDerivatives`, `EvaluateDirectionalDerivs` for code generic over [`SurfaceEvaluate`] (first co-ported by `surface.surface_p7`) |
//! | [`CsiGlobalSolver`], [`CsGlobalSolver`], [`LsiGlobalSolver::new`], [`SpGlobalSolver::new`], [`SsGlobalSolver::new`] | the constructors of `SmCSIGlobalSolver`, `SmCSGlobalSolver` (and its `BranchMayContainAnswers`), `SmLSIGlobalSolver`, `SmSPGlobalSolver`, `SmSSGlobalSolver` |
//! | [`CsIntersectEnfo`], [`CsNormalizeEnfo`], [`FindPsNormalEnfo`], [`SspIntersectEnfo`], [`SsiNormalizeEnfo`], [`SsNormalizeEnfo`], [`SssIntersectEnfo`] | the constructors of `SmCSIntersectENFO`, `SmCSNormalizeENFO`, `SmFindPSNormalENFO`, `SmSSPIntersectENFO`, `SmSSINormalizeENFO`, `SmSSNormalizeENFO`, `SmSSSIntersectENFO` |
//!
//! Deviations:
//! - **Nesting.** `StepSurface::new` returns `InvalidInput` when the base surface would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - **STEP surfaces.** `m_pSurface` is an owned copy ([`Box<Surface>`]) whether or not
//!   upstream owns it (`m_bOwnsSurface` is kept as a flag); `None` is the empty
//!   constructor's null pointer, where upstream would crash the methods return
//!   [`SolidError::InvalidInput`] (or an uninitialised domain). `GetMemoryUsed` and
//!   `WriteToDB` have no counterpart. Nested STEP surfaces read from a database are
//!   bounded by the reader's nesting limit. Temporary evaluation copies retain
//!   exact nested geometry and share `Surface`'s `MAX_GEOMETRY_NESTING` owned-geometry bound.
//! - **Caches.** Upstream keeps a surface's cache on the surface and reuses it until it
//!   changes. [`make_surface_cache`] and [`make_trim_srf_cache`] build a fresh cache for
//!   the caller (the build is deterministic); [`validate_surface_cache`] is the rest of
//!   `CacheMakeOrValidate` (the cache type check and the reuse of an existing cache,
//!   whose face state the caller passes in). The members that query a cache
//!   (`CalculateBoundingBox`, `GetDomainWithContinuity`, `CreateSilhouetteCurves`)
//!   build one as `smsurf_GetSurfaceCache` would: the trimmed surface cache. Whether
//!   upstream's cache of that surface object has its tree (`GetTree` builds it lazily;
//!   one made during a Boolean operation has none, and its continuity arrays stay empty)
//!   is the surface's cache slot ([`Surface::cache_tree_state`]): a new cache honours it
//!   (no tree for an `Unbuilt` slot, the continuities alone for a `Built` slot during a
//!   Boolean operation) and records it, and the readers of the continuity arrays that
//!   call no `GetTree` (`GetDomainWithContinuity`, `LiftCurve`) take their cache from
//!   [`continuity_surface_cache`]. The rest of the cache (its UV domain, face
//!   containment and modified-face state) is rebuilt as before. The debug cache
//!   counters are not modelled. The owner of a face's surface is an
//!   [`ObjectRef`] the surface cannot follow to the B-rep: a caller holding the face
//!   builds the cache with [`make_trim_srf_cache_with_face`] /
//!   [`get_surface_cache_with_face`], giving `GetFace()` as a
//!   [`CacheFace`]; without it a face-owned surface's trimmed cache stops at
//!   `GetFace` ([`SolidError::Unported`]).
//! - **Silhouettes.** `CreateSilhouetteCurves` always computes both the 3D and the
//!   UV curves (upstream's output arrays are optional) and returns them. It does not
//!   copy the surface's `SM_AI_COLOR` attribute onto the traced curves: a free
//!   [`Surface`] or [`Curve`] has no attribute list, so a caller holding the
//!   surface's attributes adds it. A traced curve or UV curve that is not a B-spline
//!   (upstream's `SM_CAST_PTR` null, then dereferenced) is `SM_ERR_NULL_POINTER`.
//! - **Owners.** `m_pOwner` is an [`ObjectRef`]. `SetOwner`'s `SM_NO_CHANGE_OWNER`
//!   notification is the object layer's (see part 2's `notify`). `GetFace` follows
//!   owner chains through surfaces upstream; the owning surfaces (`SmOffsetSurface`,
//!   `SmSrfInVolume`) hold their base by value and do not become its owner, so a
//!   surface owner returns `InvalidInput` (an offset passes its face down to its
//!   base's box instead).
//! - **Evaluation failures.** `IsDegeneratePoint`, `IsDegenerateCurve` and
//!   `IsFlatCorner` ignore `Evaluate`'s status upstream and read stale or undefined
//!   values on failure; the port returns the error. `IsPeriodic` ignores it as upstream
//!   ([`Surface::evaluate_ignoring_status`]: a B-spline keeps the derivatives it wrote
//!   before its zero-tangent repair failed, and curve bounded and STEP surfaces pass
//!   the array on to their basis; an offset's failure after a partial write keeps the
//!   previous values) and keeps upstream's `SER` inside a boolean function: a failing
//!   `Unitize` returns `TRUE`.
//! - **Unported callees.** `ApproximateSize` of kinds other than B-splines needs
//!   `SmIsoCurve` over any surface (the ported iso curve holds a B-spline), the zone
//!   tolerance of a surface with a face is the one its B-rep cached on it
//!   ([`Surface::owner_zone_tol_3d`]) and needs `SmFace::GetTolerance`
//!   (`topology.face_p2`) for a bare face handle without it: those calls return
//!   [`SolidError::Unported`]. Every tree build
//!   needs `SmSurface::CalculateContinuities` (`surface.surface_p6`), whose iso curves
//!   exist for the B-spline, plane and revolution kinds.
//!   [`map_to_2d_on_curve`] ports the UV-curve and the general 3D branch of
//!   `SmTol::MapTo2d(.., SmCurve, SmSurface)`; the same-surface `SmCrvOnSurf` branch
//!   (`SmTol.cpp:1118-1123`) is `Unported`.
//! - **Solver and evaluator classes.** Their classes belong to the units of their
//!   `Evaluate` / `LocalSolve` members; the structs and constructors live here. `m_pSurf`
//!   (the owner object of a surface tree upstream) is passed in by the caller.
//! - **`SmDropCurveFail` copy constructor.** [`DropCurveFail::copy_of`]; the derived
//!   `Clone` also copies the UV curves of a record without context.

use crate::curve::curve::p2::CurvePropertyType;
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::iso_curve::IsoCurve;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::a_object::{CacheId, ObjectCacheType};
use crate::geom::assert_array::labels::FACE_TYPE;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{directional_derivs, vec_lin_comb_two_vectors};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{Branch, GlobalSolver, SolverOperation};
use crate::geom::local_solve1d::{EvalFunction, FunctionValue};
use crate::geom::object::ObjectRef;
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{
    is_zero_to_tol, scaled_zero2, tol3d, zone_tol_3d as kernel_zone_tol_3d, ScaledZero,
    SmallTopoType, ANG_TOL_RAD,
};
use crate::geom::tree::{AuxDataType, NodeClass, NodeData, Tree, TreeNode, TreeOwner};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_silhouette::SurfaceSilhouette;
use crate::intersect::surface_tracer::SurfaceTrace;
use crate::kernel::Kernel;

#[cfg(test)]
use crate::math::EFF_ZERO_DEG;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::offset_surface::{OffsetSurface, SS_UNKNOWN};
use crate::surface::plane::Plane;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::p1::{SurfaceCache, SurfaceCacheOptions};
use crate::surface::trim_srf_cache::{CacheFace, TrimSrfCache};
use crate::topology::database_io::{DatabaseReader, FileType};

use super::p2::{
    check_geometry_nesting, cs_are_perpen_disjoint, evaluate_geometric, intersect_curve_and_split,
    original_surface, DropCurveFail, IsoCurveLengthIntegrand, LsiGlobalSolver, NestedGeometry,
    SurfaceEval, SurfaceEvaluate, STEP_SURFACE_TYPE, SURF_OF_EXTRUSION_TYPE,
    SURF_OF_REVOLUTION_TYPE,
};
use super::p7::{SpGlobalSolver, SsGlobalSolver};
use super::{CacheTreeState, Surface, SurfaceKind};

const OP_STEP: &str = "surface::surface::p1::StepSurface";
const OP_CACHE: &str = "surface::surface::p1::validate_surface_cache";
const OP_IVL: &str = "surface::surface::p1::find_uv_curve_ivl_in_domain";
const OP_BOX: &str = "surface::surface::p1::Surface::calculate_bounding_box";

/// Why a polar box of a curve-bounded or STEP surface is refused.
pub(crate) const POLAR_RECURSION: &str = "SmSurface::CalculateBoundingBox of a polar box on a curve-bounded or STEP surface recurses without bound upstream (plan/upstream-issues.md UB-84)";

/// The default `d3DTolerance` of `IsDegeneratePoint` and `dOpt3DTol` of
/// `IsDegenerateCurve` (`SM_ZONE_TOL_3D / 10.0`).
pub const DEGENERATE_TOL_3D: f64 = crate::geom::tol::ZONE_TOL_3D / 10.0;

/// The upstream `bUFromLeft` / `bVFromLeft` flag as a span side (`TRUE` is
/// [`Side::Left`]).
const fn side(from_left: bool) -> Side {
    if from_left {
        Side::Left
    } else {
        Side::Right
    }
}

/// `Evaluate` flags: both sides from the left, the upper half only, and the given
/// zero-tangent handling (upstream's `(uv, h, h, TRUE, TRUE, TRUE, .., bNonZero)`).
const fn upper_flags(non_zero_tangents: bool) -> EvaluateFlags {
    EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents,
    }
}

/// `D[i][j]` of an evaluation matrix (zero when the evaluation did not fill it).
fn at(m: &[Vec<Vec3>], i: usize, j: usize) -> Vec3 {
    m.get(i)
        .and_then(|r| r.get(j))
        .copied()
        .unwrap_or(Vec3::ZERO)
}

/// `B[i][j]` of a first-derivative block (zero outside it).
fn block_at(b: &[[Vec3; 2]; 2], i: usize, j: usize) -> Vec3 {
    b.get(i)
        .and_then(|r| r.get(j))
        .copied()
        .unwrap_or(Vec3::ZERO)
}

/// The first-derivative block `[[D, Dv], [Du, Duv]]` of an evaluation matrix,
/// indexed `[i][j]` like upstream's `SmVector3d[2][2]`.
fn first_block(m: &[Vec<Vec3>]) -> [[Vec3; 2]; 2] {
    [[at(m, 0, 0), at(m, 0, 1)], [at(m, 1, 0), at(m, 1, 1)]]
}

// ---- evaluation wrappers (generic over SurfaceEvaluate) -----------------------------------

/// Entry `[i][j]` of an evaluation matrix (`i` times in `u`, `j` times in `v`).
fn at_checked(m: &[Vec<Vec3>], i: usize, j: usize) -> Result<Vec3> {
    m.get(i).and_then(|r| r.get(j)).copied().ok_or_else(|| {
        SolidError::invalid_input(
            "surface::surface::p1::evaluate",
            "evaluation matrix too small",
        )
    })
}

/// `Evaluate` flags: the given sides, the upper half, the given zero-tangent handling.
const fn side_flags(u_side: Side, v_side: Side, non_zero_tangents: bool) -> EvaluateFlags {
    EvaluateFlags {
        u_side,
        v_side,
        only_upper_half: true,
        non_zero_tangents,
    }
}

/// The point and first partial derivatives `(P, S_u, S_v)` at `uv`, zero first
/// derivatives replaced by short, properly directed ones
/// (`SmSurface::Evaluate1stDerivatives`).
///
/// # Errors
///
/// Those of the surface's `Evaluate`.
pub fn evaluate_1st_derivatives<S: SurfaceEvaluate + ?Sized>(
    k: &Kernel,
    surface: &S,
    uv: Point2,
    u_side: Side,
    v_side: Side,
) -> Result<(Point3, Vec3, Vec3)> {
    let m = surface.evaluate(k, uv, 1, 1, side_flags(u_side, v_side, true))?;
    Ok((
        at_checked(&m, 0, 0)?,
        at_checked(&m, 1, 0)?,
        at_checked(&m, 0, 1)?,
    ))
}

/// The point and the first and second partial derivatives at a surface point
/// ([`evaluate_2nd_derivatives`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecondDerivatives {
    /// `rPoint`.
    pub point: Point3,
    /// `rDU`.
    pub du: Vec3,
    /// `rDV`.
    pub dv: Vec3,
    /// `rDUV`.
    pub duv: Vec3,
    /// `rDUU`.
    pub duu: Vec3,
    /// `rDVV`.
    pub dvv: Vec3,
}

/// The point and the first and second partial derivatives at `uv`, zero first
/// derivatives replaced (`SmSurface::Evaluate2ndDerivatives`).
///
/// # Errors
///
/// Those of the surface's `Evaluate`.
pub fn evaluate_2nd_derivatives<S: SurfaceEvaluate + ?Sized>(
    k: &Kernel,
    surface: &S,
    uv: Point2,
    u_side: Side,
    v_side: Side,
) -> Result<SecondDerivatives> {
    let m = surface.evaluate(k, uv, 2, 2, side_flags(u_side, v_side, true))?;
    Ok(SecondDerivatives {
        point: at_checked(&m, 0, 0)?,
        du: at_checked(&m, 1, 0)?,
        dv: at_checked(&m, 0, 1)?,
        duv: at_checked(&m, 1, 1)?,
        duu: at_checked(&m, 2, 0)?,
        dvv: at_checked(&m, 0, 2)?,
    })
}

/// The position and the directional derivatives up to `highest_deriv` (at most 3) of
/// the surface along `uv_dir` at `uv`, from exact (not zero-corrected) partial
/// derivatives (`SmSurface::EvaluateDirectionalDerivs`).
///
/// # Errors
///
/// `SM_ERR` for `highest_deriv > 3`; those of `Evaluate` and `smgu_DirectionalDerivs`.
pub fn evaluate_directional_derivs<S: SurfaceEvaluate + ?Sized>(
    k: &Kernel,
    surface: &S,
    uv: Point2,
    uv_dir: Vec2,
    highest_deriv: usize,
    u_side: Side,
    v_side: Side,
) -> Result<Vec<Vec3>> {
    const OP: &str = "surface::surface::p1::evaluate_directional_derivs";
    // check state
    if highest_deriv > 3 {
        return Err(SolidError::sm_err(OP));
    }

    // make the surface evaluation
    let m = surface.evaluate(
        k,
        uv,
        highest_deriv,
        highest_deriv,
        side_flags(u_side, v_side, false),
    )?;

    // pass the call along (smgu_DirectionalDerivs reads the matrix row-major,
    // `[i * (highest_deriv + 1) + j]`)
    let mut derivs = Vec::new();
    for i in 0..=highest_deriv {
        for j in 0..=highest_deriv {
            derivs.push(
                m.get(i)
                    .and_then(|r| r.get(j))
                    .copied()
                    .unwrap_or(Vec3::UNDEF),
            );
        }
    }
    directional_derivs(uv_dir, highest_deriv, highest_deriv, &derivs)
}

// ---- SmSTEPSurface --------------------------------------------------------------------

/// A surface evaluated in the STEP parameterisation of another surface
/// (`SmSTEPSurface`): its domain is the base surface's STEP domain and its
/// evaluations are the base surface's `EvaluateSTEP` / `EvaluateSTEPPoint`.
#[derive(Debug, Clone)]
pub struct StepSurface {
    /// `m_pSurface`: the base surface (`None` after the empty constructor).
    surface: Option<Box<Surface>>,
    /// `m_bOwnsSurface`.
    owns_surface: bool,
}

impl StepSurface {
    /// A bounded, exact evaluation copy; the nesting limit is checked by Surface.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        let surface = self
            .surface
            .as_deref()
            .map(|s| s.clone_for_evaluation_depth(depth).map(Box::new))
            .transpose()?;
        Ok(Self {
            surface,
            owns_surface: self.owns_surface,
        })
    }

    /// Exact evaluation copy; the caller validates the complete geometry graph first.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            surface: self
                .surface
                .as_deref()
                .map(|s| Box::new(s.clone_exact_bounded())),
            owns_surface: self.owns_surface,
        }
    }

    /// The base surface, mutable, for [`Surface::detach_nurb_cells`].
    pub(crate) fn nested_surfaces_mut(&mut self) -> Vec<&mut Surface> {
        self.surface.as_deref_mut().into_iter().collect()
    }

    /// `GetTypeString()`.
    pub const TYPE_STRING: &'static str = "SmSTEPSurface_TYPE";
    /// `GetClassString()`.
    pub const CLASS_STRING: &'static str = "SmSTEPSurface";

    /// `SmSTEPSurface(rSurface, bOwnsSurface)`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `surface` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn new(surface: Surface, owns_surface: bool) -> Result<Self> {
        check_geometry_nesting(
            NestedGeometry::Surface(&surface),
            1,
            "surface::surface::p1::StepSurface::new",
        )?;
        Ok(Self {
            surface: Some(Box::new(surface)),
            owns_surface,
        })
    }

    /// The empty constructor for I/O (`SmSTEPSurface()`): no base surface.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            surface: None,
            owns_surface: false,
        }
    }

    /// The base surface (`m_pSurface`).
    #[must_use]
    pub fn surface(&self) -> Option<&Surface> {
        self.surface.as_deref()
    }

    /// `m_bOwnsSurface`.
    #[must_use]
    pub const fn owns_surface(&self) -> bool {
        self.owns_surface
    }

    /// The base surface, or `InvalidInput` where upstream dereferences a null
    /// `m_pSurface`.
    fn base(&self) -> Result<&Surface> {
        self.surface.as_deref().ok_or(SolidError::invalid_input(
            OP_STEP,
            "STEP surface without base surface",
        ))
    }

    /// `GetType()`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        STEP_SURFACE_TYPE
    }

    /// `GetTypeString()`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        Self::TYPE_STRING
    }

    /// `GetClassString()`.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        Self::CLASS_STRING
    }

    /// `IsKindOf`: `SmSTEPSurface_TYPE` or a base type of `SmSurface`.
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == STEP_SURFACE_TYPE || super::p2::is_kind_of(t)
    }

    /// The base surface's STEP domain (`GetNaturalUVDomain`; an uninitialised domain
    /// without base surface).
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        self.surface
            .as_deref()
            .map_or_else(Extent2d::new, Surface::step_uv_domain)
    }

    /// The base surface's STEP point at `uv` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without base surface; those of [`Surface::evaluate_step_point`].
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.base()?.evaluate_step_point(k, uv)
    }

    /// The base surface's STEP derivatives at `uv` (`Evaluate`;
    /// `bNonZeroTangents` is not used).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without base surface; those of [`Surface::evaluate_step`].
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.base()?
            .evaluate_step(k, uv, highest_u_deriv, highest_v_deriv, flags)
    }

    /// [`StepSurface::evaluate`] for a caller that ignores its status: what the base
    /// surface's `EvaluateSTEP` leaves in the caller's array
    /// ([`Surface::evaluate_step_ignoring_status`]).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without base surface; those of
    /// [`Surface::evaluate_step_ignoring_status`].
    pub fn evaluate_ignoring_status(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Option<Vec<Vec<Vec3>>>> {
        self.base()?
            .evaluate_step_ignoring_status(k, uv, highest_u_deriv, highest_v_deriv, flags)
    }

    /// The base surface's STEP domain (`GetDomainWithContinuity`: the continuity, the
    /// point and the vector are not used).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without base surface.
    pub fn domain_with_continuity(&self) -> Result<Extent2d> {
        Ok(self.base()?.step_uv_domain())
    }

    /// The knots of a single Bezier segment over the STEP domain (`GetKnots`): the
    /// domain ends in `param`'s direction (those inside `ivl` when given), each with
    /// multiplicity `degree + 1`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` without base surface.
    pub fn knots(
        &self,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots_of(param, ivl, None)
    }

    /// [`StepSurface::knots`] whose multiplicity reads the base's `GetDegree` with its
    /// NURB made ([`Surface::degree_with_nurb`]): upstream's
    /// `m_pSurface->GetDegree` makes a lazy base's NURB first.
    ///
    /// # Errors
    ///
    /// Those of [`StepSurface::knots`]; [`SolidError::Unported`] from the make.
    pub fn knots_with_nurb(
        &self,
        k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots_of(param, ivl, Some(k))
    }

    /// The body of [`StepSurface::knots`]; with a kernel the base's degree is read with
    /// its NURB made.
    fn knots_of(
        &self,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
        k: Option<&Kernel>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        let base = self.base()?;
        let uv_domain = base.step_uv_domain();
        let mut knots = Vec::with_capacity(2);

        // output single bezier segment for the surface
        let (lo, hi) = if param == SurfParamType::U {
            (uv_domain.min().x, uv_domain.max().x)
        } else {
            (uv_domain.min().y, uv_domain.max().y)
        };
        match ivl {
            None => {
                knots.push(lo);
                knots.push(hi);
            }
            Some(ivl) => {
                // (ContainsValue's default tolerance: 0)
                if ivl.contains_value(lo, 0.0) {
                    knots.push(lo);
                }
                if ivl.contains_value(hi, 0.0) {
                    knots.push(hi);
                }
            }
        }

        // when asked - set multiplicities
        let degree = match k {
            Some(k) => base.degree_with_nurb(k, param)?,
            None => base.degree(param),
        };
        let num_knots = degree + 1;
        let mults = vec![num_knots; knots.len()];
        Ok((knots, mults))
    }

    /// `operator==`: the same type (`SmSurface::operator==`) and equal base
    /// surfaces (both missing, or equal by their `operator==`).
    ///
    /// # Errors
    ///
    /// Those of the base surfaces' [`Surface::approx_eq`].
    pub fn approx_eq(&self, k: &Kernel, other: &Surface) -> Result<bool> {
        let SurfaceKind::Step(o) = other.kind() else {
            // first check the base: a different type
            return Ok(false);
        };
        // low work
        if core::ptr::eq(self, o) {
            return Ok(true);
        }

        // check equivalence of these objects
        match (self.surface.as_deref(), o.surface.as_deref()) {
            (None, None) => Ok(true),
            (Some(a), Some(b)) => Ok(core::ptr::eq(a, b) || a.approx_eq(k, b)?),
            _ => Ok(false),
        }
    }

    /// `SetContext`: nothing to set (the kernel is passed to the calls that need it).
    pub const fn set_context(&mut self) {}

    /// Reads a STEP surface written by `WriteToDB` (`ReadFromDB`): the base surface's
    /// type header and record; the new surface owns its base surface.
    ///
    /// # Errors
    ///
    /// Those of the reader and of [`Surface::read_from_db`]; `InvalidInput` for an
    /// unreadable type header (undefined upstream) or nesting too deep.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        db.enter_nested(OP_STEP)?;
        let result = Self::read_body(k, db, version);
        db.leave_nested();
        result
    }

    /// The body of [`StepSurface::read_from_db`].
    fn read_body(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // base surface
        if db.file_type() == FileType::Ascii {
            db.go_to_next_line();
        }
        let base_type = db
            .read_type(false)?
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_STEP, "base surface type not read"))?;
        let base = Surface::read_from_db(k, base_type, db, version)?;

        // load object
        Self::new(base, true)
    }
}

// ---- tolerances of surfaces (SmTol co-ports) ----------------------------------------------

/// `SmTol::GetZoneTol3d(const SmObject*)` of a surface: its face's tolerance when it
/// has a face (the tolerance the B-rep cached on the surface,
/// [`Surface::owner_zone_tol_3d`]), else the system zone tolerance.
///
/// # Errors
///
/// Those of [`Surface::face_zone_tol_3d`]: [`SolidError::Unported`]
/// (`SmFace::GetTolerance`, `topology.face_p2`) for a surface with a face whose
/// tolerance was not cached on it.
pub fn zone_tol_3d(k: &Kernel, surface: &Surface) -> Result<f64> {
    match surface.face_zone_tol_3d()? {
        Some(tol) => Ok(tol),
        None => Ok(kernel_zone_tol_3d(k, SmallTopoType::NormalSize)),
    }
}

/// `SmTol::MapTo2d(rTol3d, rUV, rUVDir, rSurface)`: the 3D tolerance `tol_3d` mapped
/// to the parameter space of `surface` at `uv` in the UV direction `uv_dir` (the
/// smaller of the two directional values for a zero direction). Near singular first
/// derivatives the mapped value is a hundredth of the domain side.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluation (its other failures are ignored, as
/// upstream: the derivatives are then zero).
pub fn map_to_2d_dir(
    k: &Kernel,
    tol_3d: f64,
    uv: Point2,
    uv_dir: Vec2,
    surface: &Surface,
) -> Result<f64> {
    // evaluate SurfacePt = rSurface(rUVTrimCurve(dParam))
    let (_, du, dv) = first_derivatives_or_zero(k, surface, uv)?;

    // locals
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let scaled_zero = EFF_ZERO;
    let du_size = du.length();
    let dv_size = dv.length();

    // Scale dTol3d to dTolUV in the U and V directions - limit scaled dTolUV values when
    // rDU or rDV are very small (near singularities)
    let scaled_u = if du_size < scaled_zero {
        uv_domain.u_interval().length() / 100.0
    } else {
        tol_3d / du_size
    };
    let scaled_v = if dv_size < scaled_zero {
        uv_domain.v_interval().length() / 100.0
    } else {
        tol_3d / dv_size
    };

    // get directional Tol2d value when [dDirU dDirV] is not the zero vector
    //     otherwise limit Tol2d = Min(dScaledU, dScaledV)
    let size = (uv_dir.x * uv_dir.x + uv_dir.y * uv_dir.y).sqrt();
    Ok(if size < scaled_zero {
        scaled_u.min(scaled_v)
    } else {
        uv_dir.x.abs() / size * scaled_u + uv_dir.y.abs() / size * scaled_v
    })
}

/// `SmTol::MapTo2d(rTol3d, rUV, rSurface)`: [`map_to_2d_dir`] in the direction of
/// the larger first derivative (the smaller UV tolerance).
///
/// # Errors
///
/// Those of [`map_to_2d_dir`].
pub fn map_to_2d(k: &Kernel, tol_3d: f64, uv: Point2, surface: &Surface) -> Result<f64> {
    // evaluate SurfacePt = rSurface(rUVTrimCurve(dParam))
    let (_, du, dv) = first_derivatives_or_zero(k, surface, uv)?;

    // pick the max 1st deriv direction
    let uv_dir = if du.length_squared() > dv.length_squared() {
        Vec2::new(1.0, 0.0)
    } else {
        Vec2::new(0.0, 1.0)
    };

    // pass the call along for Max 1st deriv (yields Min Tol2d value)
    map_to_2d_dir(k, tol_3d, uv, uv_dir, surface)
}

/// `SmTol::MapTo2d(rTol3d, dParam, rTrimCurve, rSurface)` (the result as an
/// `SmTol2d`): for a UV curve, `tol_3d` mapped to the parameter space of `surface` at
/// the curve point `trim_curve(param)` in the curve direction, by [`map_to_2d_dir`]
/// (the curve evaluation's status is ignored, as upstream; a failed evaluation leaves
/// undefined vectors).
///
/// The UV-curve branch (`GetDim() == 2`), the same-surface `SmCrvOnSurf` branch (the
/// curve is a curve on exactly this `surface` object, [`std::ptr::eq`] with its base
/// surface as upstream's `&rSurface == GetBaseSurface()`; its UV curve is evaluated) and
/// the general 3D branch (the point and tangent dropped to the surface with
/// `SmSurface::DropPoint` and `DropVectors`) are ported. A curve on a copy of the surface
/// takes the 3D branch, as upstream's pointer comparison does for a distinct object.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations and the drop.
pub fn map_to_2d_on_curve(
    k: &Kernel,
    tol_3d: f64,
    param: f64,
    trim_curve: &Curve,
    surface: &Surface,
) -> Result<f64> {
    // `&rSurface == ((SmCrvOnSurf &)rTrimCurve).GetBaseSurface()`: object identity of the
    // curve's base surface (a reference comparison here: a copy of the surface is
    // another object, as upstream's pointer comparison)
    let own_surface = match trim_curve.kind() {
        CurveKind::CrvOnSurf(c) => std::ptr::eq(surface, c.surface()),
        _ => false,
    };

    // evaluate SurfacePt = rSurface(rUVTrimCurve(dParam)) for UVTrimCurves and SmCrvOnSurfs
    let mut pd = [Vec3::UNDEF; 2];
    let evaluated = match trim_curve.kind() {
        // (`GetUVCurve()->Evaluate(dParam, 1, TRUE, sPD)`)
        CurveKind::CrvOnSurf(c) if own_surface => c.uv_curve().evaluate(k, param, 1, true, true),
        _ => trim_curve.evaluate(k, param, 1, true, true),
    };
    if let Some(d) = ignored(evaluated)? {
        for (slot, v) in pd.iter_mut().zip(d) {
            *slot = v;
        }
    }
    let [p0, p1] = pd;
    let (uv, uv_dir) = if trim_curve.dim() == 2 || own_surface {
        (Point2::new(p0.x, p0.y), Vec2::new(p1.x, p1.y))
    } else {
        // general 3 dim Curve: drop its point and tangent to the surface's UV space
        // (statuses ignored upstream: the UV stays undefined when the drop fails)
        let drop = ignored(surface.drop_point::<crate::surface::surface::p4::NoObject>(
            k,
            p0,
            &surface.natural_uv_domain_with_nurb(k)?,
            &crate::surface::surface::p5::DropPointOptions::default(),
        ))?;
        let uv = drop.map_or(Point2::UNDEF, |d| d.result_uv);
        let uv_dir = ignored(crate::surface::surface::p7::drop_vectors_at(
            k,
            surface,
            uv,
            Side::Left,
            Side::Left,
            &[p1],
        ))?
        .and_then(|v| v.first().copied())
        .unwrap_or(Vec2::UNDEF);
        (uv, uv_dir)
    };

    // pass the call along
    Ok(tol3d(map_to_2d_dir(k, tol_3d, uv, uv_dir, surface)?))
}

/// `Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` with its status ignored: zero
/// vectors on failure, except [`SolidError::Unported`], which is returned.
fn first_derivatives_or_zero(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    match surface.evaluate_1st_derivatives(k, uv, true, true) {
        Ok(d) => Ok(d),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok((Vec3::ZERO, Vec3::ZERO, Vec3::ZERO)),
    }
}

/// `SmTol::GetScaledZero(const SmSurface&)`: scaled to the surface's midpoint and
/// its [`Surface::approximate_size`].
///
/// # Errors
///
/// [`SolidError::Unported`] from the size or the evaluation (their other failures are
/// ignored: zero size, zero point).
pub fn scaled_zero_of_surface(k: &Kernel, surface: &Surface) -> Result<f64> {
    let size = match surface.approximate_size(k, None) {
        Ok(s) => s,
        Err(e @ SolidError::Unported { .. }) => return Err(e),
        Err(_) => Vec2::ZERO,
    };
    let pnt = match surface.evaluate_point(
        k,
        surface.natural_uv_domain_with_nurb(k)?.evaluate(0.5, 0.5),
    ) {
        Ok(p) => p,
        Err(e @ SolidError::Unported { .. }) => return Err(e),
        Err(_) => Point3::ZERO,
    };
    Ok(scaled_zero2(&pnt, &size))
}

// ---- surface caches (CacheMakeOrValidate) --------------------------------------------------

/// The tessellation tolerances `CacheMakeOrValidate` gives a new surface cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheTolerances {
    /// The chord height tolerance: 1/40th of the size estimate (1/15th when both
    /// degrees are at most 2).
    pub chord_height_tol: f64,
    /// The angle tolerance in radians: 20 degrees (40 when both degrees are at most
    /// 2).
    pub angle_tol_rad: f64,
}

/// The tolerances of a new surface cache (`CacheMakeOrValidate`): the size estimate
/// is half the summed distances from the domain midpoint to the domain corners and
/// to the points at `(0.9, 0.1)` and `(0.1, 0.9)`.
///
/// # Errors
///
/// Those of the evaluations.
pub fn cache_tolerances(k: &Kernel, surface: &Surface) -> Result<CacheTolerances> {
    // Get some point positions scattered over the Surface's Natural Domain.
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let p1 = surface.evaluate_point(k, uv_domain.min())?;
    let p_mid = surface.evaluate_point(k, uv_domain.evaluate(0.5, 0.5))?;
    let p_mid2 = surface.evaluate_point(k, uv_domain.evaluate(0.9, 0.1))?;
    let p_mid3 = surface.evaluate_point(k, uv_domain.evaluate(0.1, 0.9))?;
    let p2 = surface.evaluate_point(k, uv_domain.max())?;

    // get an Idea of surface's size.
    let size =
        (p_mid.distance(p1) + p_mid.distance(p2) + p_mid.distance(p_mid2) + p_mid.distance(p_mid3))
            / 2.0;

    // use surface size estimate to set chordHeight tolerance
    let mut chord_height_tol = size / 40.0; // Tessellation CH is 1/40th of estimated surface size
    let mut angle_tol_rad = 20.0 * PI / 180.0; // Angle tolerance of 20 degrees

    // Loosen tolerances on analytic surfaces - they don't need as much subdivision.
    // (GetDegree makes a missing NURB: a lazy extrusion's or cone's 0 is not its degree)
    if surface.degree_with_nurb(k, SurfParamType::U)? <= 2
        && surface.degree_with_nurb(k, SurfParamType::V)? <= 2
    {
        chord_height_tol = size / 15.0;
        angle_tol_rad *= 2.0;
    }
    Ok(CacheTolerances {
        chord_height_tol,
        angle_tol_rad,
    })
}

/// The cache options of [`cache_tolerances`] (the other options at their defaults).
fn cache_options(tol: CacheTolerances) -> SurfaceCacheOptions {
    SurfaceCacheOptions {
        chord_height_tolerance: tol.chord_height_tol,
        ang_tol_rad: tol.angle_tol_rad,
        ..SurfaceCacheOptions::default()
    }
}

/// `SmSurface::CacheMakeOrValidate` without building: `SM_ERR` for a cache type other
/// than [`ObjectCacheType::TrimSrf`] or [`ObjectCacheType::Surface`]; else the
/// existing cache `old`, except a trimmed surface cache whose face was modified while
/// its B-rep no longer allows editing (`old_face_was_modified &&
/// brep_editing_disabled`): that one must be deleted and rebuilt. `None` when a new
/// cache must be built ([`make_trim_srf_cache`] / [`make_surface_cache`]).
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::Upstream`]) for another cache type.
pub fn validate_surface_cache(
    cache_type: ObjectCacheType,
    old: Option<CacheId>,
    old_face_was_modified: bool,
    brep_editing_disabled: bool,
) -> Result<Option<CacheId>> {
    match cache_type {
        ObjectCacheType::TrimSrf => {
            // low work - use OldCache when its given and current
            Ok(old.filter(|_| !(old_face_was_modified && brep_editing_disabled)))
        }
        // low work - use OldCache when its given
        ObjectCacheType::Surface => Ok(old),
        _ => Err(SolidError::sm_err(OP_CACHE)),
    }
}

/// `SmSurface::CacheMakeOrValidate(SM_OC_SURFACE, ..)` building a new cache: the
/// surface's [`SurfaceCache`] over its natural domain with the [`cache_tolerances`],
/// built (`BuildTree()`). `handle` is the surface's handle in the caller's object
/// space.
///
/// # Errors
///
/// Those of the tolerances and of the tree build (upstream then has no cache).
pub fn make_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
) -> Result<SurfaceCache<O>> {
    let tol = cache_tolerances(k, surface)?;

    // construct new SurfaceCache Object
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    // the cache refers to the surface itself upstream (`m_cpSurface`): an exact copy,
    // not `Clone` (the copy constructor, which fixes the knot vectors up again)
    let mut cache = SurfaceCache::new(
        surface.clone_for_evaluation()?,
        uv_domain,
        cache_options(tol),
    );
    cache.set_surface_handle(handle);

    // build the surface subdivision tree
    cache.build_tree(k, false)?;
    Ok(cache)
}

/// Whether `smsurf_GetSurfaceCache` hands out a cache whose tree is built, given the
/// surface's cache slot: a new cache is built unless the context is doing a Boolean
/// operation; an existing one is reused as it is.
#[must_use]
pub fn cache_tree_built_on_get(k: &Kernel, state: CacheTreeState) -> bool {
    match state {
        CacheTreeState::NoCache => !k.context.doing_boolean(),
        CacheTreeState::Unbuilt => false,
        CacheTreeState::Built => true,
    }
}

/// The tree step of `CacheMakeOrValidate(SM_OC_TRIMSRF, ..)` on a cache the port has
/// just made, given the surface's cache slot (`state`): a new cache (`NoCache`) is
/// built unless the context is doing a Boolean operation; upstream reuses an existing
/// one, so an `Unbuilt` cache stays without tree (even outside a Boolean operation)
/// and a `Built` one has its continuities (built here outside a Boolean operation,
/// else only the continuity arrays are restored and the tree waits for `GetTree`).
/// `build` builds the tree (and marks the slot built). A new cache left without tree
/// marks the slot `Unbuilt`.
fn trim_srf_cache_tree<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    state: CacheTreeState,
    cache: &mut TrimSrfCache<O>,
    build: impl FnOnce(&mut TrimSrfCache<O>) -> Result<()>,
) -> Result<()> {
    let doing_boolean = k.context.doing_boolean();
    match state {
        // when we are not doing a boolean operation
        //  - during a boolean operation delay surface cache tree construction
        //    until some function calls SmSurfaceCache::GetTree()
        CacheTreeState::NoCache | CacheTreeState::Built if !doing_boolean => {
            // build the surface subdivision tree annotated with UVTrimBoundary
            // classifications
            build(cache)
        }
        CacheTreeState::NoCache => {
            surface.set_cache_tree_state(CacheTreeState::Unbuilt);
            Ok(())
        }
        CacheTreeState::Built => cache.base_mut().restore_continuities(k),
        CacheTreeState::Unbuilt => Ok(()),
    }
}

/// `SmSurface::CacheMakeOrValidate(SM_OC_TRIMSRF, ..)` building a new cache: the
/// surface's [`TrimSrfCache`] with the [`cache_tolerances`], its tree built unless
/// the context is doing a Boolean operation (then [`SurfaceCache::get_tree`] builds
/// it on first use) or the surface's cache slot says upstream reuses a cache whose
/// tree is not built ([`Surface::cache_tree_state`]; see `trim_srf_cache_tree`).
///
/// # Errors
///
/// Those of the tolerances and of the tree build (upstream then deletes the cache).
pub fn make_trim_srf_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
) -> Result<TrimSrfCache<O>> {
    let tol = cache_tolerances(k, surface)?;

    // construct new SurfaceCache Object
    // the cache refers to the surface itself upstream (`m_cpSurface`): an exact copy,
    // not `Clone` (the copy constructor, which fixes the knot vectors up again)
    let state = surface.cache_tree_state();
    let mut cache = TrimSrfCache::new(surface.clone_for_evaluation()?, cache_options(tol));
    cache.base_mut().set_surface_handle(handle);
    trim_srf_cache_tree(k, surface, state, &mut cache, |c| c.build_tree(k))?;
    // a face surface whose owner is cleared keeps it (SmCacheMgr::AddToObjectCache)
    surface.keep_standalone_trim_cache(&cache)?;
    Ok(cache)
}

/// [`make_trim_srf_cache`] for a surface owned by `face` (`GetFace()`): the tree build
/// creates the face's UV trim curves and implants its boundary
/// ([`TrimSrfCache::build_tree_with_face`]). `surface` is the face's surface (a copy:
/// the face's B-rep is changed through `face` while the cache is built).
///
/// # Errors
///
/// As [`make_trim_srf_cache`], and those of the face reads.
pub fn make_trim_srf_cache_with_face<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
    face: &mut dyn CacheFace<O>,
) -> Result<TrimSrfCache<O>> {
    let tol = cache_tolerances(k, surface)?;

    // construct new SurfaceCache Object
    let state = surface.cache_tree_state();
    let mut cache = TrimSrfCache::new(surface.clone_for_evaluation()?, cache_options(tol));
    cache.base_mut().set_surface_handle(handle);
    if let Some(owner) = surface.face()? {
        cache.bind_face(owner, face.face(), face.zone_tol_3d(k, face.face())?)?;
    }
    trim_srf_cache_tree(k, surface, state, &mut cache, |c| {
        c.build_tree_with_face(k, face)
    })?;
    Ok(cache)
}

/// `smsurf_GetSurfaceCache` of a surface owned by `face`: its trimmed surface cache
/// ([`make_trim_srf_cache_with_face`]), else its surface cache, built fresh and
/// wrapped as it is ([`TrimSrfCache::from_base`]: no trimmed cache state, so the face
/// classification does not consult it, as upstream finds no trimmed cache then).
///
/// # Errors
///
/// As [`get_surface_cache`].
pub fn get_surface_cache_with_face<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
    face: &mut dyn CacheFace<O>,
) -> Result<TrimSrfCache<O>> {
    // if possible - get or make TrimSrfCache
    match make_trim_srf_cache_with_face(k, surface, handle, face) {
        Ok(c) => Ok(c),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::NotFound { .. }),
        ) => Err(e),
        // else - get or make SurfaceCache
        Err(_) => Ok(TrimSrfCache::from_base(make_surface_cache(
            k, surface, handle,
        )?)),
    }
}

/// `smsurf_GetSurfaceCache` of a surface without face (`GetFace()` is `NULL`): its
/// trimmed surface cache built standalone ([`make_trim_srf_cache`]), else its surface
/// cache wrapped as one, as [`get_surface_cache_with_face`] wraps it.
///
/// # Errors
///
/// As [`get_surface_cache_with_face`].
pub fn get_trim_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
) -> Result<TrimSrfCache<O>> {
    // if possible - get or make TrimSrfCache
    match make_trim_srf_cache(k, surface, handle) {
        Ok(c) => Ok(c),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::NotFound { .. }),
        ) => Err(e),
        // else - get or make SurfaceCache
        Err(_) => Ok(TrimSrfCache::from_base(make_surface_cache(
            k, surface, handle,
        )?)),
    }
}

/// `smsurf_GetSurfaceCache`: the surface's trimmed surface cache, else its surface
/// cache, built fresh.
///
/// # Errors
///
/// Those of [`make_surface_cache`] when both fail; [`SolidError::Unported`] from
/// either.
pub fn get_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
) -> Result<SurfaceCache<O>> {
    // if possible - get or make TrimSrfCache
    match make_trim_srf_cache(k, surface, handle) {
        Ok(c) => Ok(c.into_base()),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        // else - get or make SurfaceCache
        Err(_) => make_surface_cache(k, surface, handle),
    }
}

/// The cache-slot side of `smsurf_GetSurfaceCache` for a caller that uses no cache
/// object from it (the port's solve that follows makes its own): a surface without a
/// cache gets one, whose tree is built unless the context is doing a Boolean
/// operation ([`Surface::cache_tree_state`]).
pub fn note_surface_cache(k: &Kernel, surface: &Surface) {
    if surface.cache_tree_state() == CacheTreeState::NoCache {
        surface.set_cache_tree_state(if k.context.doing_boolean() {
            CacheTreeState::Unbuilt
        } else {
            CacheTreeState::Built
        });
    }
}

/// `smsurf_GetSurfaceCache` for a reader of the cache's continuity arrays that does not
/// call `GetTree` (`GetDomainWithContinuity`, `LiftCurve`): the arrays are filled only
/// when the cache's tree has been built. When upstream's cache has its tree (the
/// surface's cache slot is built, or a new cache is made outside a Boolean operation)
/// the cache comes from the caller's `context` as for a solve (else
/// [`get_surface_cache`]); otherwise upstream's cache has no tree and its arrays are
/// empty: [`get_surface_cache`] makes it so (a new cache marks the slot unbuilt).
///
/// # Errors
///
/// Those of [`get_surface_cache`] and of the context's cache preparation.
pub fn continuity_surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
    context: Option<&mut dyn SurfaceSolveContext<O>>,
) -> Result<SurfaceCache<O>> {
    match context {
        Some(context) if cache_tree_built_on_get(k, surface.cache_tree_state()) => {
            context.build_surface_cache(k, surface, handle)
        }
        _ => get_surface_cache(k, surface, handle),
    }
}

/// The caller's operation-scoped topology access for surface solves. Cache
/// preparation is mutable (it may attach UV trims); classification borrows the
/// prepared topology afterward. One context can serve several surfaces of the
/// same B-rep without keeping overlapping mutable borrows in their caches.
/// Preparation may update trims and tolerance metadata, not the input surface's
/// evaluated geometry or the intersected 3D curve. Numerical evaluation uses the
/// returned cache's exact surface copy, refreshed with the final face tolerance.
/// Retained caches are valid only for the same edit-free geometry/topology state.
pub trait SurfaceSolveContext<O: Copy + PartialEq> {
    /// Builds this surface's cache at the point the numerical operation first
    /// requests it, with its decomposition tree ready for the solve.
    ///
    /// # Errors
    /// Those of cache preparation and the caller's identity resolution.
    fn build_surface_cache(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>>;

    /// This surface's cache as `smsurf_GetSurfaceCache` hands it out to a reader that
    /// does not call `GetTree` (the state bits `GlobalPointSolve`'s fast-drop face
    /// test reads). Upstream's factory defers the tree during a Boolean operation, and
    /// the tree build may change those bits, so a context that keeps face caches
    /// answers the cache without the deferred tree; by default the solve's cache.
    ///
    /// # Errors
    /// As [`SurfaceSolveContext::build_surface_cache`].
    fn surface_cache_state(
        &mut self,
        k: &Kernel,
        surface: &Surface,
        handle: Option<O>,
    ) -> Result<SurfaceCache<O>> {
        self.build_surface_cache(k, surface, handle)
    }

    /// Read-only classification of the topology prepared by this context.
    fn face_queries(&self) -> Option<&dyn FaceQueries<O>>;
}

/// The caller's face-aware cache preparation for a surface/surface intersection.
///
/// `handles` name the two intersected surfaces in their caches (the first is the
/// dispatching surface, the second its argument); each is passed to
/// [`SurfaceSolveContext::build_surface_cache`] when a cache is first requested.
pub struct SurfaceIntersectContext<'a, O: Copy + PartialEq> {
    /// The operation-scoped cache and face service.
    pub solver: &'a mut dyn SurfaceSolveContext<O>,
    /// The handles naming the two surfaces in their caches.
    pub handles: [Option<O>; 2],
}

// ---- bounding boxes -------------------------------------------------------------------

/// The boxes [`Surface::calculate_bounding_box`] computes (those asked for).
#[derive(Debug, Clone, Default)]
pub struct SurfaceBoxes {
    /// The axis-aligned box (`pNormalBox`).
    pub normal_box: Option<Extent3d>,
    /// The non-axis-aligned box (`pPseudoBox`).
    pub pseudo_box: Option<PseudoBox>,
    /// The box of the normal vectors (`pPolarBox`).
    pub polar_box: Option<PolarBox>,
}

/// The solution object of [`Surface::create_silhouette_curves`]'s surface cache (its
/// trace solves keep no objects).
#[derive(Debug, Clone, Copy, PartialEq)]
struct SilhouetteObject;

impl SolutionObject for SilhouetteObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }
}

/// The traced curve named by the solutions of [`Surface::create_silhouette_curves`]'s
/// `GlobalPropertyAnalysis` (upstream's `pCurve`): its natural interval, which
/// `IdenticalSolutions` scales its merge tolerance by.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SilhouetteCurve {
    natural_interval: Extent1d,
}

impl SolutionObject for SilhouetteCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.natural_interval)
    }
}

// ---- the SmSurface members of this unit ---------------------------------------------------

impl Surface {
    /// `SmSurface::operator==`: the same object or the same type.
    #[must_use]
    pub fn base_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // when objects are equivalent
        self.type_code() == other.type_code()
    }

    /// The virtual `operator==`: [`Surface::base_eq`], then the kind's members.
    ///
    /// # Errors
    ///
    /// Those of the kinds' comparisons.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        if core::ptr::eq(self, other) {
            return Ok(true);
        }
        match (self.kind(), other.kind()) {
            (SurfaceKind::CurveBounded(s), _) => s.approx_eq(k, other),
            (SurfaceKind::Step(s), _) => s.approx_eq(k, other),
            (SurfaceKind::OffsetSurface(s), _) => s.approx_eq(k, other),
            // SmBSplineSurface::operator==: SmSurface::operator== first
            (SurfaceKind::BSpline(a), SurfaceKind::BSpline(b)) => Ok(a.approx_eq(b)),
            // SmPlane::operator==: SmBSplineSurface::operator== first
            (SurfaceKind::Plane(a), SurfaceKind::Plane(b)) => Ok(a.approx_eq(b)),
            (SurfaceKind::Cone(a), SurfaceKind::Cone(b)) => Ok(a.approx_eq(b)),
            (SurfaceKind::SurfOfRevolution(a), SurfaceKind::SurfOfRevolution(b)) => {
                a.approx_eq(k, b)
            }
            (SurfaceKind::SurfOfExtrusion(a), SurfaceKind::SurfOfExtrusion(b)) => a.approx_eq(k, b),
            (SurfaceKind::SrfInVolume(a), SurfaceKind::SrfInVolume(b)) => a.approx_eq(k, b),
            (SurfaceKind::Sphere(a), SurfaceKind::Sphere(b)) => Ok(a.approx_eq(b)),
            (SurfaceKind::Torus(a), SurfaceKind::Torus(b)) => Ok(a.approx_eq(b)),
            _ => Ok(false),
        }
    }

    /// Sets the owner and returns the previous one (`SetOwner`). A different owner
    /// drops the cached face tolerance ([`Surface::owner_zone_tol_3d`]): the B-rep
    /// records the new face's ([`Surface::set_owner_zone_tol_3d`]).
    pub fn set_owner(&mut self, new_owner: Option<ObjectRef>) -> Option<ObjectRef> {
        // (notify when pNewOwner is NULL or a Face: the object layer's notify)
        let old = core::mem::replace(self.owner_mut(), new_owner);
        if old == new_owner {
            // the bases this surface owns reach the face through it
            self.update_owned_bases();
        } else {
            // (also passes the face down to the bases this surface owns)
            self.set_owner_zone_tol_3d(None);
        }
        old
    }

    /// The face whose shape this surface defines (`GetFace`): the owner when it is a
    /// face; for a surface owner (the base of an owning `SmOffsetSurface`) the face that
    /// owner's own owner chain reaches, which the owning surface hands down (it holds
    /// this surface by value, see [`Surface::set_owner`]); `None` without owner or for a
    /// curve owner.
    ///
    /// # Errors
    ///
    /// None at present (kept fallible for the owner kinds still to come).
    pub fn face(&self) -> Result<Option<ObjectRef>> {
        // when the surface has an owner
        let Some(owner) = self.owner() else {
            return Ok(None);
        };
        // When surface defines a face's or a CFace's shape
        if owner.type_code == FACE_TYPE {
            return Ok(Some(owner));
        }
        // GenSurface owners can be another Surface (SmSrfInVolume, SmOffsetSurface -
        // continue looking for a Face owner) or a Curve (stop looking)
        if is_surface_type(owner.type_code) {
            // the owning surface holds this one by value and keeps the face its own
            // owner chain reaches here
            return Ok(self.owner_chain_face());
        }
        Ok(None)
    }

    // ---- evaluation wrappers ----------------------------------------------------------

    /// The point and first derivatives at `uv` with non-zero tangents
    /// (`Evaluate1stDerivatives`).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::evaluate`].
    pub fn evaluate_1st_derivatives(
        &self,
        k: &Kernel,
        uv: Point2,
        u_from_left: bool,
        v_from_left: bool,
    ) -> Result<(Point3, Vec3, Vec3)> {
        evaluate_1st_derivatives(k, self, uv, side(u_from_left), side(v_from_left))
    }

    /// The point, first and second derivatives at `uv` with non-zero tangents
    /// (`Evaluate2ndDerivatives`): `[P, Du, Dv, Duv, Duu, Dvv]`.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::evaluate`].
    pub fn evaluate_2nd_derivatives(
        &self,
        k: &Kernel,
        uv: Point2,
        u_from_left: bool,
        v_from_left: bool,
    ) -> Result<[Vec3; 6]> {
        let d = evaluate_2nd_derivatives(k, self, uv, side(u_from_left), side(v_from_left))?;
        Ok([d.point, d.du, d.dv, d.duv, d.duu, d.dvv])
    }

    /// The derivatives at `uv` in the STEP parameterisation (`EvaluateSTEP`): the
    /// curve bounded surface's basis surface, else the base default, [`Surface::evaluate`]
    /// with non-zero tangents (`flags.non_zero_tangents` is not used).
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn evaluate_step(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        crate::limits::probe();
        self.check_evaluation_copy_depth(0)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::CurveBounded(s) => {
                s.basis()
                    .evaluate_step(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            // the SmSurfOfRevolution override (the side and half flags are not used)
            SurfaceKind::Cone(s) => Ok(s
                .evaluate_step(uv, highest_u_deriv, highest_v_deriv)?
                .chunks(highest_v_deriv + 1)
                .map(<[Vec3]>::to_vec)
                .collect()),
            // the SmSphere override (the side and half flags are not used)
            SurfaceKind::Sphere(s) => Ok(s
                .evaluate_step(uv, highest_u_deriv, highest_v_deriv)?
                .chunks(highest_v_deriv + 1)
                .map(<[Vec3]>::to_vec)
                .collect()),
            SurfaceKind::SurfOfRevolution(s) => {
                let d = s.evaluate_step_at(k, uv, highest_u_deriv, highest_v_deriv)?;
                Ok(d.chunks(highest_v_deriv + 1)
                    .map(<[Vec3]>::to_vec)
                    .collect())
            }
            // the SmSurfOfExtrusion override (the side and half flags are not used)
            SurfaceKind::SurfOfExtrusion(s) => Ok(s
                .evaluate_step(k, uv, highest_u_deriv, highest_v_deriv)?
                .chunks(highest_v_deriv + 1)
                .map(<[Vec3]>::to_vec)
                .collect()),
            // the SmTorus override (the side and half flags are not used)
            SurfaceKind::Torus(s) => Ok(s
                .evaluate_step(uv, highest_u_deriv, highest_v_deriv)?
                .chunks(highest_v_deriv + 1)
                .map(<[Vec3]>::to_vec)
                .collect()),
            SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => {
                // pass the call along
                self.evaluate(
                    k,
                    uv,
                    highest_u_deriv,
                    highest_v_deriv,
                    EvaluateFlags {
                        non_zero_tangents: true,
                        ..flags
                    },
                )
            }
        }
    }

    /// [`Surface::evaluate`] into the caller's array `d` (flat, entry
    /// `i * (highest_v_deriv + 1) + j` the derivative of order `i` in u and `j` in v):
    /// the entries upstream's evaluation does not write keep the caller's values, as
    /// upstream's `aDerivatives` does. A curve bounded surface passes the call to its
    /// basis surface and a STEP surface to its base's `EvaluateSTEP`
    /// ([`Surface::evaluate_step_into`]); the other kinds write every entry, except
    /// the lower half when `flags.only_upper_half` is set.
    ///
    /// # Errors
    ///
    /// Those of the evaluation; `InvalidInput` when `d` is too short, for a STEP
    /// surface without base surface, or a chain deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn evaluate_into(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
        d: &mut [Vec3],
    ) -> Result<()> {
        self.evaluate_into_at(k, uv, highest_u_deriv, highest_v_deriv, flags, d, 0)
    }

    /// [`Surface::evaluate_step`] into the caller's array `d`, as
    /// [`Surface::evaluate_into`].
    ///
    /// # Errors
    ///
    /// As [`Surface::evaluate_into`].
    pub fn evaluate_step_into(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
        d: &mut [Vec3],
    ) -> Result<()> {
        self.evaluate_step_into_at(k, uv, highest_u_deriv, highest_v_deriv, flags, d, 0)
    }

    /// [`Surface::evaluate_into`] at chain depth `depth`.
    fn evaluate_into_at(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        flags: EvaluateFlags,
        d: &mut [Vec3],
        depth: usize,
    ) -> Result<()> {
        const OP: &str = "surface::surface::p1::evaluate_into";
        match self.kind() {
            // pass the call along
            SurfaceKind::CurveBounded(s) => {
                s.basis()
                    .evaluate_into_at(k, uv, hu, hv, flags, d, nesting_depth(depth, OP)?)
            }
            SurfaceKind::Step(s) => {
                let base = s
                    .surface()
                    .ok_or_else(|| SolidError::invalid_input(OP, "STEP surface without base"))?;
                base.evaluate_step_into_at(k, uv, hu, hv, flags, d, nesting_depth(depth, OP)?)
            }
            _ => {
                let m = self.evaluate(k, uv, hu, hv, flags)?;
                copy_written(&m, hu, hv, flags.only_upper_half, d, OP)
            }
        }
    }

    /// [`Surface::evaluate_step_into`] at chain depth `depth`.
    fn evaluate_step_into_at(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        flags: EvaluateFlags,
        d: &mut [Vec3],
        depth: usize,
    ) -> Result<()> {
        const OP: &str = "surface::surface::p1::evaluate_step_into";
        self.check_evaluation_copy_depth(0)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::CurveBounded(s) => {
                s.basis()
                    .evaluate_step_into_at(k, uv, hu, hv, flags, d, nesting_depth(depth, OP)?)
            }
            // the overrides (the side and half flags are not used)
            SurfaceKind::Cone(s) => s.evaluate_step_into(uv, hu, hv, d),
            SurfaceKind::Sphere(s) => s.evaluate_step_into(uv, hu, hv, d),
            SurfaceKind::Torus(s) => s.evaluate_step_into(uv, hu, hv, d),
            SurfaceKind::SurfOfRevolution(s) => s.evaluate_step_at_into(k, uv, hu, hv, d),
            SurfaceKind::SurfOfExtrusion(s) => s.evaluate_step_into(k, uv, hu, hv, d),
            // pass the call along (the base default, `Evaluate`)
            SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => self.evaluate_into_at(
                k,
                uv,
                hu,
                hv,
                EvaluateFlags {
                    non_zero_tangents: true,
                    ..flags
                },
                d,
                nesting_depth(depth, OP)?,
            ),
        }
    }

    /// [`Surface::evaluate_step`] for a caller that ignores its status: the derivative
    /// array it leaves, `None` when it fails before writing it. A curve bounded surface
    /// passes the caller's array on to its basis surface; the `SmSurfOfRevolution`
    /// override fails before writing; the base default is
    /// [`Surface::evaluate_ignoring_status`] with non-zero tangents.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::evaluate_ignoring_status`]; [`SolidError::Unported`].
    pub fn evaluate_step_ignoring_status(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Option<Vec<Vec<Vec3>>>> {
        match self.kind() {
            SurfaceKind::CurveBounded(s) => s.basis().evaluate_step_ignoring_status(
                k,
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags,
            ),
            SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_) => {
                ignored(self.evaluate_step(k, uv, highest_u_deriv, highest_v_deriv, flags))
            }
            SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => self.evaluate_ignoring_status(
                k,
                uv,
                highest_u_deriv,
                highest_v_deriv,
                EvaluateFlags {
                    non_zero_tangents: true,
                    ..flags
                },
            ),
        }
    }

    /// The point at `uv` in the STEP parameterisation (`EvaluateSTEPPoint`): the curve
    /// bounded surface's basis surface, else [`Surface::evaluate_point`].
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn evaluate_step_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.check_evaluation_copy_depth(0)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::CurveBounded(s) => s.basis().evaluate_step_point(k, uv),
            SurfaceKind::Cone(s) => s.evaluate_step_point(uv),
            SurfaceKind::Sphere(s) => s.evaluate_step_point(uv),
            SurfaceKind::Torus(s) => s.evaluate_step_point(uv),
            SurfaceKind::SurfOfRevolution(s) => s.evaluate_step_point_at(k, uv),
            SurfaceKind::SurfOfExtrusion(s) => s.evaluate_step_point(k, uv),
            SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => self.evaluate_point(k, uv),
        }
    }

    /// The derivatives at `uv` without zero-tangent adjustments (`EvaluateSimple`):
    /// the B-spline's and the plane's overrides, the curve bounded surface's basis
    /// surface, else the base default, [`Surface::evaluate`] with exact tangents.
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn evaluate_simple(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.check_evaluation_copy_depth(0)?;
        match self.kind() {
            SurfaceKind::BSpline(s) => s.evaluate_simple(
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags.u_side == Side::Left,
                flags.v_side == Side::Left,
                flags.only_upper_half,
            ),
            // inherited from SmBSplineSurface, which reads m_pNurb: upstream's analytic
            // surfaces always hold one (the port makes it on demand, kind_with_nurb)
            SurfaceKind::Cone(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_) => self.analytic_base_with_nurb(k)?.evaluate_simple(
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags.u_side == Side::Left,
                flags.v_side == Side::Left,
                flags.only_upper_half,
            ),
            SurfaceKind::Plane(s) => {
                s.evaluate_simple(uv, highest_u_deriv, highest_v_deriv, flags.only_upper_half)
            }
            SurfaceKind::CurveBounded(s) => {
                s.basis()
                    .evaluate_simple(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::OffsetSurface(s) => {
                s.evaluate_simple(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::SrfInVolume(s) => s.evaluate_simple(
                k,
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags.u_side,
                flags.v_side,
                flags.only_upper_half,
            ),
            SurfaceKind::Step(_) => {
                // pass the call along
                self.evaluate(
                    k,
                    uv,
                    highest_u_deriv,
                    highest_v_deriv,
                    EvaluateFlags {
                        non_zero_tangents: false,
                        ..flags
                    },
                )
            }
        }
    }

    /// [`Surface::evaluate_simple`] with first derivatives (`1, 1`) into the caller's
    /// 2x2 block `d`: the entries the evaluation has are written, the others keep
    /// their values. The B-spline based kinds evaluate without a heap allocation per
    /// call
    /// ([`crate::nurbs::b_spline_surface::BSplineSurface::evaluate_simple_first_into`]).
    ///
    /// # Errors
    ///
    /// As [`Surface::evaluate_simple`].
    pub fn evaluate_simple_first_into(
        &self,
        k: &Kernel,
        uv: Point2,
        flags: EvaluateFlags,
        d: &mut [[Vec3; 2]; 2],
    ) -> Result<()> {
        let base = match self.kind() {
            SurfaceKind::BSpline(s) => Some(s),
            // inherited from SmBSplineSurface, which reads m_pNurb: upstream's analytic
            // surfaces always hold one (the port makes it on demand, kind_with_nurb)
            SurfaceKind::Cone(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_) => Some(self.analytic_base_with_nurb(k)?),
            SurfaceKind::Plane(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_)
            | SurfaceKind::Step(_) => None,
        };
        if let Some(b) = base {
            self.check_evaluation_copy_depth(0)?;
            return b.evaluate_simple_first_into(
                uv,
                flags.u_side == Side::Left,
                flags.v_side == Side::Left,
                flags.only_upper_half,
                d,
            );
        }
        let r = self.evaluate_simple(k, uv, 1, 1, flags)?;
        for (row, src) in d.iter_mut().zip(&r) {
            for (slot, v) in row.iter_mut().zip(src) {
                *slot = *v;
            }
        }
        Ok(())
    }

    /// The UV direction that the tangent plane maps onto `vec` (its projection) at
    /// `uv` (`ComputeParamDirection`; not unitised). Outside the domain the point is
    /// clamped for the check unless out-of-bounds evaluation is enabled, but the
    /// evaluation uses `uv` as given (as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation (its other failures are ignored);
    /// those of [`vec_lin_comb_two_vectors`] (parallel first derivatives).
    pub fn compute_param_direction(&self, k: &Kernel, uv: Point2, vec: Vec3) -> Result<Vec2> {
        // clamp input when outOfBounds evals are disabled (the clamped point is only
        // checked; it always lies in the domain)
        let _domain = self.natural_uv_domain_with_nurb(k)?;

        // surface eval (status ignored)
        let derivs = match self.evaluate(k, uv, 1, 1, upper_flags(false)) {
            Ok(m) => first_block(&m),
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => [[Vec3::ZERO; 2]; 2],
        };

        // decompose the projection of crVec onto the tangent plane into Du and Dv
        // components
        vec_lin_comb_two_vectors(derivs[1][0], derivs[0][1], vec)
    }

    /// The position and the directional derivatives up to `highest_deriv` (at most 3)
    /// in the UV direction `uv_dir` (unitised) at `uv`
    /// (`EvaluateDirectionalDerivs`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `highest_deriv > 3`; those of the evaluation and of
    /// [`directional_derivs`].
    pub fn evaluate_directional_derivs(
        &self,
        k: &Kernel,
        uv: Point2,
        uv_dir: Vec2,
        highest_deriv: usize,
        u_from_left: bool,
        v_from_left: bool,
    ) -> Result<Vec<Vec3>> {
        evaluate_directional_derivs(
            k,
            self,
            uv,
            uv_dir,
            highest_deriv,
            side(u_from_left),
            side(v_from_left),
        )
    }

    // ---- domains --------------------------------------------------------------------

    /// The domain around `uv` that is at least `min_continuity` continuous
    /// (`GetDomainWithContinuity`): the STEP surface's override, else the base body,
    /// which shrinks the natural domain to the knots of lower continuity around `uv`,
    /// using `uv_vector` to pick a side at a discontinuity.
    ///
    /// # Errors
    ///
    /// Those of the surface cache (which upstream ignores: no cache signals and still
    /// reads it); the knot query errors are ignored, as upstream.
    pub fn domain_with_continuity(
        &self,
        k: &Kernel,
        min_continuity: ContinuityType,
        uv: Point2,
        uv_vector: Vec2,
    ) -> Result<Extent2d> {
        self.domain_with_continuity_in::<crate::surface::surface::p4::NoObject>(
            k,
            min_continuity,
            uv,
            uv_vector,
            None,
            None,
        )
    }

    /// [`Surface::domain_with_continuity`] through the caller's face-aware cache
    /// context: `handle` names this surface in the cache, so a surface owned by a
    /// face can reach that face when the continuity domain builds its cache.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::domain_with_continuity`] and of the context's cache
    /// preparation.
    pub fn domain_with_continuity_with_context<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        min_continuity: ContinuityType,
        uv: Point2,
        uv_vector: Vec2,
        context: &mut dyn SurfaceSolveContext<O>,
        handle: Option<O>,
    ) -> Result<Extent2d> {
        self.domain_with_continuity_in(k, min_continuity, uv, uv_vector, Some(context), handle)
    }

    fn domain_with_continuity_in<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        min_continuity: ContinuityType,
        uv: Point2,
        uv_vector: Vec2,
        context: Option<&mut dyn SurfaceSolveContext<O>>,
        handle: Option<O>,
    ) -> Result<Extent2d> {
        if let SurfaceKind::Step(s) = self.kind() {
            return s.domain_with_continuity();
        }
        // (no GetTree: the continuity arrays are empty unless the cache's tree was built)
        let cache = continuity_surface_cache(k, self, handle, context)?;
        self.domain_with_continuity_using(k, min_continuity, uv, uv_vector, &cache)
    }

    /// [`Surface::domain_with_continuity`] over a caller-held surface cache
    /// (`DomainWithContinuity`): upstream's `smsurf_GetSurfaceCache(this)` returns
    /// the one cache of the surface, including the trimmed cache of a face-owned
    /// surface. The continuity arrays come from that cache's tree build.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an unknown knot position.
    pub fn domain_with_continuity_from_cache<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        min_continuity: ContinuityType,
        uv: Point2,
        uv_vector: Vec2,
        cache: &SurfaceCache<O>,
    ) -> Result<Extent2d> {
        if let SurfaceKind::Step(s) = self.kind() {
            return s.domain_with_continuity();
        }
        self.domain_with_continuity_using(k, min_continuity, uv, uv_vector, cache)
    }

    fn domain_with_continuity_using<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        min_continuity: ContinuityType,
        uv: Point2,
        uv_vector: Vec2,
        cache: &SurfaceCache<O>,
    ) -> Result<Extent2d> {
        // Start out with natural domain and then shrink it if needed
        let ret_domain = self.natural_uv_domain_with_nurb(k)?;
        let mut min = ret_domain.min();
        let mut max = ret_domain.max();

        let mut vec = uv_vector;
        if vec.x.abs() < EFF_ZERO * 1000.00 {
            vec.x = 0.0;
        }
        if vec.y.abs() < EFF_ZERO * 1000.00 {
            vec.y = 0.0;
        }

        // For each dimension
        {
            let knots = self
                .knots_with_nurb(k, SurfParamType::U, None)
                .map_or_else(|_| Vec::new(), |k| k.0);
            let u_conts = cache.u_continuities();
            let n = u_conts.len();
            for (i, &cont) in u_conts.iter().enumerate() {
                if (cont as i32) < (min_continuity as i32) {
                    let ki = knot_at(&knots, i)?;
                    if i == 0 && vec.x < ki {
                        min.x = ki;
                    } else if i == n - 1 && vec.x > ki {
                        max.x = ki;
                    } else if (ki - uv.x).abs() < EFF_ZERO_SQRT {
                        if vec.x <= 0.0 && i > 0 {
                            // Right boundary
                            max.x = ki;
                            break;
                        }
                        // Left boundary
                        if i + 1 == knots.len() {
                            max.x = ki;
                        } else {
                            min.x = ki;
                        }
                    } else if ki < uv.x {
                        // Move up minimum
                        min.x = ki;
                    } else {
                        // dKnots[i] > crUV.x
                        max.x = ki;
                        break;
                    }
                }
            }
        }

        {
            let knots = self
                .knots_with_nurb(k, SurfParamType::V, None)
                .map_or_else(|_| Vec::new(), |k| k.0);
            let v_conts = cache.v_continuities();
            let n = v_conts.len();
            for (j, &cont) in v_conts.iter().enumerate() {
                if (cont as i32) < (min_continuity as i32) {
                    let kj = knot_at(&knots, j)?;
                    if j == 0 && vec.y < kj {
                        min.y = kj;
                    } else if j == n - 1 && vec.y > kj {
                        max.y = kj;
                    } else if (kj - uv.y).abs() < EFF_ZERO_SQRT {
                        // (upstream tests the unfiltered vector here)
                        if uv_vector.y <= 0.0 && j > 0 {
                            // Right boundary
                            max.y = kj;
                            break;
                        }
                        // Left boundary
                        if j + 1 == knots.len() {
                            max.y = kj;
                        } else {
                            min.y = kj;
                        }
                    } else if kj < uv.y {
                        // Move up minimum
                        min.y = kj;
                    } else {
                        // dKnots[j] > crUV.y
                        max.y = kj;
                        break;
                    }
                }
            }
        }

        Ok(Extent2d::from_points(min, max))
    }

    /// The NURBS domain of the STEP domain `anal_domain`
    /// (`ConvertDomainFromSTEPToNURBS`): the converted corners, ordered in both
    /// coordinates.
    ///
    /// # Errors
    ///
    /// Those of `SetMinMax`.
    pub fn convert_domain_from_step_to_nurbs(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
    ) -> Result<Extent2d> {
        let min_nurb_uv = self.convert_uv_from_step_to_nurbs(k, anal_domain.min())?;
        let max_nurb_uv = self.convert_uv_from_step_to_nurbs(k, anal_domain.max())?;

        // InsideOut reverses the generator direction. With SwapUV that direction is U,
        // not V, so order the converted endpoints in both coordinates.
        let mut d = Extent2d::new();
        d.set_min_max_coords(
            min_nurb_uv.x.min(max_nurb_uv.x),
            min_nurb_uv.y.min(max_nurb_uv.y),
            min_nurb_uv.x.max(max_nurb_uv.x),
            min_nurb_uv.y.max(max_nurb_uv.y),
        )?;
        Ok(d)
    }

    /// The STEP domain of the NURBS domain `nurb_domain`
    /// (`ConvertDomainFromNURBSToSTEP`): the converted corners, ordered in `v`.
    ///
    /// # Errors
    ///
    /// Those of the conversions; `SetMinMax`'s `InvalidInput` (`SM_ERR_INVALID_INPUT`:
    /// a reversed `u` interval); upstream then also sets the output to `nurb_domain`.
    pub fn convert_domain_from_nurbs_to_step(
        &self,
        k: &Kernel,
        nurb_domain: &Extent2d,
    ) -> Result<Extent2d> {
        convert_domain_from_nurbs_to_step_with(nurb_domain, |uv| {
            self.convert_uv_from_nurbs_to_step(k, uv)
        })
    }

    // ---- caches and sizes -----------------------------------------------------------

    /// The bounding boxes of the whole surface from its surface cache's top node
    /// (`CalculateBoundingBox`): the B-spline override for B-splines and planes, else
    /// the base body. `want` selects the boxes (`[normal, pseudo, polar]`); with
    /// `expand_pos_boxes_by_zone_tol_3d` the normal and pseudo boxes grow by a
    /// twentieth of the top box's bounding sphere radius. `uv_domain` and the basis
    /// guesses are not used by the base body.
    ///
    /// # Errors
    ///
    /// Those of the surface cache and of the polar box; [`SolidError::Unported`]:
    /// `SmFace::GetTolerance` (`topology.face_p2`) when a B-spline or plane with a
    /// face expands its boxes and the face's tolerance is not cached on the surface
    /// ([`Surface::owner_zone_tol_3d`]).
    pub fn calculate_bounding_box(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        want: [bool; 3],
        expand_pos_boxes_by_zone_tol_3d: bool,
    ) -> Result<SurfaceBoxes> {
        self.calculate_bounding_box_with_guesses(
            k,
            uv_domain,
            want,
            expand_pos_boxes_by_zone_tol_3d,
            None,
            None,
        )
    }

    /// [`Surface::calculate_bounding_box`] with upstream's optional basis guesses
    /// (`pOptPseudoBasisGuess`, `pOptPolarBasisGuess`): the B-spline override uses them,
    /// an offset surface passes them to its base surface, the base body ignores them.
    ///
    /// # Errors
    ///
    /// As [`Surface::calculate_bounding_box`].
    pub fn calculate_bounding_box_with_guesses(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        want: [bool; 3],
        expand_pos_boxes_by_zone_tol_3d: bool,
        pseudo_guess: Option<&PseudoBox>,
        polar_guess: Option<&PolarBox>,
    ) -> Result<SurfaceBoxes> {
        match self.kind() {
            // (SmSurfOfRevolution inherits the B-spline override)
            SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_) => {
                // the B-spline override expands by its face's tolerance (GetFace()):
                // the owner is held here, not by the B-spline, so the face's cached
                // tolerance is passed down (the GetBrep() arm is reached only without
                // a face, where a surface's GetBrep is null: SM_ZONE_TOL_3D)
                let expand_tolerance = if expand_pos_boxes_by_zone_tol_3d && (want[0] || want[1]) {
                    self.face_zone_tol_3d()?
                } else {
                    None
                };
                let req = crate::nurbs::b_spline_surface::BoundingBoxRequest {
                    normal: want[0],
                    pseudo: want[1],
                    polar: want[2],
                    pseudo_guess,
                    polar_guess,
                    expand_by_zone_tol: expand_pos_boxes_by_zone_tol_3d,
                    expand_tolerance,
                };
                let r = match self.kind() {
                    SurfaceKind::BSpline(s) => s.calculate_bounding_box(k, uv_domain, &req)?,
                    SurfaceKind::Plane(p) => plane_bounding_box(k, p, uv_domain, &req)?,
                    // (SmBSplineSurface::CalculateBoundingBox reads the NURB through
                    // GetOrCreateGwNurbPointer, which makes a missing one)
                    _ => self.analytic_base_with_nurb(k)?.calculate_bounding_box_of(
                        k,
                        uv_domain,
                        &req,
                        Some(self),
                    )?,
                };
                Ok(SurfaceBoxes {
                    normal_box: r.normal,
                    pseudo_box: r.pseudo,
                    polar_box: r.polar,
                })
            }
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) | SurfaceKind::SrfInVolume(_) => {
                self.base_calculate_bounding_box(k, want, expand_pos_boxes_by_zone_tol_3d)
            }
            // (the basis guesses are passed along to the base surface's boxes)
            SurfaceKind::OffsetSurface(s) => s.calculate_bounding_box_with(
                k,
                self,
                uv_domain,
                want,
                expand_pos_boxes_by_zone_tol_3d,
                |base| {
                    base.calculate_bounding_box_with_guesses(
                        k,
                        uv_domain,
                        [true, true, true],
                        expand_pos_boxes_by_zone_tol_3d,
                        pseudo_guess,
                        polar_guess,
                    )
                },
            ),
        }
    }

    /// [`Surface::natural_uv_domain_if_built`] as upstream's `GetNaturalUVDomain`, which makes a
    /// missing NURB (`GetOrCreateGwNurbPointer`): a lazy cone, sphere, torus or surface
    /// of revolution reads the domain of the NURB its `MakeNurb` gives
    /// ([`Surface::kind_with_nurb`]), where [`Surface::natural_uv_domain_if_built`] answers the
    /// NURB-less base's empty domain; an offset surface reads its base's (or extended
    /// surface's) the same way ([`OffsetSurface::natural_uv_domain_with_nurb`]), and so
    /// does a lazy surface of extrusion (`SmSurfOfExtrusion` does not override
    /// `GetNaturalUVDomain`: its made NURB's domain, whatever the generator's kind).
    /// A curve bounded surface and a surface in a volume pass the call to their basis /
    /// base surface, and a STEP surface answers its base's STEP domain
    /// ([`Surface::step_uv_domain_with_nurb`]), so a lazy analytic under them is made
    /// as upstream's virtual calls make it. Every other kind answers as
    /// [`Surface::natural_uv_domain_if_built`].
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the make; `InvalidInput` for a curve bounded, STEP
    /// or volume chain deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn natural_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d> {
        self.natural_uv_domain_with_nurb_at(k, 0)
    }

    /// [`Surface::step_uv_domain`] as upstream's `GetSTEPUVDomain`, whose base default
    /// is the virtual `GetNaturalUVDomain` ([`Surface::natural_uv_domain_with_nurb`]):
    /// the kinds with their own STEP domain answer it, a surface in a volume its base's,
    /// the others their natural domain with a lazy NURB made.
    ///
    /// # Errors
    ///
    /// As [`Surface::natural_uv_domain_with_nurb`].
    pub fn step_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d> {
        self.step_uv_domain_with_nurb_at(k, 0)
    }

    /// [`Surface::step_uv_domain_with_nurb`] at chain depth `depth`.
    fn step_uv_domain_with_nurb_at(&self, k: &Kernel, depth: usize) -> Result<Extent2d> {
        Ok(match self.kind() {
            SurfaceKind::Plane(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_) => self.step_uv_domain(),
            SurfaceKind::SrfInVolume(s) => s.surface().step_uv_domain_with_nurb_at(
                k,
                nesting_depth(depth, "surface::surface::p1::step_uv_domain_with_nurb")?,
            )?,
            SurfaceKind::BSpline(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_) => self.natural_uv_domain_with_nurb_at(k, depth)?,
        })
    }

    /// [`Surface::natural_uv_domain_with_nurb`] at chain depth `depth`.
    fn natural_uv_domain_with_nurb_at(&self, k: &Kernel, depth: usize) -> Result<Extent2d> {
        const OP: &str = "surface::surface::p1::natural_uv_domain_with_nurb";
        Ok(match self.kind_with_nurb(k)? {
            SurfaceKind::CurveBounded(s) => s
                .basis()
                .natural_uv_domain_with_nurb_at(k, nesting_depth(depth, OP)?)?,
            SurfaceKind::SrfInVolume(s) => s
                .surface()
                .natural_uv_domain_with_nurb_at(k, nesting_depth(depth, OP)?)?,
            SurfaceKind::Step(s) => match s.surface() {
                Some(base) => base.step_uv_domain_with_nurb_at(k, nesting_depth(depth, OP)?)?,
                None => s.natural_uv_domain(),
            },
            SurfaceKind::Cone(s) => s.rev().base().natural_uv_domain(),
            SurfaceKind::SurfOfRevolution(s) => s.base().natural_uv_domain(),
            SurfaceKind::Sphere(s) => s.base().base().natural_uv_domain(),
            SurfaceKind::Torus(s) => s.base().base().natural_uv_domain(),
            SurfaceKind::OffsetSurface(s) => s.natural_uv_domain_with_nurb(k)?,
            // (the made kind: its base holds the NURB unless the make failed)
            SurfaceKind::SurfOfExtrusion(s) => s.natural_uv_domain_for_query(),
            // (the other kinds are never lazy)
            _ => self.natural_uv_domain_if_built(),
        })
    }

    /// [`Surface::natural_uv_domain_with_nurb`] for the port's stand-in object key of
    /// this surface (where upstream names `this` in a solution, and reads its natural
    /// domain only if and when it compares objects): a lazy plane's, cone's, sphere's,
    /// torus's or surface of revolution's domain is read from a NURB made on a copy, so
    /// taking the key does not make and keep the surface's NURB ahead of upstream (a
    /// solver built next tests whether the surface has one).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::natural_uv_domain_with_nurb`].
    pub(crate) fn stand_in_key_domain(&self, k: &Kernel) -> Result<Extent2d> {
        match self.kind() {
            SurfaceKind::Plane(p) => Ok(p.natural_uv_domain_without_keeping()),
            _ => match self.revolution_natural_uv_domain_without_keeping(k)? {
                Some(domain) => Ok(domain),
                None => self.natural_uv_domain_with_nurb(k),
            },
        }
    }

    /// The B-spline part of an analytic kind with its NURB (`GetOrCreateGwNurbPointer`
    /// from a `const` method: [`Surface::kind_with_nurb`], which makes a missing NURB
    /// once and keeps it on the surface).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a kind that is not analytic;
    /// [`SolidError::Unported`] from the make.
    pub(crate) fn analytic_base_with_nurb(&self, k: &Kernel) -> Result<&BSplineSurface> {
        Ok(match self.kind_with_nurb(k)? {
            SurfaceKind::Cone(s) => s.rev().base(),
            SurfaceKind::Sphere(s) => s.base().base(),
            SurfaceKind::Torus(s) => s.base().base(),
            SurfaceKind::SurfOfRevolution(s) => s.base(),
            SurfaceKind::SurfOfExtrusion(s) => s.base(),
            _ => return unreachable_kind(),
        })
    }

    /// The base body of [`Surface::calculate_bounding_box`].
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when no surface cache can be made (upstream's `NER` on
    /// `smsurf_GetSurfaceCache`; [`SolidError::Unported`] from it passes through); those
    /// of the polar box; `SM_ERR_NULL_POINTER` (`InvalidInput`) for a top node without
    /// surface data.
    pub fn base_calculate_bounding_box(
        &self,
        k: &Kernel,
        want: [bool; 3],
        expand_pos_boxes_by_zone_tol_3d: bool,
    ) -> Result<SurfaceBoxes> {
        // a polar box asks the cache's top node for it, whose build asks this
        // surface's CalculateBoundingBox again: upstream recurses without bound when
        // that is this base body (curve-bounded and STEP surfaces), or for an offset
        // surface whose natural (extended) domain reaches past its base domain (its
        // override then runs this body again for the nodes outside)
        let recurses = match self.kind() {
            SurfaceKind::OffsetSurface(s) => !self
                .natural_uv_domain_with_nurb(k)?
                .is_contained_by(&s.base_surface().natural_uv_domain_with_nurb(k)?, EFF_ZERO),
            _ => true,
        };
        if want[2] && recurses {
            return Err(SolidError::invalid_input(OP_BOX, POLAR_RECURSION));
        }

        // get or create SurfaceCache with Subdivision Tree containing Bounding boxes
        // (smsurf_GetSurfaceCache returns NULL when no cache can be made: NER)
        let mut cache = match get_surface_cache::<()>(k, self, None) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(OP_BOX)),
        };
        let top = cache
            .get_tree(k)?
            .top_node()
            .ok_or(SolidError::invalid_input(
                OP_BOX,
                "surface tree without top node",
            ))?;
        let (bbox, aux_type) = {
            let tree = cache.get_tree(k)?;
            let node = tree.node(top)?;
            (node.bbox, node.aux_type)
        };
        let mut out = SurfaceBoxes::default();

        // set the normal box = cached normal box increased by a small amount
        if want[0] {
            let mut normal_box = bbox;
            // Expand it a little just to be safe
            if expand_pos_boxes_by_zone_tol_3d {
                let radius = normal_box.sphere_bound().radius;
                normal_box.expand_absolute(radius / 20.0);
            }
            out.normal_box = Some(normal_box);
        }

        // set the psuedo box = cached pseudo box increased by a small amount
        if want[1] {
            if aux_type == AuxDataType::BezierSurface {
                let mut pseudo = *cache.patch(top)?.pseudo_box();
                if expand_pos_boxes_by_zone_tol_3d {
                    let radius = bbox.sphere_bound().radius;
                    pseudo.expand_absolute(radius / 20.0);
                }
                out.pseudo_box = Some(pseudo);
            } else {
                let min_point = bbox.min();
                let max_point = bbox.max();
                let mut pseudo = PseudoBox::new();
                pseudo.set_basis(
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                )?;
                pseudo.set_intervals(
                    Extent1d::from_min_max(min_point.x, max_point.x),
                    Extent1d::from_min_max(min_point.y, max_point.y),
                    Extent1d::from_min_max(min_point.z, max_point.z),
                );
                out.pseudo_box = Some(pseudo);
            }
        }

        // set the polar box = cached polar box increased by a small amount
        if want[2] {
            out.polar_box = Some(cache.polar_box(k, top)?);
        }
        Ok(out)
    }

    /// The approximate lengths of the mid iso curves (`ApproximateSize`): `x` the
    /// length of the `v` iso curve through the midpoint of `domain` (default the
    /// natural domain), `y` that of the `u` iso curve (`ApproximateLength` with 5
    /// samples).
    ///
    /// # Errors
    ///
    /// Those of the evaluation copy and of the length.
    pub fn approximate_size(&self, k: &Kernel, domain: Option<&Extent2d>) -> Result<Vec2> {
        let dom = match domain {
            Some(d) => *d,
            None => self.natural_uv_domain_with_nurb(k)?,
        };
        let param = 0.5;
        let uv_mid = dom.evaluate(param, param);

        // upstream's SmIsoCurve holds the surface itself and evaluates through its
        // virtual Evaluate; a B-spline keeps the plain copy it always used
        let iso = |param: SurfParamType, value: f64| -> Result<Curve> {
            Ok(Curve::from(match self.kind() {
                SurfaceKind::BSpline(bs) => IsoCurve::new(bs.clone(), param, value, true, true),
                _ => IsoCurve::from_surface(
                    k,
                    self.clone_for_evaluation()?,
                    param,
                    value,
                    true,
                    true,
                )?,
            }))
        };

        let iso1 = iso(SurfParamType::U, uv_mid.x)?;
        let size_v = iso1
            .approximate_length(k, &iso1.natural_interval(), 5, false)?
            .0;

        let iso2 = iso(SurfParamType::V, uv_mid.y)?;
        let size_u = iso2
            .approximate_length(k, &iso2.natural_interval(), 5, false)?
            .0;

        Ok(Vec2::new(size_u, size_v))
    }

    /// The offset of this surface by `signed_offset_distance` (`CreateOffsetSurface`,
    /// virtual): a plane moves a copy along its normal, a surface of revolution offsets
    /// its generator within `approx_tol_3d`, a cone offsets its radii within
    /// `approx_tol_3d`, an offset surface offsets its base by the compound distance;
    /// the other kinds run the base body (also what `SmBSplineSurface`'s override
    /// calls), an `SmOffsetSurface` that owns a copy of this surface (default
    /// singularities). A fully collapsed cone offset succeeds with no surface
    /// (`Ok(None)`, upstream's `SM_OK` with a `NULL` output).
    ///
    /// # Errors
    ///
    /// Those of the overrides, of [`Surface::copy`] and of [`OffsetSurface::new`].
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        approx_tol_3d: f64,
    ) -> Result<Option<Self>> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => Ok(Some(Self::from(
                p.create_offset_surface(k, signed_offset_distance)?,
            ))),
            // `Kernel::fixes.revolution_round_offset_collapses` (WR-567): an offset that
            // shrinks the generatrix arc onto its centre or past it is no surface
            SurfaceKind::SurfOfRevolution(s)
                if k.fixes.revolution_round_offset_collapses
                    && self.revolution_offset_collapses(
                        k,
                        s,
                        signed_offset_distance,
                        approx_tol_3d,
                    )? =>
            {
                Ok(None)
            }
            SurfaceKind::SurfOfRevolution(s) => Ok(Some(Self::from(s.create_offset_surface(
                k,
                signed_offset_distance,
                approx_tol_3d,
            )?))),
            SurfaceKind::Cone(s) => Ok(s
                .create_offset_surface(k, signed_offset_distance, approx_tol_3d)?
                .map(Self::from)),
            SurfaceKind::Sphere(s) => Ok(s
                .create_offset_surface(k, signed_offset_distance, approx_tol_3d)?
                .map(Self::from)),
            SurfaceKind::OffsetSurface(s) => {
                s.create_offset_surface(k, signed_offset_distance).map(Some)
            }
            SurfaceKind::Torus(s) => Ok(s
                .create_offset_surface(k, signed_offset_distance, approx_tol_3d)?
                .map(Self::from)),
            // (SmSurfOfExtrusion's override is commented out upstream)
            SurfaceKind::BSpline(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::SrfInVolume(_) => {
                base_create_offset_surface(k, self, signed_offset_distance).map(Some)
            }
        }
    }

    /// Not upstream (`SmSurface::OffsetCollapses`, WR-464): whether the offset by
    /// `signed_offset_distance` shrinks a cone, cylinder, sphere or torus onto its spine
    /// (radius below `tol`), the only case where [`Self::create_offset_surface`] gives no
    /// surface. With `Kernel::fixes.revolution_round_offset_collapses` (WR-567) also a
    /// surface of revolution of one circular arc moved in past the arc's radius
    /// (`revolution_offset_collapses`). `false` for every other surface.
    ///
    /// # Errors
    ///
    /// Those of the NURB conversion of the kind.
    pub fn offset_collapses(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        tol: f64,
    ) -> Result<bool> {
        Ok(match self.kind_with_nurb(k)? {
            SurfaceKind::Cone(s) => s.offset_collapses(k, signed_offset_distance, tol),
            SurfaceKind::Sphere(s) => s.offset_collapses(signed_offset_distance, tol),
            SurfaceKind::Torus(s) => s.offset_collapses(signed_offset_distance, tol),
            SurfaceKind::SurfOfRevolution(s) if k.fixes.revolution_round_offset_collapses => {
                self.revolution_offset_collapses(k, s, signed_offset_distance, tol)?
            }
            _ => false,
        })
    }

    /// Not upstream (the fork's `SmSurfOfRevolution::OffsetCollapses`, WR-567): whether
    /// the offset by `signed_offset_distance` shrinks this surface of revolution `s`
    /// onto its spine or past it. Its generatrix is one circular arc of radius `r`
    /// (`IsArc(9, tol)`, `r > tol`; a fillet's round along a circular edge), and the
    /// offset leaves a radius `r - d (N . k) r` below `tol`, with `k` the curvature
    /// vector of the generatrix-direction iso-curve (the arc revolved: `|k| = 1/r`,
    /// pointing at the arc's centre) and `N` the surface normal, both at the middle of
    /// the natural UV domain. A failed arc test or evaluation, or a pole there, is
    /// `false`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the arc test or the evaluations; those of the
    /// natural UV domain.
    fn revolution_offset_collapses(
        &self,
        k: &Kernel,
        s: &crate::surface::surf_of_revolution::SurfOfRevolution,
        signed_offset_distance: f64,
        tol: f64,
    ) -> Result<bool> {
        // a failure upstream answers FALSE (except an unported call here)
        fn or_false<T>(r: Result<T>) -> Result<Option<T>> {
            match r {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(None),
            }
        }
        let Some(gen) = s.gen_curve() else {
            return Ok(false);
        };
        let Some(Some((_, radius, _, _))) = or_false(gen.is_arc(k, 9, tol))? else {
            return Ok(false);
        };
        if radius <= tol {
            return Ok(false);
        }

        // the curvature of the generatrix direction's iso-curve, at the middle of the domain
        let uv = self.natural_uv_domain_with_nurb(k)?.mid();
        let Some([_, du, dv, _, duu, dvv]) =
            or_false(self.evaluate_2nd_derivatives(k, uv, true, true))?
        else {
            return Ok(false);
        };
        let Some(normal) = or_false(self.evaluate_normal(k, uv, side(true), side(true)))? else {
            return Ok(false);
        };
        let (d1, d2) = if s.swap_uv() { (du, duu) } else { (dv, dvv) };
        let speed_sq = d1.length_squared();
        if speed_sq < EFF_ZERO_SQ {
            // at a pole: no generatrix tangent to measure
            return Ok(false);
        }
        let curvature = (d2 - d1 * (d2.dot(d1) / speed_sq)) / speed_sq;

        // radius r - d (N . k) r: N . k r is +1 where the normal points at the centre, -1
        // away from it
        let towards_centre = normal.dot(curvature) * radius;
        Ok(radius - signed_offset_distance * towards_centre < tol)
    }

    /// The silhouette curves of the surface over `uv_domain` for a parallel
    /// (`eye_point_or_view_vector` the view vector) or perspective projection
    /// (`CreateSilhouetteCurves`), with the walk tolerances `pOptApproxTol3d` and
    /// `cpdOptAngleTolRadians`: the 3D and UV curves the silhouette tracer
    /// ([`SurfaceSilhouette`]) finds. For a parallel projection each curve is then split
    /// at its interior points with a tangent parallel to the view vector
    /// (`GlobalPropertyAnalysis(SM_CP_PARALLEL_TO_VECTOR)` within
    /// `100 * SM_EFF_ZERO_SQRT`): the leading pieces are trimmed copies appended after
    /// the traced curves, the traced curve keeps the last piece; its UV curve is split
    /// at the same (snapped) intervals.
    ///
    /// Upstream also adds the surface's `SM_AI_COLOR` attribute to each traced curve of
    /// a parallel projection; a free [`Surface`] or [`Curve`] carries no attribute list
    /// here, so a caller holding the surface's attributes copies it.
    ///
    /// # Errors
    ///
    /// Those of the surface cache, of the trace and of the property analysis;
    /// `SM_ERR_NULL_POINTER` for a traced curve that is not a B-spline (upstream's
    /// `SM_CAST_PTR` null) or a missing UV curve. The trims' statuses are ignored, as
    /// upstream (but [`SolidError::Unported`] is propagated).
    pub fn create_silhouette_curves(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        eye_point_or_view_vector: Point3,
        perspective: bool,
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<(Vec<Curve>, Vec<Curve>)> {
        let mut cache = get_surface_cache::<SilhouetteObject>(k, self, None)?;
        self.create_silhouette_curves_with_cache(
            k,
            &mut cache,
            uv_domain,
            eye_point_or_view_vector,
            perspective,
            approx_tol_3d,
            ang_tol_rad,
        )
    }

    /// [`Surface::create_silhouette_curves`] using an already-resolved surface cache
    /// (`smsurf_GetSurfaceCache(this)` in callers holding the owning face's cache).
    ///
    /// # Errors
    ///
    /// As [`Surface::create_silhouette_curves`].
    pub fn create_silhouette_curves_with_cache<O: SolutionObject>(
        &self,
        k: &Kernel,
        cache: &mut SurfaceCache<O>,
        uv_domain: &Extent2d,
        eye_point_or_view_vector: Point3,
        perspective: bool,
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<(Vec<Curve>, Vec<Curve>)> {
        const OP: &str = "surface::surface::p1::create_silhouette_curves";

        // test to see if this surface has color: not modelled (see the doc comment)

        // Check to make sure internal surface continuities are better then C0 (the
        // check only signals upstream)
        let _ = (cache.surface_continuity(uv_domain)? as i32) <= (ContinuityType::C0 as i32);

        let mut silhouette =
            SurfaceSilhouette::new(k, self, uv_domain, perspective, eye_point_or_view_vector)?;
        // (the tracer constructor's pSC->GetTree(), whose failure it ignores)
        if let Err(e) = cache.get_tree(k) {
            if e.is_unported() {
                return Err(e);
            }
        }
        let traced = silhouette.do_trace(k, cache, approx_tol_3d, ang_tol_rad)?;
        let mut curves_3d = traced.curves_3d;
        let mut curves_uv = traced.uv_curves;

        // Post process silhouettes and break them up where the curve is parallel to
        // the view vector.
        if !perspective {
            let mut new_curves: Vec<Curve> = Vec::new();
            let mut new_uv_curves: Vec<Curve> = Vec::new();
            let vectors = [eye_point_or_view_vector];
            for (ii, curve) in curves_3d.iter_mut().enumerate() {
                // SM_CAST_PTR(SmBSplineCurve, ..): NULL (then dereferenced) otherwise
                if !matches!(curve.kind(), CurveKind::BSpline(_)) {
                    return Err(SolidError::null_pointer(OP));
                }

                // To add color attribute for curve from color of surface: not modelled

                let mut ivl = curve.natural_interval();
                // the solutions name the curve
                let opts = PropertyAnalysisOptions {
                    property_value: None,
                    opt_vectors: &vectors,
                    want_comb: false,
                    handle: Some(SilhouetteCurve {
                        natural_interval: ivl,
                    }),
                };
                let solutions = curve
                    .global_property_analysis(
                        k,
                        &ivl,
                        CurvePropertyType::ParallelToVector,
                        EFF_ZERO_SQRT * 100.0,
                        &opts,
                    )?
                    .solutions;

                for sol in solutions.as_slice() {
                    let param = sol.start.param(0)?;
                    if param < ivl.min() + EFF_ZERO_SQRT || param > ivl.max() - EFF_ZERO_SQRT {
                        continue;
                    }

                    let mut new_bsc = curve.clone();
                    let mut trim1 = Extent1d::from_min_max(param, ivl.max());
                    let mut trim2 = Extent1d::from_min_max(ivl.min(), param);
                    // may snap sIvl by tol to existing knots
                    ignore_status(new_bsc.trim(k, &mut trim2, TrimMode::Notify))?;

                    new_curves.push(new_bsc);
                    // may snap sIvl by tol to existing knots
                    ignore_status(curve.trim(k, &mut trim1, TrimMode::Notify))?;

                    let uv_curve = curves_uv.get_mut(ii).ok_or(SolidError::null_pointer(OP))?;
                    if !matches!(uv_curve.kind(), CurveKind::BSpline(_)) {
                        return Err(SolidError::null_pointer(OP));
                    }
                    let mut new_uv_bsc = uv_curve.clone();
                    // may snap sIvl by tol to existing knots
                    ignore_status(new_uv_bsc.trim(k, &mut trim2, TrimMode::Notify))?;
                    new_uv_curves.push(new_uv_bsc);
                    // may snap sIvl by tol to existing knots
                    ignore_status(uv_curve.trim(k, &mut trim1, TrimMode::Notify))?;
                    ivl = trim1;
                }
            }
            curves_3d.append(&mut new_curves);
            curves_uv.append(&mut new_uv_curves);
        }

        Ok((curves_3d, curves_uv))
    }

    // ---- shape tests ----------------------------------------------------------------

    /// Whether the surface is periodic across its `surf_param` boundaries of
    /// `uv_domain` (`IsPeriodic`): at six interior samples, equal positions (to the
    /// scaled zero) and tangent directions on both sides of the possible seam (to the
    /// angle tolerance, checked again unitised).
    ///
    /// Upstream ignores the evaluation statuses: a failed evaluation leaves what it
    /// wrote (for a collapsed B-spline, the raw derivatives its zero-tangent repair
    /// could not fix) or, failing before writing, the previous sample's values
    /// (initially undefined points), and the tests go on with those.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::evaluate_ignoring_status`]. A failing `Unitize` returns
    /// `true`, as upstream's `SER` inside a boolean function.
    pub fn is_periodic(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        surf_param: SurfParamType,
    ) -> Result<bool> {
        const NUM_TEST_POINTS: u32 = 7;

        // locals
        let ang_tol_rad_sq = ANG_TOL_RAD * ANG_TOL_RAD;
        let mut p_min = [[Vec3::UNDEF; 2]; 2];
        let mut p_max = [[Vec3::UNDEF; 2]; 2];

        // for every internal sample points (don't do end points)
        for ii in 1..NUM_TEST_POINTS {
            // Place sUVMin and sUVMax on either side of the possible seam
            let norm_param = f64::from(ii) / f64::from(NUM_TEST_POINTS);
            let uv = uv_domain.evaluate(norm_param, norm_param);
            let mut uv_min = uv;
            let mut uv_max = uv;
            if surf_param == SurfParamType::U {
                uv_min.x = uv_domain.min().x;
                uv_max.x = uv_domain.max().x;
            } else {
                // eSurfParam == SM_SP_V
                uv_min.y = uv_domain.min().y;
                uv_max.y = uv_domain.max().y;
            }

            // surface evaluations on either side of the possible seam (statuses ignored)
            if let Some(d) = self.evaluate_ignoring_status(k, uv_min, 1, 1, upper_flags(true))? {
                p_min = first_block(&d);
            }
            if let Some(d) = self.evaluate_ignoring_status(k, uv_max, 1, 1, upper_flags(true))? {
                p_max = first_block(&d);
            }

            let scaled_zero = p_max[0][0].scaled_zero();

            // Check position difference.
            if (p_min[0][0] - p_max[0][0]).length_squared() > scaled_zero {
                return Ok(false);
            }

            // Check tangent-vector directions, in V, then in U.
            for (i, j) in [(0, 1), (1, 0)] {
                let (d_min, d_max) = (block_at(&p_min, i, j), block_at(&p_max, i, j));
                if (d_min - d_max).length_squared() > ang_tol_rad_sq {
                    // Do G1 test
                    let (Ok(a), Ok(b)) = (d_min.unitize(), d_max.unitize()) else {
                        return Ok(true);
                    };
                    if (a - b).length_squared() > ang_tol_rad_sq {
                        return Ok(false);
                    }
                }
            }
        } // end iter every internal sample point

        // all done
        Ok(true) // passed all tests
    }

    /// Whether the surface over `uv_domain` (default the natural domain) degenerates
    /// to a point (`IsDegeneratePoint`): the B-spline override for B-splines and
    /// planes, else [`base_is_degenerate_point`].
    ///
    /// # Errors
    ///
    /// Those of the evaluations.
    pub fn is_degenerate_point(
        &self,
        k: &Kernel,
        tol_3d: f64,
        uv_domain: Option<&Extent2d>,
    ) -> Result<bool> {
        let natural = self.natural_uv_domain_with_nurb(k)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => s.is_degenerate_point(k, tol_3d, uv_domain),
            // inherited from SmBSplineSurface
            SurfaceKind::Cone(s) => {
                if uv_domain.is_some_and(|d| !d.are_equal(&natural, EFF_ZERO)) {
                    base_is_degenerate_point(k, self, tol_3d, uv_domain)
                } else {
                    s.rev().base().is_degenerate_point(k, tol_3d, uv_domain)
                }
            }
            SurfaceKind::SurfOfRevolution(s) => s.base().is_degenerate_point(k, tol_3d, uv_domain),
            // (`GetNaturalUVDomain` or the explicit `MakeNurb` make a recognised
            // extrusion's NURB first)
            SurfaceKind::SurfOfExtrusion(s) => s
                .base_with_nurb(k)?
                .is_degenerate_point(k, tol_3d, uv_domain),
            SurfaceKind::Sphere(s) => s.base().base().is_degenerate_point(k, tol_3d, uv_domain),
            SurfaceKind::Torus(s) => s.base().base().is_degenerate_point(k, tol_3d, uv_domain),
            SurfaceKind::Plane(p) => p.base_for_query().is_degenerate_point(k, tol_3d, uv_domain),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_is_degenerate_point(k, self, tol_3d, uv_domain),
        }
    }

    /// Whether the surface over `uv_domain` (default the natural domain) degenerates
    /// to a curve (`IsDegenerateCurve`): the B-spline override for B-splines and
    /// planes, else [`base_is_degenerate_curve`].
    ///
    /// # Errors
    ///
    /// Those of the evaluations.
    pub fn is_degenerate_curve(
        &self,
        k: &Kernel,
        angle_tol_deg: f64,
        uv_domain: Option<&Extent2d>,
        opt_3d_tol: f64,
    ) -> Result<bool> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => {
                s.is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            // inherited from SmBSplineSurface
            SurfaceKind::SurfOfRevolution(s) => {
                s.base()
                    .is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            // (the base body's `GetNaturalUVDomain` and evaluations make a recognised
            // extrusion's NURB)
            SurfaceKind::SurfOfExtrusion(s) => {
                s.base_with_nurb(k)?
                    .is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            SurfaceKind::Sphere(s) => {
                s.base()
                    .base()
                    .is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            SurfaceKind::Torus(s) => {
                s.base()
                    .base()
                    .is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            SurfaceKind::Plane(p) => {
                p.base_for_query()
                    .is_degenerate_curve(k, angle_tol_deg, uv_domain, opt_3d_tol)
            }
            SurfaceKind::Cone(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => {
                base_is_degenerate_curve(k, self, angle_tol_deg, uv_domain, opt_3d_tol)
            }
        }
    }

    /// Whether `uv` is a flat corner: non-degenerate first derivatives that are
    /// parallel to `angle_tol_deg` (`IsFlatCorner`).
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn is_flat_corner(&self, k: &Kernel, uv: Point2, angle_tol_deg: f64) -> Result<bool> {
        // Surface Eval
        let d = first_block(&self.evaluate(k, uv, 1, 1, upper_flags(false))?);

        // 1st Derivatives
        let du = d[1][0];
        let dv = d[0][1];

        // check - degenerate 1st derivatives
        if crate::geom::tol::is_degenerate_vec3(k, du, None)
            || crate::geom::tol::is_degenerate_vec3(k, dv, None)
        {
            return Ok(false);
        }

        // Let the point be a FlatCorner when sNorm3d is degenerate
        Ok(du.is_parallel_to(dv, angle_tol_deg))
    }

    /// Whether `uv` lies on a boundary of `uv_domain` within the natural domain, to
    /// `tol_3d` mapped to UV space (default the surface's zone tolerance)
    /// (`IsOnBoundary`): the side ([`SurfParamType::U`], [`SurfParamType::V`],
    /// [`SurfParamType::Both`] or [`SurfParamType::Neither`]) and the boundary values
    /// in `u` and `v` (`None` off those boundaries).
    ///
    /// # Errors
    ///
    /// Those of the tolerances.
    pub fn is_on_boundary(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        uv_point: Point2,
        tol_3d: Option<f64>,
    ) -> Result<(bool, SurfParamType, Option<f64>, Option<f64>)> {
        // pick a tolerance
        let tol_3d = match tol_3d {
            Some(t) => t,
            None => zone_tol_3d(k, self)?,
        };

        let tol_2d_u = map_to_2d_dir(k, tol_3d, uv_point, Vec2::new(1.0, 0.0), self)?;
        let tol_2d_v = map_to_2d_dir(k, tol_3d, uv_point, Vec2::new(0.0, 1.0), self)?;

        // test point against boundaries
        let (b_u, o_u) = uv_domain
            .u_interval()
            .is_value_on_boundary(uv_point.x, tol_2d_u);
        let (b_v, o_v) = uv_domain
            .v_interval()
            .is_value_on_boundary(uv_point.y, tol_2d_v);

        let param_side = match (b_u, b_v) {
            (true, true) => SurfParamType::Both,
            (true, false) => SurfParamType::U,
            (false, true) => SurfParamType::V,
            (false, false) => SurfParamType::Neither,
        };

        // return boundary value while making sure point is in natural domain
        let natural = self.natural_uv_domain_with_nurb(k)?;
        let on = param_side != SurfParamType::Neither
            && natural.u_interval().contains_value(uv_point.x, tol_2d_u)
            && natural.v_interval().contains_value(uv_point.y, tol_2d_v);
        Ok((on, param_side, b_u.then_some(o_u), b_v.then_some(o_v)))
    }

    /// Whether `uv_point` lies in the natural domain to `tol_3d` mapped to UV space
    /// (default the surface's zone tolerance), or in `uv_domain` to the numerical
    /// tolerance (`IsInDomain`).
    ///
    /// # Errors
    ///
    /// Those of the tolerances.
    pub fn is_in_domain(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        uv_point: Point2,
        tol_3d: Option<f64>,
    ) -> Result<bool> {
        // low work - inside to numerical tolerance
        if uv_domain.contains_point(uv_point, EFF_ZERO) {
            return Ok(true);
        }

        // pick a tolerance
        let tol_3d = match tol_3d {
            Some(t) => t,
            None => zone_tol_3d(k, self)?,
        };

        let tol_2d_u = map_to_2d_dir(k, tol_3d, uv_point, Vec2::new(1.0, 0.0), self)?;
        let tol_2d_v = map_to_2d_dir(k, tol_3d, uv_point, Vec2::new(0.0, 1.0), self)?;

        // return boundary value while making sure point is in natural domain
        let natural = self.natural_uv_domain_with_nurb(k)?;
        Ok(natural.u_interval().contains_value(uv_point.x, tol_2d_u)
            && natural.v_interval().contains_value(uv_point.y, tol_2d_v))
    }

    /// Whether the surface is one of the supported analytic kinds: a plane, a surface
    /// of extrusion or of revolution (`IsAnalytic`).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::is_kind_of`].
    pub fn is_analytic(&self) -> Result<bool> {
        Ok(self.is_kind_of(super::p2::PLANE_TYPE)?
            || self.is_kind_of(SURF_OF_EXTRUSION_TYPE)?
            || self.is_kind_of(SURF_OF_REVOLUTION_TYPE)?)
    }

    /// `uv_original` with each coordinate moved to the nearest knot closer than
    /// `uv_snap_tolerance` (`SnapToKnots`). Its virtual `GetKnots` makes a lazy
    /// surface's NURB first ([`Surface::knots_with_nurb`]).
    ///
    /// # Errors
    ///
    /// Those of the knot queries.
    pub fn snap_to_knots(
        &self,
        k: &Kernel,
        uv_original: Point2,
        uv_snap_tolerance: f64,
    ) -> Result<Point2> {
        let mut uv_snapped = uv_original;

        let mut min_snap_dist = BIG_DOUBLE;
        for &knot in &self.knots_with_nurb(k, SurfParamType::U, None)?.0 {
            let snap_dist = (knot - uv_original.x).abs();
            if snap_dist < min_snap_dist && snap_dist < uv_snap_tolerance {
                uv_snapped.x = knot;
                min_snap_dist = snap_dist;
            }
        }

        min_snap_dist = BIG_DOUBLE;
        for &knot in &self.knots_with_nurb(k, SurfParamType::V, None)?.0 {
            let snap_dist = (knot - uv_original.y).abs();
            if snap_dist < min_snap_dist && snap_dist < uv_snap_tolerance {
                uv_snapped.y = knot;
                min_snap_dist = snap_dist;
            }
        }
        Ok(uv_snapped)
    }
}

/// `knots[i]`, or `InvalidInput` where upstream reads past its knot array (a
/// continuity array longer than the knots).
fn knot_at(knots: &[f64], i: usize) -> Result<f64> {
    knots.get(i).copied().ok_or(SolidError::invalid_input(
        "surface::surface::p1::Surface::domain_with_continuity",
        "fewer knots than continuities",
    ))
}

/// `SmBSplineSurface::CalculateBoundingBox` on an `SmPlane`: the B-spline body on the
/// plane's B-spline part for the normal and pseudo boxes; the polar box comes from the
/// virtual `EvaluateNormal`, the plane's axis (`SmPlane::EvaluateNormal`), so it is the
/// box of that one vector (planar and analytic: no basis guess).
///
/// # Errors
///
/// Those of the B-spline body.
pub(crate) fn plane_bounding_box(
    k: &Kernel,
    plane: &Plane,
    uv_domain: &Extent2d,
    req: &crate::nurbs::b_spline_surface::BoundingBoxRequest<'_>,
) -> Result<crate::nurbs::b_spline_surface::BoundingBoxes> {
    let base_req = crate::nurbs::b_spline_surface::BoundingBoxRequest {
        polar: false,
        polar_guess: None,
        ..*req
    };
    let mut r = plane
        .base_for_query()
        .calculate_bounding_box(k, uv_domain, &base_req)?;
    if req.polar {
        let mut pb = PolarBox::new();
        pb.add_vector(plane.evaluate_normal(), None);
        r.polar = Some(pb);
    }
    Ok(r)
}

/// The `match` arm that cannot be reached (kinds already narrowed).
fn unreachable_kind<T>() -> Result<T> {
    Err(SolidError::invalid_input(OP_BOX, "unexpected surface kind"))
}

/// Whether `t` is the type code of a surface class (`IsKindOf(SmSurface_TYPE)`):
/// the codes from `SURF_BASE_TYPE + 1` below the topology codes.
pub(crate) const fn is_surface_type(t: i64) -> bool {
    t > 15000 && t < 16000
}

/// `SmSurface::CreateOffsetSurface` (base body, also what `SmBSplineSurface`'s override
/// calls): an `SmOffsetSurface` at `signed_offset_distance` that owns a copy of
/// `surface` (default singularities); the approximation tolerance is not used.
///
/// # Errors
///
/// Those of [`Surface::copy`] and of [`OffsetSurface::new`].
pub fn base_create_offset_surface(
    k: &Kernel,
    surface: &Surface,
    signed_offset_distance: f64,
) -> Result<Surface> {
    // temporary copy of the surface
    let surf_copy = surface.copy(k)?;

    // build the SmOffsetSurface surface
    Ok(Surface::from(OffsetSurface::new(
        k,
        signed_offset_distance,
        surf_copy,
        true,
        SS_UNKNOWN,
    )?))
}

/// `SmSurface::IsDegeneratePoint` (base body): the domain is a point, or its corners,
/// four interior points and the centre share one position (to `tol_3d`, at least a
/// scaled zero) with vanishing first derivatives (scaled by the domain size).
///
/// # Errors
///
/// Those of the evaluations (upstream ignores them).
pub fn base_is_degenerate_point(
    k: &Kernel,
    surface: &Surface,
    tol_3d: f64,
    uv_domain: Option<&Extent2d>,
) -> Result<bool> {
    // get target UVDomain
    let uv_domain = match uv_domain {
        Some(d) => *d,
        None => surface.natural_uv_domain_with_nurb(k)?,
    };

    // degenerate when sUVDomain is a point
    let mut uv0 = uv_domain.min();
    let mut uv1 = uv_domain.max();
    if uv0.approx_eq(uv1) {
        return Ok(true);
    }

    // evaluate interval start and end points
    let mut uv2 = Point2::new(uv0.x, uv1.y);
    let mut uv3 = Point2::new(uv1.x, uv0.y);
    let uv_size2 = uv_domain.max_dimension() * uv_domain.max_dimension();

    let eval = |uv: Point2| -> Result<[[Vec3; 2]; 2]> {
        Ok(first_block(&surface.evaluate(
            k,
            uv,
            1,
            1,
            upper_flags(false),
        )?))
    };
    let pv0 = eval(uv0)?;
    let pv1 = eval(uv1)?;
    let pv2 = eval(uv2)?;
    let pv3 = eval(uv3)?;

    // select a tolerance - scale it to sP1 location
    let scaled_zero =
        1000.0 * EFF_ZERO * (1.0 + pv0[0][0].max_dimension() + pv1[0][0].max_dimension());
    let tol2 = (tol_3d * tol_3d).max(scaled_zero * scaled_zero);
    let zero_derivs = |pv: &[[Vec3; 2]; 2]| {
        pv[1][0].length_squared() * uv_size2 < tol2 && pv[0][1].length_squared() * uv_size2 < tol2
    };

    // when corner points are close and have zero 1st derivatives
    if pv0[0][0].distance_squared(pv1[0][0]) < tol2
        && pv0[0][0].distance_squared(pv2[0][0]) < tol2
        && pv0[0][0].distance_squared(pv3[0][0]) < tol2
        && zero_derivs(&pv0)
        && zero_derivs(&pv1)
        && zero_derivs(&pv2)
        && zero_derivs(&pv3)
    {
        // save a compare point
        let pt = pv0[0][0];

        // evaluate some internal points
        uv0 = uv_domain.evaluate(0.5, 0.333);
        uv1 = uv_domain.evaluate(0.5, 0.666);
        uv2 = uv_domain.evaluate(0.333, 0.5);
        uv3 = uv_domain.evaluate(0.666, 0.5);
        let pv0 = eval(uv0)?;
        let pv1 = eval(uv1)?;
        let pv2 = eval(uv2)?;
        let pv3 = eval(uv3)?;

        // when all those points are also degenerate
        if pt.distance_squared(pv0[0][0]) < tol2
            && pt.distance_squared(pv1[0][0]) < tol2
            && pt.distance_squared(pv2[0][0]) < tol2
            && pt.distance_squared(pv3[0][0]) < tol2
            && zero_derivs(&pv0)
            && zero_derivs(&pv1)
            && zero_derivs(&pv2)
            && zero_derivs(&pv3)
        {
            // and the center point is degenerate
            let pc = eval(uv_domain.evaluate(0.5, 0.5))?;
            if pt.distance_squared(pc[0][0]) < tol2 && zero_derivs(&pc) {
                // take a guess and consider this degenerate
                return Ok(true);
            }
        }
    }

    // arrive here when any of the checked points are not close
    // enough to one another or have non-zero 1st derivatives
    Ok(false)
}

/// `SmSurface::IsDegenerateCurve` (base body): the domain is an iso-parameter line,
/// or the corners, four interior points, four diagonal points and the centre all have
/// parallel first derivatives, or all have a vanishing `Su` (or all a vanishing `Sv`)
/// to `max(scaled zero, opt_3d_tol)`. A domain that is a point is not a curve.
///
/// # Errors
///
/// Those of the evaluations (upstream ignores them).
pub fn base_is_degenerate_curve(
    k: &Kernel,
    surface: &Surface,
    angle_tol_deg: f64,
    uv_domain: Option<&Extent2d>,
    opt_3d_tol: f64,
) -> Result<bool> {
    // get target UVDomain
    let uv_domain = match uv_domain {
        Some(d) => *d,
        None => surface.natural_uv_domain_with_nurb(k)?,
    };

    // max and min domain corner points
    let uv00 = uv_domain.min();
    let uv01 = uv_domain.max();

    // return FALSE when target UVDomain has degenerate to a point
    if uv00.approx_eq(uv01) {
        return Ok(false);
    }

    // return true when target UVDomain has degenerated to a line
    let scaled_u = EFF_ZERO * (1.0 + uv_domain.x_length());
    let scaled_v = EFF_ZERO * (1.0 + uv_domain.y_length());
    let du = (uv00.x - uv01.x).abs();
    let dv = (uv00.y - uv01.y).abs();
    if (du < scaled_u && dv >= scaled_v) || (du >= scaled_u && dv < scaled_v) {
        return Ok(true);
    }

    // remaining domain corner points
    let corners = [
        uv00,
        uv01,
        Point2::new(uv00.x, uv01.y),
        Point2::new(uv01.x, uv00.y),
    ];
    // some sample points
    let samples = [
        uv_domain.evaluate(0.5, 0.333),
        uv_domain.evaluate(0.5, 0.666),
        uv_domain.evaluate(0.333, 0.5),
        uv_domain.evaluate(0.666, 0.5),
    ];
    // some diagonal points
    let diagonals = [
        uv_domain.evaluate(0.333, 0.333),
        uv_domain.evaluate(0.666, 0.666),
        uv_domain.evaluate(0.333, 0.666),
        uv_domain.evaluate(0.666, 0.333),
    ];
    // center point
    let center = uv_domain.evaluate(0.5, 0.5);

    let eval = |uv: Point2| -> Result<[[Vec3; 2]; 2]> {
        Ok(first_block(&surface.evaluate(
            k,
            uv,
            1,
            1,
            upper_flags(false),
        )?))
    };
    let eval4 = |uvs: &[Point2; 4]| -> Result<[[[Vec3; 2]; 2]; 4]> {
        Ok([eval(uvs[0])?, eval(uvs[1])?, eval(uvs[2])?, eval(uvs[3])?])
    };
    let flat = |d: &[[Vec3; 2]; 2]| d[1][0].is_parallel_to(d[0][1], angle_tol_deg);

    // Evaluate 1st derivatives at corner points
    let d0 = eval4(&corners)?;
    let mut d1: Option<[[[Vec3; 2]; 2]; 4]> = None;
    let mut d2: Option<[[[Vec3; 2]; 2]; 4]> = None;
    let mut d3: Option<[[Vec3; 2]; 2]> = None;

    // when the corner points are all dependentTangentPoints
    if d0.iter().all(flat) {
        // Evaluate 1st derivatives at sample points
        let s = eval4(&samples)?;
        d1 = Some(s);
        if s.iter().all(flat) {
            // Evaluate 1st derivatives at diagonal points
            let g = eval4(&diagonals)?;
            d2 = Some(g);
            if g.iter().all(flat) {
                // Evaluate 1st derivatives at center point
                let c = eval(center)?;
                d3 = Some(c);
                if flat(&c) {
                    // the surface has degenerated into a curve
                    return Ok(true);
                }
            }
        }
    }

    // pick a tangent tolerance
    let mut tol = EFF_ZERO * (1.0 + d0[0][0][0].max_dimension().max(d0[2][0][0].max_dimension()));
    if tol < opt_3d_tol {
        tol = opt_3d_tol;
    }
    let tol_square = tol * tol;

    // Surface degenerates to Curve when all Su (then all Sv) values are zero
    for (i, j) in [(1, 0), (0, 1)] {
        let zero =
            |d: &[[Vec3; 2]; 2]| is_zero_to_tol(block_at(d, i, j).length_squared(), tol_square);
        if d0.iter().all(zero) {
            // check sample points
            if d1.is_none() {
                d1 = Some(eval4(&samples)?);
            }
            let s = d1.unwrap_or_default();
            if s.iter().all(zero) {
                // check diagonal points
                if d2.is_none() {
                    d2 = Some(eval4(&diagonals)?);
                }
                let g = d2.unwrap_or_default();
                if g.iter().all(zero) {
                    // check center point
                    if d3.is_none() {
                        d3 = Some(eval(center)?);
                    }
                    let c = d3.unwrap_or_default();
                    if zero(&c) {
                        return Ok(true);
                    }
                }
            }
        }
    }

    // arrive here when any of the checked points are not Degenerate Curve points
    Ok(false)
}

// ---- standalone helpers ---------------------------------------------------------------------

/// The interval of `uv_curve` over `interval` that contains `param_inside` between
/// two crossings of the boundary of `trim_domain` (`FindUVCurveIvlInDomain`): the
/// curve is split at its intersections with the four domain sides (2D lines).
///
/// # Errors
///
/// Those of the intersections; `SM_ERR` when no piece contains `param_inside`.
pub fn find_uv_curve_ivl_in_domain<O: SolutionObject>(
    k: &Kernel,
    uv_curve: &Curve,
    interval: &Extent1d,
    param_inside: f64,
    trim_domain: &Extent2d,
) -> Result<Extent1d> {
    // init list of Curve Domain/TrimDomain partitions
    let mut segments = vec![interval.min(), interval.max()];
    let other_interval = Extent1d::from_min_max(0.0, 1.0);

    // Min/Max TrimDomain corners for rectangle := [UMin,UMax]X[VMin,VMax].
    let min = trim_domain.min();
    let max = trim_domain.max();

    // Work to Numerical tolerances
    let scaled_zero = trim_domain.scaled_zero();

    // UMin, VMin, UMax, VMax Domain boundary checks
    let p = |x: f64, y: f64| Point3::new(x, y, 0.0);
    let sides = [
        (p(min.x, min.y), p(min.x, max.y)),
        (p(min.x, min.y), p(max.x, min.y)),
        (p(max.x, max.y), p(max.x, min.y)),
        (p(max.x, max.y), p(min.x, max.y)),
    ];
    for (start, end) in sides {
        let line = Line::from_points(start, end, 3);
        intersect_curve_and_split::<O>(
            k,
            uv_curve,
            interval,
            &line,
            &other_interval,
            scaled_zero,
            &mut segments,
        )?;
    }

    // For every TgtCurve Segment Bndry
    for w in segments.windows(2) {
        // If segment contains input dParamInside value
        let &[lo, hi] = w else { continue };
        if lo <= param_inside && param_inside < hi {
            let mut ivl = Extent1d::new();
            ivl.set_min_max(lo, hi)?;
            return Ok(ivl);
        }
    }

    // error: arrive here when no segments contain the input dParamInside value
    Err(SolidError::sm_err(OP_IVL))
}

/// The normal section of a surface evaluation in `direction`
/// (`smsurf_EvaluateNormalSection`): the unit tangent in that direction (the
/// projection onto the principal directions; zero when it vanishes) and the
/// curvature by Euler's theorem.
///
/// # Errors
///
/// Those of [`evaluate_geometric`] and of the unitizations.
pub fn evaluate_normal_section(eval: &mut [[Vec3; 3]; 3], direction: Vec3) -> Result<(Vec3, f64)> {
    // compute SurfacePoint Properties from parametric evaluation
    let g = evaluate_geometric(eval)?;
    let mut prin_kv1 = g.principle_curvature_vector1;
    let mut prin_kv2 = g.principle_curvature_vector2;

    // unitize surfaceTangents in principle curvature directions
    if prin_kv1.length_squared() > EFF_ZERO_SQ {
        prin_kv1 = prin_kv1.unitize()?;
    }
    if prin_kv2.length_squared() > EFF_ZERO_SQ {
        prin_kv2 = prin_kv2.unitize()?;
    }

    // get tangentVector in crDirection
    let plane_proj = prin_kv1 * prin_kv1.dot(direction) + prin_kv2 * prin_kv2.dot(direction);

    // when tangentVector is degenerate - quit
    if plane_proj.length_squared() < EFF_ZERO_SQ {
        return Ok((Vec3::ZERO, 0.0));
    }

    // unitize and output the tangentVector
    let plane_proj = plane_proj.unitize()?;

    // Now calculate the curvature using Euler's theorem.
    let cos_theta = plane_proj.dot(prin_kv1);
    let sin_theta = if cos_theta >= 1.0 || cos_theta <= -1.0 {
        0.0 // Can happen, due to noise.
    } else {
        (1.0 - cos_theta * cos_theta).sqrt()
    };

    let curvature = g.principle_curvature1 * cos_theta * cos_theta
        + g.principle_curvature2 * sin_theta * sin_theta;
    Ok((plane_proj, curvature))
}

/// The body of `SmSurface::ConvertDomainFromNURBSToSTEP`, with the surface's virtual
/// `ConvertUVFromNURBSToSTEP` as `to_step` (shared by [`Surface`] and the surfaces of
/// revolution, which reach it without a [`Surface`] wrapper).
///
/// # Errors
///
/// Those of `to_step`; `SetMinMax`'s `InvalidInput` (a reversed `u` interval).
pub(crate) fn convert_domain_from_nurbs_to_step_with(
    nurb_domain: &Extent2d,
    to_step: impl Fn(Point2) -> Result<Point2>,
) -> Result<Extent2d> {
    let min_step_uv = to_step(nurb_domain.min())?;
    let max_step_uv = to_step(nurb_domain.max())?;

    // set output - InsideOut swaps the GenCurve min to max traversal direction
    let mut d = Extent2d::new();
    d.set_min_max_coords(
        min_step_uv.x,
        min2(min_step_uv.y, max_step_uv.y),
        max_step_uv.x,
        max2(min_step_uv.y, max_step_uv.y),
    )?;
    Ok(d)
}

/// Whether a step from `prev_uv` to `curr_uv` crossed a seam or a pole
/// (`sm_CrossedSeamOrPole`): never on planes and bilinear B-splines (the B-spline kinds'
/// `GetDegree`, which makes a missing NURB: [`Surface::degree_with_nurb`]); else a step
/// longer than 0.4 of the domain side in `u` or in `v`.
///
/// # Errors
///
/// [`SolidError::Unported`] from making a missing NURB.
pub fn crossed_seam_or_pole(
    k: &Kernel,
    surface: &Surface,
    prev_uv: Point2,
    curr_uv: Point2,
    domain: &Extent2d,
) -> Result<bool> {
    const MAX_STEP_RATIO: f64 = 0.4;

    // First special cases: SmPlane or bilinear.
    match surface.kind() {
        SurfaceKind::Plane(_) => return Ok(false),
        // (SM_CAST_PTR(SmBSplineSurface): the B-spline and the kinds derived from it)
        SurfaceKind::BSpline(_)
        | SurfaceKind::Cone(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_) => {
            if surface.degree_with_nurb(k, SurfParamType::U)? == 1
                && surface.degree_with_nurb(k, SurfParamType::V)? == 1
            {
                return Ok(false);
            }
        }
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {}
    }

    // Just look for a big jump.
    let step = curr_uv - prev_uv;

    // Check u and v separately.
    let dom_size_u = domain.u_interval().length();
    let dom_size_v = domain.v_interval().length();
    Ok(step.x.abs() > MAX_STEP_RATIO * dom_size_u || step.y.abs() > MAX_STEP_RATIO * dom_size_v)
}

/// `sm_GetSurfaceToEvaluate`: the original surface of the cache owning `node`'s tree
/// ([`original_surface`]).
///
/// # Errors
///
/// Those of [`original_surface`].
pub fn surface_to_evaluate<O: Copy + PartialEq>(tree: &Tree<O>, node: &TreeNode<O>) -> Result<O> {
    original_surface(tree, node)
}

/// Whether the tree of a surface cache owner kind `owner` is a surface cache's
/// (`IsKindOf(SmSurfaceCache_TYPE)`: surface, trimmed and tessellation caches).
pub(crate) const fn is_surface_cache_owner(owner: Option<TreeOwner>) -> bool {
    matches!(
        owner,
        Some(TreeOwner::SurfaceCache | TreeOwner::TrimSrfCache | TreeOwner::TessSrfCache)
    )
}

/// `sm_IsPointValid`: whether a potential solution at `uv` should be kept: always
/// unless the tree (owner kind `owner`) is a surface cache's, then the cache's point
/// test.
pub fn is_point_valid<O: Copy + PartialEq>(
    owner: Option<TreeOwner>,
    cache: &SurfaceCache<O>,
    uv: Point2,
) -> Result<bool> {
    // keep all points when SurfaceNode has no trim boundaries
    if !is_surface_cache_owner(owner) {
        return Ok(true);
    }
    cache.point_test(uv)
}

/// [`is_point_valid`] with the trimmed cache's operation-scoped face queries.
/// As upstream's `sm_IsPointValid`, a failed native classification keeps the
/// candidate; an unavailable Rust capability still propagates as `Unported`.
///
/// # Errors
/// Missing owning-face context and unported classification dependencies.
pub fn is_point_valid_with<O: Copy + PartialEq>(
    k: &Kernel,
    owner: Option<TreeOwner>,
    cache: &SurfaceCache<O>,
    uv: Point2,
    tolerance: Option<f64>,
    faces: Option<&dyn FaceQueries<O>>,
) -> Result<bool> {
    if !is_surface_cache_owner(owner) {
        return Ok(true);
    }
    Ok(
        ignore_native_classification(cache.point_test_with(k, uv, tolerance, faces))?
            .is_none_or(|(valid, _)| valid),
    )
}

/// A failed native classification has caller-specific keep/remove behavior; a
/// malformed identity or dangling Rust arena reference is not a native status.
pub(super) fn ignore_native_classification<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Err(error @ (SolidError::InvalidInput { .. } | SolidError::NotFound { .. })) => Err(error),
        native => ignored(native),
    }
}

/// `sm_IsNodeValid`: false only when `node` of a surface cache's tree (owner kind
/// `owner`) is classified outside its face while the cache's point testing is on and
/// current.
///
/// # Errors
///
/// `InvalidInput` for a node without surface data where the class is read (upstream
/// reads the data pointer as an `SmBezierAux2d`).
pub fn is_node_valid<O: Copy + PartialEq>(
    owner: Option<TreeOwner>,
    cache: &SurfaceCache<O>,
    node: &TreeNode<O>,
) -> Result<bool> {
    // no work - TreeNode is not part of a SurfaceCache (signals)
    if !is_surface_cache_owner(owner) {
        return Ok(true);
    }

    // when testing is off or cache is not current
    if !cache.point_test_enabled || cache.face_was_modified || !cache.have_t_surface_cache {
        // return TRUE so this node won't be culled
        return Ok(true);
    }
    // return FALSE = outside face, TRUE = in/on face
    match &node.data {
        NodeData::Surface(aux) => Ok(aux.node_class != NodeClass::Outside),
        _ => Err(SolidError::invalid_input(
            "surface::surface::p1::is_node_valid",
            "surface cache node without surface data",
        )),
    }
}

// ---- SmSurfaceEval accessors ---------------------------------------------------------

impl SurfaceEval<'_> {
    /// Evaluates if needed and reports a failed evaluation (`FailsEval`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn fails_eval(&mut self) -> Result<bool> {
        self.eval()?;
        Ok(self.flags()[3])
    }

    /// `values()[i][j]` after evaluating.
    fn value(&mut self, i: usize, j: usize) -> Result<Vec3> {
        self.eval()?;
        Ok(self
            .values()
            .get(i)
            .and_then(|r| r.get(j))
            .copied()
            .unwrap_or(Vec3::ZERO))
    }

    /// The position (`Pos`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn pos(&mut self) -> Result<Point3> {
        self.value(0, 0)
    }

    /// `Su`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn su(&mut self) -> Result<Vec3> {
        self.value(1, 0)
    }

    /// `Sv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn sv(&mut self) -> Result<Vec3> {
        self.value(0, 1)
    }

    /// `Suu`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn suu(&mut self) -> Result<Vec3> {
        self.value(2, 0)
    }

    /// `Suv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn suv(&mut self) -> Result<Vec3> {
        self.value(1, 1)
    }

    /// `Svv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn svv(&mut self) -> Result<Vec3> {
        self.value(0, 2)
    }
}

// ---- SmDropCurveFail copy constructor --------------------------------------------------

impl<O: SolutionObject> DropCurveFail<O> {
    /// The copy constructor: every member of `other`; the UV curves are copied only
    /// when `other` has a context.
    #[must_use]
    pub fn copy_of(other: &Self) -> Self {
        let mut copy = other.clone();
        if !other.has_context {
            copy.uv_curves.clear();
        }
        copy
    }
}

// ---- integrands ---------------------------------------------------------------------

impl IsoCurveLengthIntegrand<'_> {
    /// The area integrand at `(u_iso_value, t)`: `|Su x Sv|` (`Evaluate`; upstream
    /// computes `sqrt(EG - F^2)` first, then overwrites it with the normal's length).
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn evaluate(&self, k: &Kernel, t: f64) -> Result<f64> {
        let uv = Point2::new(self.u_iso_value, t);

        // evaluate UVPoint(UIsoValue, dT)
        let pv = first_block(&self.surface.evaluate(k, uv, 1, 1, upper_flags(false))?);

        // compute integrand of Area = Integral( sqrt(E*G - F*F) ) du dv (overwritten)
        let norm = pv[1][0].cross(pv[0][1]);
        Ok(norm.length())
    }
}

/// The outer integrand of a surface area (`SmAreaIFEO`): the strip integral of the
/// iso curve lengths over the `v` interval of `uv_domain` at a `u` value.
#[derive(Debug, Clone, Copy)]
pub struct AreaIntegrand<'s> {
    /// `m_crSurface`.
    pub surface: &'s Surface,
    /// `m_crUVDomain`.
    pub uv_domain: Extent2d,
    /// `m_dIsoCurveAccuracy`.
    pub iso_curve_accuracy: f64,
}

impl AreaIntegrand<'_> {
    /// The strip integral at `u = t` (`Evaluate`: Romberg over the `v` interval).
    ///
    /// # Errors
    ///
    /// Those of the surface evaluator; scalar numerical failures yield zero upstream.
    pub fn evaluate(&self, k: &Kernel, t: f64) -> Result<f64> {
        // set up the strip integral of [deltaX, IntervalY]
        let eval = IsoCurveLengthIntegrand::new(self.surface, t);
        crate::mass::integrator::integrate(
            self.uv_domain.min().y,
            self.uv_domain.max().y,
            crate::mass::integrator::IntegrationAlgorithm::Romberg,
            self.iso_curve_accuracy,
            |v| eval.evaluate(k, v),
        )
    }
}

// ---- SmFindSingularParamWithGivenTangentDirEFO ------------------------------------------------

/// The function whose zero is the singular parameter where the unit tangent across a
/// singular boundary points along `dir` (`SmFindSingularParamWithGivenTangentDirEFO`):
/// `F(s) = Dot((w/|w|)', dir)`, `F'(s) = Dot((w/|w|)'', dir)` with `w` the derivative
/// across the singular direction at `(s, t)` (`t` from `uv`).
#[derive(Debug, Clone, Copy)]
pub struct FindSingularParamWithGivenTangentDir<'s> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'s Kernel,

    /// `m_pSurface`.
    pub surface: &'s Surface,
    /// `m_rUV`: its `t` coordinate picks the boundary.
    pub uv: Point2,
    /// `m_rDir`: the desired tangent direction.
    pub dir: Vec3,
    /// `m_eSingularDirection`: [`SurfParamType::U`] (`s` is `u`) or `V`.
    pub singular_direction: SurfParamType,
    /// `m_dDistTol3d`.
    pub dist_tol_3d: f64,
}

impl<'s> FindSingularParamWithGivenTangentDir<'s> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'s Kernel,
        surface: &'s Surface,
        uv: Point2,
        dir: Vec3,
        singular_direction: SurfParamType,
        dist_tol_3d: f64,
    ) -> Self {
        Self {
            kernel: k,
            surface,
            uv,
            dir,
            singular_direction,
            dist_tol_3d,
        }
    }

    /// `F(param)`, `F'(param)` and the found flag (always false: tight convergence)
    /// (`Evaluate`).
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    pub fn evaluate(&self, param: f64) -> Result<(f64, f64, bool)> {
        let k = self.kernel;
        // branch on parametric direction to build local values
        let (uv_point, w, ws, wss) = if self.singular_direction == SurfParamType::U {
            (Point2::new(param, self.uv.y), (0, 1), (1, 1), (2, 1))
        } else {
            (Point2::new(self.uv.x, param), (1, 0), (1, 1), (1, 2))
        };

        // evaluate Surface to get Surf derivatives w, w', and w"
        let m = self
            .surface
            .evaluate(k, uv_point, 3, 3, upper_flags(true))?;

        // compute Unitized derivitaves of w (using w' and w") (status ignored)
        let [unit_t, unit_tt] = at(&m, w.0, w.1)
            .unitized_derivative2(at(&m, ws.0, ws.1), at(&m, wss.0, wss.1))
            .unwrap_or_else(|d| d);

        // compute F(s) and F'(s)
        Ok((unit_t.dot(self.dir), unit_tt.dot(self.dir), false))
    }
}

impl EvalFunction for FindSingularParamWithGivenTangentDir<'_> {
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _signal_errors: bool) -> Result<()> {
        // init output
        out.f = 0.0;
        out.f_prime = 0.0;
        out.found_answer = false;
        let (f, f_prime, found) = FindSingularParamWithGivenTangentDir::evaluate(self, t)?;
        out.f = f;
        out.f_prime = f_prime;
        out.found_answer = found;
        Ok(())
    }
}

// ---- solver constructors ------------------------------------------------------------------

/// The curve/surface intersection solver (`SmCSIGlobalSolver`); its solves belong to
/// unit `surface.surface_p9`.
#[derive(Debug)]
pub struct CsiGlobalSolver<'t, 's, O> {
    /// Caller context for virtual evaluations.
    pub kernel: &'s Kernel,

    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// Curve geometry supplied by the caller for leaf solves (part 9).
    pub curve: Option<&'s Curve>,
    /// Cache supplying the surface patches for leaf solves (part 9).
    pub surface_cache: Option<&'s SurfaceCache<O>>,
    /// The prepared owning-face classifier for trimmed candidate acceptance.
    pub face_queries: Option<&'s dyn FaceQueries<O>>,
    /// `m_pSurf`.
    pub surface: Option<&'s Surface>,
    /// `m_bFoundCoincidence`.
    pub found_coincidence: bool,
    /// `m_crInterval`.
    pub interval: Extent1d,
    /// `m_crUVDomain`.
    pub uv_domain: Extent2d,
    /// The handles and parameter domains of the solve's curve and surface, which the
    /// solutions name (`IdenticalSolutions` reads them through upstream's objects);
    /// `None` reads them from the handles themselves.
    pub operands: Option<SolveOperands<O>>,
}

impl<'t, 's, O: SolutionObject> CsiGlobalSolver<'t, 's, O> {
    /// The constructor: two trees, three variables.
    #[must_use]
    pub fn new(
        k: &'s Kernel,
        curve_tree: &'t Tree<O>,
        surface_tree: &'t Tree<O>,
        surface: Option<&'s Surface>,
        found_coincidence: bool,
        interval: Extent1d,
        uv_domain: Extent2d,
    ) -> Self {
        Self {
            kernel: k,
            base: GlobalSolver::with_trees(curve_tree, surface_tree, 3),
            curve: None,
            surface_cache: None,
            face_queries: None,
            surface,
            found_coincidence,
            interval,
            uv_domain,
            operands: None,
        }
    }
}

/// The curve/surface solver (`SmCSGlobalSolver`); its solves belong to unit
/// `surface.surface_p11`.
#[derive(Debug)]
pub struct CsGlobalSolver<'t, 's, O> {
    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_pSurf`.
    pub surface: Option<&'s Surface>,
    /// `m_crInterval`.
    pub interval: Extent1d,
    /// `m_crUVDomain`.
    pub uv_domain: Extent2d,
    /// The curve of the curve tree (its owner object), for the leaf solve (part 11).
    pub curve: Option<&'s Curve>,
    /// The cache holding the patches of the surface tree's nodes, for the leaf solve
    /// (part 11; upstream reaches it through the node's tree owner).
    pub surface_cache: Option<&'s SurfaceCache<O>>,
    /// The prepared owning-face classifier for trimmed candidate acceptance.
    pub face_queries: Option<&'s dyn FaceQueries<O>>,
    /// The handles and parameter domains of the solve's curve and surface, which the
    /// solutions name (`IdenticalSolutions` reads them through upstream's objects);
    /// `None` reads them from the handles themselves.
    pub operands: Option<SolveOperands<O>>,
    /// The kernel (upstream's `SmContext`) the leaf solve's evaluations and solves run
    /// with (part 11).
    pub kernel: &'s Kernel,
}

/// The parameter domain of a solve's operand (see [`SolveOperands`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperandDomain {
    /// A curve and its natural interval (`GetNaturalInterval`).
    Curve(Extent1d),
    /// A surface and its natural UV domain (`GetNaturalUVDomain`).
    Surface(Extent2d),
}

/// The operands of a curve/surface or surface/surface solve as its solutions name
/// them: each operand's handle and parameter domain, which `IdenticalSolutions`
/// reads through upstream's objects (`&crCurve`, `this`, `&crOtherSurface`). A
/// handle's own hooks answer first.
#[derive(Debug, Clone, Copy)]
pub struct SolveOperands<O> {
    /// The handles of the operands and their domains.
    pub operands: [Option<(O, OperandDomain)>; 2],
}

impl<O: SolutionObject> crate::geom::solution_array::ParameterDomains<O> for SolveOperands<O> {
    fn natural_interval(&self, curve: &O) -> Option<Extent1d> {
        curve.natural_interval().or_else(|| {
            self.operands.iter().flatten().find_map(|(h, d)| match d {
                OperandDomain::Curve(ivl) if h == curve => Some(*ivl),
                _ => None,
            })
        })
    }

    fn natural_uv_domain(&self, surface: &O) -> Option<Extent2d> {
        surface.natural_uv_domain().or_else(|| {
            self.operands.iter().flatten().find_map(|(h, d)| match d {
                OperandDomain::Surface(domain) if h == surface => Some(*domain),
                _ => None,
            })
        })
    }
}

impl<'t, 's, O: SolutionObject> CsGlobalSolver<'t, 's, O> {
    /// The constructor: two trees, three variables.
    #[must_use]
    pub fn new(
        k: &'s Kernel,
        curve_tree: &'t Tree<O>,
        surface_tree: &'t Tree<O>,
        surface: Option<&'s Surface>,
        interval: Extent1d,
        uv_domain: Extent2d,
    ) -> Self {
        Self {
            base: GlobalSolver::with_trees(curve_tree, surface_tree, 3),
            surface,
            interval,
            uv_domain,
            curve: None,
            surface_cache: None,
            face_queries: None,
            operands: None,
            kernel: k,
        }
    }

    /// `BranchMayContainAnswers`: when normalizing, false if the surface node's
    /// normals provably miss the curve node ([`cs_are_perpen_disjoint`], or
    /// [`super::p2::cs_are_perpen_disjoint_in`] when `surface_cache` holds the
    /// patches); then the base test.
    ///
    /// # Errors
    ///
    /// Those of the branch lookup and of the tests.
    pub fn branch_may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        // If we are normalizing and have a node with a surface do the
        // angular test using the Gauss map of the normal.
        if self.base.operation == SolverOperation::Normalize {
            let curve_node = self.base.branch_node(branch, 0)?;
            let surface_node = self.base.branch_node(branch, 1)?;
            let curve_tree = self.base.tree(0)?;
            // with the solve's surface cache (unit surface.surface_p11 hands it over)
            // the patch's polar box is reachable
            let disjoint = match (self.surface_cache, branch.get(1).copied().flatten()) {
                (Some(cache), Some(surface_id)) => super::p2::cs_are_perpen_disjoint_in(
                    k,
                    curve_tree,
                    curve_node,
                    surface_node,
                    cache,
                    surface_id,
                )?,
                _ => cs_are_perpen_disjoint(curve_tree, curve_node, surface_node)?,
            };
            if disjoint {
                return Ok(false);
            }
        }
        self.base.branch_may_contain_answers(branch)
    }
}

impl<'t, 's, O: SolutionObject> LsiGlobalSolver<'t, 's, O> {
    /// The constructor (`SmLSIGlobalSolver`): one tree, two variables.
    #[must_use]
    pub fn new(
        k: &'s Kernel,
        surface_tree: &'t Tree<O>,
        surface: Option<&'s Surface>,
        uv_domain: Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<Extent1d>,
        ray_fire: bool,
        distance_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            base: GlobalSolver::with_tree(surface_tree, 2),
            surface,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            ray_fire,
            distance_tolerance,
            surface_cache: None,
            face_queries: None,
            face_cache: None,
        }
    }
}

impl<'t, 's, O: SolutionObject> SpGlobalSolver<'t, 's, O> {
    /// The constructor (`SmSPGlobalSolver`): the point tree and the surface tree, two
    /// variables.
    #[must_use]
    pub fn new(
        k: &'s Kernel,
        point_tree: &'t Tree<O>,
        surface_tree: &'t Tree<O>,
        surface: Option<&'s Surface>,
        uv_domain: Extent2d,
    ) -> Self {
        Self {
            kernel: k,
            base: GlobalSolver::with_trees(point_tree, surface_tree, 2),
            surface,
            uv_domain,
            surface_cache: None,
            face_queries: None,
        }
    }
}

impl<'t, 's, O: SolutionObject> SsGlobalSolver<'t, 's, O> {
    /// The constructor (`SmSSGlobalSolver`): two trees, four variables.
    #[must_use]
    pub fn new(
        surface_tree1: &'t Tree<O>,
        surface_tree2: &'t Tree<O>,
        surfaces: [Option<&'s Surface>; 2],
        uv_domain1: Extent2d,
        uv_domain2: Extent2d,
        kernel: &'s Kernel,
    ) -> Self {
        Self {
            base: GlobalSolver::with_trees(surface_tree1, surface_tree2, 4),
            surface1: surfaces[0],
            surface2: surfaces[1],
            uv_domain1,
            uv_domain2,
            kernel,
            caches: [None, None],
            face_queries: None,
            operands: None,
        }
    }
}

// ---- evaluator constructors -------------------------------------------------------------

/// Curve/surface intersection equations (`SmCSIntersectENFO`); `Evaluate` is in unit
/// `surface.surface_p4` (`EvalNFunctions`).
#[derive(Debug, Clone, Copy)]
pub struct CsIntersectEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crCurve`.
    pub curve: &'a Curve,
    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
    /// `m_lCount`: calls to `Evaluate`.
    pub count: u64,
    /// `m_dLastSum`.
    pub last_sum: f64,
    /// `m_bDoMinimize`.
    pub do_minimize: bool,
}

impl<'a> CsIntersectEnfo<'a> {
    /// The constructor: no calls yet, intersecting (not minimizing).
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve: &'a Curve,
        surface: &'a Surface,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve,
            surface,
            squared_tolerance,
            count: 0,
            last_sum: 0.0,
            do_minimize: false,
        }
    }
}

/// Curve/surface normal equations (`SmCSNormalizeENFO`).
#[derive(Debug, Clone, Copy)]
pub struct CsNormalizeEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crCurve`.
    pub curve: &'a Curve,
    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_eSolverOperation`.
    pub solver_operation: SolverOperation,
    /// `m_dDistanceTolerance` (not used).
    pub distance_tolerance: f64,
    /// `m_cpOptVectors`: the direction to minimize for signed directed minimizes.
    pub opt_vectors: Option<&'a [Vec3]>,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
}

impl<'a> CsNormalizeEnfo<'a> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve: &'a Curve,
        surface: &'a Surface,
        solver_operation: SolverOperation,
        distance_tolerance: f64,
        opt_vectors: Option<&'a [Vec3]>,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve,
            surface,
            solver_operation,
            distance_tolerance,
            opt_vectors,
            squared_tolerance,
        }
    }
}

/// Point/surface normal equations (`SmFindPSNormalENFO`).
#[derive(Debug, Clone, Copy)]
pub struct FindPsNormalEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crPoint`.
    pub point: Point3,
    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
    /// `m_eSolverOperation` (not used).
    pub solver_operation: SolverOperation,
}

impl<'a> FindPsNormalEnfo<'a> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        point: Point3,
        surface: &'a Surface,
        squared_tolerance: f64,
        solver_operation: SolverOperation,
    ) -> Self {
        Self {
            kernel: k,
            point,
            surface,
            squared_tolerance,
            solver_operation,
        }
    }
}

/// Surface/surface/plane intersection equations (`SmSSPIntersectENFO`).
#[derive(Debug, Clone, Copy)]
pub struct SspIntersectEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_crOtherSurface`.
    pub other_surface: &'a Surface,
    /// `m_bSteepestDescent`: the plane follows the iteration.
    pub steepest_descent: bool,
    /// `m_cpPlane`.
    pub plane: Option<&'a Plane>,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
}

impl<'a> SspIntersectEnfo<'a> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        surface: &'a Surface,
        other_surface: &'a Surface,
        steepest_descent: bool,
        plane: Option<&'a Plane>,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            surface,
            other_surface,
            steepest_descent,
            plane,
            squared_tolerance,
        }
    }
}

/// Surface/surface intersection on a plane, normalizing (`SmSSINormalizeENFO`).
#[derive(Debug, Clone, Copy)]
pub struct SsiNormalizeEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_crOtherSurface`.
    pub other_surface: &'a Surface,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
    /// `m_vPlaneNormal`: `A, B, C` of `Ax + By + Cz + D = 0`.
    pub plane_normal: Vec3,
    /// `m_dPlaneD`.
    pub plane_d: f64,
}

impl<'a> SsiNormalizeEnfo<'a> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        surface: &'a Surface,
        other_surface: &'a Surface,
        squared_tolerance: f64,
        plane_normal: Vec3,
        plane_d: f64,
    ) -> Self {
        Self {
            kernel: k,
            surface,
            other_surface,
            squared_tolerance,
            plane_normal,
            plane_d,
        }
    }
}

/// Surface/surface normal equations (`SmSSNormalizeENFO`).
#[derive(Debug, Clone, Copy)]
pub struct SsNormalizeEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_crOtherSurface`.
    pub other_surface: &'a Surface,
    /// `m_eSolverOperation`.
    pub solver_operation: SolverOperation,
    /// `m_cpOptVectors`.
    pub opt_vectors: Option<&'a [Vec3]>,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
}

impl<'a> SsNormalizeEnfo<'a> {
    /// The constructor.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        surface: &'a Surface,
        other_surface: &'a Surface,
        solver_operation: SolverOperation,
        opt_vectors: Option<&'a [Vec3]>,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            surface,
            other_surface,
            solver_operation,
            opt_vectors,
            squared_tolerance,
        }
    }
}

/// Three-surface intersection equations (`SmSSSIntersectENFO`); the solution points
/// and tangents are filled by its `Evaluate`.
#[derive(Debug, Clone, Copy)]
pub struct SssIntersectEnfo<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_crSurface1`.
    pub surface1: &'a Surface,
    /// `m_crSurface2`.
    pub surface2: &'a Surface,
    /// `m_crSurface3`.
    pub surface3: &'a Surface,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
    /// `m_lCount`.
    pub count: u64,
    /// `m_sSrfPoint1..3`: the solution points (undefined until evaluated).
    pub srf_points: [Point3; 3],
    /// `m_sDU1..3`, `m_sDV1..3`: the solution tangents (undefined until evaluated).
    pub srf_tangents: [[Vec3; 2]; 3],
}

impl<'a> SssIntersectEnfo<'a> {
    /// The constructor: no calls yet; the solution members are `SmVector3d`'s
    /// undefined default.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        surface1: &'a Surface,
        surface2: &'a Surface,
        surface3: &'a Surface,
        squared_tolerance: f64,
    ) -> Self {
        let undef = Vec3::new(
            crate::math::UNDEF_DOUBLE,
            crate::math::UNDEF_DOUBLE,
            crate::math::UNDEF_DOUBLE,
        );
        Self {
            kernel: k,
            surface1,
            surface2,
            surface3,
            squared_tolerance,
            count: 0,
            srf_points: [undef; 3],
            srf_tangents: [[undef; 2]; 3],
        }
    }
}

/// Copies an evaluation matrix `m` into the flat caller array `d` (entry
/// `i * (hv + 1) + j`): every entry, or only the upper half (`i + j <= hu`) when
/// `only_upper_half` is set, leaving the others untouched.
fn copy_written(
    m: &[Vec<Vec3>],
    hu: usize,
    hv: usize,
    only_upper_half: bool,
    d: &mut [Vec3],
    op: &'static str,
) -> Result<()> {
    for i in 0..=hu {
        for j in 0..=hv {
            if only_upper_half && i + j > hu {
                continue;
            }
            let v = m
                .get(i)
                .and_then(|row| row.get(j))
                .copied()
                .ok_or_else(|| SolidError::invalid_input(op, "evaluation matrix too small"))?;
            let slot = d
                .get_mut(i * (hv + 1) + j)
                .ok_or_else(|| SolidError::invalid_input(op, "derivative array too short"))?;
            *slot = v;
        }
    }
    Ok(())
}

/// The next depth of a curve bounded, STEP or volume chain walk, `InvalidInput` past
/// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
fn nesting_depth(depth: usize, op: &'static str) -> Result<usize> {
    if depth >= crate::surface::surface::p2::MAX_GEOMETRY_NESTING {
        return Err(SolidError::invalid_input(
            op,
            "surface chain deeper than MAX_GEOMETRY_NESTING",
        ));
    }
    Ok(depth + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bilinear patch over `[0, 1]^2` spanning `(0,0,0)..(2,1,0)` with a twist.
    fn bilinear() -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(2.0, 1.0, 0.5),
            )
            .unwrap(),
        )
    }

    #[test]
    fn approx_eq_compares_spheres_and_tori_by_their_own_operator() {
        use crate::geom::axis2_placement::Axis2Placement;
        use crate::surface::sphere::Sphere;
        use crate::surface::torus::{Torus, TorusOptions};
        let k = &Kernel::default();
        let frame = Axis2Placement::IDENTITY;
        let sphere = |r: f64| Surface::from(Sphere::create_canonical(k, &frame, r).unwrap());
        let torus = |major: f64, minor: f64| {
            Surface::from(
                Torus::create_canonical(k, &frame, major, minor, TorusOptions::default(), None)
                    .unwrap(),
            )
        };
        // distinct objects, so the pointer-identity shortcut does not apply
        assert!(sphere(2.0).approx_eq(k, &sphere(2.0)).unwrap());
        assert!(torus(4.0, 0.5).approx_eq(k, &torus(4.0, 0.5)).unwrap());
        assert!(!sphere(2.0).approx_eq(k, &torus(4.0, 0.5)).unwrap());
        assert!(!torus(4.0, 0.5).approx_eq(k, &sphere(2.0)).unwrap());
        assert!(!sphere(2.0).approx_eq(k, &sphere(3.0)).unwrap());
        assert!(!torus(4.0, 0.5).approx_eq(k, &torus(4.0, 0.75)).unwrap());
    }

    #[test]
    fn map_to_2d_on_curve_uses_the_uv_curve_of_a_curve_on_this_surface() {
        use crate::curve::crv_on_surf::CrvOnSurf;
        let k = &Kernel::default();
        let crv = CrvOnSurf::from_uv_line(
            k,
            Point2::new(0.1, 0.2),
            Point2::new(0.8, 0.6),
            bilinear(),
            None,
            0,
        )
        .unwrap();
        let curve = Curve::from(crv);
        let CurveKind::CrvOnSurf(c) = curve.kind() else {
            panic!("curve on surface")
        };
        let base = c.surface();
        let d = c.uv_curve().evaluate(k, 0.5, 1, true, true).unwrap();
        let uv = Point2::new(d[0].x, d[0].y);
        let dir = Vec2::new(d[1].x, d[1].y);
        let expected = map_to_2d_dir(k, 1e-3, uv, dir, base).unwrap();
        // the curve's own base surface: the UV curve is evaluated
        let got = map_to_2d_on_curve(k, 1e-3, 0.5, &curve, base).unwrap();
        assert_eq!(got, expected);
        // another surface object: the curve's 3D point and tangent are dropped
        let other = bilinear();
        assert!(map_to_2d_on_curve(k, 1e-3, 0.5, &curve, &other).is_ok());
    }

    #[test]
    fn evaluation_wrappers() {
        let k = &Kernel::default();
        let s = bilinear();
        let uv = Point2::new(0.25, 0.5);
        let (p, du, dv) = s.evaluate_1st_derivatives(k, uv, true, true).unwrap();
        let d2 = s.evaluate_2nd_derivatives(k, uv, true, true).unwrap();
        assert_eq!(p, d2[0]);
        assert_eq!(du, d2[1]);
        assert_eq!(dv, d2[2]);
        assert_eq!(s.evaluate_step_point(k, uv).unwrap(), p);
        let step = s.evaluate_step(k, uv, 1, 1, upper_flags(false)).unwrap();
        assert_eq!(at(&step, 1, 0), du);
    }

    /// The caller-held cache overload of `GetDomainWithContinuity` returns the same
    /// domain as the one that builds the cache itself.
    #[test]
    fn domain_with_continuity_from_cache_matches_direct() {
        let k = &Kernel::default();
        let s = bilinear();
        let uv = Point2::new(0.5, 0.5);
        let dir = Vec2::new(1.0, 0.0);
        let cache =
            get_surface_cache::<crate::surface::surface::p4::NoObject>(k, &s, None).unwrap();
        assert_eq!(
            s.domain_with_continuity(k, ContinuityType::G1, uv, dir)
                .unwrap(),
            s.domain_with_continuity_from_cache(k, ContinuityType::G1, uv, dir, &cache)
                .unwrap()
        );
    }

    fn same_points(a: &Surface, b: &Surface) {
        let k = &Kernel::default();
        for uv in [(0.0, 0.0), (0.25, 0.5), (1.0, 0.75)] {
            let uv = Point2::new(uv.0, uv.1);
            assert_eq!(
                a.evaluate_point(k, uv).unwrap(),
                b.evaluate_point(k, uv).unwrap()
            );
        }
    }

    /// `CreateOffsetSurface` is virtual: a plane offsets to a moved plane, not to an
    /// `SmOffsetSurface` over it.
    #[test]
    fn create_offset_surface_dispatches_to_plane() {
        let k = Kernel::default();
        let plane = Plane::from_normal(Point3::new(1.0, 2.0, 3.0), Vec3::Z);
        let off = Surface::from(plane.clone())
            .create_offset_surface(&k, 2.0, 1e-3)
            .unwrap()
            .unwrap();
        assert!(matches!(off.kind(), SurfaceKind::Plane(_)));
        same_points(
            &off,
            &Surface::from(plane.create_offset_surface(&k, 2.0).unwrap()),
        );
        let base = Surface::from(plane)
            .evaluate_point(&k, Point2::new(0.25, 0.5))
            .unwrap();
        let moved = off.evaluate_point(&k, Point2::new(0.25, 0.5)).unwrap();
        assert_eq!(moved - base, Vec3::new(0.0, 0.0, 2.0));
    }

    /// A surface of revolution offsets its generator, with the caller's tolerance.
    #[test]
    fn create_offset_surface_dispatches_to_revolution() {
        let k = Kernel::default();
        let mut l = Line::from_points(Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 0.0, 2.0), 3);
        l.make_nurb().unwrap();
        let rev = crate::surface::surf_of_revolution::SurfOfRevolution::create_canonical(
            &k,
            Curve::from(l),
            Vec3::ZERO,
            Vec3::Z,
            false,
        )
        .unwrap();
        for tol in [1e-6, 1e-2] {
            let off = Surface::from(rev.clone())
                .create_offset_surface(&k, 0.25, tol)
                .unwrap()
                .unwrap();
            assert!(matches!(off.kind(), SurfaceKind::SurfOfRevolution(_)));
            let direct = Surface::from(rev.create_offset_surface(&k, 0.25, tol).unwrap());
            same_points(&off, &direct);
        }
    }

    /// `IsPeriodic` ignores its evaluation statuses: on a bicubic surface collapsed to
    /// one point the zero-tangent repair fails, the raw derivatives are compared and
    /// both directions are periodic; the offset constructor that asks then succeeds.
    #[test]
    fn is_periodic_ignores_failed_tangent_repairs() {
        use crate::nurbs::math::{CPoint, NOW};
        use crate::nurbs::srf_basic::Surface as NlSurface;
        let k = &Kernel::default();
        let net = vec![vec![CPoint::new(1.0, 2.0, 3.0, NOW); 4]; 4];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let nl = NlSurface::new(net, 3, 3, knots.clone(), knots).unwrap();
        let s = Surface::from(BSplineSurface::from_nurb(nl, false));
        let dom = s.natural_uv_domain_if_built();
        // the plain evaluation reports the failed repair
        assert!(s
            .evaluate(k, Point2::new(0.5, 0.5), 1, 1, upper_flags(true))
            .is_err());
        assert!(s.is_periodic(k, &dom, SurfParamType::U).unwrap());
        assert!(s.is_periodic(k, &dom, SurfParamType::V).unwrap());
        let k = Kernel::default();
        let off = OffsetSurface::new(&k, 1.0, s, true, 0).unwrap();
        assert_eq!(off.is_periodic(), [1, 1]);
    }

    /// A B-spline surface runs the base body: an owning `SmOffsetSurface`.
    #[test]
    fn create_offset_surface_wraps_b_spline() {
        let k = Kernel::default();
        let off = bilinear()
            .create_offset_surface(&k, 0.5, 1e-3)
            .unwrap()
            .unwrap();
        let SurfaceKind::OffsetSurface(o) = off.kind() else {
            panic!("expected an offset surface");
        };
        assert_eq!(o.offset_distance(), 0.5);
    }

    #[test]
    fn step_surface_wraps_base() {
        let k = &Kernel::default();
        let base = bilinear();
        let st = Surface::new(SurfaceKind::Step(
            StepSurface::new(base.clone(), true).unwrap(),
        ));
        assert_eq!(st.type_code(), STEP_SURFACE_TYPE);
        assert!(st.is_kind_of(STEP_SURFACE_TYPE).unwrap());
        let uv = Point2::new(0.5, 0.5);
        assert_eq!(
            st.evaluate_point(k, uv).unwrap(),
            base.evaluate_point(k, uv).unwrap()
        );
        let (knots, mults) = st.knots(SurfParamType::U, None).unwrap();
        assert_eq!(knots, vec![0.0, 1.0]);
        assert_eq!(mults, vec![2, 2]);
        assert!(st.approx_eq(k, &st.clone()).unwrap());
        assert!(!st.approx_eq(k, &base).unwrap());
        let empty = StepSurface::empty();
        assert!(empty.evaluate_point(k, uv).is_err());
    }

    #[test]
    fn degeneracy_and_periodicity() {
        let k = &Kernel::default();
        let s = bilinear();
        assert!(!base_is_degenerate_point(k, &s, DEGENERATE_TOL_3D, None).unwrap());
        assert!(!base_is_degenerate_curve(k, &s, EFF_ZERO_DEG, None, DEGENERATE_TOL_3D).unwrap());
        let line = Extent2d::from_bounds(0.5, 0.0, 0.5, 1.0);
        assert!(
            base_is_degenerate_curve(k, &s, EFF_ZERO_DEG, Some(&line), DEGENERATE_TOL_3D).unwrap()
        );
        let d = s.natural_uv_domain_if_built();
        assert!(!s.is_periodic(k, &d, SurfParamType::U).unwrap());
        assert!(!s.is_analytic().unwrap());
    }

    #[test]
    fn snap_and_seams() {
        let s = bilinear();
        let p = s
            .snap_to_knots(&Kernel::default(), Point2::new(0.999, 0.4), 0.01)
            .unwrap();
        assert_eq!(p, Point2::new(1.0, 0.4));
        let d = s.natural_uv_domain_if_built();
        // bilinear: never a seam
        assert!(!crossed_seam_or_pole(
            &Kernel::default(),
            &s,
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 1.0),
            &d
        )
        .unwrap());
    }

    #[test]
    fn cache_validation() {
        assert_eq!(
            validate_surface_cache(ObjectCacheType::Surface, None, false, false).unwrap(),
            None
        );
        assert!(validate_surface_cache(ObjectCacheType::Curve, None, false, false).is_err());
    }

    #[test]
    fn faces_of_owners() {
        let mut s = bilinear();
        assert_eq!(s.face().unwrap(), None);
        let face = ObjectRef {
            type_code: FACE_TYPE,
            key: crate::geom::attribute::ObjectKey(0),
        };
        assert_eq!(s.set_owner(Some(face)), None);
        assert_eq!(s.face().unwrap(), Some(face));
    }
}

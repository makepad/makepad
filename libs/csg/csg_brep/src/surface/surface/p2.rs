//! `SmSurface`, part 2: the surface object ([`Surface`]: a concrete surface kind plus
//! the owner pointer) with its type queries and owner, the virtual interface of
//! `SmSurface` dispatched over the surface kinds (evaluation, domains, degrees,
//! knots, parameterisation edits, trimming, splitting, extension, copying, reading
//! from a database stream), the lazy evaluator [`SurfaceEval`], the derivative
//! surface definition [`DerivSurfDefinition`], the drop-curve failure record
//! [`DropCurveFail`], the standalone surface numerics (`smsurf_*`: chain-rule lifts,
//! geometric properties, point classification, vector drops) and the file-static
//! helpers of `SmSurface.cpp` owned by this unit.
//!
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Surface`], [`SurfaceKind`]                      | `SmSurface` (and its derived classes)            |
//! | [`SurfaceEvaluate`]                               | the evaluation virtuals generic callers use      |
//! | `Drop` (implicit), `Clone`                        | `~SmSurface`, copy constructor (owner reset)     |
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`Surface::type_code`], [`Surface::type_string`], [`Surface::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`Surface::is_kind_of`]           | `SmSurface::IsKindOf`                            |
//! | [`Surface::owner`]                                | `GetOwner`                                       |
//! | [`Surface::copy`], [`Surface::copy_and_add_analytics`] | `Copy`, `CopyAndAddAnalytics`               |
//! | [`Surface::adjust_step_uv_domain`]                | `AdjustSTEPUVDomain`                             |
//! | [`Surface::create_extended_surface`], [`Surface::create_extended_surface_dir`] | `CreateExtendedSurface` (both) |
//! | [`Surface::drop_point_fast`]                      | `DropPointFast`                                  |
//! | [`Surface::evaluate`], [`Surface::evaluate_point`] | `Evaluate`, `EvaluatePoint`                     |
//! | [`Surface::natural_uv_domain_if_built`], [`Surface::step_uv_domain`] | `GetNaturalUVDomain`, `GetSTEPUVDomain` |
//! | [`Surface::degree`], [`Surface::number_control_points`] | `GetDegree`, `GetNumberControlPoints`       |
//! | [`Surface::max_analytic_domain`]                  | `GetMaxAnalyticDomain`                           |
//! | [`Surface::is_bilinear`], [`Surface::is_parallel_to_vector`] | `IsBilinear`, `IsParallelToVector`    |
//! | [`Surface::intersect_with_plane`]                 | `IntersectWithPlane`                             |
//! | [`Surface::reverse`], [`Surface::reparameterize`] | `Reverse`, `Reparameterize`                      |
//! | [`Surface::split_at`], [`Surface::step_inversion`] | `SplitAt`, `STEPInversion`                      |
//! | [`Surface::swap_uv`], [`Surface::transform`], [`Surface::trim_with_domain`], [`Surface::trim_with_domain_with`] | `SwapUV`, `Transform`, `TrimWithDomain` |
//! | [`Surface::knots`]                                | `GetKnots`                                       |
//! | [`Surface::convert_uv_from_step_to_nurbs`], [`Surface::convert_uv_from_nurbs_to_step`] | `ConvertUVFromSTEPToNURBS`, `ConvertUVFromNURBSToSTEP` |
//! | [`Surface::curve_on_classify`]                    | `CurveOnClassify`                                |
//! | [`Surface::global_surface_intersect`], [`base_global_surface_intersect`], [`SurfaceIntersection`] | `GlobalSurfaceIntersect` (virtual: the `SmPlane` override in `surface::plane::p2`; the `SmSurface` body) |
//! | [`Surface::global_surface_intersect_with_partial`] | `GlobalSurfaceIntersect` with the outputs left in the caller's arrays on a failure |
//! | [`Surface::point_based_surface_intersect`], [`PointIntersection`] | `PointBasedSurfaceIntersect` (UV point version) |
//! | [`Surface::read_from_db`]                         | `SmSurface::ReadFromDB`                          |
//! | [`Surface::create_plane_with_3d_curves`], [`PlaneWith3dCurves`] | `CreatePlaneWith3DCurves`          |
//! | [`derivative_limit`], [`normal_limit`]            | `GetDerivativeLimit`, `GetNormalLimit` (static)  |
//! | [`notify`]                                        | `SmSurface::Notify`                              |
//! | `base_*` functions                                | the `SmSurface.h` inline defaults of the virtuals |
//! | [`SurfaceEval`]                                   | `SmSurfaceEval` (constructors, `operator=`, `SetUV`, `SetSectDir`, `GetUV`, `Eval`) |
//! | [`DerivSurfDefinition`], [`TangencyType`], [`CurvatureType`], [`BlendEndType`] | `SmDerivSurfDefinition` (constructor, `AssertValid`, `IsFromSurfaceBoundaryCurve`, `GetNumDerivs`, `CreateCrossSectionCurves`, `SkinDerivSurface`, `GetClassString`, `GetClassType`), `SmTangencyType`, `SmCurvatureType`, `SmBlendEndType` |
//! | [`DropCurveFail`], [`DropCurveFailArgs`], [`CreateUvTrimCurvePathType`] | `SmDropCurveFail` (`operator=`, `UnInit`, `Set`), `SmCreateUVTrimCurvePathType` |
//! | [`lift_second_derivative`], [`lift_third_derivative`] | `smsurf_LiftSecondDerivative`, `smsurf_LiftThirdDerivative` |
//! | [`scale_step_angular_derivatives`]                | `smsurf_ScaleSTEPAngularDerivatives`             |
//! | [`evaluate_geometric`], [`GeometricProperties`]   | `smsurf_EvaluateGeometric`                       |
//! | [`compute_surface_point_type`], [`LocalSurfaceType`], [`SurfaceSideType`] | `smsurf_ComputeSurfacePointType`, `SmLocalSurfaceType`, `SmSurfaceSideType` |
//! | [`drop_vectors`]                                  | `smsurf_DropVectors`                             |
//! | [`surface_cache`]                                 | `smsurf_GetSurfaceCache`                         |
//! | [`original_surface`]                              | `sm_GetOriginalSurface` (static)                 |
//! | [`cc_local_solve`]                                | `sm_CCLocalSolve` (static)                       |
//! | [`merge_double_arrays`]                           | `sm_MergeDoubleArrays` (static)                  |
//! | [`convert_to_parameter_delta`]                    | `sm_ConvertToParameterDelta` (static)            |
//! | [`cs_are_perpen_disjoint`]                        | `sm_CsArePerpenDisjoint` (static)                |
//! | [`intersect_and_split`], [`intersect_curve_and_split`] | `sm_IntersectAndSplit`, `sm_IntersectCurveAndSplit` (static) |
//! | [`set_up_breaks`]                                 | `sm_SetUpBreaks` (static)                        |
//! | [`map_uv`], [`is_even`]                           | `s_MapUV`, `s_IsEven` (static)                   |
//! | [`LsiGlobalSolver`]                               | `SmLSIGlobalSolver` (`BranchMayContainAnswers`)  |
//! | [`IsoCurveLengthIntegrand`]                       | `SmIsoCurveLengthIFEO` (constructor)             |
//!
//! Deviations:
//! - **Output arrays.** `GlobalSurfaceIntersect`'s output arrays are a
//!   `&mut SurfaceIntersection` threaded through the kind dispatchers and the special
//!   cases that can fail after appending (the `*_into` functions), so a failure leaves in
//!   it what upstream leaves in the caller's arrays
//!   ([`Surface::global_surface_intersect_with_partial`]). The general intersection
//!   resets them first (`SmSurfaceIntersector::DoIntersection`): a special case that asks
//!   for it keeps none of its own outputs.
//! - **Touching loops** (not upstream, WR-405, source issue #892, opt-in
//!   `Kernel::fixes.join_touching_loops`): `create_plane_with_3d_curves`
//!   joins a loop touching another loop of its face at a vertex into that loop
//!   (`join_touching_loops`, the source fork's `JoinTouchingLoops`).
//! - **Edit notifications.** `SwapUV`'s `Notify` calls (`SetCanonical`'s for the
//!   kinds built on `SmBSplineSurface`, `SmSrfInVolume`'s own) need a context and an
//!   attribute store the surface does not hold: [`Surface::swap_uv`] notifies nobody
//!   (only `SmSurface::Notify`'s trimmed surface cache deletion runs) and the
//!   crate-private `Surface::swap_uv_notifying` hands each edited surface's identity
//!   slot to the caller's hook (the face's `SwapUV` sends them through its
//!   environment). A trimmed surface cache in use when the swap notifies stays with its
//!   caller (upstream deletes the object the caller holds).
//! - **Nesting.** [`MAX_GEOMETRY_NESTING`] (32, the database reader's bound; upstream
//!   fixtures nest at most 3) bounds wrapper geometry across curves, surfaces and
//!   volumes. Every wrapper constructor and setter checks its inner geometry with the
//!   non-recursive `check_geometry_nesting`, so the recursive dispatch below, `Clone`
//!   and `Drop` stay bounded (no limit upstream). 32 rather than 64 because a 64-deep
//!   offset chain overflows the 1 MiB wasm32 stack unoptimised (`topology::database_io`,
//!   "Stack use").
//! - **Owner tolerance.** A surface holds only an owner handle, never its B-rep, so the
//!   face tolerance upstream reads through `GetFace()->GetTolerance()` is cached on the
//!   surface by the B-rep ([`Surface::owner_zone_tol_3d`], kept in step where a face
//!   record's surface or tolerance changes) together with the face it belongs to; an
//!   owner change drops it, the copy constructor resets it and a bare face handle
//!   without it returns [`SolidError::Unported`] (`SmFace::GetTolerance`).
//! - Evaluation dispatch and copies check all owned surface, curve and volume branches,
//!   including curve-on-surface children, against the same bound before invoking
//!   recursive copies. The surface evaluator also bounds its wrapper traversal (a loop)
//!   to 64 levels and retains B-spline output written before zero-tangent correction
//!   fails, as upstream accessors do. Leaf B-spline and plane checks avoid allocating a
//!   traversal stack, while preserving the same depth-limit rejection.
//! - **Object model.** `SmSurface` is not abstract upstream, but every ported surface
//!   is a derived class: the [`SurfaceKind`] variants. The ported kinds are
//!   [`BSplineSurface`], [`CurveBoundedSurface`] (unit
//!   `surface.curve_bounded_surface`) and [`Plane`] (unit `surface.plane_p1`: the
//!   inherited `SmBSplineSurface` methods run on its B-spline part, re-dispatched to
//!   the plane's overrides; `Transform`, `TrimWithDomain`, `ReadFromDB`,
//!   `IntersectWithPlane` and `GlobalSurfaceIntersect` are those of
//!   `surface.plane_p2`, `STEPInversion` needs the point solvers of
//!   `surface.surface_p1`) and [`SurfOfRevolution`] (units
//!   `surface.surf_of_revolution_p1` and `_p3`: the inherited `SmBSplineSurface`
//!   methods run on its B-spline part, `Reverse` and `SwapUV` then rebuild its analytic
//!   data through the virtual `RebuildSTEPFromNURBParameters`; `STEPInversion`,
//!   `AdjustSTEPUVDomain` and `TrimWithDomain` belong to `surface.surf_of_revolution_p2`;
//!   `Reverse`, `Reparameterize`, `SwapUV`, `Transform`, the parameter conversions and
//!   `DropPointFast` take the kernel for it); the other analytic and generated surfaces
//!   (`SmCone`/`SmCylinder` are retained in the `Cone` variant by integration #76,
//!   `SmSphere` and `SmTorus` in the `Sphere` and `Torus` variants, `SmSurfOfExtrusion`
//!   in the `SurfOfExtrusion` variant: its overrides, else the inherited
//!   `SmBSplineSurface` bodies on its B-spline part). Each virtual method is one `match`: a
//!   kind that overrides it calls its own method, the others run the base-class body
//!   ported here. The base bodies are also public as `base_*` functions (they are
//!   what a kind without an override runs, and what difftest compares with an
//!   upstream `SmSurface()`).
//! - **Owner.** `m_pOwner` is an [`ObjectRef`] instead of a pointer; `Clone` resets
//!   it, as the copy constructor. `SetOwner` belongs to unit `surface.surface_p1`.
//!   The constructor and destructor notifications and the debug cache counters are
//!   not modelled; [`notify`] is the `Notify` override for callers that keep the
//!   surface's [`AObject`] state. `m_bUseNewPointSolvers` is always `TRUE`
//!   (`sbUseNewPointSolvers`) and is not stored.
//! - **Arguments.** Out-params become return values; `SmContext` arguments only
//!   choose where upstream allocates; evaluation, solvers and callbacks explicitly
//!   retain the caller's Kernel, including implicit iso curves. `Evaluate`'s internal
//!   `bDoZeroSampling` argument is not exposed (always `TRUE`). `SmSurfaceEval::Eval`'s
//!   `bSignalFails` only controls a debug assertion and is dropped. Upstream's
//!   error-reporting macros (`ERR`, `SE`) only log; where the base default returns a
//!   value after them (`GetNaturalUVDomain`, `GetDegree`, ...) the port returns the
//!   same value.
//! - **Unported callees.** `GlobalSurfaceIntersect` runs
//!   `SmSurfaceIntersector::DoIntersection` (unit `intersect.surface_intersector_p5`);
//!   `CreatePlaneWith3DCurves`
//!   needs `SmTrimmingTools` (`topology.trimming_tools`); `smsurf_GetSurfaceCache`
//!   needs the cache store to make surface caches (`SmSurface::CacheMakeOrValidate`,
//!   `surface.surface_p1`); the polar box test of `sm_CsArePerpenDisjoint` needs the
//!   `SmBezierPatch` of a tree node, which the tree reaches once the tree construction
//!   (`surface.surface_cache_p3`) hands the cache's patch data over;
//!   `ReadFromDB` of the analytic and generated surfaces needs their units. Those
//!   calls return [`SolidError::Unported`].
//! - **`SmDerivSurfDefinition`.** The reference curves are borrowed
//!   ([`Option<&Curve>`]). `AssertValid` runs every test (all are `SM_LEVEL_0` and the
//!   release build ignores the test level and the test requests), so it takes only
//!   the report list and the object key. `SkinDerivSurface` takes the rails mutably:
//!   upstream skins with two rails synchronise them in place through a const cast.
//! - **`SmDropCurveFail`.** The pointers it stores (context, surface, curve, UV curve
//!   array, B-rep) are never dereferenced except to copy UV curves when a context is
//!   given; they are caller handles here ([`Option<O>`]) and the context is a flag.
//!   `operator=` is [`DropCurveFail::assign`] (the copy constructor, unit
//!   `surface.surface_p1`, is `Clone`).
//! - **`smsurf_ComputeSurfacePointType`** keeps upstream's bug: the `v` side is
//!   written to the `u` side's output, and the `v` output is never written. So the
//!   `u` output stays an optional `&mut` argument and the `v` output is a flag.
//! - **`sm_MergeDoubleArrays`** keeps upstream's early exit: values left in one array
//!   after the other is exhausted are dropped.
//! - **`sm_CreateCrossSectionCurves`** keeps upstream's use of the start second
//!   derivatives for the Taylor series' third-derivative term.
//! - **`SmLSIGlobalSolver`.** The constructor is unit `surface.surface_p1`; the
//!   struct's fields are public. `LocalSolve` is unit `surface.surface_p10`
//!   ([`LsiGlobalSolver::local_solve_branch`]; it reads the node patches through the
//!   borrowed `surface_cache`), `IdenticalSolutions` (`surface.surface_p5`) returns
//!   [`SolidError::Unported`]. A
//!   failing `SmExtent3d::IntersectLine` (zero line vector, undefined box) leaves
//!   upstream's hit count uninitialised; the port returns its error.
//! - **Not ported here.** The destructors of the file-local evaluator classes
//!   (`SmFindSingularParamWithGivenTangentDirEFO`, `SmCSIntersectENFO`,
//!   `SmCSNormalizeENFO`, `SmFindPSNormalENFO`, `SmSSPIntersectENFO`,
//!   `SmSSINormalizeENFO`, `SmSSNormalizeENFO`, `SmSSSIntersectENFO`) and of
//!   `SmSurfaceEval`, `SmDerivSurfDefinition`, `SmDropCurveFail` and
//!   `SmLSIGlobalSolver` are implicit `Drop`; the classes themselves belong to other
//!   units.
//! - **Cache slot.** A face surface keeps its trimmed surface cache in its cache slot
//!   ([`Surface::take_trim_cache`], [`Surface::store_trim_cache`], [`TrimCacheSlot`]):
//!   the face lookups (`query::face::p2::face_surface_cache`) make, validate and reuse
//!   it as `SmCacheMgrTSrf::CacheMakeOrValidate` does, and a face-unaware lookup made
//!   while the face's `CreateUVTrimCurve(s)` / `IntersectVertices` has cleared the
//!   owner ([`Surface::begin_owner_cleared`]) keeps the standalone cache it made
//!   ([`Surface::keep_standalone_trim_cache`]). The stored cache's own surface copy
//!   gets a slot of its own (no self-reference); a cache taken out shares the slot
//!   again until it is stored. Other surface caches are built afresh instead of being
//!   kept on the surface (`m_pCacheObj`), but whether that cache's tree has been built
//!   is observable: `GetDomainWithContinuity`, `LiftCurve`, `GetSurfaceContinuity` and
//!   the drop-curve boundary search read the continuity arrays without `GetTree`.
//!   [`Surface::cache_tree_state`] keeps that bit of history ([`CacheTreeState`]) in a
//!   slot shared by the surface and its evaluation copies (which stand for upstream's
//!   pointers to the one object); `Clone` starts an empty slot, as the copy
//!   constructor leaves `m_pCacheObj` null. The cache builds of `surface.surface_p1`
//!   and `surface.surface_cache_p3` keep it in step.
//! - **Lazy NURB degree and knots.** Upstream `GetDegree` / `GetKnots` make a missing
//!   NURB (`GetOrCreateGwNurbPointer`); [`Surface::degree`] and [`Surface::knots`] read
//!   the stored base, so a lazy extrusion, cone, sphere or torus answers `0` / the
//!   NURB-less base. [`Surface::degree_with_nurb`] and [`Surface::knots_with_nurb`] answer
//!   as upstream and serve the callers that hold a kernel. `SurfaceEvaluate::knots`,
//!   `Surface::snap_to_knots` and `Surface::domain_with_continuity_from_cache` take the
//!   kernel for it. Callers without one still read the stored base:
//!   `SurfaceCache::surface_continuity`, the offset surface's and the surface-in-volume's
//!   inherent `knots`, and `IsoCurve::knots` (a kernel would change their signatures).
//! - **Made NURB.** An analytic kind without a NURB makes one for `const` queries
//!   ([`Surface::kind_with_nurb`]), as upstream's `GetOrCreateGwNurbPointer`. A plane
//!   and a surface of extrusion keep it in their own NURB slot (filled once from
//!   `&self`), so `kind()`, copies and `operator==` see it as upstream. A cone, sphere,
//!   torus or surface of revolution's `MakeNurb` also rebuilds its polar converter and
//!   may trim its generator, so the surface keeps the whole made kind (`MadeKindSlot`,
//!   filled once from `&self`) and [`Surface::kind`] answers it from then on: `kind()`
//!   readers, copies (`Clone` copies the made object; [`Surface::copy`] first makes the
//!   source's NURB, as the copy constructor does) and `operator==` see it. Evaluation
//!   copies share the cell (upstream's pointer to the one object); `&mut` access moves
//!   the made kind into the surface first, on a cell of its own, so an edit applies to
//!   the made object and reaches no other surface. `Clone` itself cannot make a missing
//!   NURB first (that needs the [`Kernel`]). As upstream's NURB on the
//!   object, neither is keyed on the [`Kernel`] the make used.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurvePropertyType};
use crate::curve::line::Line;
use crate::error::{ignore_status, EntityRef, Result, SolidError, SM_ERR};
use crate::geom::a_object::{self, AObject, CacheStore};
use crate::geom::assert_array::labels::DERIV_SURF_DEFINITION_TYPE;
use crate::geom::assert_array::{AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::geom::attribute::AttributeStore;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{Context, IdentitySlot};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_closest_point, plane_drop_point, vec_lin_comb_two_vectors,
};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    Branch, GlobalSolve, GlobalSolver, PreferredSolution, SolutionPairType, SolutionRequested,
    SolverOperation,
};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::polynomial::solve_quadratic;
use crate::geom::solution_array::{OperandObject, Solution, SolutionArray, SolutionObject};
use crate::geom::tol::{are_same, eff_zero_param, scaled_zero2, tol3d};
use crate::geom::tree::{AuxDataType, Tree, TreeNode, TreeOwner};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::intersect::adv_surface_intersector::{AdvSurfaceIntersector, NoSurfaceHandle};
use crate::intersect::curve_class::{CurveClassification, ResetOptions};
use crate::intersect::point_class::{ClassGeometry, ClassObject, PointClassType};
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectId, TsectPnt};
use crate::intersect::surface_intersector::p4::PointIntersectionOptions;
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{BSplineSurface, LocationType, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::{
    EvaluateFlags, OrientType, SkinnedSurfaceOptions, TrimOptions,
};
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::knots::Side;
use crate::surface::cone::Cone;
use crate::surface::curve_bounded_surface::CurveBoundedSurface;
use crate::surface::offset_surface::OffsetSurface;
use crate::surface::plane::Plane;
use crate::surface::sphere::Sphere;
use crate::surface::srf_in_volume::SrfInVolume;
use crate::surface::surf_of_extrusion::SurfOfExtrusion;
use crate::surface::surf_of_revolution::p3::intersect_revolved_into;
use crate::surface::surf_of_revolution::{Revolved, SurfOfRevolution};
use crate::surface::surface::p1::{StepSurface, SurfaceIntersectContext, SurfaceSolveContext};
use crate::surface::surface::p8::BoundaryMemo;
use crate::surface::surface::p8::FaceSolveCache;
use crate::surface::surface_cache::p1::SurfaceCache;
use crate::surface::torus::Torus;
use crate::surface::trim_srf_cache::TrimSrfCache;
use crate::topology::database_io::DatabaseReader;
use crate::topology::trimming_tools;
use crate::topology::FaceId;

// ---- type codes ---------------------------------------------------------------------

/// `SURF_BASE_TYPE` (`SmSurfTypes.h`).
const SURF_BASE_TYPE: i64 = 15000;

/// `SmSurface_TYPE` (`SURF_BASE_TYPE + 1`).
pub const TYPE: i64 = SURF_BASE_TYPE + 1;
/// `SmBSplineSurface_TYPE`.
pub const B_SPLINE_SURFACE_TYPE: i64 = SURF_BASE_TYPE + 2;
/// `SmPlane_TYPE`.
pub const PLANE_TYPE: i64 = SURF_BASE_TYPE + 11;
/// `SmCone_TYPE`.
pub const CONE_TYPE: i64 = SURF_BASE_TYPE + 12;
/// `SmCylinder_TYPE`.
pub const CYLINDER_TYPE: i64 = SURF_BASE_TYPE + 13;
/// `SmSphere_TYPE`.
pub const SPHERE_TYPE: i64 = SURF_BASE_TYPE + 14;
/// `SmTorus_TYPE`.
pub const TORUS_TYPE: i64 = SURF_BASE_TYPE + 15;
/// `SmSurfOfRevolution_TYPE`.
pub const SURF_OF_REVOLUTION_TYPE: i64 = SURF_BASE_TYPE + 20;
/// `SmSurfOfExtrusion_TYPE`.
pub const SURF_OF_EXTRUSION_TYPE: i64 = SURF_BASE_TYPE + 30;
/// `SmCurveBoundedSurface_TYPE`.
pub const CURVE_BOUNDED_SURFACE_TYPE: i64 = SURF_BASE_TYPE + 45;
/// `SmSrfInVolume_TYPE`.
pub const SRF_IN_VOLUME_TYPE: i64 = SURF_BASE_TYPE + 218;
/// `SmOffsetSurface_TYPE`.
pub const OFFSET_SURFACE_TYPE: i64 = SURF_BASE_TYPE + 400;
/// `SmSTEPSurface_TYPE`.
pub const STEP_SURFACE_TYPE: i64 = SURF_BASE_TYPE + 410;

/// `GetTypeString()` of `SmSurface`.
pub const TYPE_STRING: &str = "SmSurface_TYPE";
/// `GetClassString()` of `SmSurface`.
pub const CLASS_STRING: &str = "SmSurface";

/// `SmSurface::IsKindOf`: `SmSurface_TYPE` or a base type of `SmSurface`.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || a_object::is_kind_of(t)
}

const OP_BASE: &str = "surface::surface::p2::Surface";
const OP_MIRROR: &str = "surface::surface::p2::Surface::create_mirror_surface";
const OP_READ: &str = "surface::surface::p2::Surface::read_from_db";
const OP_EVAL: &str = "surface::surface::p2::SurfaceEval::eval";
const OP_DERIV: &str = "surface::surface::p2::DerivSurfDefinition";
const OP_GEOM: &str = "surface::surface::p2::evaluate_geometric";
const OP_DROP: &str = "surface::surface::p2::drop_vectors";
const OP_BREAKS: &str = "surface::surface::p2::set_up_breaks";
const OP_CURVE_ON_CLASSIFY: &str = "surface::surface::p2::Surface::curve_on_classify";

/// `SM_ERR` of a base-class default.
const fn base_err() -> SolidError {
    SolidError::sm_err(OP_BASE)
}

// ---- the surface object ---------------------------------------------------------------

/// The concrete kind of a [`Surface`] (the derived classes of `SmSurface`).
#[derive(Debug, Clone)]
pub enum SurfaceKind {
    /// Native cone/cylinder class, including analytic state and its NURB.
    Cone(Box<Cone>),
    /// `SmBSplineSurface`.
    BSpline(BSplineSurface),
    /// `SmCurveBoundedSurface`.
    CurveBounded(CurveBoundedSurface),
    /// `SmPlane` (an `SmBSplineSurface`).
    Plane(Plane),
    /// `SmSurfOfRevolution` (an `SmBSplineSurface`; boxed, it is the largest kind).
    SurfOfRevolution(Box<SurfOfRevolution>),
    /// `SmSTEPSurface` (unit `surface.surface_p1`): another surface in its STEP
    /// parameterisation.
    Step(StepSurface),
    /// `SmOffsetSurface` (unit `surface.offset_surface`): another surface moved along
    /// its normal.
    OffsetSurface(OffsetSurface),
    /// `SmSrfInVolume`.
    SrfInVolume(Box<SrfInVolume>),
    /// `SmSphere` (an `SmSurfOfRevolution`; boxed, it holds one).
    Sphere(Box<Sphere>),
    /// `SmTorus` (an `SmSurfOfRevolution`; boxed, it holds one).
    Torus(Box<Torus>),
    /// `SmSurfOfExtrusion` (an `SmBSplineSurface`; boxed, it holds a generator).
    SurfOfExtrusion(Box<SurfOfExtrusion>),
}

/// A surface (`SmSurface`): its concrete kind and the object owning it (`m_pOwner`:
/// a face, another surface or a curve).
#[derive(Debug)]
pub struct Surface {
    kind: SurfaceKind,
    owner: Option<ObjectRef>,
    /// For a surface owned by another surface (the base of an owning
    /// `SmOffsetSurface`): the face upstream's `GetFace` reaches by continuing up that
    /// surface's own owner chain. The owning surface holds this surface by value, so it
    /// cannot walk up; the owner keeps this in step ([`Surface::set_owner`]).
    owner_chain_face: Option<ObjectRef>,
    /// The stored zone tolerance (`SmFace::m_sZoneTol3d`, [`crate::topology::Face`]'s
    /// `tolerance`) of the face this surface reaches ([`Surface::reached_face`]), with
    /// that face: the B-rep keeps it in step when it attaches a surface to a face or sets
    /// a face tolerance. Valid only while the recorded face is still the one reached, so
    /// an owner change can never leave another face's tolerance behind.
    owner_zone_tol_3d: Option<(ObjectRef, f64)>,
    /// The state of the surface's cache slot (`SmAObject::m_pCacheObj`), shared with
    /// its evaluation copies ([`CacheSlot`]).
    cache_slot: CacheSlot,
    /// The identity a context issued this surface object for its notifications
    /// (upstream passes the object itself, known by address); a copy has none.
    notify_identity: IdentitySlot,
    /// The kind a cone, sphere, torus or surface of revolution made its NURB in on
    /// first use from a `const` method ([`Surface::kind_with_nurb`], [`MadeKindSlot`]);
    /// [`Surface::kind`] answers it once filled.
    made_kind: MadeKindSlot,
    /// Not upstream: the memo of the natural boundary curves `CoincidenceCheck` makes
    /// (`BoundaryMemo`); a copy starts an empty one, an edit replaces it.
    boundary_memo: BoundaryMemo,
}

/// What a surface's cache slot (`SmAObject::m_pCacheObj`) holds, as far as a reader of
/// the cache's continuity arrays (`GetUContinuitiesArray`, `GetVContinuitiesArray`)
/// can tell without calling `GetTree`: the arrays are filled only by
/// `SmSurfaceCache::BuildTreeBase`, which runs when a cache is made outside a Boolean
/// operation or on the first `GetTree`, and the cache then lives on with the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheTreeState {
    /// No cache: the next `smsurf_GetSurfaceCache` makes one, building its tree
    /// unless the context is doing a Boolean operation.
    #[default]
    NoCache,
    /// A cache whose tree has not been built (made during a Boolean operation): its
    /// continuity arrays stay empty until a `GetTree` builds the tree.
    Unbuilt,
    /// A cache whose tree has been built: its continuity arrays are filled.
    Built,
}

/// The trimmed surface cache a surface's cache slot holds: the `SM_OC_TRIMSRF` entry of
/// upstream's global cache queue for the surface (`SmCacheMgr::GetObjectCache`), in the
/// handle space of the surface's face ([`EntityRef`]).
#[derive(Debug, Default)]
pub enum TrimCacheSlot {
    /// No trimmed cache.
    #[default]
    Empty,
    /// The cache, not in use. Its own surface copy does not share the slot (that would
    /// make the slot own itself).
    Stored(Box<TrimSrfCache<EntityRef>>),
    /// The cache is in use: a caller took it out ([`Surface::take_trim_cache`]) and puts
    /// it back when done. Upstream's lookups in the meantime find that same object
    /// (it is in the queue before its tree is built).
    CheckedOut,
}

/// What a [`CacheSlot`] holds.
#[derive(Debug, Default)]
struct SlotState {
    tree: CacheTreeState,
    trim: TrimCacheSlot,
    /// The face was modified while the trimmed cache was in use.
    face_modified_while_out: bool,
    /// The faces whose `CreateUVTrimCurve(s)` has cleared this surface's owner (one
    /// entry per active clearing, innermost last; see
    /// [`Surface::begin_owner_cleared`]).
    cleared_for: Vec<FaceId>,
}

/// The cache slot of a surface. Upstream keeps a surface's cache on the surface object
/// and everything that works on the surface reaches it through a pointer; the port
/// passes evaluation copies ([`Surface::clone_for_evaluation`]) where upstream passes
/// that pointer. The slot records what a later reader of the cache can observe, shared
/// between a surface and its evaluation copies so that what is done through any of
/// them is seen by all: whether the cache's tree was built ([`CacheTreeState`]) and,
/// once one has been made, the surface's trimmed surface cache itself
/// ([`TrimCacheSlot`]). `Clone` (the copy constructor, which leaves `m_pCacheObj` null)
/// starts an empty slot. Single-threaded use only orders the accesses, so the result
/// is deterministic.
#[derive(Debug, Default)]
struct CacheSlot(Arc<Mutex<SlotState>>);

impl CacheSlot {
    /// The same slot (an evaluation copy stands for the same surface object).
    fn share(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// Whether `self` and `other` are the same slot (stand for the same surface
    /// object): an identity test, never an ordering.
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn with<R>(&self, f: impl FnOnce(&mut SlotState) -> R) -> R {
        // a poisoned lock only means a panic unwound through `with`; every write
        // leaves the state consistent, so it stays usable
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    fn get(&self) -> CacheTreeState {
        self.with(|s| s.tree)
    }

    fn set(&self, state: CacheTreeState) {
        self.with(|s| s.tree = state);
    }
}

/// `GetOrCreateGwNurbPointer` called from a `const` method of a cone (or cylinder),
/// sphere, torus or surface of revolution that has no NURB yet (a plane and an
/// extrusion keep theirs in their own [`NurbSlot`](crate::nurbs::b_spline_surface)):
/// upstream runs the virtual `MakeNurb` on the object and keeps the result, so every
/// later reader sees it. `SmSurfOfRevolution::MakeNurbWithSweepParams` changes more
/// than the NURB: it may trim the generator (`AdjustSTEPInterval`) and it rebuilds the
/// polar converter. The slot therefore keeps the whole made kind, filled once from
/// `&self`, and [`Surface::kind`] answers it from then on, so `kind()` readers, the copy
/// constructor ([`Clone`]) and `operator==` see the made object, as upstream.
///
/// The cell sits behind an `Arc` so that an evaluation copy (upstream's pointer to the
/// same object, [`Surface::clone_for_evaluation`]) shares it, as it shares a plane's
/// NURB cell: a NURB made through the copy stays on the surface. Every `&mut` access to
/// the kind first moves the made kind into the surface ([`MadeKindSlot::take_for_edit`]),
/// leaving this surface a cell of its own, so an edit applies to the made object (as
/// upstream's edits do) and never reaches another surface sharing the cell. The made
/// kind depends only on the surface's own data, so the result is deterministic.
#[derive(Debug, Default)]
struct MadeKindSlot(Arc<std::sync::OnceLock<Box<SurfaceKind>>>);

const OP_KIND_WITH_NURB: &str = "surface::surface::p2::Surface::kind_with_nurb";

impl MadeKindSlot {
    /// The same cell (an evaluation copy stands for the same surface object).
    fn shared(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// The made kind, if any.
    fn get(&self) -> Option<&SurfaceKind> {
        self.0.get().map(AsRef::as_ref)
    }

    /// Stores `made` when no kind was made yet (the first fill wins, as upstream's
    /// `GetOrCreateGwNurbPointer` makes once) and returns the stored kind.
    fn fill(&self, made: SurfaceKind) -> &SurfaceKind {
        self.0.get_or_init(|| Box::new(made))
    }

    /// Takes the made kind out for an edit of this surface and leaves this surface an
    /// empty cell of its own. A cell shared with evaluation copies keeps its kind for
    /// them; this surface then takes an exact copy of it.
    fn take_for_edit(&mut self) -> Option<SurfaceKind> {
        match Arc::try_unwrap(core::mem::take(&mut self.0)) {
            Ok(cell) => cell.into_inner().map(|made| *made),
            Err(shared) => shared.get().map(|made| clone_kind_exact(made)),
        }
    }
}

/// An exact copy of a kind kept in a [`MadeKindSlot`] (a cone, sphere, torus or surface
/// of revolution): the B-spline objects are copied as they are
/// ([`SurfOfRevolution::clone_exact`]), not through the copy constructor's knot fix-up,
/// and a torus keeps its inside-out flag, which its copy constructor resets.
fn clone_kind_exact(kind: &SurfaceKind) -> SurfaceKind {
    match kind {
        SurfaceKind::Cone(s) => {
            let mut c = s.as_ref().clone();
            c.rev = s.rev().clone_exact();
            SurfaceKind::Cone(Box::new(c))
        }
        SurfaceKind::Sphere(s) => {
            let mut c = s.as_ref().clone();
            c.base = s.base().clone_exact();
            SurfaceKind::Sphere(Box::new(c))
        }
        SurfaceKind::Torus(s) => {
            let mut c = s.as_ref().clone();
            c.base = s.base().clone_exact();
            c.inside_out = s.inside_out;
            SurfaceKind::Torus(Box::new(c))
        }
        SurfaceKind::SurfOfRevolution(s) => {
            SurfaceKind::SurfOfRevolution(Box::new(s.clone_exact()))
        }
        other => other.clone(),
    }
}

/// The copy constructor: copies the geometry, resets the owner (an owning offset in it
/// then re-owns its base, whose chain no longer reaches a face).
impl Clone for Surface {
    fn clone(&self) -> Self {
        let mut s = Self {
            // (the made kind when a `const` query made the NURB: the copy constructor
            // copies the object as it is)
            kind: self.kind().clone(),
            owner: None,
            owner_chain_face: None,
            owner_zone_tol_3d: None,
            cache_slot: CacheSlot::default(),
            notify_identity: IdentitySlot::new(),
            made_kind: MadeKindSlot::default(),
            boundary_memo: BoundaryMemo::new(),
        };
        s.update_owned_bases();
        s
    }
}

impl Surface {
    /// A temporary evaluation copy: retain the owner and exact NURBS data rather
    /// than invoking the upstream copy constructor's knot fix-up.
    pub fn clone_for_evaluation(&self) -> Result<Self> {
        self.clone_for_evaluation_depth(0)
    }

    /// Check every owned geometry branch before invoking recursive `Clone` impls.
    /// This includes trim curves, curve-on-surface children and volume chains, not only
    /// the surface basis.
    pub(crate) fn check_evaluation_copy_depth(&self, initial_depth: usize) -> Result<()> {
        // These variants own no nested geometry. Preserve the depth-limit error
        // through the general walker, but avoid its stack allocation for leaves.
        if initial_depth <= MAX_GEOMETRY_NESTING
            && matches!(self.kind(), SurfaceKind::BSpline(_) | SurfaceKind::Plane(_))
        {
            return Ok(());
        }
        check_geometry_nesting(
            NestedGeometry::Surface(self),
            initial_depth,
            "surface::surface::p2::clone_for_evaluation",
        )
    }

    /// The same nesting bound for a temporary curve evaluation copy.
    pub(crate) fn check_curve_evaluation_copy_depth(
        curve: &Curve,
        initial_depth: usize,
    ) -> Result<()> {
        check_geometry_nesting(
            NestedGeometry::Curve(curve),
            initial_depth,
            "curve::curve::p2::clone_for_evaluation",
        )
    }

    /// Bound a volume chain before its field-wise, exact recursive `Clone`.
    pub(crate) fn check_volume_evaluation_copy_depth(
        volume: &crate::surface::volume::Volume,
        initial_depth: usize,
    ) -> Result<()> {
        check_geometry_nesting(
            NestedGeometry::Volume(volume),
            initial_depth,
            "surface::surface::p2::clone_for_evaluation",
        )
    }
}

/// Maximum nesting of wrapper geometry (offset, STEP, curve-bounded, in-volume and
/// generated surfaces; offset, projected, on-surface, in-volume, iso and composite
/// curves; volume map chains), counted across curves, surfaces and volumes.
///
/// Every wrapper constructor and setter checks it, so the recursive dispatch,
/// cloning and destruction of nested geometry stay bounded on wasm32 (no stack
/// guard page). Upstream fixtures nest at most 3 levels; equal to the database
/// reader's [`MAX_READ_NESTING`](crate::topology::database_io::MAX_READ_NESTING).
pub const MAX_GEOMETRY_NESTING: usize = 32;

/// A root for [`check_geometry_nesting`].
#[derive(Clone, Copy)]
pub(crate) enum NestedGeometry<'a> {
    /// A surface and everything it owns.
    Surface(&'a Surface),
    /// A curve and everything it owns.
    Curve(&'a Curve),
    /// A volume map and the rest of its chain.
    Volume(&'a crate::surface::volume::Volume),
}

/// Walk `root` without recursion and fail when any owned geometry lies deeper than
/// [`MAX_GEOMETRY_NESTING`], counting `root` itself at `initial_depth`. A wrapper
/// constructor passes its new inner geometry at depth 1.
///
/// # Errors
///
/// `InvalidInput` (at `op`) past [`MAX_GEOMETRY_NESTING`].
pub(crate) fn check_geometry_nesting(
    root: NestedGeometry<'_>,
    initial_depth: usize,
    op: &'static str,
) -> Result<()> {
    use crate::curve::curve::CurveKind;
    type Geometry<'a> = NestedGeometry<'a>;
    let mut pending = vec![(root, initial_depth)];
    while let Some((geometry, depth)) = pending.pop() {
        if depth > MAX_GEOMETRY_NESTING {
            return Err(SolidError::invalid_input(
                op,
                "owned geometry nesting exceeds MAX_GEOMETRY_NESTING",
            ));
        }
        let mut push = |geometry| pending.push((geometry, depth + 1));
        match geometry {
            Geometry::Surface(surface) => match surface.kind() {
                SurfaceKind::BSpline(_) | SurfaceKind::Plane(_) => {}
                SurfaceKind::Cone(s) => {
                    if let Some(c) = s.rev().gen_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                SurfaceKind::Sphere(s) => {
                    if let Some(c) = s.base().gen_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                SurfaceKind::Torus(s) => {
                    if let Some(c) = s.base().gen_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                SurfaceKind::SurfOfRevolution(s) => {
                    if let Some(c) = s.gen_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                SurfaceKind::SurfOfExtrusion(s) => {
                    if let Some(c) = s.gen_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                SurfaceKind::Step(s) => {
                    if let Some(s) = s.surface() {
                        push(Geometry::Surface(s));
                    }
                }
                SurfaceKind::CurveBounded(s) => {
                    for c in s.boundaries_3d().iter().chain(s.boundaries_uv()) {
                        push(Geometry::Curve(c));
                    }
                    push(Geometry::Surface(s.basis()));
                }
                SurfaceKind::SrfInVolume(s) => {
                    push(Geometry::Volume(s.volume()));
                    push(Geometry::Surface(s.surface()));
                }
                SurfaceKind::OffsetSurface(s) => {
                    push(Geometry::Surface(s.base_surface()));
                    if let Some(e) = s.extended_base_surface() {
                        push(Geometry::Surface(e));
                    }
                }
            },
            Geometry::Curve(curve) => match curve.kind() {
                CurveKind::Projected(c) => push(Geometry::Curve(c.curve())),
                CurveKind::Iso(c) => push(Geometry::Surface(c.surface())),
                CurveKind::Offset(c) => {
                    if let Some(c) = c.base_curve() {
                        push(Geometry::Curve(c));
                    }
                }
                CurveKind::CrvInVolume(c) => {
                    push(Geometry::Volume(c.volume()));
                    push(Geometry::Curve(c.curve()));
                }
                CurveKind::CrvOnSurf(c) => {
                    push(Geometry::Curve(c.uv_curve()));
                    push(Geometry::Surface(c.surface()));
                }
                CurveKind::TangentField(c) => {
                    push(Geometry::Curve(c.base().uv_curve()));
                    push(Geometry::Surface(c.base().surface()));
                }
                CurveKind::Composite(c) => {
                    for segment in c.segments() {
                        push(Geometry::Curve(&segment.curve));
                    }
                }
                CurveKind::BSpline(_)
                | CurveKind::Hermite(_)
                | CurveKind::Hyperbola(_)
                | CurveKind::Conic(_)
                | CurveKind::Line(_)
                | CurveKind::Ellipse(_)
                | CurveKind::Parabola(_) => {}
            },
            Geometry::Volume(volume) => {
                if let Some(next) = volume.next_map() {
                    push(Geometry::Volume(next));
                }
            }
        }
    }
    Ok(())
}

impl Surface {
    /// Bounded traversal of compound surfaces for an evaluation copy (at most
    /// [`MAX_GEOMETRY_NESTING`]).
    pub(crate) fn clone_for_evaluation_depth(&self, depth: usize) -> Result<Self> {
        self.check_evaluation_copy_depth(depth)?;
        let kind = match &self.kind {
            SurfaceKind::BSpline(s) => SurfaceKind::BSpline(s.clone_exact()),
            SurfaceKind::Plane(s) => SurfaceKind::Plane(s.clone_shared()),
            SurfaceKind::Cone(s) => SurfaceKind::Cone(Box::new(Cone {
                rev: s.rev().clone_for_evaluation(depth + 1)?,
                base_radius: s.base_radius,
                semi_angle_deg: s.semi_angle_deg,
                inside_out: s.inside_out,
                cylinder: s.cylinder,
            })),
            SurfaceKind::Sphere(s) => SurfaceKind::Sphere(Box::new(Sphere {
                base: s.base().clone_for_evaluation(depth + 1)?,
                radius: s.radius,
                inside_out: s.inside_out,
            })),
            SurfaceKind::Torus(s) => SurfaceKind::Torus(Box::new(Torus {
                base: s.base().clone_for_evaluation(depth + 1)?,
                major_radius: s.major_radius,
                minor_radius: s.minor_radius,
                // The copy constructor resets this flag; an evaluation copy must not.
                inside_out: s.inside_out,
                keeps_inside_out: s.keeps_inside_out,
            })),
            SurfaceKind::SurfOfRevolution(s) => {
                SurfaceKind::SurfOfRevolution(Box::new(s.clone_for_evaluation(depth + 1)?))
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                SurfaceKind::SurfOfExtrusion(Box::new(s.clone_for_evaluation_shared(depth + 1)?))
            }
            SurfaceKind::Step(s) => SurfaceKind::Step(s.clone_for_evaluation(depth + 1)?),
            SurfaceKind::CurveBounded(s) => {
                SurfaceKind::CurveBounded(s.clone_for_evaluation(depth + 1)?)
            }
            SurfaceKind::SrfInVolume(s) => {
                SurfaceKind::SrfInVolume(Box::new(s.clone_for_evaluation(depth + 1)?))
            }
            SurfaceKind::OffsetSurface(s) => {
                SurfaceKind::OffsetSurface(s.clone_for_evaluation(depth + 1)?)
            }
        };
        Ok(Self {
            kind,
            owner: self.owner,
            owner_chain_face: self.owner_chain_face,
            owner_zone_tol_3d: self.owner_zone_tol_3d,
            cache_slot: self.cache_slot.share(),
            notify_identity: IdentitySlot::new(),
            made_kind: self.made_kind.shared(),
            boundary_memo: BoundaryMemo::new(),
        })
    }

    /// Detaches the NURB cells this surface's planes and surfaces of extrusion (and the
    /// made-kind cells of its cones, spheres, tori and surfaces of revolution) share
    /// with evaluation copies, through the whole surface tree (offset, STEP,
    /// curve-bounded and in-volume bases). Iterative (an explicit stack), so no input
    /// depth bound applies. Surfaces held by curves (a curve-bounded surface's trim
    /// curves, generator curves) are not reached.
    pub(crate) fn detach_nurb_cells(&mut self) {
        let mut stack: Vec<&mut Surface> = vec![self];
        while let Some(s) = stack.pop() {
            // (a kept made kind moves into the surface, on a cell of its own)
            s.take_made_kind_for_edit();
            match &mut s.kind {
                SurfaceKind::Plane(p) => p.detach_nurb(),
                SurfaceKind::SurfOfExtrusion(e) => e.detach_nurb(),
                SurfaceKind::Step(st) => stack.extend(st.nested_surfaces_mut()),
                SurfaceKind::CurveBounded(c) => stack.extend(c.nested_surfaces_mut()),
                SurfaceKind::SrfInVolume(v) => stack.extend(v.nested_surfaces_mut()),
                SurfaceKind::OffsetSurface(o) => stack.extend(o.nested_surfaces_mut()),
                _ => {}
            }
        }
    }

    /// Exact recursive copy after the caller has validated the complete geometry graph.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        let kind = match &self.kind {
            SurfaceKind::BSpline(s) => SurfaceKind::BSpline(s.clone_exact()),
            SurfaceKind::Plane(s) => SurfaceKind::Plane(s.clone_shared()),
            SurfaceKind::Cone(s) => {
                let mut c = s.as_ref().clone();
                c.rev = s.rev().clone_exact();
                SurfaceKind::Cone(Box::new(c))
            }
            SurfaceKind::Sphere(s) => {
                let mut c = s.as_ref().clone();
                c.base = s.base().clone_exact();
                SurfaceKind::Sphere(Box::new(c))
            }
            SurfaceKind::Torus(s) => {
                // (an exact evaluation copy: the copy constructor's reset of the
                // inside-out flag does not apply)
                let mut c = s.as_ref().clone();
                c.base = s.base().clone_exact();
                c.inside_out = s.inside_out;
                SurfaceKind::Torus(Box::new(c))
            }
            SurfaceKind::SurfOfRevolution(s) => {
                SurfaceKind::SurfOfRevolution(Box::new(s.clone_exact()))
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                let mut c = s.clone_exact();
                c.base = s.base().clone_exact_shared();
                SurfaceKind::SurfOfExtrusion(Box::new(c))
            }
            SurfaceKind::Step(s) => SurfaceKind::Step(s.clone_exact_bounded()),
            SurfaceKind::CurveBounded(s) => SurfaceKind::CurveBounded(s.clone_exact_bounded()),
            SurfaceKind::SrfInVolume(s) => {
                SurfaceKind::SrfInVolume(Box::new(s.clone_exact_bounded()))
            }
            SurfaceKind::OffsetSurface(s) => SurfaceKind::OffsetSurface(s.clone_exact_bounded()),
        };
        Self {
            kind,
            owner: self.owner,
            owner_chain_face: self.owner_chain_face,
            owner_zone_tol_3d: self.owner_zone_tol_3d,
            cache_slot: self.cache_slot.share(),
            notify_identity: IdentitySlot::new(),
            made_kind: self.made_kind.shared(),
            boundary_memo: BoundaryMemo::new(),
        }
    }
}

impl From<BSplineSurface> for Surface {
    fn from(s: BSplineSurface) -> Self {
        Self::new(SurfaceKind::BSpline(s))
    }
}

impl From<Cone> for Surface {
    fn from(s: Cone) -> Self {
        Self::new(SurfaceKind::Cone(Box::new(s)))
    }
}

impl From<Torus> for Surface {
    fn from(s: Torus) -> Self {
        Self::new(SurfaceKind::Torus(Box::new(s)))
    }
}

impl From<Sphere> for Surface {
    fn from(s: Sphere) -> Self {
        Self::new(SurfaceKind::Sphere(Box::new(s)))
    }
}

impl From<StepSurface> for Surface {
    fn from(s: StepSurface) -> Self {
        Self::new(SurfaceKind::Step(s))
    }
}

impl From<Plane> for Surface {
    fn from(s: Plane) -> Self {
        Self::new(SurfaceKind::Plane(s))
    }
}

impl From<SurfOfExtrusion> for Surface {
    fn from(s: SurfOfExtrusion) -> Self {
        Self::new(SurfaceKind::SurfOfExtrusion(Box::new(s)))
    }
}

impl From<SurfOfRevolution> for Surface {
    fn from(s: SurfOfRevolution) -> Self {
        Self::new(SurfaceKind::SurfOfRevolution(Box::new(s)))
    }
}

/// The curves of a surface/surface intersection (`GlobalSurfaceIntersect`'s
/// outputs): one entry per intersection curve in each array.
#[derive(Debug, Clone, Default)]
pub struct SurfaceIntersection {
    /// `pOpt3DCurves`: the 3D intersection curves.
    pub curves_3d: Vec<Curve>,
    /// `pOptSurface1UVCurves`: their UV curves on this surface (`None` for a null
    /// entry).
    pub surface1_uv_curves: Vec<Option<Curve>>,
    /// `pOptSurface2UVCurves`: their UV curves on the other surface (`None` for a
    /// null entry).
    pub surface2_uv_curves: Vec<Option<Curve>>,
    /// `pOptCurveTypes`: the kind of each intersection.
    pub curve_types: Vec<TsectCurveType>,
    /// `pOptDeviations`: the maximum distance of each curve from the surfaces.
    pub deviations: Vec<f64>,
}

/// The curve of a point-based surface/surface intersection
/// (`PointBasedSurfaceIntersect`'s outputs). Each member is `None` where upstream
/// leaves the out-param `NULL` or unset (it is set only when exactly one curve was
/// found).
#[derive(Debug, Clone, Default)]
pub struct PointIntersection {
    /// `rp3DCurve`: the 3D curve (`None` when no curve passes through all points).
    pub curve_3d: Option<Curve>,
    /// `rpSurface1UVCurve`: its UV curve on this surface.
    pub surface1_uv_curve: Option<Curve>,
    /// `rpSurface2UVCurve`: its UV curve on the other surface.
    pub surface2_uv_curve: Option<Curve>,
    /// `reCurveType`: the kind of intersection.
    pub curve_type: Option<TsectCurveType>,
    /// `rdDeviation`: the maximum distance between the curve and the surfaces.
    pub deviation: Option<f64>,
}

/// The kind of an intersection curve (`SmTsectCurveType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TsectCurveType {
    /// `SM_TC_UNKNOWN`.
    #[default]
    Unknown = 0,
    /// `SM_TC_TOUCHING`: a single point where the surfaces touch.
    Touching = 1,
    /// `SM_TC_CROSSING`: the surface normals are not parallel.
    Crossing = 2,
    /// `SM_TC_TANGENT`: the surfaces touch along a curve.
    Tangent = 3,
    /// `SM_TC_COINCIDENT`: the surfaces butt up against one another.
    Coincident = 4,
    /// `SM_TC_NEAR_TANGENT`: a small angle of intersection.
    NearTangent = 5,
    /// `SM_TC_REGION_BOUNDARY`: bounds a region of coincidence.
    RegionBoundary = 6,
}

/// The plane through a set of coplanar curves (`CreatePlaneWith3DCurves`' outputs).
#[derive(Debug, Clone)]
pub struct PlaneWith3dCurves {
    /// `rpNewPlanarSurface`: the plane (a bilinear B-spline, copied with analytics).
    pub plane: Surface,
    /// `sLoopCounts`: the number of curves in each loop.
    pub loop_counts: Vec<usize>,
    /// `sOrderedCurves`: indices into the input curves, ordered into loops (outer
    /// loop counter-clockwise followed by its inner loops clockwise).
    pub ordered_curves: Vec<usize>,
    /// `sOrients`: the orientation of each ordered curve in its loop.
    pub orients: Vec<OrientType>,
    /// `*pOuterLoopCounts`: the number of loops in each outer loop.
    pub outer_loop_counts: Vec<usize>,
}

impl Surface {
    /// The key this surface object is notified under, once a context has issued it one
    /// (the kernel issues it; there is no setter).
    #[must_use]
    pub fn notify_key(&self) -> Option<crate::geom::attribute::ObjectKey> {
        self.notify_identity
            .value()
            .map(crate::geom::attribute::ObjectKey)
    }

    /// The slot of this surface object's notification identity, for the context that
    /// issues it.
    pub(crate) fn notify_identity_slot(&mut self) -> &mut IdentitySlot {
        &mut self.notify_identity
    }

    /// A surface of the given kind without owner (and without cache).
    #[must_use]
    pub fn new(kind: SurfaceKind) -> Self {
        Self {
            kind,
            owner: None,
            owner_chain_face: None,
            owner_zone_tol_3d: None,
            cache_slot: CacheSlot::default(),
            notify_identity: IdentitySlot::new(),
            made_kind: MadeKindSlot::default(),
            boundary_memo: BoundaryMemo::new(),
        }
    }

    /// The state of this surface's cache slot (see [`CacheTreeState`]), shared with its
    /// evaluation copies.
    #[must_use]
    pub fn cache_tree_state(&self) -> CacheTreeState {
        self.cache_slot.get()
    }

    /// Records the state of this surface's cache slot, for this surface and every
    /// evaluation copy of it: the cache builds keep it in step, and a caller that
    /// rebuilds a surface which already had a cache upstream (an import) restores it.
    pub fn set_cache_tree_state(&self, state: CacheTreeState) {
        self.cache_slot.set(state);
    }

    /// Takes this surface's trimmed surface cache out of its cache slot (shared with its
    /// evaluation copies) for use, leaving [`TrimCacheSlot::CheckedOut`] until
    /// [`Surface::store_trim_cache`] returns it: [`TrimCacheSlot::Stored`] with the cache,
    /// whose own surface copy shares this slot again while it is out; otherwise what the
    /// slot holds ([`TrimCacheSlot::Empty`] or [`TrimCacheSlot::CheckedOut`]), unchanged.
    #[must_use]
    pub fn take_trim_cache(&self) -> TrimCacheSlot {
        let taken = self.cache_slot.with(|s| match &s.trim {
            TrimCacheSlot::Stored(_) => core::mem::replace(&mut s.trim, TrimCacheSlot::CheckedOut),
            TrimCacheSlot::Empty => TrimCacheSlot::Empty,
            TrimCacheSlot::CheckedOut => TrimCacheSlot::CheckedOut,
        });
        match taken {
            TrimCacheSlot::Stored(mut cache) => {
                cache.base_mut().surface_mut().cache_slot = self.cache_slot.share();
                TrimCacheSlot::Stored(cache)
            }
            other => other,
        }
    }

    /// Stores `cache` as the trimmed surface cache of the surface it caches (whose cache
    /// slot its own surface copy shares: upstream's
    /// `SmCacheMgr::AddToObjectCache(SM_OC_TRIMSRF, ..)`, or the return of a cache taken
    /// with [`Surface::take_trim_cache`]), and records its tree state. A face
    /// modification noted while the cache was out ([`Surface::mark_trim_cache_face_modified`])
    /// is applied to it. The cache's own surface copy then gets a slot of its own, so
    /// that the slot does not own itself.
    pub fn store_trim_cache(mut cache: TrimSrfCache<EntityRef>) {
        let slot = cache.base().surface().cache_slot.share();
        cache.base_mut().surface_mut().cache_slot = CacheSlot::default();
        let tree = if cache.base().tree().is_some() {
            CacheTreeState::Built
        } else {
            CacheTreeState::Unbuilt
        };
        slot.with(|s| {
            if core::mem::take(&mut s.face_modified_while_out) {
                cache.base_mut().face_was_modified = true;
            }
            s.tree = tree;
            s.trim = TrimCacheSlot::Stored(Box::new(cache));
        });
    }

    /// `SetCacheObj(new SmTrimSrfCache(*this))`: attaches a new trimmed surface cache
    /// with the default tolerances and no tree, set up by `configure`, as this
    /// surface's (a caller that makes the cache itself).
    ///
    /// # Errors
    ///
    /// Those of the surface copy.
    pub fn attach_trim_cache(
        &self,
        configure: impl FnOnce(&mut TrimSrfCache<EntityRef>),
    ) -> Result<()> {
        let mut cache = TrimSrfCache::new(
            self.clone_for_evaluation()?,
            crate::surface::surface_cache::p1::SurfaceCacheOptions::default(),
        );
        configure(&mut cache);
        Self::store_trim_cache(cache);
        Ok(())
    }

    /// `SmFace::Notify(SM_NO_PRE_EDIT)` on this face surface's trimmed surface cache
    /// (`GetObjectCache(SM_OC_TRIMSRF, ..)`): sets its `m_bFaceWasModified`; a cache in
    /// use gets it when it is stored again.
    pub fn mark_trim_cache_face_modified(&self) {
        self.cache_slot.with(|s| match &mut s.trim {
            TrimCacheSlot::Stored(cache) => cache.base_mut().face_was_modified = true,
            TrimCacheSlot::CheckedOut => s.face_modified_while_out = true,
            TrimCacheSlot::Empty => {}
        });
    }

    /// Takes the owner of `other` (the same surface object as it is now): a cache's
    /// copy of the surface it caches follows that surface's owner.
    pub(crate) fn adopt_owner_of(&mut self, other: &Self) {
        self.owner = other.owner;
        self.owner_chain_face = other.owner_chain_face;
        self.owner_zone_tol_3d = other.owner_zone_tol_3d;
        self.update_owned_bases();
    }

    /// Marks this surface's trimmed surface cache as in use without one having been
    /// stored (a cache under construction: upstream adds it to the queue before its
    /// tree is built).
    pub fn check_out_trim_cache(&self) {
        self.cache_slot.with(|s| s.trim = TrimCacheSlot::CheckedOut);
    }

    /// Deletes this surface's trimmed surface cache (`SmCacheMgr::DeleteObjectsCache(
    /// SM_OC_TRIMSRF, ..)`), or ends a checkout without returning a cache (a build that
    /// failed). The tree state is left as it is (the surface cache of `SM_OC_SURFACE`
    /// keeps its own).
    pub fn delete_trim_cache(&self) {
        self.cache_slot.with(|s| {
            s.trim = TrimCacheSlot::Empty;
            s.face_modified_while_out = false;
        });
    }

    /// Whether this surface's cache slot holds a trimmed surface cache (stored or in
    /// use).
    #[must_use]
    pub fn has_trim_cache(&self) -> bool {
        self.cache_slot
            .with(|s| !matches!(s.trim, TrimCacheSlot::Empty))
    }

    /// Records that face `f`'s `CreateUVTrimCurve(s)` cleared this face surface's owner
    /// (the start of its `SmTemporaryChangeValue<SmObject*>(m_pOwner, NULL)` scope), for
    /// this surface and every copy that shares its cache slot: a face-unaware
    /// `smsurf_GetSurfaceCache` of it in the meantime makes the standalone trimmed cache
    /// upstream keeps on the surface ([`Surface::keep_standalone_trim_cache`]).
    /// [`Surface::end_owner_cleared`] ends it.
    pub fn begin_owner_cleared(&self, f: FaceId) {
        self.cache_slot.with(|s| s.cleared_for.push(f));
    }

    /// Ends the innermost [`Surface::begin_owner_cleared`] scope of face `f`.
    pub fn end_owner_cleared(&self, f: FaceId) {
        self.cache_slot.with(|s| {
            if let Some(i) = s.cleared_for.iter().rposition(|&g| g == f) {
                s.cleared_for.remove(i);
            }
        });
    }

    /// `SmCacheMgrTSrf::CacheMakeOrValidate` keeping the trimmed cache it made with
    /// the surface: for a face surface whose owner is cleared
    /// ([`Surface::begin_owner_cleared`]) and that has no trimmed cache yet, a copy of
    /// the standalone `cache` just built for it (its handles all name the surface,
    /// which becomes the face's) is stored as the surface's trimmed cache, so that the
    /// face's later lookups reuse it as upstream's do. Any other surface keeps nothing
    /// (the port builds its caches afresh).
    ///
    /// # Errors
    ///
    /// Those of the handle mapping (a cache of this surface has only the surface's
    /// handle, so none in practice).
    pub fn keep_standalone_trim_cache<O: Copy + PartialEq>(
        &self,
        cache: &TrimSrfCache<O>,
    ) -> Result<()> {
        let face = self.cache_slot.with(|s| match s.trim {
            TrimCacheSlot::Empty => s.cleared_for.last().copied(),
            TrimCacheSlot::Stored(_) | TrimCacheSlot::CheckedOut => None,
        });
        if let Some(f) = face {
            if cache.base().surface().shares_cache_slot_with(self) {
                Self::store_trim_cache(cache.map_handles(|_| Ok(EntityRef::Face(f)))?);
            }
        }
        Ok(())
    }

    /// Whether this surface's trimmed surface cache is in use (taken out of its slot
    /// by a caller that has not given it back yet).
    #[must_use]
    pub fn trim_cache_in_use(&self) -> bool {
        self.cache_slot
            .with(|s| matches!(s.trim, TrimCacheSlot::CheckedOut))
    }

    /// Gives this surface (a copy of `src` standing for it in a B-rep copied into new
    /// arenas, its owner already renamed) what `src`'s cache slot holds: its tree state
    /// and its kept trimmed surface cache, the cache's topology handles renamed by
    /// `map` and its face binding keyed to this surface's owner. `src` keeps its own.
    /// Upstream has one surface object, so nothing is copied there; this keeps a copy's
    /// later cache lookups seeing the cache the original's earlier ones made
    /// ([`Clone`] gives a copy an empty slot).
    ///
    /// # Errors
    ///
    /// Those of `map` and of the cache copy; [`SolidError::InvalidInput`] when `src`'s
    /// trimmed cache is in use.
    pub fn copy_cache_slot_of(
        &self,
        src: &Self,
        map: impl Fn(EntityRef) -> Result<EntityRef>,
    ) -> Result<()> {
        match src.take_trim_cache() {
            TrimCacheSlot::Empty => {}
            TrimCacheSlot::CheckedOut => {
                return Err(SolidError::invalid_input(
                    "surface::copy_cache_slot_of",
                    "the trimmed surface cache is in use",
                ))
            }
            TrimCacheSlot::Stored(cache) => {
                let copied = cache.map_handles(&map).and_then(|mut copy| {
                    let surface = copy.base_mut().surface_mut();
                    surface.cache_slot = self.cache_slot.share();
                    surface.adopt_owner_of(self);
                    copy.base_mut().rekey_trimmed_face()?;
                    Ok(copy)
                });
                Self::store_trim_cache(*cache);
                Self::store_trim_cache(copied?);
            }
        }
        self.set_cache_tree_state(src.cache_tree_state());
        Ok(())
    }

    /// Makes this surface share `other`'s cache slot: `self` is a copy that stands for
    /// the same surface object (a private snapshot of a B-rep standing for the B-rep
    /// itself).
    pub fn share_cache_slot_of(&mut self, other: &Self) {
        self.cache_slot = other.cache_slot.share();
    }

    /// Whether `self` and `other` share one cache slot (stand for the same surface
    /// object).
    #[must_use]
    pub fn shares_cache_slot_with(&self, other: &Self) -> bool {
        self.cache_slot.same(&other.cache_slot)
    }

    /// A copy as [`Clone`] that shares this surface's cache slot, standing for the same
    /// surface object as far as its caches go: a cache built on the copy is this
    /// surface's (a port-side stand-in for one upstream surface pointer).
    #[must_use]
    pub(crate) fn clone_sharing_cache(&self) -> Self {
        let mut copy = self.clone();
        copy.cache_slot = self.cache_slot.share();
        copy
    }

    /// The boundary curve memo of `CoincidenceCheck` (see `BoundaryMemo`).
    pub(crate) const fn boundary_memo(&self) -> &BoundaryMemo {
        &self.boundary_memo
    }

    /// The concrete kind: once a `const` query made a cone's, sphere's, torus's or
    /// surface of revolution's missing NURB ([`Surface::kind_with_nurb`]), the made kind
    /// kept on this surface ([`MadeKindSlot`]), as upstream keeps the made NURB on the
    /// object.
    #[must_use]
    pub fn kind(&self) -> &SurfaceKind {
        self.made_kind.get().unwrap_or(&self.kind)
    }

    /// Moves a kind made by [`Surface::kind_with_nurb`] into this surface before an
    /// edit (the edit applies to the made object, as upstream's), detaching it from the
    /// evaluation copies sharing the cell.
    fn take_made_kind_for_edit(&mut self) {
        if let Some(made) = self.made_kind.take_for_edit() {
            self.kind = made;
        }
    }

    /// The concrete kind, mutable: a kind made by [`Surface::kind_with_nurb`] is moved
    /// into the surface first, and a plane's or extrusion's NURB cell (and a made kind's
    /// cell) is detached from the evaluation copies sharing it, so an edit applies to
    /// the made object and stays with this surface.
    pub fn kind_mut(&mut self) -> &mut SurfaceKind {
        self.take_made_kind_for_edit();
        match &mut self.kind {
            SurfaceKind::Plane(s) => s.detach_nurb(),
            SurfaceKind::SurfOfExtrusion(s) => s.detach_nurb(),
            _ => {}
        }
        // an edit makes another surface value: start an empty boundary memo
        self.boundary_memo = BoundaryMemo::new();
        &mut self.kind
    }

    /// The concrete kind with its NURB (`GetOrCreateGwNurbPointer` from a `const`
    /// method): this surface's kind when it is not analytic or has its NURB, else the
    /// kind with the NURB its virtual `MakeNurb` makes (status ignored, as upstream: a
    /// NURB that cannot be made stays missing), made once on an exact copy and kept on
    /// this surface ([`MadeKindSlot`]; a plane's and an extrusion's NURB in their own
    /// NURB slot), so that [`Surface::kind`] answers it from then on, as upstream keeps
    /// it on the object.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from `MakeNurb`.
    pub fn kind_with_nurb(&self, k: &Kernel) -> Result<&SurfaceKind> {
        if let Some(made) = self.made_kind.get() {
            return Ok(made);
        }
        let missing = match &self.kind {
            SurfaceKind::Cone(s) => s.rev().base().gw_nurb().is_none(),
            SurfaceKind::Sphere(s) => s.base().base().gw_nurb().is_none(),
            SurfaceKind::Torus(s) => s.base().base().gw_nurb().is_none(),
            SurfaceKind::SurfOfRevolution(s) => s.base().gw_nurb().is_none(),
            SurfaceKind::SurfOfExtrusion(s) => {
                // made once and kept in the extrusion's own NURB slot
                // (`SurfOfExtrusion::base_with_nurb`; status ignored, as upstream)
                if s.base().gw_nurb().is_none() {
                    match s.base_with_nurb(k) {
                        Err(e) if e.is_unported() => return Err(e),
                        _ => {}
                    }
                }
                false
            }
            _ => false,
        };
        if !missing {
            return Ok(&self.kind);
        }
        let mut kind = clone_kind_exact(&self.kind);
        match &mut kind {
            SurfaceKind::Cone(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::Sphere(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::Torus(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::SurfOfRevolution(s) => ignore_status(s.make_nurb(k))?,
            _ => {}
        }
        Ok(self.made_kind.fill(kind))
    }

    /// [`SurfOfExtrusion::base_with_nurb`] of this surface (a surface of extrusion),
    /// which makes a missing NURB once and keeps it on the extrusion.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when this surface is not a surface of extrusion; those of
    /// [`SurfOfExtrusion::base_with_nurb`].
    pub(crate) fn extrusion_base_with_nurb(&self, k: &Kernel) -> Result<Cow<'_, BSplineSurface>> {
        let SurfaceKind::SurfOfExtrusion(s) = &self.kind else {
            return Err(SolidError::sm_err(OP_KIND_WITH_NURB));
        };
        s.base_with_nurb(k)
    }

    /// Whether a made kind is kept ([`MadeKindSlot`]; for tests).
    #[cfg(test)]
    pub(crate) fn has_made_nurb(&self) -> bool {
        self.made_kind.get().is_some()
    }

    /// Whether `self` and `other` share one made-kind cell (stand for the same surface
    /// object; for tests).
    #[cfg(test)]
    pub(crate) fn shares_made_kind_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.made_kind.0, &other.made_kind.0)
    }

    /// The concrete kind, by value (a made kind, [`Surface::kind`], when one is kept).
    #[must_use]
    pub fn into_kind(mut self) -> SurfaceKind {
        self.take_made_kind_for_edit();
        self.kind
    }

    /// The owner (`GetOwner`).
    #[must_use]
    pub const fn owner(&self) -> Option<ObjectRef> {
        self.owner
    }

    /// The owner, mutable (`m_pOwner`; [`Surface::set_owner`] is `SetOwner`, which also
    /// keeps the bases this surface owns in step).
    pub fn owner_mut(&mut self) -> &mut Option<ObjectRef> {
        &mut self.owner
    }

    /// The face reached through a surface owner (see the field).
    pub(crate) const fn owner_chain_face(&self) -> Option<ObjectRef> {
        self.owner_chain_face
    }

    /// Makes this surface owned by an owning surface (`m_pSurface->SetOwner(this)`)
    /// whose own owner chain reaches `chain_face`, and passes that down to the bases
    /// this surface owns in turn.
    pub(crate) fn set_surface_owner(&mut self, owner: ObjectRef, chain_face: Option<ObjectRef>) {
        self.owner = Some(owner);
        self.owner_chain_face = chain_face;
        // a new owner chain: the cached face tolerance belonged to the old one
        self.owner_zone_tol_3d = None;
        self.update_owned_bases();
    }

    /// Restores an owner state copied from another surface (for a field-wise copy of an
    /// owning surface: [`Clone`] resets the owner), and passes it down the bases this
    /// surface owns.
    pub(crate) fn restore_owner_state(&mut self, from: &Self) {
        self.owner = from.owner;
        self.owner_chain_face = from.owner_chain_face;
        self.owner_zone_tol_3d = from.owner_zone_tol_3d;
        self.update_owned_bases();
    }

    /// A copy standing for this same object where upstream shares it (the base of a
    /// non-owning `SmOffsetSurface` is the caller's surface, owner included):
    /// [`Clone`] resets the owner, this keeps it.
    pub(crate) fn clone_keeping_owner(&self) -> Self {
        let mut c = self.clone();
        c.restore_owner_state(self);
        c
    }

    /// Passes the face this surface reaches down the chain of bases it owns (an owning
    /// offset's base, its base's base, ...). Iterative: the chain length is bounded by
    /// `MAX_SURFACE_NESTING` at construction but is walked without recursion.
    pub(crate) fn update_owned_bases(&mut self) {
        let face = self.reached_face();
        // the bases reach the same face, so they carry its cached tolerance too
        let tol = face.zip(self.owner_zone_tol_3d());
        let mut cur = self;
        while let SurfaceKind::OffsetSurface(off) = &mut cur.kind {
            let Some(base) = off.owned_base_mut() else {
                break;
            };
            base.owner = Some(crate::surface::offset_surface::BASE_OWNER);
            base.owner_chain_face = face;
            base.owner_zone_tol_3d = tol;
            cur = base;
        }
    }

    /// The zone tolerance of the owning face (`GetFace()->GetTolerance()`,
    /// `SmFace::m_sZoneTol3d`), when the B-rep has recorded it on this surface
    /// ([`Surface::set_owner_zone_tol_3d`]); `None` for a surface without face, or for a
    /// bare owner handle whose B-rep never recorded the face's tolerance.
    #[must_use]
    pub fn owner_zone_tol_3d(&self) -> Option<f64> {
        match (self.reached_face(), self.owner_zone_tol_3d) {
            (Some(face), Some((cached, tol))) if face == cached => Some(tol),
            _ => None,
        }
    }

    /// Records the zone tolerance of the face this surface reaches (`None` clears it),
    /// and passes it down the bases this surface owns. Called by the B-rep wherever a
    /// face record's surface or tolerance changes; a surface reaching no face records
    /// nothing.
    pub fn set_owner_zone_tol_3d(&mut self, tol: Option<f64>) {
        self.owner_zone_tol_3d = tol.and_then(|t| self.reached_face().map(|f| (f, t)));
        self.update_owned_bases();
    }

    /// `GetFace() ? GetFace()->GetTolerance() : none`: `Ok(None)` for a surface without
    /// face, the cached face tolerance ([`Surface::owner_zone_tol_3d`]) for one with a
    /// face.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::face`]; [`SolidError::Unported`] (`SmFace::GetTolerance`,
    /// `topology.face_p2`) for a surface with a face whose B-rep has not recorded the
    /// face's tolerance on it (a bare owner handle: the surface cannot reach the B-rep).
    pub fn face_zone_tol_3d(&self) -> Result<Option<f64>> {
        if self.face()?.is_none() {
            return Ok(None);
        }
        match self.owner_zone_tol_3d() {
            Some(tol) => Ok(Some(tol)),
            // UNPORTED(topology.face_p2): SmFace::GetTolerance (a bare face handle
            // without the tolerance its B-rep records here)
            // Proof (unreachable): a B-rep face's surface always carries the face
            // tolerance: Brep::add_face_record and each set_owner(Some(face)) rebind (merge
            // adapter, sweep store, brep p7, primitive merge) call
            // sync_face_surface_tolerance; 0 hits in the green difftest sweep, only
            // hand-built owners in unit tests reach this.
            None => Err(SolidError::unported(
                "SmFace::GetTolerance",
                "topology.face_p2",
            )),
        }
    }

    /// `GetFace()`'s result: the owner when it is a face, the face reached through a
    /// surface owner, else none.
    pub(crate) fn reached_face(&self) -> Option<ObjectRef> {
        match self.owner {
            Some(o) if o.type_code == crate::geom::assert_array::labels::FACE_TYPE => Some(o),
            Some(o) if crate::surface::surface::p1::is_surface_type(o.type_code) => {
                self.owner_chain_face
            }
            _ => None,
        }
    }

    // ---- type queries ---------------------------------------------------------------

    /// The type code (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match &self.kind {
            SurfaceKind::BSpline(_) => BSplineSurface::TYPE,
            SurfaceKind::CurveBounded(s) => s.type_code(),
            SurfaceKind::Step(s) => s.type_code(),
            SurfaceKind::SrfInVolume(s) => s.type_code(),
            SurfaceKind::Plane(s) => s.type_code(),
            SurfaceKind::Cone(s) => s.type_code(),
            SurfaceKind::Sphere(s) => s.type_code(),
            SurfaceKind::Torus(s) => s.type_code(),
            SurfaceKind::SurfOfRevolution(s) => s.type_code(),
            SurfaceKind::SurfOfExtrusion(s) => s.type_code(),
            SurfaceKind::OffsetSurface(s) => s.type_code(),
        }
    }

    /// The type name (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        match &self.kind {
            SurfaceKind::BSpline(s) => s.type_string(),
            SurfaceKind::CurveBounded(s) => s.type_string(),
            SurfaceKind::Step(s) => s.type_string(),
            SurfaceKind::SrfInVolume(s) => s.type_string(),
            SurfaceKind::Plane(s) => s.type_string(),
            SurfaceKind::Cone(s) => s.type_string(),
            SurfaceKind::Sphere(s) => s.type_string(),
            SurfaceKind::Torus(s) => s.type_string(),
            SurfaceKind::SurfOfRevolution(s) => s.type_string(),
            SurfaceKind::SurfOfExtrusion(s) => s.type_string(),
            SurfaceKind::OffsetSurface(s) => s.type_string(),
        }
    }

    /// The class name (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        match &self.kind {
            SurfaceKind::BSpline(s) => s.class_string(),
            SurfaceKind::CurveBounded(s) => s.class_string(),
            SurfaceKind::Step(s) => s.class_string(),
            SurfaceKind::SrfInVolume(s) => s.class_string(),
            SurfaceKind::Plane(s) => s.class_string(),
            SurfaceKind::Cone(s) => s.class_string(),
            SurfaceKind::Sphere(s) => s.class_string(),
            SurfaceKind::Torus(s) => s.class_string(),
            SurfaceKind::SurfOfRevolution(s) => s.class_string(),
            SurfaceKind::SurfOfExtrusion(s) => s.class_string(),
            SurfaceKind::OffsetSurface(s) => s.class_string(),
        }
    }

    /// `IsKindOf`: the concrete type or one of its base types.
    ///
    /// # Errors
    ///
    /// Those of the kind's `IsKindOf`.
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.is_kind_of(t),
            SurfaceKind::CurveBounded(s) => Ok(s.is_kind_of(t)),
            SurfaceKind::Step(_) => Ok(StepSurface::is_kind_of(t)),
            SurfaceKind::SrfInVolume(s) => Ok(s.is_kind_of(t)),
            SurfaceKind::Plane(s) => s.is_kind_of(t),
            SurfaceKind::Cone(s) => s.is_kind_of(t),
            SurfaceKind::Sphere(s) => s.is_kind_of(t),
            SurfaceKind::Torus(s) => s.is_kind_of(t),
            SurfaceKind::SurfOfRevolution(s) => s.is_kind_of(t),
            SurfaceKind::SurfOfExtrusion(s) => s.is_kind_of(t),
            SurfaceKind::OffsetSurface(_) => Ok(OffsetSurface::is_kind_of(t)),
        }
    }

    // ---- copies ---------------------------------------------------------------------

    /// An exact copy (`Copy`); the owner is not copied. The kernel is the context an
    /// offset surface's copy is constructed in.
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::copy`]; an owned geometry chain deeper than
    /// [`MAX_GEOMETRY_NESTING`]; none
    /// for the other ported kinds (the base default is `SM_ERR`).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        self.check_evaluation_copy_depth(0)?;
        self.make_revolution_nurb_for_copy(k)?;
        match self.kind() {
            SurfaceKind::BSpline(s) => Ok(Self::from(s.copy())),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_copy(),
            SurfaceKind::SrfInVolume(s) => Ok(Self::from(s.copy(k)?)),
            SurfaceKind::Plane(s) => Ok(Self::from(s.copy())),
            SurfaceKind::Cone(s) => Ok(Self::from(s.as_ref().clone())),
            SurfaceKind::Sphere(s) => Ok(Self::from(s.copy())),
            SurfaceKind::Torus(s) => Ok(Self::from(s.copy())),
            SurfaceKind::SurfOfRevolution(s) => Ok(Self::from(s.copy())),
            SurfaceKind::SurfOfExtrusion(s) => Ok(Self::from(s.copy()?)),
            SurfaceKind::OffsetSurface(s) => Ok(Self::from(s.copy(k)?)),
        }
    }

    /// The start of a cone's, sphere's, torus's or surface of revolution's copy
    /// constructor: `SmBSplineSurface(const SmBSplineSurface&)` reads the source's NURB
    /// through `GetOrCreateGwNurbPointer`, which makes a missing one on the source
    /// (status ignored) and keeps it there ([`Surface::kind_with_nurb`]). A plane and an
    /// extrusion do this in their own `copy`.
    fn make_revolution_nurb_for_copy(&self, k: &Kernel) -> Result<()> {
        if matches!(
            self.kind(),
            SurfaceKind::Cone(_)
                | SurfaceKind::Sphere(_)
                | SurfaceKind::Torus(_)
                | SurfaceKind::SurfOfRevolution(_)
        ) {
            self.kind_with_nurb(k)?;
        }
        Ok(())
    }

    /// A copy with analytic information where possible (`CopyAndAddAnalytics`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is [`Surface::copy`]).
    pub fn copy_and_add_analytics(&self, k: &Kernel) -> Result<Self> {
        self.check_evaluation_copy_depth(0)?;
        self.make_revolution_nurb_for_copy(k)?;
        match self.kind() {
            SurfaceKind::BSpline(s) => s.copy_and_add_analytics(k),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) | SurfaceKind::SrfInVolume(_) => {
                base_copy_and_add_analytics()
            }
            // SmBSplineSurface::CopyAndAddAnalytics: not SmBSplineSurface_TYPE, so Copy
            SurfaceKind::Plane(s) => Ok(Self::from(s.copy())),
            SurfaceKind::Cone(s) => Ok(Self::from(s.as_ref().clone())),
            SurfaceKind::Sphere(s) => Ok(Self::from(s.copy())),
            SurfaceKind::Torus(s) => Ok(Self::from(s.copy())),
            SurfaceKind::SurfOfRevolution(s) => Ok(Self::from(s.copy())),
            SurfaceKind::SurfOfExtrusion(s) => Ok(Self::from(s.copy()?)),
            // the base body: the virtual Copy
            SurfaceKind::OffsetSurface(s) => Ok(Self::from(s.copy(k)?)),
        }
    }

    /// A copy mirrored about the XY plane of `mirror_plane` (`CreateMirrorSurface`).
    ///
    /// Every `SmBSplineSurface`-derived kind (plane, cone, sphere, torus, surface of
    /// revolution or extrusion) inherits the B-spline override: the result is a plain
    /// B-spline surface over the NURB the kind makes first. The offset surface and the
    /// surface in a volume mirror what they wrap; the base default is `SM_ERR`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the curve bounded and the STEP surface; those of the kind's NURB
    /// construction and of the override.
    pub fn create_mirror_surface(&self, k: &Kernel, mirror_plane: &Axis2Placement) -> Result<Self> {
        self.check_evaluation_copy_depth(0)?;
        // the copy constructor's `GetOrCreateGwNurbPointer` makes the NURB of the
        // source first; a kind without one makes it on a copy
        let base = match &self.kind {
            SurfaceKind::BSpline(s) => s.create_mirror_surface(mirror_plane)?,
            SurfaceKind::Plane(s) => {
                if s.base().gw_nurb().is_some() {
                    s.base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.copy();
                    c.make_nurb()?;
                    c.base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::SurfOfRevolution(s) => {
                if s.base().gw_nurb().is_some() {
                    s.base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.copy();
                    c.make_nurb(k)?;
                    c.base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::Cone(s) => {
                if s.rev().base().gw_nurb().is_some() {
                    s.rev().base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.as_ref().clone();
                    c.make_nurb(k)?;
                    c.rev().base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::Sphere(s) => {
                if s.base().base().gw_nurb().is_some() {
                    s.base().base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.copy();
                    c.make_nurb(k)?;
                    c.base().base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::Torus(s) => {
                if s.base().base().gw_nurb().is_some() {
                    s.base().base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.copy();
                    c.make_nurb(k)?;
                    c.base().base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                if s.base().gw_nurb().is_some() {
                    s.base().create_mirror_surface(mirror_plane)?
                } else {
                    let mut c = s.copy()?;
                    c.make_nurb(k)?;
                    c.base().create_mirror_surface(mirror_plane)?
                }
            }
            SurfaceKind::OffsetSurface(s) => {
                return s.create_mirror_surface(k, mirror_plane).map(Self::from);
            }
            SurfaceKind::SrfInVolume(s) => {
                return s.create_mirror_surface(k, mirror_plane).map(Self::from);
            }
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => {
                // SmSurface::CreateMirrorSurface: SE(SM_ERR)
                return Err(SolidError::sm_err(OP_MIRROR));
            }
        };
        Ok(Self::from(base))
    }

    // ---- evaluation -----------------------------------------------------------------

    /// The point and partial derivatives at `uv` (`Evaluate`): `D[i][j]` is the
    /// derivative `i` times in `u` and `j` times in `v`.
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.check_evaluation_copy_depth(0)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => s.evaluate(uv, highest_u_deriv, highest_v_deriv, flags),
            SurfaceKind::CurveBounded(s) => {
                s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::Step(s) => s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags),
            SurfaceKind::SrfInVolume(s) => {
                s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::Plane(s) => {
                s.evaluate(uv, highest_u_deriv, highest_v_deriv, flags.only_upper_half)
            }
            // Preserve the cone's analytic position correction and inherited NURB derivatives.
            SurfaceKind::Cone(s) => s.evaluate_rows(k, uv, highest_u_deriv, highest_v_deriv, flags),
            SurfaceKind::SurfOfRevolution(s) => {
                s.base()
                    .evaluate(uv, highest_u_deriv, highest_v_deriv, flags)
            }
            // (not overridden: the SmBSplineSurface evaluation)
            SurfaceKind::SurfOfExtrusion(_) => self.extrusion_base_with_nurb(k)?.evaluate(
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags,
            ),
            // (not overridden: the SmBSplineSurface evaluation)
            SurfaceKind::Sphere(s) => {
                s.base()
                    .base()
                    .evaluate(uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::Torus(s) => {
                s.base()
                    .base()
                    .evaluate(uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::OffsetSurface(s) => {
                s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
        }
    }

    /// [`Surface::evaluate`] for a caller that ignores its status: the derivative array
    /// the evaluation leaves, `None` when it fails before writing it (the caller's
    /// array is unchanged).
    ///
    /// - The `SmBSplineSurface` evaluation (B-splines and surfaces of revolution) keeps
    ///   what it wrote when its zero-tangent repair fails.
    /// - A curve bounded surface passes the caller's array on to its basis surface's
    ///   `Evaluate`, and a STEP surface to its base's `EvaluateSTEP`
    ///   ([`Surface::evaluate_step_ignoring_status`]).
    /// - A surface in a volume fails before writing: `None`.
    /// - A plane never fails upstream; the port's errors are its guards where upstream
    ///   writes outside the array, and are returned.
    /// - An offset surface's failure is `None`. Upstream fails before writing except
    ///   for a zero distance (the base's partial array) and a pole's extrapolated
    ///   tangent (the entries before it); there the port keeps the previous values.
    ///   [`Surface::is_periodic`] reaches an offset only through a wrapper, since an
    ///   offset's flags are copied from an offset base.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::evaluate_ignoring_status`]; `InvalidInput` for a STEP
    /// surface without base surface; a plane's guards; [`SolidError::Unported`].
    pub fn evaluate_ignoring_status(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Option<Vec<Vec<Vec3>>>> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => {
                s.evaluate_ignoring_status(uv, highest_u_deriv, highest_v_deriv, flags)
            }
            // (not overridden: the SmBSplineSurface evaluation, on the m_pNurb upstream's
            // analytic surfaces always hold)
            SurfaceKind::SurfOfRevolution(_) | SurfaceKind::Sphere(_) | SurfaceKind::Torus(_) => {
                self.analytic_base_with_nurb(k)?.evaluate_ignoring_status(
                    uv,
                    highest_u_deriv,
                    highest_v_deriv,
                    flags,
                )
            }
            SurfaceKind::SurfOfExtrusion(_) => self
                .extrusion_base_with_nurb(k)?
                .evaluate_ignoring_status(uv, highest_u_deriv, highest_v_deriv, flags),
            // pass the call along
            SurfaceKind::CurveBounded(s) => {
                s.basis()
                    .evaluate_ignoring_status(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::Step(s) => {
                s.evaluate_ignoring_status(k, uv, highest_u_deriv, highest_v_deriv, flags)
            }
            SurfaceKind::SrfInVolume(s) => {
                crate::error::ignored(s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags))
            }
            SurfaceKind::Plane(_) => self
                .evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags)
                .map(Some),
            SurfaceKind::OffsetSurface(s) => {
                crate::error::ignored(s.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags))
            }
            // SmCone::Evaluate: its only failure is the inherited NURB evaluation, which
            // leaves what that evaluation wrote (no STEP position correction follows)
            SurfaceKind::Cone(c) => {
                match self.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags) {
                    Ok(d) => Ok(Some(d)),
                    Err(e) if e.is_unported() => Err(e),
                    Err(_) => c.rev().base().evaluate_ignoring_status(
                        uv,
                        highest_u_deriv,
                        highest_v_deriv,
                        flags,
                    ),
                }
            }
        }
    }

    /// The point at `uv` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.check_evaluation_copy_depth(0)?;
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => s.evaluate_point(uv),
            SurfaceKind::CurveBounded(s) => s.evaluate_point(k, uv),
            SurfaceKind::Step(s) => s.evaluate_point(k, uv),
            SurfaceKind::SrfInVolume(s) => s.evaluate_point(k, uv),
            SurfaceKind::Plane(s) => Ok(s.evaluate_point(uv)),
            SurfaceKind::Cone(s) => s.evaluate_point(k, uv),
            // (the inherited SmBSplineSurface evaluation, on the m_pNurb upstream's analytic
            // surfaces always hold)
            SurfaceKind::SurfOfRevolution(_) | SurfaceKind::Sphere(_) | SurfaceKind::Torus(_) => {
                self.analytic_base_with_nurb(k)?.evaluate_point(uv)
            }
            SurfaceKind::SurfOfExtrusion(_) => self.extrusion_base_with_nurb(k)?.evaluate_point(uv),
            SurfaceKind::OffsetSurface(s) => s.evaluate_point(k, uv),
        }
    }

    // ---- simple queries -------------------------------------------------------------

    /// The natural parameter domain (`GetNaturalUVDomain`) as far as it is built, without
    /// a kernel: unlike upstream, which makes a missing NURB first, a lazy cone, sphere,
    /// torus or surface of revolution (no NURB yet) answers its NURB-less base's empty
    /// domain. Callers that hold a kernel use [`Surface::natural_uv_domain_with_nurb`];
    /// this is for surfaces known to be built (B-splines, planes, made NURBs) and for
    /// kernel-free code.
    #[must_use]
    pub fn natural_uv_domain_if_built(&self) -> Extent2d {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.natural_uv_domain(),
            SurfaceKind::CurveBounded(s) => s.natural_uv_domain(),
            SurfaceKind::Step(s) => s.natural_uv_domain(),
            SurfaceKind::SrfInVolume(s) => s.natural_uv_domain(),
            SurfaceKind::Plane(s) => s.natural_uv_domain(),
            SurfaceKind::Cone(s) => s.rev().base().natural_uv_domain(),
            SurfaceKind::SurfOfRevolution(s) => s.base().natural_uv_domain(),
            SurfaceKind::SurfOfExtrusion(s) => s.natural_uv_domain_for_query(),
            SurfaceKind::Sphere(s) => s.base().base().natural_uv_domain(),
            SurfaceKind::Torus(s) => s.base().base().natural_uv_domain(),
            SurfaceKind::OffsetSurface(s) => s.natural_uv_domain(),
        }
    }

    /// The STEP parameter domain (`GetSTEPUVDomain`; the plane's analytic domain,
    /// else the base default, the natural domain).
    #[must_use]
    pub fn step_uv_domain(&self) -> Extent2d {
        match self.kind() {
            SurfaceKind::Plane(s) => s.step_uv_domain(),
            SurfaceKind::Cone(s) => s.rev().step_uv_domain(),
            SurfaceKind::Sphere(s) => s.step_uv_domain(),
            // (GetSTEPUVDomain is not overridden)
            SurfaceKind::Torus(s) => s.base().step_uv_domain(),
            SurfaceKind::SurfOfRevolution(s) => s.step_uv_domain(),
            SurfaceKind::SurfOfExtrusion(s) => s.step_uv_domain(),
            SurfaceKind::SrfInVolume(s) => s.step_uv_domain(),
            SurfaceKind::BSpline(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_) => base_step_uv_domain(self),
        }
    }

    /// The degree in `param`'s direction (`GetDegree`).
    #[must_use]
    pub fn degree(&self, param: SurfParamType) -> usize {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.degree(param),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_degree(),
            SurfaceKind::SrfInVolume(s) => s.degree(param),
            SurfaceKind::Plane(s) => s.base_for_query().degree(param),
            SurfaceKind::Cone(s) => s.rev().base().degree(param),
            SurfaceKind::SurfOfRevolution(s) => s.base().degree(param),
            SurfaceKind::SurfOfExtrusion(s) => s.base().degree(param),
            SurfaceKind::Sphere(s) => s.base().base().degree(param),
            SurfaceKind::Torus(s) => s.base().base().degree(param),
            SurfaceKind::OffsetSurface(s) => s.degree(),
        }
    }

    /// [`Surface::degree`] as upstream's `GetDegree`, which makes a missing NURB
    /// (`GetOrCreateGwNurbPointer`): a surface of extrusion (the recogniser's lazy
    /// generator), cone, sphere, torus or surface of revolution without one makes it and
    /// keeps it ([`Surface::kind_with_nurb`]), where [`Surface::degree`] would answer
    /// `0`. A failed make leaves no NURB, so `0`, as upstream.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the make.
    pub fn degree_with_nurb(&self, k: &Kernel, param: SurfParamType) -> Result<usize> {
        match self.kind() {
            SurfaceKind::SurfOfExtrusion(s) if s.base().gw_nurb().is_none() => {
                match s.base_with_nurb(k) {
                    Ok(base) => Ok(base.degree(param)),
                    Err(e) if e.is_unported() => Err(e),
                    Err(_) => Ok(0), // FS
                }
            }
            _ => {
                if self.revolution_nurb_missing() {
                    self.kind_with_nurb(k)?;
                }
                Ok(self.degree(param))
            }
        }
    }

    /// The B-spline part of a cone, sphere, torus or surface of revolution with its NURB
    /// (`GetOrCreateGwNurbPointer`): a missing one is made and kept on this surface
    /// ([`Surface::kind_with_nurb`]; status ignored, so a failed make leaves the part
    /// without NURB). `None` for the other kinds.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from `MakeNurb`.
    pub(crate) fn revolution_base_with_nurb(&self, k: &Kernel) -> Result<Option<&BSplineSurface>> {
        if self.revolution_nurb_missing() {
            self.kind_with_nurb(k)?;
        }
        Ok(match self.kind() {
            SurfaceKind::Cone(s) => Some(s.rev().base()),
            SurfaceKind::Sphere(s) => Some(s.base().base()),
            SurfaceKind::Torus(s) => Some(s.base().base()),
            SurfaceKind::SurfOfRevolution(s) => Some(s.base()),
            _ => None,
        })
    }

    /// The natural domain of a cone, sphere, torus or surface of revolution without NURB,
    /// read from the NURB `MakeNurb` gives an exact copy (status ignored: a failed make
    /// reads the NURB-less part), which this surface does not keep; `None` for a surface
    /// that has its NURB or is of another kind. For the port's own bookkeeping, where
    /// upstream makes no NURB ([`Surface::stand_in_key_domain`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from `MakeNurb`.
    pub(crate) fn revolution_natural_uv_domain_without_keeping(
        &self,
        k: &Kernel,
    ) -> Result<Option<Extent2d>> {
        if !self.revolution_nurb_missing() {
            return Ok(None);
        }
        let mut kind = clone_kind_exact(self.kind());
        Ok(match &mut kind {
            SurfaceKind::Cone(s) => {
                ignore_status(s.make_nurb(k))?;
                Some(s.rev().base().natural_uv_domain())
            }
            SurfaceKind::Sphere(s) => {
                ignore_status(s.make_nurb(k))?;
                Some(s.base().base().natural_uv_domain())
            }
            SurfaceKind::Torus(s) => {
                ignore_status(s.make_nurb(k))?;
                Some(s.base().base().natural_uv_domain())
            }
            SurfaceKind::SurfOfRevolution(s) => {
                ignore_status(s.make_nurb(k))?;
                Some(s.base().natural_uv_domain())
            }
            _ => None,
        })
    }

    /// Whether this surface is a cone, sphere, torus or surface of revolution without
    /// NURB (one [`Surface::kind_with_nurb`] makes and keeps).
    fn revolution_nurb_missing(&self) -> bool {
        match self.kind() {
            SurfaceKind::Cone(s) => s.rev().base().gw_nurb().is_none(),
            SurfaceKind::Sphere(s) => s.base().base().gw_nurb().is_none(),
            SurfaceKind::Torus(s) => s.base().base().gw_nurb().is_none(),
            SurfaceKind::SurfOfRevolution(s) => s.base().gw_nurb().is_none(),
            _ => false,
        }
    }

    /// The number of control points in `param`'s direction
    /// (`GetNumberControlPoints`).
    #[must_use]
    pub fn number_control_points(&self, k: &Kernel, param: SurfParamType) -> usize {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.number_control_points(param),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_number_control_points(),
            SurfaceKind::Plane(s) => s.base_for_query().number_control_points(param),
            SurfaceKind::Cone(s) => s.rev().base().number_control_points(param),
            SurfaceKind::SurfOfRevolution(s) => s.base().number_control_points(param),
            // (GetOrCreateGwNurbPointer; a failed make leaves no NURB, so 0)
            SurfaceKind::SurfOfExtrusion(s) => s
                .base_with_nurb_or_base(k)
                .map_or(0, |b| b.number_control_points(param)),
            SurfaceKind::Sphere(s) => s.base().base().number_control_points(param),
            SurfaceKind::Torus(s) => s.base().base().number_control_points(param),
        }
    }

    /// The largest domain the analytic form allows (`GetMaxAnalyticDomain`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.max_analytic_domain(k),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => Ok(base_max_analytic_domain()),
            SurfaceKind::SrfInVolume(s) => s.max_analytic_domain(k),
            SurfaceKind::Plane(s) => Ok(s.max_analytic_domain()),
            SurfaceKind::Cone(s) => s.max_analytic_domain(k),
            SurfaceKind::Sphere(s) => Ok(s.max_analytic_domain()),
            SurfaceKind::Torus(s) => Ok(s.max_analytic_domain()),
            SurfaceKind::SurfOfRevolution(s) => s.max_analytic_domain(k),
            SurfaceKind::SurfOfExtrusion(s) => s.max_analytic_domain(k),
            SurfaceKind::OffsetSurface(s) => s.max_analytic_domain(k),
        }
    }

    /// Whether the surface is bilinear (`IsBilinear`).
    #[must_use]
    pub fn is_bilinear(&self, k: &Kernel) -> bool {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.is_bilinear(),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_is_bilinear(),
            SurfaceKind::Plane(s) => s.base_for_query().is_bilinear(),
            SurfaceKind::Cone(s) => s.rev().base().is_bilinear(),
            SurfaceKind::SurfOfRevolution(s) => s.base().is_bilinear(),
            // (GetNumberControlPoints makes a missing NURB)
            SurfaceKind::SurfOfExtrusion(s) => {
                s.base_with_nurb_or_base(k).is_ok_and(|b| b.is_bilinear())
            }
            SurfaceKind::Sphere(s) => s.base().base().is_bilinear(),
            SurfaceKind::Torus(s) => s.base().base().is_bilinear(),
        }
    }

    /// Whether the surface is a sweep parallel to `vector` (`IsParallelToVector`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override.
    pub fn is_parallel_to_vector(&self, k: &Kernel, vector: Vec3) -> Result<bool> {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.is_parallel_to_vector(vector),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => Ok(base_is_parallel_to_vector()),
            SurfaceKind::Plane(s) => Ok(s.is_parallel_to_vector(vector)),
            // (not a plane: the SmBSplineSurface net test)
            SurfaceKind::Cone(s) => s.rev().base().is_parallel_to_vector(vector),
            SurfaceKind::SurfOfRevolution(s) => s.base().is_parallel_to_vector(vector),
            SurfaceKind::SurfOfExtrusion(s) => {
                s.base_with_nurb_or_base(k)?.is_parallel_to_vector(vector)
            }
            SurfaceKind::Sphere(s) => s.base().base().is_parallel_to_vector(vector),
            SurfaceKind::Torus(s) => s.base().base().is_parallel_to_vector(vector),
        }
    }

    /// The unique knots in `param`'s direction and their multiplicities, optionally
    /// limited to `ivl` (`GetKnots`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn knots(
        &self,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.knots(param, ivl),
            SurfaceKind::CurveBounded(_) => base_knots(),
            SurfaceKind::Step(s) => s.knots(param, ivl),
            SurfaceKind::SrfInVolume(s) => s.knots(param, ivl),
            SurfaceKind::Plane(s) => s.base_for_query().knots(param, ivl),
            SurfaceKind::Cone(s) => s.rev().base().knots(param, ivl),
            SurfaceKind::SurfOfRevolution(s) => s.base().knots(param, ivl),
            SurfaceKind::SurfOfExtrusion(s) => s.base().knots(param, ivl),
            SurfaceKind::Sphere(s) => s.base().base().knots(param, ivl),
            SurfaceKind::Torus(s) => s.base().base().knots(param, ivl),
            SurfaceKind::OffsetSurface(s) => s.knots(param, ivl),
        }
    }

    /// [`Surface::knots`] as upstream's `GetKnots`, which reaches the NURB through
    /// `GetOrCreateGwNurbPointer` (a STEP surface: its base's `GetDegree`): a surface of
    /// extrusion, cone, sphere, torus or surface of revolution without its NURB makes
    /// it and keeps it ([`Surface::kind_with_nurb`]) and answers from it, where
    /// [`Surface::knots`] reads the NURB-less base. A failed make answers as
    /// [`Surface::knots`].
    ///
    /// # Errors
    ///
    /// Those of [`Surface::knots`]; [`SolidError::Unported`] from the make.
    pub fn knots_with_nurb(
        &self,
        k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        // (a cone, sphere, torus or surface of revolution makes its NURB and keeps it,
        // `kind_with_nurb`; a failed make leaves none, status ignored)
        if self.revolution_nurb_missing() {
            self.kind_with_nurb(k)?;
        }
        match self.kind() {
            SurfaceKind::Step(s) => s.knots_with_nurb(k, param, ivl),
            SurfaceKind::SurfOfExtrusion(s) if s.base().gw_nurb().is_none() => {
                match s.base_with_nurb(k) {
                    Ok(b) => b.knots(param, ivl),
                    Err(e) if e.is_unported() => Err(e),
                    Err(_) => self.knots(param, ivl),
                }
            }
            _ => self.knots(param, ivl),
        }
    }

    /// `uv` of the STEP parameterisation in the NURBS parameterisation
    /// (`ConvertUVFromSTEPToNURBS`: the cone's, sphere's, torus's or surface of revolution's
    /// override, the surface in volume's (its base surface's), else the base default,
    /// the identity).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::convert_uv_from_step_to_nurbs`].
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::Cone(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
            SurfaceKind::Sphere(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
            SurfaceKind::Torus(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
            // SmSrfInVolume forwards to its base surface
            SurfaceKind::SrfInVolume(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
            SurfaceKind::SurfOfRevolution(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
            // (GetNaturalUVDomain makes a missing NURB)
            SurfaceKind::SurfOfExtrusion(_) => match self.kind_with_nurb(k)? {
                SurfaceKind::SurfOfExtrusion(s) => s.convert_uv_from_step_to_nurbs(k, step_uv),
                _ => Err(SolidError::sm_err(OP_KIND_WITH_NURB)),
            },
            _ => Ok(base_convert_uv(step_uv)),
        }
    }

    /// `uv` of the NURBS parameterisation in the STEP parameterisation
    /// (`ConvertUVFromNURBSToSTEP`: the cone's, sphere's, torus's or surface of revolution's
    /// override, the surface in volume's (its base surface's), else the base default,
    /// the identity).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::convert_uv_from_nurbs_to_step`].
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::Cone(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
            SurfaceKind::Sphere(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
            SurfaceKind::Torus(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
            // SmSrfInVolume forwards to its base surface
            SurfaceKind::SrfInVolume(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
            SurfaceKind::SurfOfRevolution(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
            // (GetNaturalUVDomain makes a missing NURB)
            SurfaceKind::SurfOfExtrusion(_) => match self.kind_with_nurb(k)? {
                SurfaceKind::SurfOfExtrusion(s) => s.convert_uv_from_nurbs_to_step(k, nurbs_uv),
                _ => Err(SolidError::sm_err(OP_KIND_WITH_NURB)),
            },
            _ => Ok(base_convert_uv(nurbs_uv)),
        }
    }

    /// [`Surface::step_inversion`] whose `GlobalPointSolve` runs under the caller's
    /// face context (`context`: the solve context and this surface's handle in it), so
    /// a face-owned surface's solutions are classified against the face's trims
    /// (upstream reaches the face through `GetFace()`). The B-spline and plane kinds
    /// (`SmBSplineSurface::STEPInversion`) take the context; the other kinds' overrides
    /// run as [`Surface::step_inversion`].
    ///
    /// # Errors
    ///
    /// As [`Surface::step_inversion`], with those of the context.
    pub fn step_inversion_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
        context: (&mut dyn SurfaceSolveContext<O>, Option<O>),
    ) -> Result<(Point2, LocationType)> {
        match self.kind() {
            SurfaceKind::BSpline(s) => s.step_inversion_on_with_context(
                k,
                self,
                anal_domain,
                point_on_surf,
                tol_3d,
                uv_guess,
                context,
            ),
            SurfaceKind::Plane(s) => s.base().step_inversion_on_with_context(
                k,
                self,
                anal_domain,
                point_on_surf,
                tol_3d,
                uv_guess,
                context,
            ),
            _ => self.step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess),
        }
    }

    /// The parameter of `point_on_surf` in `anal_domain` and its location
    /// (`STEPInversion`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => {
                s.step_inversion_on(k, self, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_step_inversion(),
            SurfaceKind::OffsetSurface(s) => {
                s.step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            SurfaceKind::SrfInVolume(s) => s.step_inversion(k, anal_domain, point_on_surf, tol_3d),
            // SmBSplineSurface::STEPInversion solves on the plane through the virtual
            // point solvers
            SurfaceKind::Plane(s) => {
                s.base()
                    .step_inversion_on(k, self, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            SurfaceKind::Cone(s) => s
                .step_inversion_on_domain(k, anal_domain, point_on_surf, tol_3d, uv_guess)
                .and_then(|r| {
                    if r.inside {
                        Ok((r.uv, r.location))
                    } else {
                        Err(SolidError::sm_err(
                            "surface::surface::Surface::step_inversion",
                        ))
                    }
                }),
            SurfaceKind::SurfOfRevolution(s) => {
                s.step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                s.step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            // SmSphere::STEPInversion: SM_ERR for a point off the sphere
            SurfaceKind::Sphere(s) => {
                let r = s.step_inversion(anal_domain, point_on_surf, tol_3d, uv_guess);
                if r.on_surface {
                    Ok((r.uv, r.location))
                } else {
                    Err(SolidError::sm_err(
                        "surface::surface::Surface::step_inversion",
                    ))
                }
            }
            // the inherited SmSurfOfRevolution::STEPInversion: with a planar generator
            // it makes no virtual call
            SurfaceKind::Torus(s) if s.base().planar_generator => {
                s.base()
                    .step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess)
            }
            // a torus record read with a non-planar generator flag: the inherited
            // general-solver branch converts through the torus's virtual
            // ConvertUVFromSTEPToNURBS / ConvertUVFromNURBSToSTEP
            SurfaceKind::Torus(s) => {
                let r = s.base().step_inversion_non_planar(
                    k,
                    anal_domain,
                    point_on_surf,
                    tol_3d,
                    uv_guess,
                    Some(self),
                    |uv| s.convert_uv_from_step_to_nurbs(k, uv),
                    |uv| s.convert_uv_from_nurbs_to_step(k, uv),
                )?;
                Ok((r.uv, r.location))
            }
        }
    }

    /// `DropPointFast`: the plane and surface of revolution overrides (their
    /// solutions name no object; the surface of revolution's are merged as
    /// upstream's, which name the surface; the cone, sphere and torus inherit the
    /// revolution's with their own `EvaluateSTEPPoint`, `EvaluatePoint` and
    /// `LocalPointSolve`); the other ported kinds run the base default, which fails.
    ///
    /// # Errors
    ///
    /// Those of [`Plane::drop_point_fast`] and [`SurfOfRevolution::drop_point_fast`];
    /// `SM_ERR` (the base default).
    pub fn drop_point_fast<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        operation: SolverOperation,
        test_point: Point3,
        in_pointing_vector: Option<Vec3>,
        distance_tolerance: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<SolutionArray<O>> {
        // (the analytic kinds' DropPointFast reads the NURB through
        // GetNaturalUVDomain / GetOrCreateGwNurbPointer, which makes it when missing)
        match self.kind_with_nurb(k)? {
            SurfaceKind::Plane(s) => s.drop_point_fast(
                uv_domain,
                operation,
                test_point,
                distance_tolerance,
                target_distance,
                None,
            ),
            // the inherited SmSurfOfRevolution::DropPointFast, its virtual
            // EvaluateSTEPPoint, EvaluatePoint and LocalPointSolve those of the
            // cone/sphere/torus; solutions name a stand-in for this surface, as for
            // the revolution below
            SurfaceKind::Cone(s) => Ok(s
                .rev()
                .drop_point_fast_using(
                    k,
                    uv_domain,
                    operation,
                    test_point,
                    in_pointing_vector,
                    distance_tolerance,
                    target_distance,
                    requested,
                    Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
                    &|_, uv| s.evaluate_step_point(uv),
                    &|k, uv| self.evaluate_point(k, uv),
                    &|k, domain, operation, point, guess| {
                        let r = self.local_point_solve::<OperandObject>(
                            k, domain, operation, point, guess,
                        )?;
                        Ok((r.found, r.solution))
                    },
                )?
                .without_objects()),
            SurfaceKind::Sphere(s) => Ok(s
                .base()
                .drop_point_fast_using(
                    k,
                    uv_domain,
                    operation,
                    test_point,
                    in_pointing_vector,
                    distance_tolerance,
                    target_distance,
                    requested,
                    Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
                    &|_, uv| s.evaluate_step_point(uv),
                    &|k, uv| self.evaluate_point(k, uv),
                    &|k, domain, operation, point, guess| {
                        let r = self.local_point_solve::<OperandObject>(
                            k, domain, operation, point, guess,
                        )?;
                        Ok((r.found, r.solution))
                    },
                )?
                .without_objects()),
            SurfaceKind::Torus(s) => Ok(s
                .base()
                .drop_point_fast_using(
                    k,
                    uv_domain,
                    operation,
                    test_point,
                    in_pointing_vector,
                    distance_tolerance,
                    target_distance,
                    requested,
                    Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
                    &|_, uv| s.evaluate_step_point(uv),
                    &|k, uv| self.evaluate_point(k, uv),
                    &|k, domain, operation, point, guess| {
                        let r = self.local_point_solve::<OperandObject>(
                            k, domain, operation, point, guess,
                        )?;
                        Ok((r.found, r.solution))
                    },
                )?
                .without_objects()),
            // upstream's solutions name this surface, whose natural UV domain
            // IdenticalSolutions reads: solve naming a stand-in for it, return the
            // solutions without objects
            SurfaceKind::SurfOfRevolution(s) => Ok(s
                .drop_point_fast(
                    k,
                    uv_domain,
                    operation,
                    test_point,
                    in_pointing_vector,
                    distance_tolerance,
                    target_distance,
                    requested,
                    Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
                )?
                .without_objects()),
            // SmSurfOfExtrusion::DropPointFast: its solutions name this surface, as the
            // revolution's above
            SurfaceKind::SurfOfExtrusion(s) => Ok(s
                .drop_point_fast(
                    k,
                    uv_domain,
                    operation,
                    test_point,
                    in_pointing_vector,
                    distance_tolerance,
                    target_distance,
                    requested,
                    Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
                )?
                .without_objects()),
            SurfaceKind::BSpline(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_drop_point_fast(),
        }
    }

    /// `IntersectWithPlane`: the special-case intersection with `plane` and whether
    /// the general intersector is still needed. The `SmPlane` override
    /// ([`Plane::intersect_with_plane`]; `plane` is taken as without owner) and the
    /// surface of revolution's (whose special cases use only the plane, the other
    /// domain and the tolerance); the other ported kinds run the base default, which
    /// fails.
    ///
    /// # Errors
    ///
    /// Those of [`Plane::intersect_with_plane`] (with this surface's owner for the
    /// zone tolerance) and [`SurfOfRevolution::intersect_with_plane`]; `SM_ERR` (the
    /// base default).
    pub fn intersect_with_plane(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        plane: &Plane,
        other_uv_domain: &Extent2d,
        _use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        _ang_tol_rad: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::Plane(s) => s.intersect_with_plane_owned(
                k,
                uv_domain,
                plane,
                other_uv_domain,
                approx_tol_3d,
                [
                    crate::surface::plane::p2::PlaneOwner::of(self)?,
                    crate::surface::plane::p2::PlaneOwner::NoFace,
                ],
            ),
            SurfaceKind::Cone(s) => {
                s.intersect_with_plane(k, uv_domain, plane, other_uv_domain, approx_tol_3d)
            }
            SurfaceKind::Sphere(s) => {
                s.intersect_with_plane(k, uv_domain, plane, other_uv_domain, approx_tol_3d)
            }
            SurfaceKind::Torus(s) => {
                s.intersect_with_plane(k, plane, other_uv_domain, approx_tol_3d)
            }
            SurfaceKind::SurfOfRevolution(s) => {
                s.intersect_with_plane(k, plane, other_uv_domain, approx_tol_3d)
            }
            // SmSurfOfExtrusion::IntersectWithPlane (same signature: an override); the
            // plane is taken as without owner, as for the plane above
            SurfaceKind::SurfOfExtrusion(s) => s
                .intersect_with_plane_on(
                    k,
                    self,
                    uv_domain,
                    plane,
                    crate::surface::plane::p2::PlaneOwner::NoFace,
                    other_uv_domain,
                    approx_tol_3d,
                )
                .map(|x| (x.needs_more_intersections, SurfaceIntersection::from(x))),
            SurfaceKind::BSpline(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_intersect_with_plane(),
        }
    }

    // ---- edits ----------------------------------------------------------------------

    /// Sets the domain from a STEP domain and rebuilds the NURB
    /// (`AdjustSTEPUVDomain`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn adjust_step_uv_domain(
        &mut self,
        k: &Kernel,
        new_step_uv_domain: &Extent2d,
    ) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        match &mut self.kind {
            SurfaceKind::BSpline(s) => s.adjust_step_uv_domain(new_step_uv_domain),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_adjust_step_uv_domain(),
            SurfaceKind::Plane(s) => s.adjust_step_uv_domain(new_step_uv_domain),
            SurfaceKind::Cone(s) => s.adjust_step_uv_domain(k, new_step_uv_domain),
            SurfaceKind::Sphere(s) => s.adjust_step_uv_domain(k, new_step_uv_domain),
            SurfaceKind::Torus(s) => s.adjust_step_uv_domain(k, new_step_uv_domain),
            SurfaceKind::SurfOfRevolution(s) => s.adjust_step_uv_domain(k, new_step_uv_domain),
            SurfaceKind::SurfOfExtrusion(s) => s.adjust_step_uv_domain(k, new_step_uv_domain),
        }
    }

    /// Reverses the parameterisation in `param`'s direction (`Reverse`; a surface of
    /// revolution runs the B-spline reversal, whose virtual
    /// `RebuildSTEPFromNURBParameters` rebuilds its analytic data, status ignored).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn reverse(&mut self, k: &Kernel, param: SurfParamType) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        match &mut self.kind {
            SurfaceKind::BSpline(s) => s.reverse(param),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_edit(),
            SurfaceKind::SrfInVolume(s) => s.reverse(k, param),
            SurfaceKind::Plane(s) => s.reverse(param),
            SurfaceKind::Cone(s) => {
                // SmBSplineSurface::Reverse: GetOrCreateGwNurbPointer (the cone's
                // MakeNurb when missing, status ignored), the NURB reversal, then the
                // cone's RebuildSTEPFromNURBParameters (status ignored)
                if s.rev().base().gw_nurb().is_none() {
                    ignore_status(s.make_nurb(k))?;
                }
                s.base_mut().reverse(param)?;
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::Sphere(s) => {
                // SmBSplineSurface::Reverse: GetOrCreateGwNurbPointer (the sphere's
                // MakeNurb when missing, status ignored), the NURB reversal, then the
                // sphere's RebuildSTEPFromNURBParameters (status ignored)
                if s.base().base().gw_nurb().is_none() {
                    ignore_status(s.make_nurb(k))?;
                }
                s.base.base_mut().reverse(param)?;
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::Torus(s) => {
                // SmBSplineSurface::Reverse: GetOrCreateGwNurbPointer (the torus's
                // MakeNurb when missing, status ignored), the NURB reversal, then the
                // torus's RebuildSTEPFromNURBParameters (status ignored)
                if s.base().base().gw_nurb().is_none() {
                    ignore_status(s.make_nurb(k))?;
                }
                s.base.base_mut().reverse(param)?;
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::SurfOfRevolution(s) => {
                // Upstream reaches the NURB through the virtual GetOrCreateGwNurbPointer,
                // which would call SmSurfOfRevolution::MakeNurb for a surface without
                // one; every constructed surface of revolution has its NURB, so the
                // stored base is used directly.
                s.base_mut().reverse(param)?;
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                // SmBSplineSurface::Reverse: GetOrCreateGwNurbPointer (the extrusion's
                // MakeNurb when missing, status ignored), the NURB reversal; the
                // extrusion keeps the base RebuildSTEPFromNURBParameters (no work)
                if s.base().gw_nurb().is_none() {
                    ignore_status(s.make_nurb(k))?;
                }
                s.base_mut().reverse(param)
            }
            SurfaceKind::OffsetSurface(s) => s.reverse(k, param),
        }
    }

    /// Maps the domain onto `new_domain` (`Reparameterize`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn reparameterize(&mut self, k: &Kernel, new_domain: &Extent2d) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        match &mut self.kind {
            SurfaceKind::BSpline(s) => s.reparameterize(new_domain),
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_edit(),
            SurfaceKind::Plane(s) => s.reparameterize(new_domain),
            // the inherited SmSurfOfRevolution::Reparameterize with the cone's iso curves
            SurfaceKind::Cone(s) => s.reparameterize(k, new_domain),
            SurfaceKind::Sphere(s) => s.reparameterize(k, new_domain),
            SurfaceKind::Torus(s) => s.reparameterize(k, new_domain),
            SurfaceKind::SurfOfRevolution(s) => s.reparameterize(k, new_domain),
            SurfaceKind::SurfOfExtrusion(s) => s.reparameterize(k, new_domain),
        }
    }

    /// Swaps the `u` and `v` directions (`SwapUV`; a surface of revolution runs the
    /// B-spline swap, whose virtual `ToggleSwapUVBit` flips its swap flag and whose
    /// `SetCanonical` rebuilds its analytic data, status ignored).
    ///
    /// Notifies nobody: [`Surface::swap_uv_notifying`] sends the edit notifications.
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn swap_uv(&mut self, k: &Kernel) -> Result<()> {
        self.swap_uv_notifying(k, &mut |_, _, _| Ok(()))
    }

    /// [`Surface::swap_uv`] with upstream's edit notifications: each B-spline swap's
    /// `SetCanonical` (every kind built on `SmBSplineSurface`) and `SmSrfInVolume::SwapUV`
    /// send `Notify(SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT, this, SM_NO_GET_OWNER(this),
    /// NULL)` on the edited surface; an offset surface notifies through its extended
    /// and base surfaces. Each goes first through `SmSurface::Notify`, whose
    /// `SM_NO_PRE_EDIT` deletes that surface's trimmed surface cache (one in use stays
    /// with its caller), then to `notify` with the edited surface's identity slot and
    /// type code (`SmAObject::Notify`, the caller's).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::swap_uv`] and of `notify`.
    pub(crate) fn swap_uv_notifying(
        &mut self,
        k: &Kernel,
        notify: &mut SurfaceEditNotify<'_>,
    ) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        let type_code = self.type_code();
        let Self {
            kind,
            notify_identity: slot,
            cache_slot,
            ..
        } = self;
        // SetCanonical's (or SmSrfInVolume's) Notify of this surface
        // (the arms that notify more than this surface's SetCanonical do not use it)
        let mut own = |op| edit_notify(cache_slot, slot, type_code, notify, op);
        match kind {
            SurfaceKind::BSpline(s) => s.swap_uv_notifying(&mut own),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_edit(),
            SurfaceKind::SrfInVolume(s) => {
                // check input - Can not swap UV on an unowned surface
                let base = s.owned_surface_mut()?;
                edit_notify(
                    cache_slot,
                    slot,
                    type_code,
                    notify,
                    NotifyOperation::PreEdit,
                )?;
                // base surface Swap
                base.swap_uv_notifying(k, notify)?;
                edit_notify(
                    cache_slot,
                    slot,
                    type_code,
                    notify,
                    NotifyOperation::PostEdit,
                )
            }
            SurfaceKind::Plane(s) => s.swap_uv_notifying(&mut own),
            SurfaceKind::Cone(s) => {
                // SmBSplineSurface::SwapUV: the inherited
                // SmSurfOfRevolution::ToggleSwapUVBit before SetCanonical's notifications
                // (restored on failure), the NURB swap, then SetCanonical's virtual
                // RebuildSTEPFromNURBParameters (the cone's, status ignored)
                s.rev.toggle_swap_uv_bit();
                if let Err(e) = s.base_mut().swap_uv_notifying(&mut own) {
                    s.rev.toggle_swap_uv_bit();
                    return Err(e);
                }
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::Sphere(s) => {
                // SmBSplineSurface::SwapUV: the inherited
                // SmSurfOfRevolution::ToggleSwapUVBit before SetCanonical's notifications
                // (restored on failure), the NURB swap, then SetCanonical's virtual
                // RebuildSTEPFromNURBParameters (the sphere's, status ignored)
                s.base.toggle_swap_uv_bit();
                if let Err(e) = s.base.base_mut().swap_uv_notifying(&mut own) {
                    s.base.toggle_swap_uv_bit();
                    return Err(e);
                }
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::Torus(s) => {
                // SmBSplineSurface::SwapUV: the inherited
                // SmSurfOfRevolution::ToggleSwapUVBit before SetCanonical's notifications
                // (restored on failure), the NURB swap, then SetCanonical's virtual
                // RebuildSTEPFromNURBParameters (the torus's, status ignored)
                s.base.toggle_swap_uv_bit();
                if let Err(e) = s.base.base_mut().swap_uv_notifying(&mut own) {
                    s.base.toggle_swap_uv_bit();
                    return Err(e);
                }
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::SurfOfRevolution(s) => {
                // As in `reverse`: the stored NURB stands for GetOrCreateGwNurbPointer;
                // the bit toggles before SetCanonical's notifications (restored on failure).
                s.toggle_swap_uv_bit();
                if let Err(e) = s.base_mut().swap_uv_notifying(&mut own) {
                    s.toggle_swap_uv_bit();
                    return Err(e);
                }
                ignore_status(s.rebuild_step_from_nurb_parameters(k))
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                // SmBSplineSurface::SwapUV: GetCanonical's GetOrCreateGwNurbPointer (the
                // extrusion's MakeNurb when missing, status ignored), the extrusion's
                // ToggleSwapUVBit before SetCanonical's notifications (restored on
                // failure) and the NURB swap (its RebuildSTEPFromNURBParameters is the
                // base's: no work)
                if s.base().gw_nurb().is_none() {
                    ignore_status(s.make_nurb(k))?;
                }
                s.toggle_swap_uv_bit();
                let status = s.base_mut().swap_uv_notifying(&mut own);
                if status.is_err() {
                    s.toggle_swap_uv_bit();
                }
                status
            }
            SurfaceKind::OffsetSurface(s) => s.swap_uv_notifying(k, notify),
        }
    }

    /// Scales about the origin (optional) then rotates and moves (`Transform`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`), including
    /// `InvalidInput` if a volume append would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`].
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    /// Carry geometry depth through only the branches the native transform visits.
    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        match &mut self.kind {
            SurfaceKind::Cone(s) => s.transform(k, rotate_n_move, scale),
            SurfaceKind::Sphere(s) => s.transform(k, rotate_n_move, scale),
            SurfaceKind::Torus(s) => s.transform(k, rotate_n_move, scale),
            SurfaceKind::SurfOfRevolution(s) => {
                s.transform_at_depth(k, rotate_n_move, scale, depth)
            }
            SurfaceKind::SurfOfExtrusion(s) => s.transform_at_depth(k, rotate_n_move, scale, depth),
            SurfaceKind::BSpline(s) => s.transform(rotate_n_move, scale),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_edit(),
            SurfaceKind::OffsetSurface(s) => s.transform_at_depth(k, rotate_n_move, scale, depth),
            SurfaceKind::SrfInVolume(s) => s.transform_at_depth(rotate_n_move, scale, depth),
            SurfaceKind::Plane(s) => s.transform(k, rotate_n_move, scale),
        }
    }

    /// Trims to `trim_domain`, which may be snapped to knots (`TrimWithDomain`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn trim_with_domain(&mut self, k: &Kernel, trim_domain: &mut Extent2d) -> Result<()> {
        self.trim_with_domain_with(k, trim_domain, TrimOptions::default())
    }

    /// [`Surface::trim_with_domain`] with [`TrimOptions`] (not upstream,
    /// source issue #472). Only a plain B-spline surface reads them
    /// ([`BSplineSurface::trim_with_domain_with`]); the other kinds trim as
    /// [`Surface::trim_with_domain`]. The default options are upstream's.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::trim_with_domain`].
    pub fn trim_with_domain_with(
        &mut self,
        k: &Kernel,
        trim_domain: &mut Extent2d,
        options: TrimOptions,
    ) -> Result<()> {
        self.take_made_kind_for_edit();
        self.boundary_memo = BoundaryMemo::new();
        match &mut self.kind {
            SurfaceKind::BSpline(s) => s.trim_with_domain_with(trim_domain, options),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_edit(),
            SurfaceKind::OffsetSurface(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::SrfInVolume(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::Plane(s) => s.trim_with_domain(trim_domain),
            SurfaceKind::Cone(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::Sphere(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::Torus(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::SurfOfRevolution(s) => s.trim_with_domain(k, trim_domain),
            SurfaceKind::SurfOfExtrusion(s) => s.trim_with_domain(k, trim_domain),
        }
    }

    // ---- new surfaces ---------------------------------------------------------------

    /// The two halves of the surface split at `param` in `surf_param`'s direction
    /// (`SplitAt`; the kernel is the context the halves are constructed in). An analytic
    /// kind without its NURB yet makes it first and keeps it on the surface
    /// ([`Surface::kind_with_nurb`], upstream's `GetOrCreateGwNurbPointer`).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn split_at(
        &self,
        k: &Kernel,
        param: f64,
        surf_param: SurfParamType,
    ) -> Result<(Self, Self)> {
        match self.kind() {
            SurfaceKind::BSpline(s) => {
                let (l, r) = s.split_at(param, surf_param)?;
                Ok((Self::from(l), Self::from(r)))
            }
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => base_split_at(),
            SurfaceKind::SrfInVolume(s) => {
                let (l, r) = s.split_at(k, param, surf_param)?;
                Ok((Self::from(l), Self::from(r)))
            }
            SurfaceKind::Plane(s) => {
                let (l, r) = s.base_for_query().split_at(param, surf_param)?;
                Ok((Self::from(l), Self::from(r)))
            }
            SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_) => {
                // SmBSplineSurface::SplitAt: GetOrCreateGwNurbPointer (the analytic's
                // MakeNurb when missing, status ignored; the NURB stays on the surface)
                let (l, r) = match self.kind_with_nurb(k)? {
                    SurfaceKind::Cone(s) => s.rev().base().split_at(param, surf_param)?,
                    SurfaceKind::Sphere(s) => s.base().base().split_at(param, surf_param)?,
                    SurfaceKind::Torus(s) => s.base().base().split_at(param, surf_param)?,
                    SurfaceKind::SurfOfRevolution(s) => s.base().split_at(param, surf_param)?,
                    SurfaceKind::SurfOfExtrusion(s) => s.base().split_at(param, surf_param)?,
                    _ => return base_split_at(),
                };
                Ok((Self::from(l), Self::from(r)))
            }
            SurfaceKind::OffsetSurface(s) => s.split_at(k, param, surf_param),
        }
    }

    /// The surface extended by `dist` on every side (`CreateExtendedSurface`; the
    /// kernel is the context the new surface is constructed in).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn create_extended_surface(
        &self,
        k: &Kernel,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<Self> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => Ok(Self::from(s.create_extended_surface(
                k,
                dist,
                extension_continuity,
            )?)),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) | SurfaceKind::SrfInVolume(_) => {
                base_create_extended_surface()
            }
            SurfaceKind::Plane(s) => Ok(Self::from(
                s.create_extended_surface(dist, extension_continuity)?,
            )),
            SurfaceKind::Sphere(s) => Ok(Self::from(
                s.base().base().create_extended_surface_dispatched(
                    SurfParamType::Both,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::Torus(s) => Ok(Self::from(
                s.base().base().create_extended_surface_dispatched(
                    SurfParamType::Both,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::Cone(s) => Ok(Self::from(
                s.rev().base().create_extended_surface_dispatched(
                    SurfParamType::Both,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::OffsetSurface(s) => {
                s.create_extended_surface(k, dist, extension_continuity)
            }
            SurfaceKind::SurfOfRevolution(s) => Ok(Self::from(s.base().create_extended_surface(
                k,
                dist,
                extension_continuity,
            )?)),
            // (`GetOrCreateGwNurbPointer` makes a recognised extrusion's NURB)
            SurfaceKind::SurfOfExtrusion(s) => Ok(Self::from(
                s.base_with_nurb(k)?
                    .create_extended_surface(k, dist, extension_continuity)?,
            )),
        }
    }

    /// The surface extended by `dist` in `ext_direction` (`CreateExtendedSurface`
    /// with a direction; the kernel is the context the new surface is constructed in).
    ///
    /// # Errors
    ///
    /// Those of the kind's override (the base default is `SM_ERR`).
    pub fn create_extended_surface_dir(
        &self,
        k: &Kernel,
        ext_direction: SurfParamType,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<Self> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => Ok(Self::from(s.create_extended_surface_dir(
                k,
                ext_direction,
                dist,
                extension_continuity,
            )?)),
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) | SurfaceKind::SrfInVolume(_) => {
                base_create_extended_surface()
            }
            SurfaceKind::Plane(s) => Ok(Self::from(s.create_extended_surface_dir(
                ext_direction,
                dist,
                extension_continuity,
            )?)),
            SurfaceKind::Sphere(s) => Ok(Self::from(
                s.base().base().create_extended_surface_dispatched(
                    ext_direction,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::Torus(s) => Ok(Self::from(
                s.base().base().create_extended_surface_dispatched(
                    ext_direction,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::Cone(s) => Ok(Self::from(
                s.rev().base().create_extended_surface_dispatched(
                    ext_direction,
                    dist,
                    extension_continuity,
                    &|uv| {
                        self.is_singularity(k, uv, EFF_ZERO, false)
                            .map(|r| (r.singular, r.direction))
                    },
                    &|domain, param| self.is_closed(k, domain, param, None, false).map(|r| r.0),
                )?,
            )),
            SurfaceKind::OffsetSurface(s) => {
                s.create_extended_surface_dir(k, ext_direction, dist, extension_continuity)
            }
            SurfaceKind::SurfOfRevolution(s) => {
                Ok(Self::from(s.base().create_extended_surface_dir(
                    k,
                    ext_direction,
                    dist,
                    extension_continuity,
                )?))
            }
            // (`GetOrCreateGwNurbPointer` makes a recognised extrusion's NURB)
            SurfaceKind::SurfOfExtrusion(s) => Ok(Self::from(
                s.base_with_nurb(k)?.create_extended_surface_dir(
                    k,
                    ext_direction,
                    dist,
                    extension_continuity,
                )?,
            )),
        }
    }

    // ---- database -------------------------------------------------------------------

    /// Reads a surface of type `surface_type` (`ReadFromDB`, a virtual static
    /// dispatching on the type).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a type that is not a surface; [`SolidError::Unported`] for the
    /// surface types not ported yet; those of the type's reader.
    pub fn read_from_db(
        k: &Kernel,
        surface_type: i64,
        db: &mut DatabaseReader,
        version: u64,
    ) -> Result<Self> {
        db.enter_nested(OP_READ)?;
        let result = Self::read_from_db_body(k, surface_type, db, version);
        db.leave_nested();
        result
    }

    fn read_from_db_body(
        k: &Kernel,
        surface_type: i64,
        db: &mut DatabaseReader,
        version: u64,
    ) -> Result<Self> {
        // pass the call along to the appropriate derived type
        match surface_type {
            B_SPLINE_SURFACE_TYPE => Ok(Self::from(BSplineSurface::read_from_db(db, version)?)),
            PLANE_TYPE => Ok(Self::from(Plane::read_from_db(db, version)?)),
            SURF_OF_EXTRUSION_TYPE => {
                Ok(Self::from(SurfOfExtrusion::read_from_db(k, db, version)?))
            }
            SURF_OF_REVOLUTION_TYPE => {
                Ok(Self::from(SurfOfRevolution::read_from_db(k, db, version)?))
            }
            CONE_TYPE | CYLINDER_TYPE => Ok(Self::from(Cone::read_from_db(
                k,
                db,
                version,
                surface_type == CYLINDER_TYPE,
            )?)),
            SPHERE_TYPE => Ok(Self::from(Sphere::read_from_db(k, db, version)?)),
            TORUS_TYPE => Ok(Self::from(Torus::read_from_db(k, db, version)?)),
            CURVE_BOUNDED_SURFACE_TYPE => Ok(Self::from(CurveBoundedSurface::read_from_db(
                k, db, version,
            )?)),
            OFFSET_SURFACE_TYPE => Ok(Self::from(OffsetSurface::read_from_db(k, db, version)?)),
            SRF_IN_VOLUME_TYPE => Ok(Self::from(SrfInVolume::read_from_db(k, db, version)?)),
            STEP_SURFACE_TYPE => Ok(Self::from(StepSurface::read_from_db(k, db, version)?)),
            _ => Err(SolidError::sm_err(OP_READ)),
        }
    }

    // ---- intersections and classification -------------------------------------------

    /// Splits the curve of `classification` at this surface's natural boundaries and
    /// classifies the pieces (`CurveOnClassify`): the classification is reset to one
    /// interval, its face cleared and its surfaces set to `this` (the handle naming
    /// this surface) and none, then intersected with the natural bounds; an unknown
    /// first start or last end takes its interval's mid classification.
    ///
    /// # Errors
    ///
    /// Those of the reset and of
    /// [`CurveClassification::intersect_natural_bounds`] (`geometry` resolves the
    /// classification's handles); `SM_ERR` for a classification left without
    /// intervals (upstream indexes interval 0 unchecked).
    pub fn curve_on_classify<O: ClassObject>(
        &self,
        k: &Kernel,
        this: O,
        geometry: &impl ClassGeometry<O>,
        classification: &mut CurveClassification<O>,
    ) -> Result<()> {
        // init output
        classification.reset(k, &ResetOptions::default())?;
        classification.set_face(None);
        classification.set_surface1(Some(this));
        classification.set_surface2(None);

        // 1. split the ClassificationCurve into classified intervals at natural boundary
        // curve intersections
        classification.intersect_natural_bounds(k, geometry, self, this)?;

        // classify the first Start and the last End from their Mids
        let num_ivls = classification.size();
        if num_ivls == 0 {
            return Err(SolidError::sm_err(OP_CURVE_ON_CLASSIFY));
        }
        let start_ivl = classification.at_mut(0)?;
        if start_ivl.start.point_class() == PointClassType::Unknown {
            let (pc, obj) = (start_ivl.mid.point_class(), start_ivl.mid.object());
            start_ivl.start.set_class_object(pc, obj);
        }
        let end_ivl = classification.at_mut(num_ivls - 1)?;
        if end_ivl.end.point_class() == PointClassType::Unknown {
            let (pc, obj) = (end_ivl.mid.point_class(), end_ivl.mid.object());
            end_ivl.end.set_class_object(pc, obj);
        }
        Ok(())
    }

    /// The intersection curves of this surface over `uv_domain` with `other` over
    /// `other_uv_domain` (`GlobalSurfaceIntersect`): the `SmPlane` override (the
    /// plane/plane special case first) or the `SmSurfOfRevolution` override, else
    /// [`base_global_surface_intersect`].
    ///
    /// # Errors
    ///
    /// Those of the overrides and of [`base_global_surface_intersect`].
    pub fn global_surface_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<SurfaceIntersection> {
        self.global_surface_intersect_in::<crate::surface::surface::p4::NoObject>(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            None,
        )
    }

    /// [`Surface::global_surface_intersect`] through the caller's face-aware cache
    /// context. `handles` name the two intersected surfaces (this surface and
    /// `other`) in their caches, so the context can bind a surface owned by a face to
    /// that face (upstream reaches it through the surface pointer).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::global_surface_intersect`] and of the context's cache
    /// preparation.
    #[allow(clippy::too_many_arguments)]
    pub fn global_surface_intersect_with_context<O: SolutionObject + Copy + PartialEq>(
        &self,
        k: &Kernel,
        handles: [Option<O>; 2],
        context: &mut dyn SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<SurfaceIntersection> {
        self.global_surface_intersect_in(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            Some(SurfaceIntersectContext {
                solver: context,
                handles,
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn global_surface_intersect_in<O: SolutionObject + Copy + PartialEq>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<SurfaceIntersectContext<'_, O>>,
    ) -> Result<SurfaceIntersection> {
        let mut out = SurfaceIntersection::default();
        self.global_surface_intersect_into(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            context,
            &mut out,
        )?;
        Ok(out)
    }

    /// [`Surface::global_surface_intersect`] with the outputs upstream leaves in the
    /// caller's arrays also on a failure (`SmApiIntersectSurfaces` hands its array to
    /// `GlobalSurfaceIntersect` and keeps what is in it): on an error, the curves the
    /// failing special-case intersection appended before it (the general intersection
    /// resets the arrays first).
    #[allow(clippy::too_many_arguments)]
    pub fn global_surface_intersect_with_partial(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> (SurfaceIntersection, Result<()>) {
        let mut out = SurfaceIntersection::default();
        let status = self.global_surface_intersect_into(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            None::<SurfaceIntersectContext<'_, crate::surface::surface::p4::NoObject>>,
            &mut out,
        );
        (out, status)
    }

    /// [`Surface::global_surface_intersect_in`] into `out` (upstream's output arrays):
    /// the kind's override fills it; on an error `out` keeps what the failing
    /// special-case intersection appended before it.
    #[allow(clippy::too_many_arguments)]
    fn global_surface_intersect_into<O: SolutionObject + Copy + PartialEq>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<SurfaceIntersectContext<'_, O>>,
        out: &mut SurfaceIntersection,
    ) -> Result<()> {
        match self.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => crate::surface::plane::p2::global_surface_intersect_into(
                k,
                self,
                p,
                uv_domain,
                other,
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                context,
                out,
            ),
            // SmCone::GlobalSurfaceIntersect: its special cases, else the base
            // SmSurface::GlobalSurfaceIntersect of this surface
            SurfaceKind::Cone(c) => c.global_surface_intersect_on_into(
                Some(self),
                k,
                uv_domain,
                crate::surface::cone::p3::IntersectTarget::Surface(other),
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                |req, out| {
                    base_global_surface_intersect_into(
                        req.kernel,
                        self,
                        req.domain,
                        other,
                        req.other_domain,
                        req.use_surface_edges,
                        req.approx_tol_3d,
                        req.ang_tol_rad,
                        context,
                        out,
                    )
                },
                out,
            ),
            // SmSphere::GlobalSurfaceIntersect: its special cases, else the base
            // SmSurface::GlobalSurfaceIntersect of this surface
            SurfaceKind::Sphere(s) => s.global_surface_intersect_with_into(
                k,
                uv_domain,
                other,
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                |k, out| {
                    base_global_surface_intersect_into(
                        k,
                        self,
                        uv_domain,
                        other,
                        other_uv_domain,
                        use_surface_edges,
                        approx_tol_3d,
                        ang_tol_rad,
                        context,
                        out,
                    )
                },
                out,
            ),
            // the inherited SmSurfOfRevolution::GlobalSurfaceIntersect: the plane
            // special case runs the inherited IntersectWithPlane with the torus's iso
            // circles, the sphere, cone and surface of revolution cases the inherited
            // intersections with the torus's domain conversion; each then takes the
            // general intersection when it needs more (which resets the output arrays
            // first: SmSurfaceIntersector::DoIntersection); any other surface takes the
            // general intersection
            SurfaceKind::Torus(t) => {
                // (each special case resets the outputs on entry)
                *out = SurfaceIntersection::default();
                // IntersectWithSphere / IntersectWithCone: a planar generator, else
                // the general intersection
                let revolved = |other: &dyn Revolved, out: &mut SurfaceIntersection| {
                    if !t.base().planar_generator {
                        return Ok(true);
                    }
                    intersect_revolved_into(
                        t.as_ref(),
                        k,
                        uv_domain,
                        other,
                        other_uv_domain,
                        approx_tol_3d,
                        out,
                    )
                };
                let more = match other.kind_with_nurb(k)? {
                    // the plane's boxes take its face's tolerance
                    SurfaceKind::Plane(p) => t.intersect_with_plane_owned_into(
                        k,
                        p,
                        crate::surface::plane::p2::PlaneOwner::of(other)?,
                        other_uv_domain,
                        approx_tol_3d,
                        out,
                    )?,
                    SurfaceKind::Sphere(o) => revolved(o.as_ref(), out)?,
                    SurfaceKind::Cone(o) if o.type_code() == CONE_TYPE => {
                        revolved(o.as_ref(), out)?
                    }
                    // (IntersectWithSurfOfRevolution checks the generator itself)
                    SurfaceKind::SurfOfRevolution(o) => intersect_revolved_into(
                        t.as_ref(),
                        k,
                        uv_domain,
                        o.as_ref(),
                        other_uv_domain,
                        approx_tol_3d,
                        out,
                    )?,
                    _ => true,
                };
                if more {
                    base_global_surface_intersect_into(
                        k,
                        self,
                        uv_domain,
                        other,
                        other_uv_domain,
                        use_surface_edges,
                        approx_tol_3d,
                        ang_tol_rad,
                        context,
                        out,
                    )?;
                }
                Ok(())
            }
            SurfaceKind::SurfOfRevolution(s) => s.global_surface_intersect_into(
                self,
                k,
                uv_domain,
                other,
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                context,
                out,
            ),
            // (its special cases append nothing before an error: the result replaces
            // the outputs, which stay empty on an error)
            SurfaceKind::SurfOfExtrusion(s) => {
                *out = SurfaceIntersection::default();
                *out = s.global_surface_intersect_on_in(
                    k,
                    self,
                    uv_domain,
                    other,
                    other_uv_domain,
                    use_surface_edges,
                    approx_tol_3d,
                    ang_tol_rad,
                    context,
                )?;
                Ok(())
            }
            SurfaceKind::BSpline(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => base_global_surface_intersect_into(
                k,
                self,
                uv_domain,
                other,
                other_uv_domain,
                use_surface_edges,
                approx_tol_3d,
                ang_tol_rad,
                context,
                out,
            ),
        }
    }

    /// The intersection curve of this surface with `other` through the given pairs
    /// of UV points (`PointBasedSurfaceIntersect`, UV point version): the points are
    /// handed to `SmAdvSurfaceIntersector::DoPointIntersection` (the base driver,
    /// [`crate::intersect::surface_intersector::SurfaceIntersector::do_point_intersection`],
    /// which reaches the advanced intersector's `ComputeSingularityPoint` override).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for point lists of different lengths or of 20
    /// points or more (upstream writes past its arrays); those of the intersector's
    /// construction and of `DoPointIntersection`.
    pub fn point_based_surface_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        surface1_points: &[Point2],
        surface2_points: &[Point2],
        extend: [bool; 2],
        directions: [Option<Vec3>; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<PointIntersection> {
        if surface1_points.len() >= 20 || surface1_points.len() != surface2_points.len() {
            return Err(SolidError::invalid_input(
                "surface::surface::p2::Surface::point_based_surface_intersect",
                "need fewer than 20 point pairs",
            ));
        }

        // allocate the surf/surf intersector
        let mut ssi: AdvSurfaceIntersector<'_, '_, NoSurfaceHandle> =
            AdvSurfaceIntersector::new(k, self, uv_domain, other, other_uv_domain, false)?;

        // store every given surface point in a SmTsectPnt object (upstream's are on
        // its stack, here in the intersector's arena)
        let tsect_points: Vec<TsectId> = surface1_points
            .iter()
            .zip(surface2_points)
            .map(|(&uv1, &uv2)| {
                let mut tsp = TsectPnt::new();
                *tsp.uv_pos_mut(SrfIndex::First) = uv1;
                *tsp.uv_pos_mut(SrfIndex::Second) = uv2;
                ssi.base.alloc_point(tsp)
            })
            .collect();

        // pass the call along
        let [extend_before_start, extend_after_end] = extend;
        let [start_direction, end_direction] = directions;
        let curve = ssi.base.do_point_intersection(
            k,
            &tsect_points,
            &PointIntersectionOptions {
                extend_before_start,
                extend_after_end,
                start_direction,
                end_direction,
                approx_tol_3d,
                ang_tol_rad,
            },
        )?;
        let [surface1_uv_curve, surface2_uv_curve] = curve.uv_curves;
        Ok(PointIntersection {
            curve_3d: curve.curve_3d,
            surface1_uv_curve,
            surface2_uv_curve,
            curve_type: curve.curve_type,
            deviation: curve.deviation,
        })
    }

    /// A plane containing a set of coplanar 3D curves, with the curves ordered into
    /// loops (`CreatePlaneWith3DCurves`): the plane is a bilinear surface over the
    /// curves' bounding sphere grown by 10%, centred on the projection of the sphere
    /// centre, copied with analytics.
    ///
    /// # Errors
    ///
    /// Those of the loop ordering ([`trimming_tools::order_curves_into_loops`], whose
    /// status is `SM_ERR` for open loops or curves off the plane, and
    /// [`trimming_tools::find_outer_loops`]), of the plane frame and of the bilinear
    /// surface.
    pub fn create_plane_with_3d_curves(
        k: &Kernel,
        curves: &[Curve],
        tol_3d: f64,
    ) -> Result<PlaneWith3dCurves> {
        // order curves into loops - clean up curves, build plane, check planarity
        let r = trimming_tools::order_curves_into_loops(k, curves, tol_3d)?;
        r.status()?;
        let trimming_tools::OrderedLoops {
            mut ordered,
            mut loop_counts,
            mut orients,
            plane_point: plane_base_pt,
            mut plane_normal,
            curves_box: face_extent,
            ..
        } = r;

        let mut outer_loop_counts = trimming_tools::find_outer_loops(
            k,
            curves,
            &mut ordered,
            &mut loop_counts,
            &mut orients,
            &mut plane_normal,
            tol_3d,
        )?;

        // not upstream (WR-405, #892, opt-in): an inner loop touching another loop of
        // its face at a vertex is part of that loop
        if k.fixes.join_touching_loops {
            join_touching_loops(
                k,
                curves,
                &mut ordered,
                &mut loop_counts,
                &mut orients,
                &mut outer_loop_counts,
                tol_3d,
            )?;
        }

        // make ortho-normal vector set from PlaneNormal vector
        let [_, plane_x_vect, mut plane_y_vect] = plane_normal.make_unit_ortho_vectors(None)?;

        // make planeNormal lie in +z direction
        if plane_normal.dot(plane_x_vect.cross(plane_y_vect)) < 0.0 {
            plane_y_vect = -plane_y_vect;
        }

        // Make plane X and Y vectors longer than the plane's bounding box
        let sphere = face_extent.sphere_bound();
        let face_center = sphere.center;
        let face_radius = sphere.radius;

        let box_expansion = 1.1;
        let plane_x_vect = box_expansion * face_radius * plane_x_vect;
        let plane_y_vect = box_expansion * face_radius * plane_y_vect;

        // get the center point of the limited plane.
        let v = face_center - plane_base_pt;
        let center_pt = face_center - (v.dot(plane_normal)) * plane_normal;

        // get points at the plane's four corners
        let pt00 = center_pt - plane_x_vect - plane_y_vect;
        let pt10 = center_pt + plane_x_vect - plane_y_vect;
        let pt11 = center_pt + plane_x_vect + plane_y_vect;
        let pt01 = center_pt - plane_x_vect + plane_y_vect;

        // make a bilinear BSpline planar surface from the 4 plane corner points
        let bs = BSplineSurface::create_bilinear_surface(pt00, pt10, pt01, pt11)?;

        // Copy pBS, when possible as an analytic surface (the SmBSplineSurface override)
        let plane = bs.copy_and_add_analytics(k)?;

        Ok(PlaneWith3dCurves {
            plane,
            loop_counts,
            ordered_curves: ordered,
            orients,
            outer_loop_counts,
        })
    }
}

/// Not upstream (WR-405, source issue #892; the source fork's
/// `SmTrimmingTools::JoinTouchingLoops`): joins each loop that touches another loop
/// of its outer loop's set at a vertex into that loop, so the face's boundary passes
/// the vertex twice in one loop. `ordered`, `loop_counts`, `orients` and
/// `outer_loop_counts` are `find_outer_loops`' output (indices into `curves`).
/// Within each set, while a curve of one loop starts (as its loop runs it) where a
/// curve of a later loop starts, the later loop is spliced into the earlier one there:
/// the earlier loop's curves up to that vertex, the later loop's from that vertex
/// round to it again, then the rest of the earlier loop. Each loop keeps its
/// direction; the outer loop stays first. The outputs change only when a loop joins.
///
/// # Errors
///
/// `SM_ERR` when the counts do not match the curves; those of the curve evaluations.
fn join_touching_loops(
    k: &Kernel,
    curves: &[Curve],
    ordered: &mut Vec<usize>,
    loop_counts: &mut Vec<usize>,
    orients: &mut Vec<OrientType>,
    outer_loop_counts: &mut Vec<usize>,
    tol_3d: f64,
) -> Result<()> {
    const OP: &str = "surface::surface::join_touching_loops";
    let bad = || SolidError::sm_err(OP);
    // each outer loop's set of loops, each loop its (curve, orientation)s
    let mut sets: Vec<Vec<Vec<(usize, OrientType)>>> = Vec::new();
    let (mut curve, mut lp) = (0usize, 0usize);
    for &loops in outer_loop_counts.iter() {
        let mut set = Vec::new();
        for _ in 0..loops {
            let count = *loop_counts.get(lp).ok_or_else(bad)?;
            lp += 1;
            let mut members = Vec::new();
            for _ in 0..count {
                let index = *ordered.get(curve).ok_or_else(bad)?;
                let orient = *orients.get(curve).ok_or_else(bad)?;
                if curves.get(index).is_none() {
                    return Err(bad());
                }
                members.push((index, orient));
                curve += 1;
            }
            set.push(members);
        }
        sets.push(set);
    }
    if lp != loop_counts.len() || curve != ordered.len() || curve != orients.len() {
        return Err(bad());
    }

    // where a curve starts as its loop runs it
    let start = |(index, orient): (usize, OrientType)| -> Result<Point3> {
        let c = curves.get(index).ok_or_else(bad)?;
        let ivl = c.natural_interval();
        let t = if orient == OrientType::Same {
            ivl.min()
        } else {
            ivl.max()
        };
        c.evaluate_point(k, t)
    };

    let mut joined = false;
    // every join removes a loop, so a set joins at most its loop count less one times
    for set in &mut sets {
        let mut found = true;
        while found {
            found = false;
            let mut join = None;
            'search: for (aa, loop_a) in set.iter().enumerate() {
                for (bb, loop_b) in set.iter().enumerate().skip(aa + 1) {
                    for (ia, &curve_a) in loop_a.iter().enumerate() {
                        let a = start(curve_a)?;
                        for (ib, &curve_b) in loop_b.iter().enumerate() {
                            if a.distance(start(curve_b)?) >= tol_3d {
                                continue;
                            }
                            join = Some((aa, bb, ia, ib));
                            break 'search;
                        }
                    }
                }
            }
            if let Some((aa, bb, ia, ib)) = join {
                let loop_b = set.remove(bb);
                let loop_a = set.get_mut(aa).ok_or_else(bad)?;
                // loop aa up to the vertex, loop bb round from it, then the rest of aa
                let rest = loop_a.split_off(ia);
                loop_a.extend(loop_b.iter().skip(ib).copied());
                loop_a.extend(loop_b.iter().take(ib).copied());
                loop_a.extend(rest);
                found = true;
                joined = true;
            }
        }
    }
    if !joined {
        return Ok(());
    }

    // set output
    ordered.clear();
    orients.clear();
    loop_counts.clear();
    outer_loop_counts.clear();
    for set in &sets {
        outer_loop_counts.push(set.len());
        for members in set {
            loop_counts.push(members.len());
            for &(index, orient) in members {
                ordered.push(index);
                orients.push(orient);
            }
        }
    }
    Ok(())
}

// ---- generic evaluation interface --------------------------------------------------------

/// The `SmSurface` virtuals that code generic over a surface type calls (implemented
/// by [`Surface`] and by the ported kinds, so a [`BSplineSurface`] can be used
/// without wrapping it).
pub trait SurfaceEvaluate {
    /// `LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, found, sol)`: whether an
    /// answer was found, the solution value and the parameters of its start (`0` when
    /// none).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] (upstream ignores the status; a failed solve leaves the
    /// default solution).
    fn local_point_solve_minimize(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        point: Point3,
        guess: Point2,
    ) -> Result<(bool, f64, Point2)>;

    /// `DropPoint(point, domain, guess, ..)` with the default operation: the parameters
    /// and distance of the drop, `None` when it failed.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] (upstream ignores the status; a failed drop is `None`).
    fn drop_point_minimize(
        &self,
        k: &Kernel,
        point: Point3,
        domain: &Extent2d,
        guess: Option<Point2>,
    ) -> Result<Option<(Point2, f64)>>;

    /// `GetNaturalUVDomain` without a kernel (a lazy analytic [`Surface`] answers its
    /// NURB-less base's empty domain, see [`Surface::natural_uv_domain_if_built`]).
    fn natural_uv_domain_if_built(&self) -> Extent2d;

    /// `GetNaturalUVDomain`, virtual: a lazy analytic [`Surface`] makes its NURB first
    /// and reads that NURB's domain ([`Surface::natural_uv_domain_with_nurb`]). Code
    /// generic over a surface type reads the domain through this.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the make.
    fn natural_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d>;

    /// `Evaluate` (see [`Surface::evaluate`]).
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>>;

    /// `EvaluatePoint`.
    ///
    /// # Errors
    ///
    /// Those of the evaluation.
    fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3>;

    /// The virtual `operator==` (unit `surface.surface_p1`).
    ///
    /// # Errors
    ///
    /// Those of the comparison.
    fn surface_approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool>;

    /// `GetKnots`, virtual: a lazy surface makes its NURB first
    /// ([`Surface::knots_with_nurb`]).
    ///
    /// # Errors
    ///
    /// Those of the knot query.
    fn knots(
        &self,
        k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)>;

    /// `IsKindOf`.
    ///
    /// # Errors
    ///
    /// Those of the type query.
    fn is_kind_of(&self, t: i64) -> Result<bool>;

    /// `EvaluateNormal` (virtual): by default the `SmSurface` body
    /// ([`base_evaluate_normal`](super::p7::base_evaluate_normal), unit
    /// `surface.surface_p7`), which `SmBSplineSurface` does not override.
    ///
    /// # Errors
    ///
    /// Only [`SolidError::Unported`].
    fn evaluate_normal(&self, k: &Kernel, uv: Point2, u_side: Side, v_side: Side) -> Result<Vec3> {
        super::p7::base_evaluate_normal(k, self, uv, u_side, v_side)
    }

    /// `LocalCurveSolve(.., SM_SO_MAXIMIZE, SM_EFF_ZERO_SQRT, ..)`: the curve parameter
    /// and surface parameters of the gap maximum near the guesses, if any.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::local_curve_solve`].
    fn local_curve_maximize(
        &self,
        k: &Kernel,
        curve: &Curve,
        domain: &Extent2d,
        interval: &Extent1d,
        guess_uv: Point2,
        guess_t: f64,
    ) -> Result<Option<(f64, Point2)>>;
}

impl SurfaceEvaluate for Surface {
    fn local_point_solve_minimize(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        point: Point3,
        guess: Point2,
    ) -> Result<(bool, f64, Point2)> {
        // (status ignored upstream: a failed solve leaves the default solution)
        let Some(r) = crate::error::ignored(self.local_point_solve::<super::p4::NoObject>(
            k,
            domain,
            SolverOperation::Minimize,
            point,
            guess,
        ))?
        else {
            return Ok((false, 0.0, Point2::new(0.0, 0.0)));
        };
        Ok((
            r.found,
            r.solution.start.solution_value,
            Point2::new(r.solution.start.param(0)?, r.solution.start.param(1)?),
        ))
    }

    fn drop_point_minimize(
        &self,
        k: &Kernel,
        point: Point3,
        domain: &Extent2d,
        guess: Option<Point2>,
    ) -> Result<Option<(Point2, f64)>> {
        let opts = super::p5::DropPointOptions {
            guess_uv: guess,
            ..Default::default()
        };
        // (status ignored upstream: a failed drop leaves bSuccess FALSE)
        let r =
            crate::error::ignored(self.drop_point::<super::p4::NoObject>(k, point, domain, &opts))?;
        Ok(r.and_then(|r| r.success.then_some((r.result_uv, r.gap))))
    }

    fn surface_approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        self.approx_eq(k, other)
    }

    fn natural_uv_domain_if_built(&self) -> Extent2d {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        Surface::natural_uv_domain_if_built(self)
    }

    fn natural_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d> {
        Surface::natural_uv_domain_with_nurb(self, k)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        Self::evaluate(self, k, uv, highest_u_deriv, highest_v_deriv, flags)
    }

    fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        Self::evaluate_point(self, k, uv)
    }

    fn knots(
        &self,
        k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        Self::knots_with_nurb(self, k, param, ivl)
    }

    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Self::is_kind_of(self, t)
    }

    fn evaluate_normal(&self, k: &Kernel, uv: Point2, u_side: Side, v_side: Side) -> Result<Vec3> {
        Self::evaluate_normal(self, k, uv, u_side, v_side)
    }

    fn local_curve_maximize(
        &self,
        k: &Kernel,
        curve: &Curve,
        domain: &Extent2d,
        interval: &Extent1d,
        guess_uv: Point2,
        guess_t: f64,
    ) -> Result<Option<(f64, Point2)>> {
        Ok(self
            .local_curve_solve(
                k,
                domain,
                curve,
                interval,
                SolverOperation::Maximize,
                EFF_ZERO_SQRT,
                &[],
                guess_uv,
                guess_t,
            )?
            .map(|s| (s.t, s.uv)))
    }
}

impl SurfaceEvaluate for BSplineSurface {
    fn local_point_solve_minimize(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        point: Point3,
        guess: Point2,
    ) -> Result<(bool, f64, Point2)> {
        // the SmSurface body, on a Surface holding a copy
        Surface::from(self.clone_exact()).local_point_solve_minimize(k, domain, point, guess)
    }

    fn drop_point_minimize(
        &self,
        k: &Kernel,
        point: Point3,
        domain: &Extent2d,
        guess: Option<Point2>,
    ) -> Result<Option<(Point2, f64)>> {
        Surface::from(self.clone_exact()).drop_point_minimize(k, point, domain, guess)
    }

    fn local_curve_maximize(
        &self,
        k: &Kernel,
        curve: &Curve,
        domain: &Extent2d,
        interval: &Extent1d,
        guess_uv: Point2,
        guess_t: f64,
    ) -> Result<Option<(f64, Point2)>> {
        // SmBSplineSurface does not override SmSurface::LocalCurveSolve: run it on a
        // `Surface` holding this B-spline surface.
        SurfaceEvaluate::local_curve_maximize(
            &Surface::from(self.clone()),
            k,
            curve,
            domain,
            interval,
            guess_uv,
            guess_t,
        )
    }

    fn surface_approx_eq(&self, _k: &Kernel, other: &Self) -> Result<bool> {
        // SmBSplineSurface::operator== (two B-splines: SmSurface::operator== holds)
        Ok(self.approx_eq(other))
    }

    fn natural_uv_domain_if_built(&self) -> Extent2d {
        BSplineSurface::natural_uv_domain(self)
    }

    fn natural_uv_domain_with_nurb(&self, _k: &Kernel) -> Result<Extent2d> {
        // (a B-spline surface is its own NURB)
        Ok(BSplineSurface::natural_uv_domain(self))
    }

    fn evaluate(
        &self,
        _k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        Self::evaluate(self, uv, highest_u_deriv, highest_v_deriv, flags)
    }

    fn evaluate_point(&self, _k: &Kernel, uv: Point2) -> Result<Point3> {
        Self::evaluate_point(self, uv)
    }

    fn knots(
        &self,
        _k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        Self::knots(self, param, ivl)
    }

    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Self::is_kind_of(self, t)
    }
}

// ---- base-class defaults ----------------------------------------------------------------

/// `SmSurface::Copy`: `SM_ERR`.
pub const fn base_copy() -> Result<Surface> {
    Err(base_err())
}

/// `SmSurface::CopyAndAddAnalytics`: `Copy`, which fails for the base class.
pub const fn base_copy_and_add_analytics() -> Result<Surface> {
    base_copy()
}

/// `SmSurface::AdjustSTEPUVDomain`: `SM_ERR`.
pub const fn base_adjust_step_uv_domain() -> Result<()> {
    Err(base_err())
}

/// `SmSurface::CreateExtendedSurface` (both overloads): `SM_ERR`.
pub const fn base_create_extended_surface() -> Result<Surface> {
    Err(base_err())
}

/// `SmSurface::DropPointFast`: `SM_ERR`.
pub const fn base_drop_point_fast<O>() -> Result<SolutionArray<O>> {
    Err(base_err())
}

/// `SmSurface::Evaluate`: `SM_ERR`.
pub const fn base_evaluate() -> Result<Vec<Vec<Vec3>>> {
    Err(base_err())
}

/// `SmSurface::EvaluatePoint`: `SM_ERR`.
pub const fn base_evaluate_point() -> Result<Point3> {
    Err(base_err())
}

/// `SmSurface::GetNaturalUVDomain`: reports `SM_ERR` and returns the degenerate
/// domain `(0,0)-(0,0)`.
#[must_use]
pub const fn base_natural_uv_domain() -> Extent2d {
    Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0)
}

/// `SmSurface::GetSTEPUVDomain`: the natural domain of `surface`.
#[must_use]
pub fn base_step_uv_domain(surface: &Surface) -> Extent2d {
    // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
    surface.natural_uv_domain_if_built()
}

/// `SmSurface::GetDegree`: reports `SM_ERR` and returns `SM_ERR` (1001) as the
/// degree.
#[must_use]
pub const fn base_degree() -> usize {
    SM_ERR as usize
}

/// `SmSurface::GetNumberControlPoints`: `0`.
#[must_use]
pub const fn base_number_control_points() -> usize {
    0
}

/// `SmSurface::GlobalSurfaceIntersect`: the intersection curves of `this` over
/// `uv_domain` with `other` over `other_uv_domain`, through `SmAdvSurfaceIntersector`
/// (the general intersection of the kinds without an override, and the fall-back of the
/// overrides): [`crate::intersect::surface_intersector::p5::do_intersection`].
///
/// # Errors
///
/// Those of [`crate::intersect::surface_intersector::p5::do_intersection`].
#[allow(clippy::too_many_arguments)] // upstream's argument list
pub fn base_global_surface_intersect(
    k: &Kernel,
    this: &Surface,
    uv_domain: &Extent2d,
    other: &Surface,
    other_uv_domain: &Extent2d,
    use_surface_edges: [bool; 2],
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
) -> Result<SurfaceIntersection> {
    crate::intersect::surface_intersector::p5::do_intersection(
        k,
        this,
        uv_domain,
        other,
        other_uv_domain,
        use_surface_edges,
        approx_tol_3d,
        ang_tol_rad,
    )
}

/// [`base_global_surface_intersect`] through the caller's face-aware cache context.
/// `handles` name the two surfaces in their caches.
#[allow(clippy::too_many_arguments)]
pub(crate) fn base_global_surface_intersect_in<O: SolutionObject + Copy + PartialEq>(
    k: &Kernel,
    this: &Surface,
    uv_domain: &Extent2d,
    other: &Surface,
    other_uv_domain: &Extent2d,
    use_surface_edges: [bool; 2],
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
    context: Option<SurfaceIntersectContext<'_, O>>,
) -> Result<SurfaceIntersection> {
    match context {
        Some(context) => crate::intersect::surface_intersector::p5::do_intersection_with_context(
            k,
            this,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            context.solver,
            context.handles,
        ),
        None => crate::intersect::surface_intersector::p5::do_intersection(
            k,
            this,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
        ),
    }
}

/// [`base_global_surface_intersect_in`] into `out`: `SmSurfaceIntersector::DoIntersection`
/// resets the output arrays first, then sets them. Its only append before a later error
/// (`GrazingBoxProcess`'s touching point, whose UV degenerate curves cannot fail for a
/// corner inside the domains) is not reached, so on an error `out` is empty.
#[allow(clippy::too_many_arguments)]
pub(crate) fn base_global_surface_intersect_into<O: SolutionObject + Copy + PartialEq>(
    k: &Kernel,
    this: &Surface,
    uv_domain: &Extent2d,
    other: &Surface,
    other_uv_domain: &Extent2d,
    use_surface_edges: [bool; 2],
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
    context: Option<SurfaceIntersectContext<'_, O>>,
    out: &mut SurfaceIntersection,
) -> Result<()> {
    *out = SurfaceIntersection::default();
    *out = base_global_surface_intersect_in(
        k,
        this,
        uv_domain,
        other,
        other_uv_domain,
        use_surface_edges,
        approx_tol_3d,
        ang_tol_rad,
        context,
    )?;
    Ok(())
}

/// `SmSurface::GetMaxAnalyticDomain`: reports `SM_ERR` and returns an initialised
/// (empty) domain.
#[must_use]
pub const fn base_max_analytic_domain() -> Extent2d {
    Extent2d::new()
}

/// `SmSurface::IsBilinear`: `FALSE`.
#[must_use]
pub const fn base_is_bilinear() -> bool {
    false
}

/// `SmSurface::IsParallelToVector`: `FALSE`.
#[must_use]
pub const fn base_is_parallel_to_vector() -> bool {
    false
}

/// `SmSurface::IntersectWithPlane`: `SM_ERR` (no output is set).
pub const fn base_intersect_with_plane() -> Result<(bool, SurfaceIntersection)> {
    Err(base_err())
}

/// `SmSurface::Reverse`, `Reparameterize`, `SwapUV`, `Transform` and
/// `TrimWithDomain`: `SM_ERR`.
pub const fn base_edit() -> Result<()> {
    Err(base_err())
}

/// `SmSurface::SplitAt`: `SM_ERR`.
pub const fn base_split_at() -> Result<(Surface, Surface)> {
    Err(base_err())
}

/// `SmSurface::STEPInversion`: `SM_ERR`.
pub const fn base_step_inversion() -> Result<(Point2, LocationType)> {
    Err(base_err())
}

/// `SmSurface::GetKnots`: clears the outputs and fails with `SM_ERR`.
pub const fn base_knots() -> Result<(Vec<f64>, Vec<usize>)> {
    Err(base_err())
}

/// `SmSurface::ConvertUVFromSTEPToNURBS` and `ConvertUVFromNURBSToSTEP`: the
/// identity.
#[must_use]
pub const fn base_convert_uv(uv: Point2) -> Point2 {
    uv
}

/// `SmSurface::GetDerivativeLimit`: the smallest first derivative magnitude that is
/// not numerical noise at `pos`, `10 * SM_EFF_ZERO * (1 + max|pos|)`.
#[must_use]
pub fn derivative_limit(pos: Point3) -> f64 {
    // Have to multiply by 10.0.  [my_ssi_analytic_demo case 9; B163.]
    10.0 * EFF_ZERO * (1.0 + pos.max_dimension())
}

/// `SmSurface::GetNormalLimit`: the smallest believable cross product of unit
/// vectors, `1e-8`.
#[must_use]
pub const fn normal_limit() -> f64 {
    1.0e-8
}

/// The edit notification hook of [`Surface::swap_uv_notifying`]: called with the
/// edited surface's identity slot, its type code and `SM_NO_PRE_EDIT` /
/// `SM_NO_POST_EDIT`.
pub(crate) type SurfaceEditNotify<'a> =
    dyn FnMut(&mut IdentitySlot, i64, NotifyOperation) -> Result<()> + 'a;

/// `SmSurface::Notify` of an edited surface ([`Surface::swap_uv_notifying`]): its
/// `SM_NO_PRE_EDIT` deletes the stored trimmed surface cache
/// (`SmCacheMgr::DeleteObjectsCache(SM_OC_TRIMSRF, this)`; a cache in use stays with
/// its caller), then `notify` gets the event (`SmAObject::Notify`).
fn edit_notify(
    cache_slot: &CacheSlot,
    slot: &mut IdentitySlot,
    type_code: i64,
    notify: &mut SurfaceEditNotify<'_>,
    operation: NotifyOperation,
) -> Result<()> {
    if operation == NotifyOperation::PreEdit {
        cache_slot.with(|s| {
            if matches!(s.trim, TrimCacheSlot::Stored(_)) {
                s.trim = TrimCacheSlot::Empty;
                s.face_modified_while_out = false;
            }
        });
    }
    notify(slot, type_code, operation)
}

/// `SmSurface::Notify`: drops the surface's trimmed-surface cache before an edit and
/// on destruction, then passes the notification to `SmAObject::Notify`.
///
/// # Errors
///
/// Those of `SmCacheMgr::DeleteObjectsCache` and `SmAObject::Notify`.
pub fn notify<T>(
    caches: &mut CacheStore<T>,
    object: &mut AObject,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    match operation {
        NotifyOperation::PreEdit | NotifyOperation::Destruction => {
            a_object::delete_objects_cache(caches, object)?;
        }
        // SM_NO_UNKNOWN only prints a message; every other event has nothing to do here
        _ => {}
    }

    // Propagate notification up hierarchy
    a_object::notify(attributes, context, operation, this, data1, data2, data3)
}

/// `smsurf_GetSurfaceCache`: the surface's trimmed-surface cache, else its surface
/// cache, built fresh ([`crate::surface::surface::p1::get_surface_cache`]).
///
/// # Errors
///
/// Those of the cache builds.
pub fn surface_cache<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
) -> Result<SurfaceCache<O>> {
    crate::surface::surface::p1::get_surface_cache(k, surface, handle)
}

/// `sm_GetOriginalSurface`: the surface of the surface cache owning the tree of
/// `node` (its handle, `SmSurfaceCache::GetSurface`).
///
/// # Errors
///
/// `SM_ERR` when the tree is not owned by a surface cache or the cache has no
/// surface handle (upstream signals and reads through the owner anyway).
pub fn original_surface<O: Copy + PartialEq>(tree: &Tree<O>, _node: &TreeNode<O>) -> Result<O> {
    const OP: &str = "surface::surface::p2::original_surface";
    match tree.owner() {
        Some(TreeOwner::SurfaceCache | TreeOwner::TrimSrfCache | TreeOwner::TessSrfCache) => {
            tree.owner_object()?.ok_or(SolidError::sm_err(OP))
        }
        _ => Err(SolidError::sm_err(OP)),
    }
}

// ---- SmSurfaceEval ----------------------------------------------------------------------

/// A lazily evaluated surface point (`SmSurfaceEval`): position and derivatives up
/// to second order at a UV point, computed on first use.
///
/// `values()[i][j]` is the derivative `i` times in `u` and `j` times in `v` for
/// `i + j <= 2` (`[2][2]` holds the normal once `EvalNormal`, unit
/// `surface.surface_p3`, has run).
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct SurfaceEval<'s> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'s Kernel,

    /// `m_bNonZeroTangents`.
    non_zero_tangents: bool,
    /// `m_sUV`.
    uv: Point2,
    /// `m_bUFromLeft`.
    u_from_left: bool,
    /// `m_bVFromLeft`.
    v_from_left: bool,
    /// `m_sSectDir`.
    sect_dir: Vec3,
    /// `m_sPVV`.
    pvv: [[Vec3; 3]; 3],
    /// `m_sSectTangent`.
    sect_tangent: Vec3,
    /// `m_dSectCurvature`.
    sect_curvature: f64,
    /// `m_pSrf` (not owned).
    surface: Option<&'s Surface>,
    /// `m_bUVSet`.
    uv_set: bool,
    /// `m_bNormalSet`.
    normal_set: bool,
    /// `m_bSectDirSet`.
    sect_dir_set: bool,
    /// `m_bFailsEval`.
    fails_eval: bool,
}

/// The members of a [`SurfaceEval`] its lazy normal evaluations write.
pub(super) struct EvalStateMut<'a> {
    /// `m_sPVV`.
    pub(super) pvv: &'a mut [[Vec3; 3]; 3],
    /// `m_sSectTangent`.
    pub(super) sect_tangent: &'a mut Vec3,
    /// `m_dSectCurvature`.
    pub(super) sect_curvature: &'a mut f64,
    /// `m_bNormalSet`.
    pub(super) normal_set: &'a mut bool,
    /// `m_bSectDirSet`.
    pub(super) sect_dir_set: &'a mut bool,
}

impl<'s> SurfaceEval<'s> {
    /// An evaluator of `surface` with nothing set (`SmSurfaceEval(pSurf)`).
    #[must_use]
    pub const fn new(k: &'s Kernel, surface: Option<&'s Surface>) -> Self {
        Self {
            kernel: k,
            non_zero_tangents: true,
            uv: Point2::UNDEF,
            u_from_left: true,
            v_from_left: true,
            sect_dir: Vec3::UNDEF,
            pvv: [[Vec3::UNDEF; 3]; 3],
            sect_tangent: Vec3::UNDEF,
            sect_curvature: UNDEF_DOUBLE,
            surface,
            uv_set: false,
            normal_set: false,
            sect_dir_set: false,
            fails_eval: false,
        }
    }

    /// An evaluator of `surface` at `(u, v)` (`SmSurfaceEval(pSurf, dU, dV)`).
    #[must_use]
    pub fn with_uv(k: &'s Kernel, surface: Option<&'s Surface>, u: f64, v: f64) -> Self {
        let mut s = Self::new(k, surface);
        s.set_uv_coords(u, v);
        s
    }

    /// An evaluator of `surface` at `(u, v)` with cross-section direction
    /// `sect_dir` (`SmSurfaceEval(pSurf, dU, dV, rSectDir)`).
    #[must_use]
    pub fn with_uv_and_sect_dir(
        k: &'s Kernel,
        surface: Option<&'s Surface>,
        u: f64,
        v: f64,
        sect_dir: Vec3,
    ) -> Self {
        let mut s = Self::with_uv(k, surface, u, v);
        s.set_sect_dir(sect_dir);
        s
    }

    /// Moves to `uv`; a different point drops every evaluated value (`SetUV`).
    pub fn set_uv(&mut self, uv: Point2) {
        if self.uv.x != uv.x || self.uv.y != uv.y {
            self.uv = uv;
            self.uv_set = false;
            self.normal_set = false;
            self.sect_dir_set = false;
            self.fails_eval = false;
        }
    }

    /// Moves to `(u, v)`; a different point drops every evaluated value
    /// (`SetUV(dU, dV)`).
    pub fn set_uv_coords(&mut self, u: f64, v: f64) {
        if self.uv.x != u || self.uv.y != v {
            self.uv = Point2::new(u, v);
            self.uv_set = false;
            self.normal_set = false;
            self.sect_dir_set = false;
            self.fails_eval = false;
        }
    }

    /// Sets the cross-section direction; drops the cross-section values
    /// (`SetSectDir`).
    pub fn set_sect_dir(&mut self, sect_dir: Vec3) {
        self.sect_dir = sect_dir;
        self.sect_dir_set = false;
    }

    /// The UV point (`GetUV`).
    #[must_use]
    pub const fn uv(&self) -> Point2 {
        self.uv
    }

    /// The surface (`GetSurface`).
    #[must_use]
    pub const fn surface(&self) -> Option<&'s Surface> {
        self.surface
    }

    /// The cross-section direction (`m_sSectDir`).
    #[must_use]
    pub const fn sect_dir(&self) -> Vec3 {
        self.sect_dir
    }

    /// The side flags `(m_bUFromLeft, m_bVFromLeft)`.
    #[must_use]
    pub(super) const fn sides(&self) -> (bool, bool) {
        (self.u_from_left, self.v_from_left)
    }

    /// Mutable access to the members the lazy evaluations of unit
    /// `surface.surface_p4` (`EvalNormal`, `EvalNormalSection`) write.
    pub(super) const fn state_mut(&mut self) -> EvalStateMut<'_> {
        EvalStateMut {
            pvv: &mut self.pvv,
            sect_tangent: &mut self.sect_tangent,
            sect_curvature: &mut self.sect_curvature,
            normal_set: &mut self.normal_set,
            sect_dir_set: &mut self.sect_dir_set,
        }
    }

    /// The evaluation matrix as stored (`m_sPVV`; entries not evaluated yet keep
    /// their previous values).
    #[must_use]
    pub const fn values(&self) -> &[[Vec3; 3]; 3] {
        &self.pvv
    }

    /// The stored cross-section values `(m_sSectTangent, m_dSectCurvature)` (set by
    /// `EvalNormalSection`, unit `surface.surface_p4`).
    #[must_use]
    pub const fn stored_section(&self) -> (Vec3, f64) {
        (self.sect_tangent, self.sect_curvature)
    }

    /// The state flags `[m_bUVSet, m_bNormalSet, m_bSectDirSet, m_bFailsEval]`.
    #[must_use]
    pub const fn flags(&self) -> [bool; 4] {
        [
            self.uv_set,
            self.normal_set,
            self.sect_dir_set,
            self.fails_eval,
        ]
    }

    /// Evaluates position and derivatives up to second order unless done already
    /// (`Eval`): the upper half of the `2 x 2` matrix of `SmSurface::Evaluate`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the evaluation failed (now or before, until the point moves);
    /// `SM_ERR_NULL_POINTER` without surface (upstream dereferences `NULL`);
    /// [`SolidError::Unported`] from the evaluation.
    pub fn eval(&mut self) -> Result<()> {
        let k = self.kernel;
        if self.uv_set {
            return Ok(());
        }
        if self.fails_eval {
            return Err(SolidError::sm_err(OP_EVAL));
        }

        let surface = self.surface.ok_or(SolidError::null_pointer(OP_EVAL))?;
        let flags = EvaluateFlags {
            u_side: if self.u_from_left {
                Side::Left
            } else {
                Side::Right
            },
            v_side: if self.v_from_left {
                Side::Left
            } else {
                Side::Right
            },
            only_upper_half: true,
            non_zero_tangents: self.non_zero_tangents,
        };
        let (status, derivs) = eval_second_derivatives_with_output(k, surface, self.uv, flags);
        // Upstream Evaluate writes this array even when zero-tangent correction
        // ultimately fails. Its accessors still read the partially written values.
        if let Err(error) = status {
            if error.is_unported() {
                return Err(error);
            }
            for (i, row) in self.pvv.iter_mut().enumerate() {
                for (j, slot) in row.iter_mut().enumerate() {
                    if i + j <= 2 {
                        if let Some(value) = derivs.get(i).and_then(|r| r.get(j)) {
                            *slot = *value;
                        }
                    }
                }
            }
            self.fails_eval = true;
            return Err(SolidError::sm_err(OP_EVAL));
        }
        for (i, row) in self.pvv.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                if i + j <= 2 {
                    *slot = derivs
                        .get(i)
                        .and_then(|r| r.get(j))
                        .copied()
                        .ok_or(SolidError::invalid_input(OP_EVAL, "short evaluation"))?;
                }
            }
        }

        self.uv_set = true;
        Ok(())
    }
}

/// Preserve partial B-spline output through virtual surface wrappers. The walk is
/// bounded to 64 levels (a loop; construction already caps nesting at
/// [`MAX_GEOMETRY_NESTING`]).
fn eval_second_derivatives_with_output(
    k: &Kernel,
    mut surface: &Surface,
    uv: Point2,
    flags: EvaluateFlags,
) -> (Result<()>, Vec<Vec<Vec3>>) {
    let mut step = false;
    for _ in 0..64 {
        // (the analytic kinds evaluate on the m_pNurb upstream's always hold)
        let kind = match surface.kind_with_nurb(k) {
            Ok(kind) => kind,
            Err(error) => return (Err(error), Vec::new()),
        };
        match kind {
            SurfaceKind::BSpline(s) => {
                let flags = if step {
                    EvaluateFlags {
                        non_zero_tangents: true,
                        ..flags
                    }
                } else {
                    flags
                };
                return s.evaluate_with_partial_output(uv, flags);
            }
            SurfaceKind::Cone(cone) => {
                let result = if step {
                    surface.evaluate_step(k, uv, 2, 2, flags)
                } else {
                    surface.evaluate(k, uv, 2, 2, flags)
                };
                return match result {
                    Ok(values) => (Ok(()), values),
                    Err(error) => {
                        // SmCone::Evaluate leaves the inherited NURB derivatives in
                        // the caller's buffer when zero-tangent correction fails.
                        let values = if !step
                            && matches!(&error, SolidError::Upstream { code, .. } if *code == SM_ERR)
                        {
                            cone.rev().base().evaluate_with_partial_output(uv, flags).1
                        } else {
                            Vec::new()
                        };
                        (Err(error), values)
                    }
                };
            }
            SurfaceKind::SurfOfRevolution(s) if !step => {
                return s.base().evaluate_with_partial_output(uv, flags);
            }
            SurfaceKind::SurfOfExtrusion(_) if !step => {
                return match surface.extrusion_base_with_nurb(k) {
                    Ok(base) => base.evaluate_with_partial_output(uv, flags),
                    Err(error) => (Err(error), Vec::new()),
                };
            }
            SurfaceKind::CurveBounded(s) => surface = s.basis(),
            SurfaceKind::Step(s) => {
                let Some(base) = s.surface() else {
                    return (Err(SolidError::sm_err(OP_EVAL)), Vec::new());
                };
                surface = base;
                step = true;
            }
            _ => {
                let result = if step {
                    surface.evaluate_step(k, uv, 2, 2, flags)
                } else {
                    surface.evaluate(k, uv, 2, 2, flags)
                };
                return match result {
                    Ok(values) => (Ok(()), values),
                    Err(error) => (Err(error), Vec::new()),
                };
            }
        }
    }
    (
        Err(SolidError::invalid_input(
            OP_EVAL,
            "surface evaluation nesting exceeds 64",
        )),
        Vec::new(),
    )
}

// ---- SmDerivSurfDefinition ----------------------------------------------------------------

/// How the take-off direction of a derivative surface is chosen
/// (`SmTangencyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TangencyType {
    /// `SM_TT_NO_TANGENCY` (surface boundary).
    NoTangency = 0,
    /// `SM_TT_TAN_TO_EDGES` (surface boundary).
    TanToEdges = 1,
    /// `SM_TT_MATCH_NEIGHBORS` (surface boundary).
    MatchNeighbors = 2,
    /// `SM_TT_DIR_TO_REFERENCE_CURVE` (surface boundary).
    DirToReferenceCurve = 3,
    /// `SM_TT_G1_FROM_CURVE` (base curve).
    G1FromCurve = 4,
    /// `SM_TT_ALONG_VECTOR` (base curve).
    AlongVector = 5,
    /// `SM_TT_PERPENDICULAR_TO_VECTOR` (base curve).
    PerpendicularToVector = 6,
    /// `SM_TT_PARALLEL_TO_CURVATURE` (base curve).
    ParallelToCurvature = 7,
    /// `SM_TT_PERPENDICULAR_TO_CURVATURE` (base curve).
    PerpendicularToCurvature = 8,
}

impl TangencyType {
    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::NoTangency,
            1 => Self::TanToEdges,
            2 => Self::MatchNeighbors,
            3 => Self::DirToReferenceCurve,
            4 => Self::G1FromCurve,
            5 => Self::AlongVector,
            6 => Self::PerpendicularToVector,
            7 => Self::ParallelToCurvature,
            8 => Self::PerpendicularToCurvature,
            _ => return None,
        })
    }
}

/// The continuity a derivative or blend surface is built for (`SmCurvatureType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurvatureType {
    /// `SM_CT_NO_CURVATURE`: ruled surface.
    NoCurvature = 0,
    /// `SM_CT_G2_FROM_SURFACE`.
    G2FromSurface = 1,
    /// `SM_CT_G3_FROM_SURFACE`.
    G3FromSurface = 2,
    /// `SM_CT_G2_FROM_CURVE`.
    G2FromCurve = 3,
    /// `SM_CT_FROM_PARAMETERS`.
    FromParameters = 4,
}

impl CurvatureType {
    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::NoCurvature,
            1 => Self::G2FromSurface,
            2 => Self::G3FromSurface,
            3 => Self::G2FromCurve,
            4 => Self::FromParameters,
            _ => return None,
        })
    }
}

/// The connection of a blend surface to the surfaces it blends
/// (`SmBlendEndType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendEndType {
    /// `SM_BE_NO_CUSP`: smooth at both edges.
    NoCusp = 0,
    /// `SM_BE_1ST_CUSP`: cusped at the first edge.
    FirstCusp = 1,
    /// `SM_BE_2ND_CUSP`: cusped at the second edge.
    SecondCusp = 2,
    /// `SM_BE_BOTH_CUSP`: cusped at both edges.
    BothCusp = 3,
}

impl BlendEndType {
    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::NoCusp,
            1 => Self::FirstCusp,
            2 => Self::SecondCusp,
            3 => Self::BothCusp,
            _ => return None,
        })
    }
}

/// The options controlling how derivative surfaces are built
/// (`SmDerivSurfDefinition`; see `SmSurface.h` for their meaning).
#[derive(Debug, Clone, Copy)]
pub struct DerivSurfDefinition<'c> {
    /// `m_lNumSamples`.
    pub num_samples: u64,
    /// `m_eTangencyType`.
    pub tangency_type: TangencyType,
    /// `m_dStartDerivDirAngleDeg`.
    pub start_deriv_dir_angle_deg: f64,
    /// `m_dEndDerivDirAngleDeg`.
    pub end_deriv_dir_angle_deg: f64,
    /// `m_pTangentReference` (not owned).
    pub tangent_reference: Option<&'c Curve>,
    /// `m_dStartTanLength`.
    pub start_tan_length: f64,
    /// `m_dEndTanScale`.
    pub end_tan_scale: f64,
    /// `m_dStartAngleDeg`.
    pub start_angle_deg: f64,
    /// `m_dEndAngleDeg`.
    pub end_angle_deg: f64,
    /// `m_vVector`.
    pub vector: Vec3,
    /// `m_eCurvatureType`.
    pub curvature_type: CurvatureType,
    /// `m_eBlendEndType`.
    pub blend_end_type: BlendEndType,
    /// `m_dStartRadius`.
    pub start_radius: f64,
    /// `m_dEndRadScale`.
    pub end_rad_scale: f64,
    /// `m_pCurvatureReference` (not owned).
    pub curvature_reference: Option<&'c Curve>,
    /// `m_bFlipCurvature`.
    pub flip_curvature: bool,
}

impl Default for DerivSurfDefinition<'_> {
    /// `SmDerivSurfDefinition()`: 100 samples, matching neighbours, G2 from the
    /// surface, no cusps, unit start tangent length and end tangent scale, zero
    /// angles, radii and vector, no reference curves.
    fn default() -> Self {
        Self {
            num_samples: 100,
            tangency_type: TangencyType::MatchNeighbors, // preserve previous behavior
            start_deriv_dir_angle_deg: 0.0,
            end_deriv_dir_angle_deg: 0.0,
            tangent_reference: None,
            start_tan_length: 1.0,
            end_tan_scale: 1.0,
            start_angle_deg: 0.0,
            end_angle_deg: 0.0,
            vector: Vec3::new(0.0, 0.0, 0.0),
            curvature_type: CurvatureType::G2FromSurface,
            blend_end_type: BlendEndType::NoCusp,
            start_radius: 0.0,
            end_rad_scale: 0.0,
            curvature_reference: None,
            flip_curvature: false,
        }
    }
}

/// The sample values the cross sections of [`DerivSurfDefinition::create_cross_section_curves`]
/// interpolate: start (and optionally end) positions and derivatives, one entry per
/// cross section.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrossSectionValues<'a> {
    /// `rStartPos`.
    pub start_pos: &'a [Vec3],
    /// `rStart1stDerivs`.
    pub start_1st_derivs: &'a [Vec3],
    /// `pOptStart2ndDerivs`.
    pub start_2nd_derivs: Option<&'a [Vec3]>,
    /// `pOptStart3rdDerivs`.
    pub start_3rd_derivs: Option<&'a [Vec3]>,
    /// `pOptEndPos`.
    pub end_pos: Option<&'a [Vec3]>,
    /// `pOptEnd1stDerivs`.
    pub end_1st_derivs: Option<&'a [Vec3]>,
    /// `pOptEnd2ndDerivs`.
    pub end_2nd_derivs: Option<&'a [Vec3]>,
    /// `pOptEnd3rdDerivs`.
    pub end_3rd_derivs: Option<&'a [Vec3]>,
}

/// `SM_IS_CONTAINED(c, a, b)`: `c` inside the closed interval between `a` and `b`
/// grown by their scaled zero.
fn is_contained(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    (a <= (c + z) && (c - z) <= b) || (b <= (c + z) && (c - z) <= a)
}

/// `smos_Max` of two doubles.
pub(crate) fn smos_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// The `SM_ASSERT_BOOLEAN_REPORT` / `SM_ASSERT_VALUE_REPORT` macros of
/// `SmDerivSurfDefinition::AssertValid` (release build: the predicate always runs):
/// `true` when `passed`; otherwise, with a list, a report for test `test_index`
/// (tolerance `tol`, value `val`) is added unless an equal one is there.
fn deriv_report(
    list: &mut Option<&mut AssertArray>,
    key: ObjectKey,
    test_index: u64,
    passed: bool,
    tol: f64,
    val: f64,
) -> Result<bool> {
    if passed {
        if let Some(l) = list.as_deref_mut() {
            l.set_last_add(false);
        }
        return Ok(true);
    }
    if let Some(l) = list.as_deref_mut() {
        let r = AssertReport::new(ReportSpec {
            reporter: Some(key),
            test_index,
            test_class: DERIV_SURF_DEFINITION_TYPE,
            owner: Some(key),
            owner_type: DERIV_SURF_DEFINITION_TYPE,
            owner_type_string: Some(DerivSurfDefinition::CLASS_STRING),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: tol3d(tol),
            value: val,
            list_index: 0,
            message: None,
        })?;
        l.add_unique_delete_duplicate(r);
    }
    Ok(false)
}

impl DerivSurfDefinition<'_> {
    /// `GetClassString()`.
    pub const CLASS_STRING: &'static str = "SmDerivSurfDefinition";
    /// `GetTypeString()`.
    pub const TYPE_STRING: &'static str = "SmDerivSurfDefinition_TYPE";
    /// `SmDerivSurfDefinition_TYPE`.
    pub const TYPE: i64 = DERIV_SURF_DEFINITION_TYPE;

    /// The class name (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        Self::CLASS_STRING
    }

    /// The class type (`GetClassType`).
    #[must_use]
    pub const fn class_type(&self) -> i64 {
        Self::TYPE
    }

    /// Whether the surface is built from a surface boundary curve
    /// (`IsFromSurfaceBoundaryCurve`).
    #[must_use]
    pub fn is_from_surface_boundary_curve(&self) -> bool {
        self.tangency_type == TangencyType::NoTangency
            || self.tangency_type == TangencyType::TanToEdges
            || self.tangency_type == TangencyType::MatchNeighbors
            || self.tangency_type == TangencyType::DirToReferenceCurve
    }

    /// The number of cross derivatives the curvature type needs (`GetNumDerivs`).
    #[must_use]
    pub const fn num_derivs(&self) -> usize {
        match self.curvature_type {
            CurvatureType::NoCurvature => 1,
            CurvatureType::G3FromSurface => 3,
            CurvatureType::G2FromSurface
            | CurvatureType::G2FromCurve
            | CurvatureType::FromParameters => 2,
        }
    }

    /// Whether the stored options are consistent (`AssertValid`); failed checks are
    /// reported to `list` (as object `key`).
    ///
    /// # Errors
    ///
    /// Those of [`AssertReport::new`].
    pub fn assert_valid(&self, list: Option<&mut AssertArray>, key: ObjectKey) -> Result<bool> {
        type T = TangencyType;
        type C = CurvatureType;
        let mut list = list;
        let tt = self.tangency_type;
        let ct = self.curvature_type;

        // init return
        let mut rtn = true;

        // Building from a basecurve or a surface boundary curve
        let base_curve = tt == T::G1FromCurve
            || tt == T::AlongVector
            || tt == T::PerpendicularToVector
            || tt == T::ParallelToCurvature
            || tt == T::PerpendicularToCurvature;

        // enough sample points
        rtn &= deriv_report(
            &mut list,
            key,
            0,
            self.num_samples >= 1,
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;

        // m_dStartDerivDirAngleDeg, m_dEndDerivDirAngleDeg limited to +/-360 degrees
        let start = self.start_deriv_dir_angle_deg;
        let end = self.end_deriv_dir_angle_deg;
        let ok = (tt != T::TanToEdges && tt != T::MatchNeighbors)
            || (is_contained(start, -360.0, 360.0) && is_contained(end, -360.0, 360.0));
        let val = if start < -360.0 || start > 360.0 {
            start
        } else {
            end
        };
        rtn &= deriv_report(&mut list, key, 1, ok, EFF_ZERO, val)?;

        // CrossTangent reference
        let has_ref = self.tangent_reference.is_some();
        #[allow(clippy::nonminimal_bool)] // upstream's predicate, kept as written
        let ok = (base_curve && tt == T::G1FromCurve && has_ref)
            || (!base_curve && tt == T::DirToReferenceCurve && has_ref)
            || !has_ref;
        rtn &= deriv_report(&mut list, key, 2, ok, UNDEF_DOUBLE, UNDEF_DOUBLE)?;

        // CrossTangent length interpolation
        let ok = self.start_tan_length >= 0.0 && self.end_tan_scale >= 0.0;
        let val = smos_max(self.start_tan_length, self.end_tan_scale);
        rtn &= deriv_report(&mut list, key, 3, ok, 0.0, val)?;

        // Rotate angles about basecurve limited to +/-360 degrees
        let start = self.start_angle_deg;
        let end = self.end_angle_deg;
        let ok = is_contained(start, -360.0, 360.0) && is_contained(end, -360.0, 360.0);
        let val = if start < -360.0 || start > 360.0 {
            start
        } else {
            end
        };
        rtn &= deriv_report(&mut list, key, 4, ok, EFF_ZERO, val)?;

        // m_vVector value
        let ok = (tt != T::AlongVector && tt != T::PerpendicularToVector)
            || self.vector.is_initialized();
        rtn &= deriv_report(&mut list, key, 5, ok, UNDEF_DOUBLE, UNDEF_DOUBLE)?;

        // curvature type
        let ok = (base_curve && (ct == C::G2FromCurve || ct == C::FromParameters))
            || (!base_curve && (ct == C::G2FromSurface || ct == C::G3FromSurface))
            || ct == C::NoCurvature;
        rtn &= deriv_report(&mut list, key, 6, ok, UNDEF_DOUBLE, UNDEF_DOUBLE)?;

        // Dvv lengths
        let ok = self.start_radius >= 0.0 && self.end_rad_scale >= 0.0;
        let val = smos_max(self.start_radius, self.end_rad_scale);
        rtn &= deriv_report(&mut list, key, 7, ok, 0.0, val)?;

        let ok = ct != C::G2FromCurve || self.curvature_reference.is_some();
        rtn &= deriv_report(&mut list, key, 8, ok, UNDEF_DOUBLE, UNDEF_DOUBLE)?;

        // all done
        Ok(rtn)
    }

    /// One cross-section curve per sample (`CreateCrossSectionCurves`): a uniform
    /// interpolating curve of degree `2 * derivatives + 1` on `[0, 1]` with the given
    /// start position and derivatives and the given end values or, without end
    /// values, the values at `s = 1` of the start Taylor series.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for arrays of different sizes, a derivative without the lower ones,
    /// end values not matching the start values' orders, or end derivatives without
    /// end positions (and vice versa); those of
    /// [`BSplineCurve::create_interpolating_curve`].
    pub fn create_cross_section_curves(
        &self,
        values: &CrossSectionValues<'_>,
    ) -> Result<Vec<BSplineCurve>> {
        let err = || SolidError::sm_err(OP_DERIV);
        let n = values.start_pos.len();
        let size_ok = |a: Option<&[Vec3]>| a.is_none_or(|a| a.len() == n);
        let start2 = values.start_2nd_derivs;
        let start3 = values.start_3rd_derivs;
        let end_pos = values.end_pos;
        let end1 = values.end_1st_derivs;
        let end2 = values.end_2nd_derivs;
        let end3 = values.end_3rd_derivs;

        // size constraint
        if values.start_1st_derivs.len() != n
            || !size_ok(start2)
            || !size_ok(start3)
            || !size_ok(end_pos)
            || !size_ok(end1)
            || !size_ok(end2)
            || !size_ok(end3)
        {
            return Err(err());
        }

        // lesser derivative constraint
        if (start3.is_some() && start2.is_none())
            || (end3.is_some() && end2.is_none())
            || (end2.is_some() && end1.is_none())
            || (end1.is_some() && end_pos.is_none())
        {
            return Err(err());
        }

        // just start or matching end and start value constraints
        if end3.is_some() != start3.is_some() || end2.is_some() != start2.is_some() {
            return Err(err());
        }

        // start pos and 1st deriv are always required - just require end pos when end 1st derivs are given
        if end1.is_some() != end_pos.is_some() {
            return Err(err());
        }

        // pick crossSection degree high enough to interpolate all the given end values
        let num_derivatives: usize = if start3.is_some() {
            3
        } else if start2.is_some() {
            2
        } else {
            1
        };
        let degree = num_derivatives * 2 + 1;

        // CreateInterpolatingCurve() locals
        let mut tgt_pnts = [Vec3::UNDEF; 2];
        let mut tgt_1sts = [Vec3::UNDEF; 2];
        let mut tgt_higher = vec![Vec3::UNDEF; if num_derivatives == 3 { 4 } else { 2 }];
        let mut c = Vec3::new(0.0, 0.0, 0.0);
        let mut d = Vec3::new(0.0, 0.0, 0.0);
        let at = |a: &[Vec3], i: usize| a.get(i).copied().ok_or_else(err);
        let set = |a: &mut [Vec3], i: usize, v: Vec3| -> Result<()> {
            *a.get_mut(i).ok_or_else(err)? = v;
            Ok(())
        };

        let mut cross_sections = Vec::with_capacity(n);
        // for every sample input
        for ii in 0..n {
            let p0 = at(values.start_pos, ii)?;
            let d0 = at(values.start_1st_derivs, ii)?;
            // shuffle start input for CreateInterpolatingCurve() call
            tgt_pnts[0] = p0;
            tgt_1sts[0] = d0;
            if let Some(s2) = start2 {
                set(&mut tgt_higher, 0, at(s2, ii)?)?;
            }
            if let Some(s3) = start3 {
                set(&mut tgt_higher, 2, at(s3, ii)?)?;
            }

            if let (Some(ep), Some(e1)) = (end_pos, end1) {
                // given end values
                tgt_pnts[1] = at(ep, ii)?;
                tgt_1sts[1] = at(e1, ii)?;
                if let Some(e2) = end2 {
                    set(&mut tgt_higher, 1, at(e2, ii)?)?;
                }
                if let Some(e3) = end3 {
                    set(&mut tgt_higher, 3, at(e3, ii)?)?;
                }
            } else {
                // Compute end points using 1st, 2nd, or 3rd degree taylor series approximation
                // get high order taylor series coefficients
                if let Some(s2) = start2 {
                    c = at(s2, ii)?;
                }
                if let (Some(s2), Some(_)) = (start2, start3) {
                    // upstream reads the 2nd derivatives here
                    d = at(s2, ii)?;
                }

                // Compute CrossSection(s) for s = 1
                tgt_pnts[1] = p0 + d0 + c / 2.0 + d / 6.0;
                tgt_1sts[1] = d0 + c + d / 2.0;
                if start2.is_some() {
                    set(&mut tgt_higher, 1, c + d)?;
                }
                if start3.is_some() {
                    set(&mut tgt_higher, 3, d)?;
                }
            }

            // build an interpolating curve that equals target values at Curve(0) evaluations
            let curve = BSplineCurve::create_interpolating_curve(
                ParamMethod::Uniform,
                3,
                degree,
                &tgt_pnts,
                &tgt_1sts,
                if num_derivatives >= 2 {
                    Some(&tgt_higher)
                } else {
                    None
                },
                false,
            )?;
            cross_sections.push(curve);
        }

        Ok(cross_sections)
    }

    /// The derivative surface skinned through `cross_sections` along the rails
    /// (`SkinDerivSurface`): synchronised `u` cross sections at rail parameters
    /// `params`, degree in the sweep direction that of the rails (the larger of the
    /// two), interpolating when `tolerance` is `0`.
    ///
    /// # Errors
    ///
    /// Those of the rail degrees and of [`BSplineSurface::create_skinned_surface`].
    pub fn skin_deriv_surface(
        &self,
        k: &Kernel,
        cross_sections: &[BSplineCurve],
        params: &[f64],
        start_rail: &mut BSplineCurve,
        end_rail: Option<&mut BSplineCurve>,
        tolerance: f64,
    ) -> Result<BSplineSurface> {
        // CreateSkinnedSurface locals
        let degree = match end_rail.as_deref() {
            Some(e) => {
                let a = start_rail.degree()?;
                let b = e.degree()?;
                if a > b {
                    a
                } else {
                    b
                }
            }
            None => start_rail.degree()?,
        };

        // sweep the set of sample curves to make the derivative surface
        BSplineSurface::create_skinned_surface(
            k,
            cross_sections,
            SkinnedSurfaceOptions {
                synchronized: true,
                cross_section_surf_param: SurfParamType::U,
                approx_tol: tolerance,
                rail1: Some(start_rail),
                rail2: end_rail,
                rail1_used_as_spine_curve: false,
                intersections_with_rails: Some(params),
                deriv_surfs: Some([None, None]),
                degree,
            },
        )
    }
}

// ---- SmDropCurveFail ----------------------------------------------------------------------

/// `UNSURE` (`SmTypes.h`): the third value of an `SmBoolean`.
pub const UNSURE: i32 = 2;

/// `SM_ERR_UNKNOWN` (`SmMessages.h`): the status of an unused failure record.
pub const SM_ERR_UNKNOWN: i32 = 1003;

/// `SM_SS_NONE` (`SmTypes.h`): no singular sides.
pub const SS_NONE: u64 = 0;

/// The failure and recovery paths of `CreateUVTrimCurve`, `DropCurve` and
/// `DropAndTrimCurve` (`SmCreateUVTrimCurvePathType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CreateUvTrimCurvePathType {
    /// `SM_CTC_UNKNOWN`.
    #[default]
    Unknown = 0,
    /// `SM_CTC_OKAY`.
    Okay,
    /// `SM_CTC_FROM_CRVONSURF`.
    FromCrvOnSurf,
    /// `SM_CTC_REVERSEDROP_RECOVERY`.
    ReverseDropRecovery,
    /// `SM_CTC_TWOSINGULARENDPTS_RECOVERY`.
    TwoSingularEndPtsRecovery,
    /// `SM_CTC_SHORTPIECE_SEAMCROSS_RECOVERY`.
    ShortPieceSeamCrossRecovery,
    /// `SM_CTC_PICKONESEAM_RECOVERY`.
    PickOneSeamRecovery,
    /// `SM_CTC_10PERCENT_SEAMCROSS_RECOVERY`.
    TenPercentSeamCrossRecovery,
    /// `SM_CTC_1PERCENT_UVTRIMCURVETRIM_RECOVERY1`.
    OnePercentUvTrimCurveTrimRecovery1,
    /// `SM_CTC_1PERCENT_UVTRIMCURVETRIM_RECOVERY2`.
    OnePercentUvTrimCurveTrimRecovery2,
    /// `SM_CTC_1PERCENT_EDGETRIM_RECOVERY1`.
    OnePercentEdgeTrimRecovery1,
    /// `SM_CTC_1PERCENT_EDGETRIM_RECOVERY2`.
    OnePercentEdgeTrimRecovery2,
    /// `SM_CTC_1PERCENT_CURVETRIM_RECOVERY`.
    OnePercentCurveTrimRecovery,
    /// `SM_CTC_101POINTDROP_DEGENCURVE_RECOVERY`.
    PointDropDegenCurveRecovery,
    /// `SM_CTC_101POINTDROP_FITCURVE_RECOVERY`.
    PointDropFitCurveRecovery,
    /// `SM_CTC_ALLRECOVERIES_FAIL`.
    AllRecoveriesFail,
    /// `SM_CTC_DROPCURVE_OKAY`.
    DropCurveOkay,
    /// `SM_CTC_DEGENSURFACE_FAIL`.
    DegenSurfaceFail,
    /// `SM_CTC_DROPDOMAIN_UNCONTAINED_FAIL`.
    DropDomainUncontainedFail,
    /// `SM_CTC_FIRSTDROP_UNCONTAINED_FAIL`.
    FirstDropUncontainedFail,
    /// `SM_CTC_BAD_FIND_DEGENPARAM_WARNING`.
    BadFindDegenParamWarning,
    /// `SM_CTC_KEPT_BEYOND_TOL_DROP`.
    KeptBeyondTolDrop,
    /// `SM_CTC_DROPSTARTPOINT_FAIL`.
    DropStartPointFail,
    /// `SM_CTC_SPLIT_DROPSTEP_FAIL`.
    SplitDropStepFail,
    /// `SM_CTC_DROPANDTRIMCURVE_OKAY`.
    DropAndTrimCurveOkay,
}

impl CreateUvTrimCurvePathType {
    /// Every enumerator, in upstream order (the index is the upstream value).
    pub const ALL: [Self; 25] = [
        Self::Unknown,
        Self::Okay,
        Self::FromCrvOnSurf,
        Self::ReverseDropRecovery,
        Self::TwoSingularEndPtsRecovery,
        Self::ShortPieceSeamCrossRecovery,
        Self::PickOneSeamRecovery,
        Self::TenPercentSeamCrossRecovery,
        Self::OnePercentUvTrimCurveTrimRecovery1,
        Self::OnePercentUvTrimCurveTrimRecovery2,
        Self::OnePercentEdgeTrimRecovery1,
        Self::OnePercentEdgeTrimRecovery2,
        Self::OnePercentCurveTrimRecovery,
        Self::PointDropDegenCurveRecovery,
        Self::PointDropFitCurveRecovery,
        Self::AllRecoveriesFail,
        Self::DropCurveOkay,
        Self::DegenSurfaceFail,
        Self::DropDomainUncontainedFail,
        Self::FirstDropUncontainedFail,
        Self::BadFindDegenParamWarning,
        Self::KeptBeyondTolDrop,
        Self::DropStartPointFail,
        Self::SplitDropStepFail,
        Self::DropAndTrimCurveOkay,
    ];

    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        Self::ALL.get(usize::try_from(code).ok()?).copied()
    }
}

/// What a failed `DropCurve` had computed (`SmDropCurveFail`), for
/// `SmEdgeuse::CreateUVTrimCurve`'s recoveries. `O` names the objects upstream
/// keeps pointers to (they are never dereferenced).
#[derive(Debug, Clone)]
pub struct DropCurveFail<O> {
    /// `m_sFailStatus`: the failing status ([`SM_ERR_UNKNOWN`] when unused).
    pub fail_status: i32,
    /// `m_lFromCall`: 0 unused, 1 `DropCurve`, 2 `DropAndTrimCurve`, 3
    /// `CreateUVTrimCurve`.
    pub from_call: u64,
    /// `m_eCreatePathType`.
    pub create_path_type: CreateUvTrimCurvePathType,
    /// `m_cpContext != NULL`.
    pub has_context: bool,
    /// `m_sUVDomain`.
    pub uv_domain: Extent2d,
    /// `m_cp3dSurface`.
    pub surface: Option<O>,
    /// `m_cp3dCurve`.
    pub curve: Option<O>,
    /// `m_sInterval`.
    pub interval: Extent1d,
    /// `m_sApproxTol3d`.
    pub approx_tol_3d: f64,
    /// `m_dMaxDropToSurfArg`.
    pub max_drop_to_surf_arg: f64,
    /// `m_dMaxApproxDev`.
    pub max_approx_dev: f64,
    /// `m_pUVCurvesArg`.
    pub uv_curves_arg: Option<O>,
    /// `m_bKeepAllDropCurves` (`SmBoolean`: 0, 1 or [`UNSURE`]).
    pub keep_all_drop_curves: i32,
    /// `m_bDegenCurve`.
    pub degen_curve: i32,
    /// `m_bDegenSurfU`.
    pub degen_surf_u: i32,
    /// `m_bDegenSurfV`.
    pub degen_surf_v: i32,
    /// `m_bDropDomainIsContained`.
    pub drop_domain_is_contained: i32,
    /// `m_dMaxDropToSurf`.
    pub max_drop_to_surf: f64,
    /// `m_sSolutions`.
    pub solutions: SolutionArray<O>,
    /// `m_sUVCurves`: copies of the curves made before the failure.
    pub uv_curves: Vec<BSplineCurve>,
    /// `m_bClosedU`.
    pub closed_u: i32,
    /// `m_bClosedV`.
    pub closed_v: i32,
    /// `m_eSrfClosure`.
    pub srf_closure: SurfParamType,
    /// `m_sSrfPolePoints`.
    pub srf_pole_points: Vec<Point3>,
    /// `m_sSrfPoleNormals`.
    pub srf_pole_normals: Vec<Vec3>,
    /// `m_lSingularities`.
    pub singularities: u64,
    /// `m_lApproxSingularities`.
    pub approx_singularities: u64,
    /// `m_sCtrlPoly`.
    pub ctrl_poly: Vec<Point3>,
    /// `m_sKnots`.
    pub knots: Vec<f64>,
    /// `m_pBrep` (kept by [`DropCurveFail::un_init`] and [`DropCurveFail::set`]).
    pub brep: Option<O>,
}

/// The arguments of [`DropCurveFail::set`] (`SmDropCurveFail::Set`); the defaults
/// are upstream's for the optional ones.
#[derive(Debug, Clone)]
pub struct DropCurveFailArgs<'a, O> {
    /// arg 1, `sFailStatus`.
    pub fail_status: i32,
    /// arg 2, `lFromCall`.
    pub from_call: u64,
    /// arg 3, `eCreatePathType`.
    pub create_path_type: CreateUvTrimCurvePathType,
    /// arg 4, `cpContext != NULL`.
    pub has_context: bool,
    /// arg 5, `sUVDomain`.
    pub uv_domain: Extent2d,
    /// arg 6, `cp3dSurface`.
    pub surface: Option<O>,
    /// arg 7, `cp3dCurve`.
    pub curve: Option<O>,
    /// arg 8, `sInterval`.
    pub interval: Extent1d,
    /// arg 9, `sApproxTol3d`.
    pub approx_tol_3d: f64,
    /// arg 10, `dMaxDropToSurfArg`.
    pub max_drop_to_surf_arg: f64,
    /// arg 11, `dMaxApproxDev`.
    pub max_approx_dev: f64,
    /// arg 12, `pUVCurvesArg`.
    pub uv_curves_arg: Option<O>,
    /// arg 13, `bKeepAllDropCurves`.
    pub keep_all_drop_curves: i32,
    /// arg 14, `bDegenCurve`.
    pub degen_curve: i32,
    /// arg 15, `bDegenSurfU`.
    pub degen_surf_u: i32,
    /// arg 16, `bDegenSurfV`.
    pub degen_surf_v: i32,
    /// arg 17, `bDropDomainIsContained`.
    pub drop_domain_is_contained: i32,
    /// arg 18, `dMaxDropToSurf`.
    pub max_drop_to_surf: f64,
    /// arg 19, `bClosedU` (default `UNSURE`).
    pub closed_u: i32,
    /// arg 20, `bClosedV` (default `UNSURE`).
    pub closed_v: i32,
    /// arg 21, `eSrfClosure` (default `SM_SP_UNKNOWN`).
    pub srf_closure: SurfParamType,
    /// arg 22, `pSrfPolePoints` (default `NULL`).
    pub srf_pole_points: Option<&'a [Point3]>,
    /// arg 23, `pSrfPoleNormals` (default `NULL`).
    pub srf_pole_normals: Option<&'a [Vec3]>,
    /// arg 24, `lSingularities` (default `SM_SS_NONE`).
    pub singularities: u64,
    /// arg 25, `lApproxSingularities` (default `SM_SS_NONE`).
    pub approx_singularities: u64,
    /// arg 26, `pSolutions` (default `NULL`).
    pub solutions: Option<&'a SolutionArray<O>>,
    /// arg 27, `pCtrlPoly` (default `NULL`).
    pub ctrl_poly: Option<&'a [Point3]>,
    /// arg 28, `pKnots` (default `NULL`).
    pub knots: Option<&'a [f64]>,
    /// arg 29, `pUVCurves` (default `NULL`): copied only with a context.
    pub uv_curves: Option<&'a [BSplineCurve]>,
}

impl<O> Default for DropCurveFailArgs<'_, O> {
    fn default() -> Self {
        Self {
            fail_status: SM_ERR_UNKNOWN,
            from_call: 0,
            create_path_type: CreateUvTrimCurvePathType::Unknown,
            has_context: false,
            uv_domain: Extent2d::new(),
            surface: None,
            curve: None,
            interval: Extent1d::new(),
            approx_tol_3d: UNDEF_DOUBLE,
            max_drop_to_surf_arg: UNDEF_DOUBLE,
            max_approx_dev: UNDEF_DOUBLE,
            uv_curves_arg: None,
            keep_all_drop_curves: UNSURE,
            degen_curve: UNSURE,
            degen_surf_u: UNSURE,
            degen_surf_v: UNSURE,
            drop_domain_is_contained: UNSURE,
            max_drop_to_surf: -1.0,
            closed_u: UNSURE,
            closed_v: UNSURE,
            srf_closure: SurfParamType::Unknown,
            srf_pole_points: None,
            srf_pole_normals: None,
            singularities: SS_NONE,
            approx_singularities: SS_NONE,
            solutions: None,
            ctrl_poly: None,
            knots: None,
            uv_curves: None,
        }
    }
}

impl<O: SolutionObject> Default for DropCurveFail<O> {
    /// `SmDropCurveFail()`: [`DropCurveFail::un_init`] with no B-rep.
    fn default() -> Self {
        let mut s = Self {
            fail_status: SM_ERR_UNKNOWN,
            from_call: 0,
            create_path_type: CreateUvTrimCurvePathType::Unknown,
            has_context: false,
            uv_domain: Extent2d::new(),
            surface: None,
            curve: None,
            interval: Extent1d::new(),
            approx_tol_3d: UNDEF_DOUBLE,
            max_drop_to_surf_arg: UNDEF_DOUBLE,
            max_approx_dev: UNDEF_DOUBLE,
            uv_curves_arg: None,
            keep_all_drop_curves: 0,
            degen_curve: 0,
            degen_surf_u: 0,
            degen_surf_v: 0,
            drop_domain_is_contained: 0,
            max_drop_to_surf: UNDEF_DOUBLE,
            solutions: SolutionArray::new(),
            uv_curves: Vec::new(),
            closed_u: 0,
            closed_v: 0,
            srf_closure: SurfParamType::Unknown,
            srf_pole_points: Vec::new(),
            srf_pole_normals: Vec::new(),
            singularities: 0,
            approx_singularities: 0,
            ctrl_poly: Vec::new(),
            knots: Vec::new(),
            brep: None,
        };
        s.un_init();
        s
    }
}

impl<O: SolutionObject> DropCurveFail<O> {
    /// Resets to the unused state (`UnInit`); the B-rep is kept.
    pub fn un_init(&mut self) {
        self.fail_status = SM_ERR_UNKNOWN; // mem 1
        self.from_call = 0;
        self.create_path_type = CreateUvTrimCurvePathType::Unknown; // mem 3

        // DropCurve(Input args)
        self.has_context = false; // mem 4
        self.uv_domain.init(); // mem 5
        self.surface = None; // mem 6
        self.curve = None; // mem 7
        self.interval.init(); // mem 8
        self.approx_tol_3d = UNDEF_DOUBLE; // mem 9
        self.max_drop_to_surf_arg = UNDEF_DOUBLE; // mem 10
        self.max_approx_dev = UNDEF_DOUBLE; // mem 11
        self.uv_curves_arg = None; // mem 12
        self.keep_all_drop_curves = UNSURE; // mem 13

        self.degen_curve = UNSURE; // mem 14
        self.degen_surf_u = UNSURE; // mem 15
        self.degen_surf_v = UNSURE; // mem 16
        self.drop_domain_is_contained = UNSURE; // mem 17

        self.max_drop_to_surf = -1.0; // mem 18

        self.solutions.reset(); // mem 19
        self.uv_curves.clear(); // mem 20

        self.closed_u = UNSURE; // mem 21
        self.closed_v = UNSURE; // mem 22
        self.srf_closure = SurfParamType::Unknown; // mem 23
        self.srf_pole_points.clear(); // mem 24
        self.srf_pole_normals.clear(); // mem 25

        self.singularities = SS_NONE; // mem 26
        self.approx_singularities = SS_NONE; // mem 27

        self.ctrl_poly.clear(); // mem 28
        self.knots.clear(); // mem 29
                            // mem 30 - don't clear the Brep pointer
    }

    /// Whether the record is unused (`IsUnInit`).
    #[must_use]
    pub const fn is_un_init(&self) -> bool {
        self.fail_status == SM_ERR_UNKNOWN
    }

    /// Records a failure (`Set`): every member from `args`; the arrays are copied
    /// (the UV curves only with a context); the B-rep is kept.
    pub fn set(&mut self, args: &DropCurveFailArgs<'_, O>) {
        self.fail_status = args.fail_status;
        self.from_call = args.from_call;
        self.create_path_type = args.create_path_type;
        // DropCurve(Input args)
        self.has_context = args.has_context;
        self.uv_domain = args.uv_domain;
        self.surface = args.surface;
        self.curve = args.curve;
        self.interval = args.interval;
        self.approx_tol_3d = args.approx_tol_3d;
        self.max_drop_to_surf_arg = args.max_drop_to_surf_arg;
        self.max_approx_dev = args.max_approx_dev;
        self.uv_curves_arg = args.uv_curves_arg;
        self.keep_all_drop_curves = args.keep_all_drop_curves;

        // DropCurve Degen Geom classification
        self.degen_curve = args.degen_curve;
        self.degen_surf_u = args.degen_surf_u;
        self.degen_surf_v = args.degen_surf_v;

        // DropCurve input Domain check
        self.drop_domain_is_contained = args.drop_domain_is_contained;

        // DropCurve surface classification
        self.max_drop_to_surf = args.max_drop_to_surf;
        self.solutions.reset();
        if let Some(s) = args.solutions {
            self.solutions.append(s);
        }
        self.uv_curves.clear();
        if let Some(curves) = args.uv_curves {
            if args.has_context {
                self.uv_curves.extend(curves.iter().cloned());
            }
        }

        self.closed_u = args.closed_u;
        self.closed_v = args.closed_v;
        self.srf_closure = args.srf_closure;
        self.srf_pole_points.clear();
        if let Some(p) = args.srf_pole_points {
            self.srf_pole_points.extend_from_slice(p);
        }
        self.srf_pole_normals.clear();
        if let Some(n) = args.srf_pole_normals {
            self.srf_pole_normals.extend_from_slice(n);
        }
        self.singularities = args.singularities;
        self.approx_singularities = args.approx_singularities;

        // copy array state
        self.ctrl_poly.clear();
        if let Some(c) = args.ctrl_poly {
            self.ctrl_poly.extend_from_slice(c);
        }
        self.knots.clear();
        if let Some(k) = args.knots {
            self.knots.extend_from_slice(k);
        }
        // mem 30: the Brep must be set explicitly by the caller
    }

    /// `operator=`: a copy of `other`, whose UV curves are copied only when it has
    /// a context.
    pub fn assign(&mut self, other: &Self) {
        let mut copy = other.clone();
        if !other.has_context {
            copy.uv_curves.clear();
        }
        *self = copy;
    }
}

// ---- standalone surface numerics (smsurf_*) ------------------------------------------------

/// The second derivative of a curve on a surface `S(C(s))` by the chain rule
/// (`smsurf_LiftSecondDerivative`): `cs`, `css` are the UV curve's first and second
/// derivatives, the rest the surface's first and second partial derivatives.
#[must_use]
pub fn lift_second_derivative(
    cs: Vec3,
    css: Vec3,
    du: Vec3,
    dv: Vec3,
    duu: Vec3,
    duv: Vec3,
    dvv: Vec3,
) -> Vec3 {
    css.x * du + css.y * dv + cs.x * cs.x * duu + 2.0 * cs.x * cs.y * duv + cs.y * cs.y * dvv
}

/// The third derivative of a curve on a surface `S(C(s))` by the chain rule
/// (`smsurf_LiftThirdDerivative`).
#[must_use]
pub fn lift_third_derivative(
    cs: Vec3,
    css: Vec3,
    csss: Vec3,
    su: Vec3,
    sv: Vec3,
    suu: Vec3,
    suv: Vec3,
    svv: Vec3,
    suuu: Vec3,
    suuv: Vec3,
    suvv: Vec3,
    svvv: Vec3,
) -> Vec3 {
    csss.x * su
        + csss.y * sv
        + 3.0 * cs.x * css.x * suu
        + 3.0 * (css.x * cs.y + css.y * cs.x) * suv
        + 3.0 * cs.y * css.y * svv
        + cs.x * cs.x * cs.x * suuu
        + 3.0 * cs.x * cs.x * cs.y * suuv
        + 3.0 * cs.x * cs.y * cs.y * suvv
        + cs.y * cs.y * cs.y * svvv
}

/// Scales STEP derivatives from per-radian to per-degree in the angular parameters
/// (`smsurf_ScaleSTEPAngularDerivatives`): `derivatives[i * (hv + 1) + j]` (the
/// derivative `i` times in `u`, `j` times in `v`) is multiplied by
/// `(PI/180)^(i [u angular] + j [v angular])`; mixed derivatives with
/// `i + j > min(hu, hv)` are left alone.
///
/// # Errors
///
/// `InvalidInput` when `derivatives` holds fewer than `(hu + 1) * (hv + 1)` values.
pub fn scale_step_angular_derivatives(
    derivatives: &mut [Vec3],
    highest_u_deriv: usize,
    highest_v_deriv: usize,
    u_is_angular: bool,
    v_is_angular: bool,
) -> Result<()> {
    let rad_per_deg = PI / 180.0;
    let u_scale = if u_is_angular { rad_per_deg } else { 1.0 };
    let v_scale = if v_is_angular { rad_per_deg } else { 1.0 };
    let max_mixed_order = highest_u_deriv.min(highest_v_deriv);
    let mut u_factor = 1.0;
    for i in 0..=highest_u_deriv {
        let mut factor = u_factor;
        for j in 0..=highest_v_deriv {
            if !(i != 0 && j != 0 && i + j > max_mixed_order) {
                let d = derivatives.get_mut(i * (highest_v_deriv + 1) + j).ok_or(
                    SolidError::invalid_input(
                        "surface::surface::p2::scale_step_angular_derivatives",
                        "derivative array too short",
                    ),
                )?;
                *d *= factor;
            }
            factor *= v_scale;
        }
        u_factor *= u_scale;
    }
    Ok(())
}

/// The geometric properties of a surface point ([`evaluate_geometric`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometricProperties {
    /// `rdGaussianCurvature`: `K1 * K2`.
    pub gaussian_curvature: f64,
    /// `rdNormalCurvature`: the mean curvature `(K1 + K2) / 2`.
    pub normal_curvature: f64,
    /// `rdPrincipleCurvature1`: `K1`, the larger principal curvature.
    pub principle_curvature1: f64,
    /// `rdPrincipleCurvature2`: `K2`.
    pub principle_curvature2: f64,
    /// `rEFGOfFirstFundForm`: `[E, F, G]` of the first fundamental form.
    pub efg_of_first_fund_form: Vec3,
    /// `rLMNOfSecondFundForm`: `[L, M, N]` of the second fundamental form.
    pub lmn_of_second_fund_form: Vec3,
    /// `rPrincipleCurvatureVector1`: unit tangent in the first principal direction.
    pub principle_curvature_vector1: Vec3,
    /// `rPrincipleCurvatureVector2`: unit tangent in the second principal direction.
    pub principle_curvature_vector2: Vec3,
}

/// Curvatures, fundamental forms and principal directions from an evaluation
/// matrix (`smsurf_EvaluateGeometric`): `eval[0][0]` position, `[1][0]` `Du`, `[0][1]`
/// `Dv`, `[1][1]` `Duv`, `[2][0]` `Duu`, `[0][2]` `Dvv`, and `[2][2]` an optional
/// normal used where `Du x Dv` vanishes. A vanishing first derivative in `eval` is
/// replaced by one rebuilt from the other and the normal (in place, as upstream).
///
/// # Errors
///
/// `SM_ERR` at a singular point (no normal) or where the fundamental forms
/// degenerate.
pub fn evaluate_geometric(eval: &mut [[Vec3; 3]; 3]) -> Result<GeometricProperties> {
    let err = || SolidError::sm_err(OP_GEOM);

    // local parametric surface properties
    let duv = eval[1][1];
    let duu = eval[2][0];
    let dvv = eval[0][2];
    let mut norm = eval[1][0].cross(eval[0][1]);

    // when parametric surface is singular at point
    if norm.length_squared() < EFF_ZERO_SQ {
        // Note that we often store the normal in [2][2]; try that one.
        norm = eval[2][2];
        if norm.length_squared() < EFF_ZERO_SQ {
            return Err(err());
        }
    } // end singular surface point check

    // normalize SurfaceNormal
    norm = norm.unitize()?;

    // If one of the first derivatives is zero - backwards derive what it was after
    // stepoff by crossing with normal.
    if eval[1][0].length_squared() < EFF_ZERO_SQ {
        eval[1][0] = eval[0][1].cross(norm);
    }
    if eval[0][1].length_squared() < EFF_ZERO_SQ {
        eval[0][1] = norm.cross(eval[1][0]);
    }
    let du = eval[1][0];
    let dv = eval[0][1];

    // compute the terms of the 1st and 2nd fundamental forms
    let e = du.dot(du);
    let f = du.dot(dv);
    let g = dv.dot(dv);
    let l = duu.dot(norm);
    let m = duv.dot(norm);
    let n = dvv.dot(norm);

    // get gaussian curvature checking for divide by zero
    let numer1 = l * n - m * m;
    let denom1 = e * g - f * f;
    if denom1.abs() <= EFF_ZERO * numer1.abs() {
        return Err(err());
    }
    let k = numer1 / denom1;

    // get mean curvature checking for divide by zero
    let numer2 = e * n - 2.0 * f * m + g * l;
    let denom2 = 2.0 * (e * g - f * f);
    if denom2.abs() <= EFF_ZERO * numer2.abs() {
        return Err(err());
    }
    let h = numer2 / denom2;

    // compute principal curvature values
    let mut hhk = h * h - k;
    if hhk < EFF_ZERO_SQ {
        // was SM_EFF_ZERO_SQRT, but it's a square
        hhk = 0.0; // [B16, 091015]
    }
    let tmp = sqrt(hhk);
    let k1 = h + tmp; // K1 is the larger (algebraically) principal curvature
    let k2 = h - tmp;

    let mut out = GeometricProperties {
        gaussian_curvature: k,
        normal_curvature: h,
        principle_curvature1: k1,
        principle_curvature2: k2,
        efg_of_first_fund_form: Vec3::new(e, f, g),
        lmn_of_second_fund_form: Vec3::new(l, m, n),
        principle_curvature_vector1: Vec3::UNDEF,
        principle_curvature_vector2: Vec3::UNDEF,
    };

    // Note that if the principle curvatures are equal then we have an umbilical point
    // or a planar point.
    let scaled_zero = EFF_ZERO_SQRT * (1.0 + k1.abs() + k2.abs());
    if (k1 - k2).abs() < scaled_zero {
        // For umbilical take any two orthoginal vectors.
        let v1 = du.unitize()?;
        let v2 = v1.cross(dv).cross(v1).unitize()?; // orthoganilize
        out.principle_curvature_vector1 = v1;
        out.principle_curvature_vector2 = v2;
        return Ok(out);
    }

    // Solve for V = v1*du + v2*dv holding v1 = 1
    let coeff = [e * m - f * l, e * n - g * l, f * n - g * m];
    let ans = solve_quadratic(&coeff, EFF_ZERO);
    let (mut v1, mut v2);
    if ans.is_empty() {
        // Have some sort of bad matrix values. Just take any two perpendicular vectors
        // in the tangent plane. In case one was (near) zero, use the larger:
        if du.max_dimension() > dv.max_dimension() {
            v1 = du;
            v2 = norm.cross(v1);
        } else {
            v1 = dv;
            v2 = v1.cross(norm);
        }
    } else {
        let a0 = ans.first().copied().ok_or_else(err)?;
        v1 = du + a0 * dv;
        if v1.length_squared() < EFF_ZERO_SQ {
            v1 = du;
        }
        v2 = norm.cross(v1);
    }

    // Pair each direction with its curvature by measuring the normal curvature along
    // v1 directly.
    {
        let w = v1;
        let wu = w.dot(du);
        let wv = w.dot(dv);
        let a = (g * wu - f * wv) / denom1; // dDenom1 = E*G - F*F, checked non-zero above
        let b = (-f * wu + e * wv) / denom1;
        let i = e * a * a + 2.0 * f * a * b + g * b * b;
        if i > EFF_ZERO_SQ {
            let kn1 = (l * a * a + 2.0 * m * a * b + n * b * b) / i;
            if (kn1 - k2).abs() < (kn1 - k1).abs() {
                core::mem::swap(&mut v1, &mut v2);
            }
        }
    }

    if v1.length_squared() > EFF_ZERO_SQ {
        v1 = v1.unitize()?;
    }
    if v2.length_squared() > EFF_ZERO_SQ {
        v2 = v2.unitize()?;
    }

    let norm_tmp = v1.cross(v2);
    if norm_tmp.dot(norm) < 0.0 {
        v2 = -v2;
    }

    out.principle_curvature_vector1 = v1;
    out.principle_curvature_vector2 = v2;
    Ok(out)
}

/// The shape of a surface at a point (`SmLocalSurfaceType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSurfaceType {
    /// `SM_LS_PLANAR`: both iso-parameter curves are planar.
    Planar = 0,
    /// `SM_LS_ELLIPTICAL`: both bend the same way (a bowl or hill).
    Elliptical = 1,
    /// `SM_LS_HYPERBOLIC`: they bend in different directions (a saddle).
    Hyperbolic = 2,
    /// `SM_LS_CYLINDRICAL`: only one of them is planar.
    Cylindrical = 3,
}

/// The side of the tangent plane an iso-parameter curve bends to
/// (`SmSurfaceSideType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceSideType {
    /// `SM_SS_ON`: in the plane perpendicular to the normal.
    On = 0,
    /// `SM_SS_ABOVE`: bends towards the normal.
    Above = 1,
    /// `SM_SS_BELOW`: bends away from the normal.
    Below = 2,
}

impl SurfaceSideType {
    /// The enumerator of an upstream value, `None` outside the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::On,
            1 => Self::Above,
            2 => Self::Below,
            _ => return None,
        })
    }
}

/// Classifies a surface point from its normal and second derivatives
/// (`smsurf_ComputeSurfacePointType`). The `u` classification is written to
/// `side_uu`; `want_side_vv` (`pOptSurfSideDVV !=
/// NULL`) only redirects the `v` side there, the `v` output is never written. (Upstream
/// crashes when the `v` side is requested without the `u` side; nothing is written
/// then.)
#[must_use]
pub fn compute_surface_point_type(
    normal: Vec3,
    duu: Vec3,
    dvv: Vec3,
    side_uu: Option<&mut SurfaceSideType>,
    want_side_vv: bool,
) -> LocalSurfaceType {
    // let tol = 10 * EFF_ZERO * (1+vector lengths)
    let tol = EFF_ZERO_SQRT * 10.0 * (1.0 + normal.length() + duu.length() + dvv.length());

    let dot_uu = normal.dot(duu);
    let dot_vv = normal.dot(dvv);

    let classify = |d: f64| {
        if d.abs() < tol {
            SurfaceSideType::On
        } else if d >= tol {
            SurfaceSideType::Above
        } else {
            SurfaceSideType::Below
        }
    };
    let surf_side_duu = classify(dot_uu);
    let surf_side_dvv = classify(dot_vv);

    // set outputs
    let mut side_uu = side_uu;
    if let Some(s) = side_uu.as_deref_mut() {
        *s = surf_side_duu;
    }
    if want_side_vv {
        if let Some(s) = side_uu {
            *s = surf_side_dvv;
        }
    }

    if surf_side_duu == SurfaceSideType::On && surf_side_dvv == SurfaceSideType::On {
        LocalSurfaceType::Planar
    } else if surf_side_duu == SurfaceSideType::On || surf_side_dvv == SurfaceSideType::On {
        LocalSurfaceType::Cylindrical
    } else if surf_side_duu == surf_side_dvv {
        LocalSurfaceType::Elliptical
    } else {
        LocalSurfaceType::Hyperbolic
    }
}

/// The UV components of `vectors` in the tangent plane spanned by `du` and `dv`
/// (`smsurf_DropVectors`): `vec ~ u * du + v * dv`. Zero tangents give zero
/// components; tangents shorter than `sqrt(SM_EFF_ZERO)` are unitised for the
/// fallback projection.
///
/// # Errors
///
/// `SM_ERR` when the tangents are parallel or the projected equations are singular.
pub fn drop_vectors(du: Vec3, dv: Vec3, vectors: &[Vec3]) -> Result<Vec<Vec2>> {
    let zero = Vec2::new(0.0, 0.0);
    let mut sdu = du;
    let mut sdv = dv;

    // check state - surface tangents must be nonZero
    let du_length_sq = sdu.length_squared();
    let dv_length_sq = sdv.length_squared();
    if du_length_sq < EFF_ZERO_SQ || dv_length_sq < EFF_ZERO_SQ {
        return Ok(vec![zero; vectors.len()]);
    }

    // unitize short tangent vectors (probably near singularities)
    if du_length_sq < EFF_ZERO {
        sdu = sdu.unitize_or_z();
    }
    if dv_length_sq < EFF_ZERO {
        sdv = sdv.unitize_or_z();
    }
    let norm = sdu.cross(sdv);

    // normalize the normal vector to make stable calculations near singularity cases
    if norm.length_squared() < EFF_ZERO_SQ {
        return Err(SolidError::sm_err(OP_DROP));
    }
    let norm = norm.unitize_or_z();

    // First, in regular cases, write the vector as a linear combination of the basis.
    let mut out = vec![zero; vectors.len()];
    let mut ok = false;
    for (slot, &vec) in out.iter_mut().zip(vectors) {
        match vec_lin_comb_two_vectors(du, dv, vec) {
            Ok(uv) => {
                *slot = uv;
                ok = true;
            }
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => {
                ok = false;
                break;
            }
        }
    }
    if ok {
        return Ok(out);
    }

    // Linear combination didn't work: project to plane, using unitized basis vectors.
    let mut plane = Axis2Placement::default();
    let orig = Vec3::new(0.0, 0.0, 0.0);
    let _ = plane.set_step_canonical(orig, norm, sdu);

    // for every vector being dropped
    for (slot, &vec) in out.iter_mut().zip(vectors) {
        // If drop vector is zero, then its projection is zero.
        if vec.length_squared() < EFF_ZERO_SQ {
            *slot = zero;
            continue;
        }

        // get in-plane 2d vectors for the sDU and sDV surface tangent vectors.
        let du_2d = plane_drop_point(&plane, sdu);
        let dv_2d = plane_drop_point(&plane, sdv);
        let vec_2d = plane_drop_point(&plane, vec);
        let eq1 = Vec3::new(du_2d.x, dv_2d.x, vec_2d.x);
        let eq2 = Vec3::new(du_2d.y, dv_2d.y, vec_2d.y);
        let sol = eq1
            .solve_two_linear_equations(eq2)
            .map_err(|_| SolidError::sm_err(OP_DROP))?;
        *slot = Vec2::new(sol.x, sol.y);
    }

    Ok(out)
}

// ---- file-static helpers -------------------------------------------------------------------

/// `sm_ConvertToParameterDelta`: the parameter step along a curve with tangent
/// `tangent` that moves by the projection of `delta` on it (`0` for a zero tangent).
#[must_use]
pub fn convert_to_parameter_delta(tangent: Vec3, delta: Vec3) -> f64 {
    let tan_len = tangent.length();
    if tan_len < EFF_ZERO {
        return 0.0;
    }
    let unit_tan = tangent / tan_len;
    let dot = unit_tan.dot(delta);
    dot / tan_len
}

/// `sm_MergeDoubleArrays`: merges two increasing sequences into one, values within
/// their scaled zero (`SM_ARE_SAME`) merged. When neither array is empty, the merge
/// stops as soon as one of them is exhausted (upstream drops the rest of the other).
#[must_use]
pub fn merge_double_arrays(params1: &[f64], params2: &[f64]) -> Vec<f64> {
    // low work - one or both of the arrays is empty
    if params1.is_empty() {
        return params2.to_vec();
    }
    if params2.is_empty() {
        return params1.to_vec();
    }

    let mut out = Vec::with_capacity(params1.len() + params2.len());
    let mut i1 = 0;
    let mut i2 = 0;
    while let (Some(&p1), Some(&p2)) = (params1.get(i1), params2.get(i2)) {
        // values within SM_EFF_ZERO of one another are merged into one
        if are_same(p1, p2) {
            out.push(p1);
            i1 += 1;
            i2 += 1;
        }
        // else add next smallest value to output
        else if p1 < p2 {
            out.push(p1);
            i1 += 1;
        } else {
            out.push(p2);
            i2 += 1;
        }
    }
    out
}

/// `s_MapUV`: `(u, v)` mirrored in the unit square along the directions with
/// negative `dir_u` / `dir_v`, then swapped for [`OrientType::Opposite`].
#[must_use]
pub fn map_uv(u: f64, v: f64, orient: OrientType, dir_u: f64, dir_v: f64) -> Point2 {
    let mut ret = Point2::new(u, v);
    if dir_u < 0.0 {
        ret.x = 1.0 - ret.x;
    }
    if dir_v < 0.0 {
        ret.y = 1.0 - ret.y;
    }
    if orient == OrientType::Opposite {
        ret = ret.swap_xy();
    }
    ret
}

/// `s_IsEven`.
#[must_use]
pub const fn is_even(num: i32) -> bool {
    (num / 2) * 2 == num
}

/// `sm_SetUpBreaks`: trims `breaks` to the values inside `ivl` and makes it start at
/// `ivl.min()` and end at `ivl.max()` (adding them when farther than
/// `SM_EFF_ZERO_SQRT`).
///
/// # Errors
///
/// `InvalidInput` for empty `breaks` (upstream reads past the array).
pub fn set_up_breaks(breaks: &mut Vec<f64>, ivl: &Extent1d) -> Result<()> {
    let empty = || SolidError::invalid_input(OP_BREAKS, "empty break array");
    loop {
        let first = *breaks.first().ok_or_else(empty)?;
        if ivl.contains_value(first, 0.0) {
            break;
        }
        breaks.remove(0);
        if breaks.is_empty() {
            breaks.push(ivl.min());
            break;
        }
    }
    let first = *breaks.first().ok_or_else(empty)?;
    if (first - ivl.min()).abs() > EFF_ZERO_SQRT {
        breaks.insert(0, ivl.min());
    }

    loop {
        let last = *breaks.last().ok_or_else(empty)?;
        if ivl.contains_value(last, 0.0) {
            break;
        }
        breaks.pop();
        if breaks.len() == 1 {
            break;
        }
    }
    let last = *breaks.last().ok_or_else(empty)?;
    if (last - ivl.max()).abs() > EFF_ZERO_SQRT {
        breaks.push(ivl.max());
    }
    Ok(())
}

/// Inserts `t` into the sorted `split_params` unless a value within `tol` is there.
fn insert_split(split_params: &mut Vec<f64>, t: f64, tol: f64) {
    let mut insert_index = split_params.len();
    let mut split = true;
    for (jj, &p) in split_params.iter().enumerate() {
        if (t - p).abs() < tol {
            split = false;
            break;
        } else if t < p {
            insert_index = jj;
            break;
        }
    }
    if split {
        split_params.insert(insert_index, t);
    }
}

/// `sm_IntersectAndSplit`: adds the parameters where `uv_curve` over `interval`
/// crosses the plane through `plane_pnt` with normal `plane_norm` to the sorted
/// `split_params` (skipping values within `SM_EFF_ZERO_SQRT` of one there).
///
/// `handle` is `uv_curve`'s handle in the solver's object space (upstream the
/// solutions name the curve itself): `IdenticalSolutions` merges two solutions
/// within 1/10000th of its natural interval, so it must report the curve's natural
/// interval (`None` would merge only within `100 * SM_EFF_ZERO`).
///
/// # Errors
///
/// Those of `SmCurve::GlobalPropertyAnalysis`.
pub fn intersect_and_split<O: SolutionObject>(
    k: &Kernel,
    uv_curve: &Curve,
    interval: &Extent1d,
    plane_pnt: Point3,
    plane_norm: Vec3,
    handle: Option<O>,
    split_params: &mut Vec<f64>,
) -> Result<()> {
    let plane_d = -plane_norm.dot(plane_pnt);
    let vectors = [plane_norm];
    let analysis = uv_curve.global_property_analysis::<O>(
        k,
        interval,
        CurvePropertyType::PlaneIntersection,
        EFF_ZERO_SQRT,
        &PropertyAnalysisOptions {
            property_value: Some(plane_d),
            opt_vectors: &vectors,
            want_comb: false,
            handle,
        },
    )?;

    for sol in analysis.solutions.as_slice() {
        insert_split(split_params, sol.start.parameters[0], EFF_ZERO_SQRT);
    } // end for all solutions
    Ok(())
}

/// `sm_IntersectCurveAndSplit`: adds the parameters where `uv_curve` over `interval`
/// crosses `line` over `line_interval` (2D intersections at distance `scaled_zero`)
/// to the sorted `split_params` (skipping values within `SM_EFF_ZERO_PARAM` of one
/// there).
///
/// # Errors
///
/// Those of `SmCurve::GlobalCurveIntersect`.
pub fn intersect_curve_and_split<O: SolutionObject>(
    k: &Kernel,
    uv_curve: &Curve,
    interval: &Extent1d,
    line: &Line,
    line_interval: &Extent1d,
    scaled_zero: f64,
    split_params: &mut Vec<f64>,
) -> Result<()> {
    let line = Curve::from(line.clone());
    // Find UVCurve/Line 2d XSects
    let sols = uv_curve.global_curve_intersect::<O>(
        k,
        interval,
        &line,
        line_interval,
        scaled_zero,
        &CurveIntersectOptions::default(),
    )?;

    // for every solution - sort new solutions into rSplitParameters
    for sol in sols.as_slice() {
        insert_split(split_params, sol.start.parameters[0], eff_zero_param());
    } // end for all solutions
    Ok(())
}

/// `sm_CCLocalSolve`: the extrema of the distance between `curve1` over `interval1`
/// and `curve2` over `interval2` (`op` is [`SolverOperation::Minimize`] or
/// `Maximize`): the end points with position discontinuities (`conts[0] <= C0` at
/// the start, `<= DISCONTINUOUS` at the end) solved against the other curve, then a
/// curve/curve solve from the interval midpoints. Each solution has two variables
/// (the parameters on `curve1` and `curve2`).
///
/// # Errors
///
/// Those of the evaluations and local solves.
pub fn cc_local_solve<O: SolutionObject>(
    k: &Kernel,
    curve1: &Curve,
    interval1: &Extent1d,
    conts1: [ContinuityType; 2],
    curve2: &Curve,
    interval2: &Extent1d,
    conts2: [ContinuityType; 2],
    op: SolverOperation,
    tol_3d: f64,
) -> Result<SolutionArray<O>> {
    let mut solutions = SolutionArray::new();
    let no_opts = PointSolveOptions::default();
    let c0 = ContinuityType::C0 as i32;
    let discontinuous = ContinuityType::Discontinuous as i32;
    let set_param = |s: &mut Solution<O>, i: usize, v: f64| -> Result<()> {
        *s.start.param_mut(i)? = v;
        Ok(())
    };

    // If we have a total discontinuity (i.e. end of curve) then we always test.  If we
    // have a derivative discontinuity then we just test the ones on the left side.
    if (conts1[0] as i32) <= c0 {
        let pnt = curve1.evaluate_point(k, interval1.min())?;
        let (found, mut sol) = curve2.local_point_solve::<O>(
            k,
            interval2,
            op,
            pnt,
            &no_opts,
            interval2.evaluate(0.5),
        )?;
        if found {
            sol.num_variables = 2;
            let p0 = sol.start.param(0)?;
            set_param(&mut sol, 1, p0)?;
            set_param(&mut sol, 0, interval1.min())?;
            solutions.add(&sol);
        }
    }
    if (conts1[1] as i32) <= discontinuous {
        let pnt = curve1.evaluate_point(k, interval1.max())?;
        let (found, mut sol) = curve2.local_point_solve::<O>(
            k,
            interval2,
            op,
            pnt,
            &no_opts,
            interval2.evaluate(0.5),
        )?;
        if found {
            sol.num_variables = 2;
            let p0 = sol.start.param(0)?;
            set_param(&mut sol, 1, p0)?;
            set_param(&mut sol, 0, interval1.max())?;
            solutions.add(&sol);
        }
    }
    if (conts2[0] as i32) <= c0 {
        let pnt = curve2.evaluate_point(k, interval2.min())?;
        let (found, mut sol) = curve1.local_point_solve::<O>(
            k,
            interval1,
            op,
            pnt,
            &no_opts,
            interval1.evaluate(0.5),
        )?;
        if found {
            sol.num_variables = 2;
            set_param(&mut sol, 1, interval2.min())?;
            solutions.add(&sol);
        }
    }
    if (conts2[1] as i32) <= discontinuous {
        let pnt = curve2.evaluate_point(k, interval2.max())?;
        let (found, mut sol) = curve1.local_point_solve::<O>(
            k,
            interval1,
            op,
            pnt,
            &no_opts,
            interval1.evaluate(0.5),
        )?;
        if found {
            sol.num_variables = 2;
            set_param(&mut sol, 1, interval2.max())?;
            solutions.add(&sol);
        }
    }

    // Take midpoint of interval for guess.
    let guess1 = interval1.evaluate(0.5);
    let guess2 = interval2.evaluate(0.5);
    let (found, mut sol) = curve1.local_curve_solve::<O>(
        k,
        interval1,
        curve2,
        interval2,
        op,
        tol_3d,
        None,
        None,
        [guess1, guess2],
        [None, None],
    )?;
    if found {
        sol.num_variables = 2;
        solutions.add(&sol);
    }
    Ok(solutions)
}

/// `sm_CsArePerpenDisjoint`: true when no line from the curve node's span to the
/// surface node's Bezier patch can be perpendicular to both (the patch's normal
/// cone and the span's tangent cone are disjoint from the cone between the boxes).
///
/// # Errors
///
/// Those of `SmExtent3d::ComputeConeBoundTo`; [`SolidError::Unported`] once the
/// patch's polar box is needed (the `SmBezierPatch` of a tree node is held by the
/// surface cache, which this signature does not take). Production solves never
/// reach that: every `SmCSGlobalSolver` the port builds holds its surface cache and
/// runs [`cs_are_perpen_disjoint_in`]; this cache-less form serves solvers built
/// without one (tests, the `surface.surface_p2` runner's node-only cases).
pub fn cs_are_perpen_disjoint<O>(
    curve_tree: &Tree<O>,
    curve_node: &TreeNode<O>,
    surface_node: &TreeNode<O>,
) -> Result<bool> {
    // Surface node has a polar box
    if surface_node.aux_type == AuxDataType::BezierSurface
        || surface_node.aux_type == AuxDataType::BezierSurfaceHead
    {
        let (cone_vec, cone_angle_rad) = surface_node.bbox.cone_bound_to(&curve_node.bbox)?;
        if cone_angle_rad > PI / 2.0 {
            return Ok(false);
        }

        // SmBezierPatch::GetPolarBox is SurfaceCache::polar_box_unstored, but the patch
        // data lives in the surface cache, which a solver built with one passes to
        // cs_are_perpen_disjoint_in (every production CS solve; see the doc). Without
        // the cache the polar box is out of reach (then IsDisjointWithCone, and for a
        // curve span node the chord cone test smgu_ConesAreDisjointPerpen with the
        // span's turning angle plus 3 degrees).
        let _ = (curve_tree, cone_vec);
        return Err(SolidError::unported(
            "SmBezierPatch::GetPolarBox",
            "surface.surface_cache_p1",
        ));
    }

    // node does not have a polar box to check
    Ok(false)
}

/// [`cs_are_perpen_disjoint`] for a surface node `surface_id` whose Bezier patch is
/// held by `cache` (the polar box is computed without being stored,
/// [`SurfaceCache::polar_box_unstored`]; the span's ends are those stored when the
/// curve tree was built). Added for the leaf solve of unit `surface.surface_p11`.
///
/// # Errors
///
/// Those of the patch and polar box lookups; [`SolidError::Unported`] propagates.
pub fn cs_are_perpen_disjoint_in<O: Copy + PartialEq>(
    k: &Kernel,
    curve_tree: &Tree<O>,
    curve_node: &TreeNode<O>,
    surface_node: &TreeNode<O>,
    cache: &SurfaceCache<O>,
    surface_id: crate::geom::tree::NodeId,
) -> Result<bool> {
    // Surface node has a polar box
    if surface_node.aux_type == AuxDataType::BezierSurface
        || surface_node.aux_type == AuxDataType::BezierSurfaceHead
    {
        // (SER inside a SmBoolean function: a failed cone bound returns its non-zero
        // status, TRUE)
        let Some((cone_vec, cone_angle_rad)) =
            crate::error::ignored(surface_node.bbox.cone_bound_to(&curve_node.bbox))?
        else {
            return Ok(true);
        };
        if cone_angle_rad > PI / 2.0 {
            return Ok(false);
        }

        if cache
            .polar_box_unstored(k, surface_id)?
            .is_disjoint_with_cone(cone_vec, cone_angle_rad)
        {
            return Ok(true);
        }

        if curve_node.aux_type != AuxDataType::BezierCurve
            && curve_node.aux_type != AuxDataType::CurveData
        {
            return Ok(false);
        }

        let span = curve_tree.curve_aux(curve_node)?;
        let [start, end] = span.ends;
        let line_vec = end - start;
        if line_vec.length_squared() > EFF_ZERO_SQ {
            let curve_angle_rad = span.max_turning_angle_deg * PI / 180.0 + 3.0 * PI / 180.0;
            if crate::geom::geom_utility::p1::cones_are_disjoint_perpen(
                line_vec,
                curve_angle_rad,
                cone_vec,
                cone_angle_rad,
            ) {
                return Ok(true);
            }
        }
    }

    // node does not have a polar box to check
    Ok(false)
}

// ---- SmLSIGlobalSolver ------------------------------------------------------------------------

/// The line/surface intersection global solver (`SmLSIGlobalSolver`): one tree (the
/// surface cache tree), solutions `[line t, surface u, surface v]`.
pub struct LsiGlobalSolver<'t, 's, O> {
    /// Caller context for virtual evaluations.
    pub kernel: &'s Kernel,

    /// The `SmGlobalSolver` state.
    pub base: GlobalSolver<'t, O>,
    /// `m_pSurf`: the surface being solved for.
    pub surface: Option<&'s Surface>,
    /// `m_crUVDomain`: the intersection domain.
    pub uv_domain: Extent2d,
    /// `m_vLinePoint`.
    pub line_point: Point3,
    /// `m_vLineVector`.
    pub line_vector: Vec3,
    /// `m_cpLineInterval`: the line interval, if any.
    pub line_interval: Option<Extent1d>,
    /// `m_bRayFire`: skip intersections at negative line parameters.
    pub ray_fire: bool,
    /// `m_dDistanceTolerance`: smallest distance between distinct points.
    pub distance_tolerance: f64,
    /// The cache holding the patches of the surface tree's nodes, read by the leaf
    /// solve (part 10; upstream reaches it through the node's tree owner).
    pub surface_cache: Option<&'s SurfaceCache<O>>,
    /// Prepared face classification shared with other caches in this operation.
    pub face_queries: Option<&'s dyn crate::surface::surface_cache::face_context::FaceQueries<O>>,
    /// The point test of the face's trimmed cache when the cache is one
    /// (`SmTrimSrfCache::PointTest`, the virtual `PointTest` `sm_IsPointValid`
    /// calls); `None` tests against the cache's domain (`SmSurfaceCache::PointTest`).
    pub face_cache: Option<&'s mut (dyn FaceSolveCache<O> + 's)>,
}

impl<O: std::fmt::Debug> std::fmt::Debug for LsiGlobalSolver<'_, '_, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LsiGlobalSolver")
            .field("base", &self.base)
            .field("surface", &self.surface)
            .field("uv_domain", &self.uv_domain)
            .field("line_point", &self.line_point)
            .field("line_vector", &self.line_vector)
            .field("line_interval", &self.line_interval)
            .field("ray_fire", &self.ray_fire)
            .field("distance_tolerance", &self.distance_tolerance)
            .field("surface_cache", &self.surface_cache)
            .field("face_cache", &self.face_cache.is_some())
            .field("face_queries", &self.face_queries.is_some())
            .finish_non_exhaustive()
    }
}

impl<O: SolutionObject> LsiGlobalSolver<'_, '_, O> {
    /// `BranchMayContainAnswers`: false when the line provably misses the bounding
    /// sphere (or, with an interval or ray, its part of the line) or the box of the
    /// surface node `branch[0]`, grown by the distance tolerance.
    ///
    /// # Errors
    ///
    /// Those of the branch lookup and of `SmExtent3d::IntersectLine` (a zero line
    /// vector or an undefined box; upstream then reads an uninitialised count).
    pub fn may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        let surface_node = self.base.branch_node(branch, 0)?;

        // compute SphereBound Radius for SurfaceNode with tolerance
        let surf_box = &surface_node.bbox;
        let sphere = surf_box.sphere_bound();
        let sph_center = sphere.center;
        let sph_radius = sphere.radius;
        let min_radius = sphere.min_radius;
        let tol = self.distance_tolerance;
        let rad_dist_sq = (sph_radius + tol) * (sph_radius + tol);

        // get Line/SphereCenter minimum distance
        let line_param = line_closest_point(self.line_point, self.line_vector, sph_center);
        let point_on_line = self.line_point + line_param * self.line_vector;
        let dist_to_line_sq = point_on_line.distance_squared(sph_center);

        // when the Line is outside the SphereBound - No Answers possible
        if dist_to_line_sq > rad_dist_sq {
            return Ok(false);
        }

        // For RayFiring - and sphere is behind rayStartPoint
        if self.ray_fire && line_param < 0.0 {
            // and rayStart is outside of the sphereBound
            let dist_to_start_sq = self.line_point.distance_squared(sph_center);
            if dist_to_start_sq > rad_dist_sq {
                return Ok(false);
            }
        }

        // when given a line interval
        if let Some(ivl) = &self.line_interval {
            // If sphere is below line interval
            if line_param < ivl.min() {
                let start_pt = self.line_point + ivl.min() * self.line_vector;
                if start_pt.distance_squared(sph_center) > rad_dist_sq {
                    return Ok(false);
                }
            }

            // if sphere is above line interval
            if line_param > ivl.max() {
                let end_pt = self.line_point + ivl.max() * self.line_vector;
                if end_pt.distance_squared(sph_center) > rad_dist_sq {
                    return Ok(false);
                }
            }
        }

        // check linePoint bounding box intersections
        if dist_to_line_sq <= (min_radius + tol) * (min_radius + tol)
            || surf_box.contains_point(point_on_line, tol)
        {
            Ok(true)
        } else {
            // check lineVector for bounding box intersections
            let hit = surf_box.intersect_line(point_on_line, self.line_vector)?;
            Ok(hit.count() > 0)
        }
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for LsiGlobalSolver<'t, '_, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.may_contain_answers(branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        self.local_solve_branch(k, branch)
    }

    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let k = self.kernel;
        self.surface_identical_solutions(k, sol1, sol2)
    }
}

// ---- SmIsoCurveLengthIFEO -----------------------------------------------------------------------

/// The integrand of the length of a `u` iso-parameter curve (`SmIsoCurveLengthIFEO`,
/// used for surface areas). Its `Evaluate` belongs to unit `surface.surface_p1`.
#[derive(Debug, Clone, Copy)]
pub struct IsoCurveLengthIntegrand<'s> {
    /// `m_crSurface`.
    pub surface: &'s Surface,
    /// `m_dUIsoValue`.
    pub u_iso_value: f64,
}

impl<'s> IsoCurveLengthIntegrand<'s> {
    /// `SmIsoCurveLengthIFEO(crSurface, dUIsoValue)`.
    #[must_use]
    pub const fn new(surface: &'s Surface, u_iso_value: f64) -> Self {
        Self {
            surface,
            u_iso_value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent3d::Extent3d;
    use crate::geom::tree::NodeId;
    use crate::geom::vector2d::Vec2;

    fn bilinear() -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(2.0, 3.0, 1.0),
            )
            .unwrap(),
        )
    }

    /// `SetCanonical`'s `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` of each edited surface, in
    /// upstream's order: the B-spline kinds notify themselves (the stored trimmed
    /// surface cache deleted at `SM_NO_PRE_EDIT`), an offset only its base surface (it
    /// has no extended one here), a surface in a volume itself around its base.
    #[test]
    fn swap_uv_notifying_names_each_edited_surface() {
        use crate::surface::volume::{Volume, VolumeKind};
        use NotifyOperation::{PostEdit, PreEdit};
        let k = Kernel::default();
        let mut ctx = Context::default();
        let mut run = |s: &mut Surface| {
            let mut events = Vec::new();
            s.swap_uv_notifying(&k, &mut |slot, type_code, op| {
                events.push((ctx.geometry_key(slot)?, type_code, op));
                Ok(())
            })
            .map(|()| events)
        };

        for mut s in [
            bilinear(),
            Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)),
        ] {
            s.attach_trim_cache(|_| {}).unwrap();
            assert!(s.has_trim_cache());
            let ty = s.type_code();
            let events = run(&mut s).unwrap();
            let key = s.notify_key().unwrap();
            assert_eq!(events, vec![(key, ty, PreEdit), (key, ty, PostEdit)]);
            assert!(!s.has_trim_cache());
            // the plain entry point notifies nobody
            s.swap_uv(&k).unwrap();
        }

        let mut off = Surface::from(OffsetSurface::new(&k, 1.0, bilinear(), true, 0).unwrap());
        let events = run(&mut off).unwrap();
        assert!(off.notify_key().is_none());
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.1 == BSplineSurface::TYPE));
        assert_eq!(events[0].0, events[1].0);
        assert_eq!((events[0].2, events[1].2), (PreEdit, PostEdit));

        // SmSrfInVolume::SwapUV: its own PRE/POST around its base surface's
        let in_volume = |owner_flag| {
            Surface::from(
                SrfInVolume::new(
                    &k,
                    bilinear(),
                    true,
                    Volume::new(VolumeKind::Base),
                    0,
                    owner_flag,
                )
                .unwrap(),
            )
        };
        let mut siv = in_volume(3);
        let ty = siv.type_code();
        let events = run(&mut siv).unwrap();
        let own = siv.notify_key().unwrap();
        let SurfaceKind::SrfInVolume(v) = &siv.kind else {
            panic!("not a surface in a volume");
        };
        let base = v.surface().notify_key().unwrap();
        assert_ne!(own, base);
        let b = BSplineSurface::TYPE;
        assert_eq!(
            events,
            vec![
                (own, ty, PreEdit),
                (base, b, PreEdit),
                (base, b, PostEdit),
                (own, ty, PostEdit)
            ]
        );
        // an unowned surface is refused before any notification
        let mut unowned = in_volume(2);
        assert!(run(&mut unowned).is_err());
        assert!(unowned.notify_key().is_none());
    }

    #[test]
    fn evaluation_copy_leaf_depth_boundary_matches_general_walk() {
        for surface in [
            bilinear(),
            Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)),
        ] {
            for depth in [
                0,
                MAX_GEOMETRY_NESTING,
                MAX_GEOMETRY_NESTING + 1,
                usize::MAX,
            ] {
                let general = check_geometry_nesting(
                    NestedGeometry::Surface(&surface),
                    depth,
                    "surface::surface::p2::clone_for_evaluation",
                );
                let leaf = surface.check_evaluation_copy_depth(depth);
                assert_eq!(format!("{leaf:?}"), format!("{general:?}"));
                assert_eq!(leaf.is_ok(), depth <= MAX_GEOMETRY_NESTING);
            }
        }
    }

    #[test]
    fn evaluation_copy_bounds_compound_surface_nesting() {
        let mut surface = Surface::from(BSplineSurface::new());
        for _ in 0..MAX_GEOMETRY_NESTING {
            surface = Surface::from(StepSurface::new(surface, true).unwrap());
        }
        assert!(surface.clone_for_evaluation().is_ok());
        assert!(matches!(
            StepSurface::new(surface.clone(), true),
            Err(SolidError::InvalidInput { .. })
        ));
        // The general iso constructor checks before clamping through nested bases.
        assert!(crate::curve::iso_curve::IsoCurve::from_surface(
            &Kernel::default(),
            surface,
            SurfParamType::U,
            0.5,
            true,
            true,
        )
        .is_err());
    }

    #[test]
    fn constructors_bound_nested_trim_curves_and_volume_maps() {
        use crate::curve::offset_curve::OffsetCurve;
        use crate::surface::volume::{Volume, VolumeKind};
        let mut curve = Curve::from(BSplineCurve::new(3));
        for _ in 0..MAX_GEOMETRY_NESTING - 1 {
            curve = Curve::from(
                OffsetCurve::new(3, curve, Extent1d::from_raw(0.0, 1.0), Vec3::Z, 1.0, true)
                    .unwrap(),
            );
        }
        let bounded = |curve| {
            CurveBoundedSurface::new(
                Surface::from(BSplineSurface::new()),
                vec![curve],
                vec![],
                true,
            )
        };
        let surface = Surface::from(bounded(curve.clone()).unwrap());
        assert!(surface.clone_for_evaluation().is_ok());
        let curve = Curve::from(
            OffsetCurve::new(3, curve, Extent1d::from_raw(0.0, 1.0), Vec3::Z, 1.0, true).unwrap(),
        );
        assert!(matches!(
            bounded(curve),
            Err(SolidError::InvalidInput { .. })
        ));

        // MAX_GEOMETRY_NESTING maps fit below a wrapper, one more only on its own
        let mut volume = Volume::new(VolumeKind::Base);
        for _ in 0..MAX_GEOMETRY_NESTING - 1 {
            volume = Volume::with_maps(VolumeKind::Base, None, Some(volume)).unwrap();
        }
        let in_volume = |volume| {
            SrfInVolume::new(
                &Kernel::default(),
                Surface::from(BSplineSurface::new()),
                true,
                volume,
                0,
                3,
            )
        };
        let surface = Surface::from(in_volume(volume.clone()).unwrap());
        assert!(surface.clone_for_evaluation().is_ok());
        volume
            .add_next_map(Volume::new(VolumeKind::Base), 2)
            .unwrap();
        assert!(matches!(
            in_volume(volume.clone()),
            Err(SolidError::InvalidInput { .. })
        ));
        // a chain at the limit refuses one more map, leaving it unchanged
        let before = volume.clone();
        assert!(matches!(
            volume.add_next_map(Volume::new(VolumeKind::Base), 2),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(format!("{volume:?}"), format!("{before:?}"));
        assert!(matches!(
            Volume::with_maps(VolumeKind::Base, None, Some(volume)),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn curve_on_surface_rejects_surface_depth_before_domain_and_owned_copy() {
        use crate::curve::crv_on_surf::CrvOnSurf;
        let k = Kernel::default();
        let mut surface = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        for _ in 0..MAX_GEOMETRY_NESTING {
            surface = Surface::from(StepSurface::new(surface, true).unwrap());
        }
        assert!(surface.clone_for_evaluation().is_ok());
        for owner_flag in 0..=3 {
            assert!(matches!(
                CrvOnSurf::new(
                    &k,
                    Curve::from(BSplineCurve::new(2)),
                    surface.clone_for_evaluation().unwrap(),
                    None,
                    owner_flag,
                ),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        assert!(matches!(
            CrvOnSurf::from_uv_line(
                &k,
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                surface.clone_for_evaluation().unwrap(),
                None,
                2,
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            CrvOnSurf::empty(Curve::from(BSplineCurve::new(2)), surface),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    /// An offset chain is walked into its base by the evaluation copy (and bounded).
    #[test]
    fn evaluation_copy_walks_offset_bases() {
        use crate::surface::offset_surface::OffsetSurface;
        let k = Kernel::default();
        let plane = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        let off = Surface::from(OffsetSurface::new(&k, 1.0, plane, true, 0).unwrap());
        let copy = off.clone_for_evaluation().unwrap();
        let uv = Point2::new(0.25, 0.5);
        assert_eq!(
            copy.evaluate_point(&k, uv).unwrap(),
            off.evaluate_point(&k, uv).unwrap()
        );
        // the deepest chain the constructor allows is copied or refused, never overflows
        let mut deep = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        let mut levels = 0;
        while let Ok(o) = OffsetSurface::new(&k, 0.0, deep.clone(), true, 0) {
            deep = Surface::from(o);
            levels += 1;
            assert!(levels <= MAX_GEOMETRY_NESTING);
        }
        assert!(levels >= 10);
        let _ = deep.clone_for_evaluation();
    }

    #[test]
    fn failed_zero_tangent_eval_keeps_the_written_position_and_derivatives() {
        let k = &Kernel::default();
        let surface = Surface::from(
            BSplineSurface::create_bilinear_surface(Vec3::ZERO, Vec3::ZERO, Vec3::Y, Vec3::Y)
                .unwrap(),
        );
        let mut eval = SurfaceEval::new(k, Some(&surface));
        eval.set_uv(Point2::new(0.5, 0.5));
        assert!(eval.eval().is_err());
        assert_eq!(eval.values()[0][0], Vec3::new(0.0, 0.5, 0.0));
        assert_eq!(eval.values()[1][0], Vec3::ZERO);
        assert_eq!(eval.values()[0][1], Vec3::Y);
        assert!(eval.fails_eval);
    }

    #[test]
    fn merge_drops_tail() {
        assert_eq!(merge_double_arrays(&[], &[1.0, 2.0]), vec![1.0, 2.0]);
        assert_eq!(merge_double_arrays(&[1.0], &[]), vec![1.0]);
        // upstream stops when one array is exhausted
        assert_eq!(
            merge_double_arrays(&[0.0, 1.0], &[1.0, 2.0, 3.0]),
            vec![0.0, 1.0]
        );
        assert_eq!(merge_double_arrays(&[0.5], &[0.0, 1.0]), vec![0.0, 0.5]);
    }

    #[test]
    fn set_up_breaks_clamps() {
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        let mut b = vec![-1.0, 0.5, 2.0];
        set_up_breaks(&mut b, &ivl).unwrap();
        assert_eq!(b, vec![0.0, 0.5, 1.0]);
        let mut b = vec![5.0, 6.0];
        set_up_breaks(&mut b, &ivl).unwrap();
        assert_eq!(b, vec![0.0, 1.0]);
        assert!(set_up_breaks(&mut Vec::new(), &ivl).is_err());
    }

    #[test]
    fn small_helpers() {
        assert!(is_even(4) && !is_even(3) && is_even(-2) && !is_even(-3));
        let p = map_uv(0.25, 0.5, OrientType::Opposite, -1.0, 1.0);
        assert_eq!(p, Vec2::new(0.5, 0.75));
        assert_eq!(convert_to_parameter_delta(Vec3::ZERO, Vec3::X), 0.0);
        assert_eq!(convert_to_parameter_delta(2.0 * Vec3::X, Vec3::X), 0.5);
        assert!(derivative_limit(Vec3::new(1.0, -3.0, 2.0)) > 0.0);
        assert_eq!(normal_limit(), 1.0e-8);
    }

    #[test]
    fn lift_matches_chain_rule_for_linear_curve() {
        let z = Vec3::ZERO;
        let d =
            lift_second_derivative(Vec3::new(1.0, 0.0, 0.0), z, Vec3::X, Vec3::Y, Vec3::Z, z, z);
        assert_eq!(d, Vec3::Z);
        let t = lift_third_derivative(
            Vec3::new(1.0, 1.0, 0.0),
            z,
            z,
            z,
            z,
            z,
            z,
            z,
            Vec3::X,
            z,
            z,
            Vec3::Y,
        );
        assert_eq!(t, Vec3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn scale_step_derivatives() {
        let mut d = vec![Vec3::new(1.0, 1.0, 1.0); 4];
        scale_step_angular_derivatives(&mut d, 1, 1, true, false).unwrap();
        let s = PI / 180.0;
        assert_eq!(d[0], Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(d[2], Vec3::new(s, s, s));
        // mixed order 2 > min(1, 1): untouched
        assert_eq!(d[3], Vec3::new(1.0, 1.0, 1.0));
        assert!(scale_step_angular_derivatives(&mut d, 2, 2, true, true).is_err());
    }

    #[test]
    fn geometric_properties_of_cylinder_point() {
        // cylinder of radius 2 about z: S(u,v) = (2cos u, 2sin u, v) at u = 0
        let mut e = [[Vec3::UNDEF; 3]; 3];
        e[0][0] = Vec3::new(2.0, 0.0, 0.0);
        e[1][0] = Vec3::new(0.0, 2.0, 0.0);
        e[0][1] = Vec3::Z;
        e[1][1] = Vec3::ZERO;
        e[2][0] = Vec3::new(-2.0, 0.0, 0.0);
        e[0][2] = Vec3::ZERO;
        let g = evaluate_geometric(&mut e).unwrap();
        assert!(g.gaussian_curvature.abs() < 1e-12);
        let ks = [g.principle_curvature1, g.principle_curvature2];
        assert!(ks.iter().any(|k| (k.abs() - 0.5).abs() < 1e-12));
        // singular without normal
        let mut s = [[Vec3::ZERO; 3]; 3];
        assert!(evaluate_geometric(&mut s).is_err());
    }

    /// `TestSmPrincipalDirectionPairing` (`SM_API_test`, `TestSmGeneral.cpp`): curvatures 1
    /// and 0 with directions (5,-12,0)/13 and (12,5,0)/13 on a skewed UV frame.
    #[test]
    fn principal_direction_pairing() {
        let mut e = [[Vec3::ZERO; 3]; 3];
        e[1][0] = Vec3::new(1.0, 0.0, 0.0);
        e[0][1] = Vec3::new(1.0, 1.0, 0.0);
        e[2][0] = Vec3::new(0.0, 0.0, 25.0 / 169.0);
        e[1][1] = Vec3::new(0.0, 0.0, -35.0 / 169.0);
        e[0][2] = Vec3::new(0.0, 0.0, 49.0 / 169.0);
        let g = evaluate_geometric(&mut e).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12;
        assert!(close(g.gaussian_curvature, 0.0) && close(g.normal_curvature, 0.5));
        assert!(close(g.principle_curvature1, 1.0) && close(g.principle_curvature2, 0.0));
        let same = |a: Vec3, b: Vec3| (a.dot(b).abs() - 1.0).abs() <= 1e-12;
        assert!(same(
            g.principle_curvature_vector1,
            Vec3::new(5.0 / 13.0, -12.0 / 13.0, 0.0)
        ));
        assert!(same(
            g.principle_curvature_vector2,
            Vec3::new(12.0 / 13.0, 5.0 / 13.0, 0.0)
        ));
        assert!(close(
            g.principle_curvature_vector1
                .cross(g.principle_curvature_vector2)
                .z,
            1.0
        ));
    }

    #[test]
    fn point_type_keeps_upstream_bug() {
        let mut uu = SurfaceSideType::On;
        let t = compute_surface_point_type(Vec3::Z, Vec3::Z, -Vec3::Z, Some(&mut uu), true);
        assert_eq!(t, LocalSurfaceType::Hyperbolic);
        assert_eq!(uu, SurfaceSideType::Below);
        let t = compute_surface_point_type(Vec3::Z, Vec3::Z, Vec3::X, Some(&mut uu), false);
        assert_eq!(t, LocalSurfaceType::Cylindrical);
        assert_eq!(uu, SurfaceSideType::Above);
    }

    #[test]
    fn drop_vectors_basis() {
        let r = drop_vectors(Vec3::X, 2.0 * Vec3::Y, &[Vec3::new(3.0, 4.0, 5.0)]).unwrap();
        assert_eq!(r, vec![Vec2::new(3.0, 2.0)]);
        let r = drop_vectors(Vec3::ZERO, Vec3::Y, &[Vec3::X, Vec3::Y]).unwrap();
        assert_eq!(r, vec![Vec2::new(0.0, 0.0); 2]);
        assert!(drop_vectors(Vec3::X, 2.0 * Vec3::X, &[Vec3::Y]).is_err());
    }

    #[test]
    fn surface_dispatch() {
        let k = &Kernel::default();
        let mut s = bilinear();
        assert_eq!(s.type_code(), B_SPLINE_SURFACE_TYPE);
        assert!(s.is_kind_of(TYPE).unwrap());
        assert!(s.owner().is_none());
        let d = s.natural_uv_domain_if_built();
        assert_eq!(s.step_uv_domain().min(), d.min());
        assert_eq!(s.degree(SurfParamType::U), 1);
        let p = s.evaluate_point(k, Point2::new(1.0, 1.0)).unwrap();
        assert!((p - Vec3::new(2.0, 3.0, 1.0)).length() < 1e-12);
        let ev = s
            .evaluate(k, Point2::new(0.5, 0.5), 1, 1, EvaluateFlags::default())
            .unwrap();
        assert_eq!(ev.len(), 2);
        assert!(s
            .drop_point_fast::<ObjectRef2>(
                &Kernel::default(),
                &d,
                SolverOperation::Minimize,
                Vec3::ZERO,
                None,
                0.0,
                None,
                SolutionRequested::All
            )
            .is_err());
        s.swap_uv(&Kernel::default()).unwrap();
        let (a, b) = s
            .split_at(&Kernel::default(), 0.5, SurfParamType::U)
            .unwrap();
        assert!(a.natural_uv_domain_if_built().u_max() <= 0.5 + 1e-12);
        assert!(b.natural_uv_domain_if_built().u_min() >= 0.5 - 1e-12);
        assert_eq!(
            s.convert_uv_from_step_to_nurbs(&Kernel::default(), Point2::new(0.1, 0.2))
                .unwrap(),
            Point2::new(0.1, 0.2)
        );
        assert!(base_copy().is_err());
        assert_eq!(base_degree(), 1001);
        assert_eq!(base_natural_uv_domain().max(), Point2::new(0.0, 0.0));
    }

    #[test]
    fn read_from_db_unported_types() {
        let mut db = DatabaseReader::default();
        // the offset surface and the sphere are both read now (#62, #100): an empty
        // stream fails in their readers, not with Unported
        for ty in [OFFSET_SURFACE_TYPE, SPHERE_TYPE] {
            let e = Surface::read_from_db(&Kernel::default(), ty, &mut db, 1).unwrap_err();
            assert!(!e.is_unported());
        }
        // a plane is read (an empty stream fails without Unported)
        let e = Surface::read_from_db(&Kernel::default(), PLANE_TYPE, &mut db, 1).unwrap_err();
        assert!(!e.is_unported());
        assert!(
            !Surface::read_from_db(&Kernel::default(), 12345, &mut db, 1)
                .unwrap_err()
                .is_unported()
        );
    }

    #[test]
    fn surface_eval_is_lazy() {
        let k = &Kernel::default();
        let s = bilinear();
        let mut ev = SurfaceEval::with_uv(k, Some(&s), 0.5, 0.5);
        assert_eq!(ev.flags(), [false; 4]);
        ev.eval().unwrap();
        assert!(ev.flags()[0]);
        assert!(!ev.values()[2][2].is_initialized());
        ev.set_uv_coords(0.5, 0.5);
        assert!(ev.flags()[0]);
        ev.set_uv(Point2::new(0.25, 0.5));
        assert!(!ev.flags()[0]);
        let mut none = SurfaceEval::new(k, None);
        assert!(none.eval().is_err());
    }

    #[test]
    fn deriv_surf_definition_checks() {
        let d = DerivSurfDefinition::default();
        let mut list = AssertArray::new();
        assert!(d.assert_valid(Some(&mut list), ObjectKey(1)).unwrap());
        assert!(list.is_empty());
        assert!(d.is_from_surface_boundary_curve());
        assert_eq!(d.num_derivs(), 2);
        let bad = DerivSurfDefinition {
            num_samples: 0,
            start_angle_deg: 400.0,
            curvature_type: CurvatureType::G2FromCurve,
            ..DerivSurfDefinition::default()
        };
        assert!(!bad.assert_valid(Some(&mut list), ObjectKey(1)).unwrap());
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn cross_sections() {
        let d = DerivSurfDefinition::default();
        let pos = [Vec3::ZERO, Vec3::X];
        let d1 = [Vec3::Z, Vec3::Z];
        let v = CrossSectionValues {
            start_pos: &pos,
            start_1st_derivs: &d1,
            ..CrossSectionValues::default()
        };
        let c = d.create_cross_section_curves(&v).unwrap();
        assert_eq!(c.len(), 2);
        let bad = CrossSectionValues {
            start_pos: &pos,
            start_1st_derivs: &d1[..1],
            ..CrossSectionValues::default()
        };
        assert!(d.create_cross_section_curves(&bad).is_err());
    }

    #[test]
    fn drop_curve_fail_record() {
        let mut f: DropCurveFail<ObjectRef2> = DropCurveFail::default();
        assert!(f.is_un_init());
        let knots = [0.0, 1.0];
        f.set(&DropCurveFailArgs {
            fail_status: 1001,
            from_call: 1,
            knots: Some(&knots),
            ..DropCurveFailArgs::default()
        });
        assert!(!f.is_un_init());
        assert_eq!(f.knots, knots.to_vec());
        let mut g = DropCurveFail::default();
        g.assign(&f);
        assert_eq!(g.fail_status, 1001);
        f.un_init();
        assert!(f.is_un_init() && f.knots.is_empty());
    }

    #[test]
    fn lsi_branch_test() {
        let mut tree: Tree<ObjectRef2> = Tree::new_spatial(&Extent3d::from_points(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ));
        let node = TreeNode {
            bbox: Extent3d::from_points(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0)),
            ..tree.nodes()[0]
        };
        let id: NodeId = tree.add_node(node);
        let mut solver = LsiGlobalSolver {
            base: GlobalSolver::with_tree(&tree, 3),
            surface: None,
            uv_domain: Extent2d::new(),
            line_point: Vec3::new(0.5, 0.5, -5.0),
            line_vector: Vec3::Z,
            line_interval: None,
            ray_fire: false,
            distance_tolerance: 1e-6,
            surface_cache: None,
            face_queries: None,
            face_cache: None,
            kernel: &Kernel::default(),
        };
        let mut branch: Branch = Default::default();
        branch[0] = Some(id);
        assert!(solver.may_contain_answers(&branch).unwrap());
        solver.line_point = Vec3::new(5.0, 5.0, 0.0);
        assert!(!solver.may_contain_answers(&branch).unwrap());
        // no surface cache to read the node patch from
        assert!(solver.local_solve(&branch).is_err());
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct ObjectRef2;
    impl SolutionObject for ObjectRef2 {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Surface
        }
    }
}

#[cfg(test)]
mod curve_on_classify_tests {
    use super::*;
    use crate::intersect::point_class::ClassObjectKind;
    use crate::surface::volume::Volume;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum H {
        Curve,
        Surface,
        Temporary(usize, ClassObjectKind),
    }
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Curve => ClassObjectKind::Curve,
                Self::Surface => ClassObjectKind::Surface,
                Self::Temporary(_, kind) => *kind,
            }
        }
        fn temporary(index: usize, kind: ClassObjectKind) -> Option<Self> {
            Some(Self::Temporary(index, kind))
        }
    }
    struct G(Curve);
    impl ClassGeometry<H> for G {
        fn curve(&self, o: H) -> Option<&Curve> {
            (o == H::Curve).then_some(&self.0)
        }
        fn surface(&self, _: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _: H) -> Option<Point3> {
            None
        }
    }

    fn classify(from: Vec3, to: Vec3) -> CurveClassification<H> {
        let k = Kernel::default();
        let g = G(Line::create_line_segment(3, from, to, None).unwrap().into());
        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(H::Curve), false);
        cc.set_size(&k, 1).unwrap();
        cc.at_mut(0)
            .unwrap()
            .set_interval(&Extent1d::from_min_max(0., 1.));
        cc.set_src_zone_tol_3d(1e-7);
        let plane = Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1., 1.),
            &Extent2d::from_bounds(0., 0., 1., 1.),
        ));
        plane
            .curve_on_classify(&k, H::Surface, &g, &mut cc)
            .unwrap();
        cc
    }

    #[test]
    fn inside_curve_ends_take_the_mid_classification() {
        let cc = classify(Vec3::new(0.2, 0.2, 0.), Vec3::new(0.8, 0.8, 0.));
        assert_eq!(cc.size(), 1);
        assert_eq!(cc.surface1(), Some(H::Surface));
        let ivl = cc.at(0).unwrap();
        assert_eq!(ivl.mid.point_class(), PointClassType::Surface);
        assert_eq!(ivl.start.point_class(), PointClassType::Surface);
        assert_eq!(ivl.end.point_class(), PointClassType::Surface);
        assert_eq!(ivl.start.object(), Some(H::Surface));
    }
}

#[cfg(test)]
mod owner_zone_tol_tests {
    use super::*;
    use crate::geom::assert_array::labels::FACE_TYPE;
    use crate::geom::attribute::ObjectKey;
    use crate::surface::offset_surface::OffsetSurface;
    use crate::topology::{Brep, Face, FaceId};

    fn face_ref(key: u64) -> ObjectRef {
        ObjectRef {
            type_code: FACE_TYPE,
            key: ObjectKey(key),
        }
    }

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(Point3::ZERO, Vec3::Z))
    }

    /// A B-rep with one face of tolerance `tol` holding `surface`.
    fn brep_with(surface: Surface, tol: f64) -> (Brep, FaceId) {
        let mut b = Brep::new();
        let shell = b.add_shell().unwrap();
        let f = b
            .add_face_record(Face {
                surface: Some(surface),
                tolerance: tol,
                ..Face::new(shell)
            })
            .unwrap();
        (b, f)
    }

    fn surface_of(b: &Brep, f: FaceId) -> &Surface {
        b.face(f).unwrap().surface.as_ref().unwrap()
    }

    #[test]
    fn add_face_record_caches_the_face_tolerance() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        assert!(s.owner_zone_tol_3d().is_none());
        assert!(s.face_zone_tol_3d().unwrap_err().is_unported());
        let (b, f) = brep_with(s, 0.25);
        let s = surface_of(&b, f);
        assert_eq!(s.owner_zone_tol_3d(), Some(0.25));
        assert_eq!(s.face_zone_tol_3d().unwrap(), Some(0.25));
        let k = Kernel::default();
        assert_eq!(
            crate::surface::surface::p1::zone_tol_3d(&k, s).unwrap(),
            0.25
        );
    }

    #[test]
    fn unowned_surface_records_nothing() {
        let (b, f) = brep_with(plane(), 0.25);
        let s = surface_of(&b, f);
        assert_eq!(s.owner_zone_tol_3d(), None);
        assert_eq!(s.face_zone_tol_3d().unwrap(), None);
    }

    #[test]
    fn owner_changes_drop_the_cache() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        let (mut b, f) = brep_with(s, 0.25);
        let s = b.face_mut(f).unwrap().surface.as_mut().unwrap();
        // the same owner keeps it
        s.set_owner(Some(face_ref(1)));
        assert_eq!(s.owner_zone_tol_3d(), Some(0.25));
        // another face drops it
        s.set_owner(Some(face_ref(2)));
        assert_eq!(s.owner_zone_tol_3d(), None);
        assert!(s.face_zone_tol_3d().unwrap_err().is_unported());
        // and so does no owner, also when the face comes back
        b.sync_face_surface_tolerance(f).unwrap();
        let s = b.face_mut(f).unwrap().surface.as_mut().unwrap();
        assert_eq!(s.owner_zone_tol_3d(), Some(0.25));
        s.set_owner(None);
        assert_eq!(s.owner_zone_tol_3d(), None);
        s.set_owner(Some(face_ref(2)));
        assert_eq!(s.owner_zone_tol_3d(), None);
    }

    #[test]
    fn direct_owner_writes_cannot_leak_the_cache() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        s.set_owner_zone_tol_3d(Some(0.5));
        *s.owner_mut() = Some(face_ref(2));
        assert_eq!(s.owner_zone_tol_3d(), None);
        *s.owner_mut() = None;
        assert_eq!(s.owner_zone_tol_3d(), None);
    }

    #[test]
    fn evaluation_copy_keeps_and_clone_resets() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        s.set_owner_zone_tol_3d(Some(0.5));
        let e = s.clone_for_evaluation().unwrap();
        assert_eq!(e.owner_zone_tol_3d(), Some(0.5));
        let c = s.clone();
        assert_eq!(c.owner(), None);
        assert_eq!(c.owner_zone_tol_3d(), None);
        let kept = s.clone_keeping_owner();
        assert_eq!(kept.owner_zone_tol_3d(), Some(0.5));
    }

    #[test]
    fn owned_offset_base_reaches_the_cache() {
        let k = Kernel::default();
        let mut off = Surface::from(OffsetSurface::new(&k, 1.0, plane(), true, 0).unwrap());
        off.set_owner(Some(face_ref(1)));
        let base = |s: &Surface| match s.kind() {
            SurfaceKind::OffsetSurface(o) => o.base_surface().owner_zone_tol_3d(),
            _ => panic!("not an offset"),
        };
        assert_eq!(base(&off), None);
        off.set_owner_zone_tol_3d(Some(0.125));
        assert_eq!(off.owner_zone_tol_3d(), Some(0.125));
        assert_eq!(base(&off), Some(0.125));
        off.set_owner(None);
        assert_eq!(base(&off), None);
    }

    /// The offset paths #307 rewires through the exact snapshot helpers: an owned base
    /// keeps the cached face tolerance in an evaluation copy, and a shared base (the same
    /// object upstream) keeps its own face's tolerance in `OffsetSurface::copy`.
    #[test]
    fn offset_snapshots_keep_the_cache() {
        let k = Kernel::default();
        let base = |s: &Surface| match s.kind() {
            SurfaceKind::OffsetSurface(o) => o.base_surface().owner_zone_tol_3d(),
            _ => panic!("not an offset"),
        };
        let mut owned = Surface::from(OffsetSurface::new(&k, 1.0, plane(), true, 0).unwrap());
        owned.set_owner(Some(face_ref(1)));
        owned.set_owner_zone_tol_3d(Some(0.125));
        let e = owned.clone_for_evaluation().unwrap();
        assert_eq!(e.owner_zone_tol_3d(), Some(0.125));
        assert_eq!(base(&e), Some(0.125));

        let mut shared_base = plane();
        shared_base.set_owner(Some(face_ref(2)));
        shared_base.set_owner_zone_tol_3d(Some(0.75));
        let shared = OffsetSurface::new(&k, 1.0, shared_base, false, 0).unwrap();
        let c = shared.copy(&k).unwrap();
        assert_eq!(c.base_surface().owner_zone_tol_3d(), Some(0.75));
        let e = Surface::from(shared).clone_for_evaluation().unwrap();
        assert_eq!(base(&e), Some(0.75));
    }

    #[test]
    fn face_set_tolerance_updates_the_cache() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        let (mut b, f) = brep_with(s, 0.25);
        crate::topology::face::p3::set_tolerance(&mut b, f, 0.5, true, false).unwrap();
        assert_eq!(surface_of(&b, f).owner_zone_tol_3d(), Some(0.5));
        // only if larger: unchanged
        crate::topology::face::p3::set_tolerance(&mut b, f, 0.1, true, false).unwrap();
        assert_eq!(surface_of(&b, f).owner_zone_tol_3d(), Some(0.5));
        crate::topology::face::p3::set_tolerance(&mut b, f, 0.1, false, false).unwrap();
        assert_eq!(surface_of(&b, f).owner_zone_tol_3d(), Some(0.1));
    }

    #[test]
    fn rescue_surface_owner_restores_the_cache() {
        let mut s = plane();
        s.set_owner(Some(face_ref(1)));
        s.set_owner_zone_tol_3d(Some(0.5));
        let rescue = crate::topology::face::p2::RescueSurfaceOwner::new(&s);
        s.set_owner(None);
        assert_eq!(s.owner_zone_tol_3d(), None);
        rescue.restore(&mut s);
        assert_eq!(s.owner_zone_tol_3d(), Some(0.5));
    }
}

#[cfg(test)]
mod kernel_dispatch_tests {
    use super::*;
    use crate::curve::iso_curve::IsoCurve;
    use crate::surface::surface::p1::{evaluate_1st_derivatives, CsIntersectEnfo};
    use crate::topology::database_io::{FileType, MAX_READ_NESTING};

    #[test]
    fn analytic_evaluation_keeps_the_callers_context_and_class() {
        let k = &Kernel::default();
        let mut kernel = Kernel::default();
        kernel.tolerances.zone_tol_3d = 0.002;
        for cylinder in [false, true] {
            let cone = if cylinder {
                Cone::create_canonical_cylinder(&kernel, &Axis2Placement::IDENTITY, 2.0).unwrap()
            } else {
                Cone::create_canonical(
                    &kernel,
                    &Axis2Placement::IDENTITY,
                    2.0,
                    4.0,
                    3.0,
                    false,
                    true,
                )
                .unwrap()
            };
            let surface = Surface::from(cone.clone());
            assert_eq!(
                surface.type_code(),
                if cylinder { CYLINDER_TYPE } else { CONE_TYPE }
            );
            let uv = surface.natural_uv_domain_if_built().evaluate(0.25, 0.5);
            let expected = cone.evaluate_point(&kernel, uv).unwrap();
            assert_eq!(surface.evaluate_point(&kernel, uv).unwrap(), expected);
            assert_eq!(surface.copy(k).unwrap().type_code(), surface.type_code());
            assert!(matches!(
                surface.copy(k).unwrap().kind(),
                SurfaceKind::Cone(_)
            ));
            let mut lazy = SurfaceEval::with_uv(&kernel, Some(&surface), uv.x, uv.y);
            assert!(core::ptr::eq(lazy.kernel, core::ptr::from_ref(&kernel)));
            assert_eq!(lazy.pos().unwrap(), expected);
            let curve = Curve::from(Line::from_points(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 3));
            let callback = CsIntersectEnfo::new(&kernel, &curve, &surface, 1e-12);
            assert!(core::ptr::eq(callback.kernel, core::ptr::from_ref(&kernel)));
            let derivatives =
                evaluate_1st_derivatives(&kernel, &surface, uv, Side::Left, Side::Left).unwrap();
            assert_eq!(derivatives.0, expected);
            let iso = IsoCurve::from_surface(
                &kernel,
                surface.clone(),
                SurfParamType::U,
                uv.x,
                true,
                true,
            )
            .unwrap();
            assert_eq!(
                Curve::from(iso).evaluate_point(&kernel, uv.y).unwrap(),
                expected
            );
            // Part 9 builds the boundaries through the cone's own iso curves.
            assert!(surface
                .create_iso_boundaries(&kernel, SurfParamType::U, 0.0)
                .unwrap()
                .is_some());
            let step = StepSurface::new(surface, true).unwrap();
            let step_uv = Point2::new(90.0, 1.0);
            assert_eq!(
                step.evaluate_point(&kernel, step_uv).unwrap(),
                cone.evaluate_step_point(step_uv).unwrap()
            );
        }
    }

    #[test]
    fn evaluation_and_read_errors_keep_resource_bounds() {
        let kernel = Kernel::default();
        let cone = Cone::create_canonical(
            &kernel,
            &Axis2Placement::IDENTITY,
            2.0,
            4.0,
            3.0,
            false,
            false,
        )
        .unwrap();
        let mut surface = Surface::from(cone);
        assert!(surface
            .evaluate(
                &kernel,
                Point2::new(0.5, 0.5),
                usize::MAX,
                usize::MAX,
                EvaluateFlags::default()
            )
            .is_err());
        // Wrapper constructors refuse to nest past MAX_GEOMETRY_NESTING, so a
        // 70-deep chain cannot be built; the deepest buildable chains still
        // evaluate and copy without overflowing.
        let mut bounded = surface.clone();
        let mut levels = 0;
        loop {
            match CurveBoundedSurface::new(bounded.clone(), Vec::new(), Vec::new(), true) {
                Ok(b) => bounded = Surface::from(b),
                Err(e) => {
                    assert!(matches!(e, SolidError::InvalidInput { .. }));
                    break;
                }
            }
            levels += 1;
            assert!(levels <= MAX_GEOMETRY_NESTING);
        }
        let _ = bounded.evaluate_simple(
            &kernel,
            Point2::new(0.5, 0.5),
            0,
            0,
            EvaluateFlags::default(),
        );
        let mut levels = 0;
        loop {
            match StepSurface::new(surface.clone(), true) {
                Ok(s) => surface = Surface::from(s),
                Err(e) => {
                    assert!(matches!(e, SolidError::InvalidInput { .. }));
                    break;
                }
            }
            levels += 1;
            assert!(levels <= MAX_GEOMETRY_NESTING);
        }
        let _ = surface.evaluate_point(&kernel, Point2::new(0.5, 0.5));
        let _ = surface.evaluate_step_point(&kernel, Point2::new(0.5, 0.5));
        let _ = surface.copy(&kernel);
        let _ = surface.copy_and_add_analytics(&kernel);
        let mut reader = DatabaseReader::new();
        reader.open_for_read(b"2\n0\n0\n".to_vec(), FileType::Ascii);
        for _ in 0..128 {
            assert!(Surface::read_from_db(&kernel, CONE_TYPE, &mut reader, 30).is_err());
        }
        for _ in 0..MAX_READ_NESTING {
            reader.enter_nested("test").unwrap();
        }
        assert!(reader.enter_nested("test").is_err());
        for _ in 0..MAX_READ_NESTING {
            reader.leave_nested();
        }
    }
}

#[cfg(test)]
mod issue_892_tests {
    use crate::curve::curve::Curve;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::kernel::Kernel;
    use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};
    use crate::math::{Point3, Vec3};
    use crate::primitive::api_curves::create_line_segment;
    use crate::sweep::api_brep::create_linear_sweep;

    /// A 10 x 6 plate with a triangular hole whose corner touches the outline at
    /// (0, 3), extruded 2 up, is the plate less the hole with the opt-in (WR-405):
    /// volume 2 (60 - 1), ten faces.
    #[test]
    fn issue_892_hole_touching_outline_is_one_loop() {
        let points = |p: &[(f64, f64)]| -> Vec<Curve> {
            p.iter()
                .zip(p.iter().cycle().skip(1))
                .map(|(&(ax, ay), &(bx, by))| {
                    Curve::from(
                        create_line_segment(Point3::new(ax, ay, 0.0), Point3::new(bx, by, 0.0))
                            .unwrap(),
                    )
                })
                .collect()
        };
        let mut curves = points(&[(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0), (0.0, 3.0)]);
        curves.extend(points(&[(0.0, 3.0), (2.0, 2.5), (2.0, 3.5)]));
        let refs: Vec<Option<&Curve>> = curves.iter().map(Some).collect();

        let mut k = Kernel::default();
        k.fixes.join_touching_loops = true;
        // the plane's loops: the hole joined into the outline, one loop of 8 curves
        let plane =
            crate::surface::surface::Surface::create_plane_with_3d_curves(&k, &curves, 1e-6)
                .unwrap();
        assert_eq!(
            (plane.outer_loop_counts.clone(), plane.loop_counts.clone()),
            (vec![1], vec![8])
        );
        let mut plate =
            create_linear_sweep(&mut k, &refs, Vec3::new(0.0, 0.0, 1.0), 2.0, true).unwrap();
        let faces = crate::topology::brep::p1::faces(&plate, None)
            .unwrap()
            .len();
        assert_eq!(faces, 10);
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let volume = brep_volume(&k, &mut plate, 1e-6, &attributes).unwrap();
        assert!((volume - 118.0).abs() < 1e-6, "volume {volume}");
    }
}

#[cfg(test)]
mod made_kind_tests {
    use super::*;
    use crate::math::{Point3, Vec3};
    use crate::surface::torus::TorusOptions;

    /// A torus without NURB (the constructor makes none), inside out so that the made
    /// kind must keep the flag the copy constructor resets.
    fn lazy_torus(k: &Kernel) -> Surface {
        let mut t = Torus::new(
            k,
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent2d::from_bounds(0.0, 0.0, 270.0, 180.0),
            5.0,
            1.0,
            TorusOptions {
                swap_uv: false,
                inside_out: false,
                circle_nurb: None,
            },
        )
        .unwrap();
        t.inside_out = true;
        Surface::from(t)
    }

    fn torus_of(s: &Surface) -> &Torus {
        let SurfaceKind::Torus(t) = s.kind() else {
            panic!("a torus");
        };
        t
    }

    #[test]
    fn a_const_query_keeps_the_made_kind_on_the_surface() {
        let k = Kernel::default();
        let s = lazy_torus(&k);
        assert!(torus_of(&s).base().base().gw_nurb().is_none());
        let eval_copy = s.clone_for_evaluation().unwrap();

        // GetNaturalUVDomain makes the NURB (GetOrCreateGwNurbPointer) and keeps it
        let domain = s.natural_uv_domain_with_nurb(&k).unwrap();
        assert!(s.has_made_nurb());
        let made = torus_of(&s);
        assert!(
            made.base().base().gw_nurb().is_some(),
            "kind() sees the NURB"
        );
        assert!(
            made.base().have_polar_conversion(),
            "and the rebuilt converter"
        );
        assert!(made.inside_out, "the made kind is an exact copy");
        assert_eq!(s.natural_uv_domain_if_built(), domain);

        // the made object is the one an in-place MakeNurb gives
        let mut in_place = lazy_torus(&k);
        let SurfaceKind::Torus(t) = in_place.kind_mut() else {
            panic!("a torus");
        };
        t.make_nurb(&k).unwrap();
        assert!(
            s.approx_eq(&k, &in_place).unwrap(),
            "operator== sees the NURB"
        );
        assert_eq!(in_place.natural_uv_domain_if_built(), domain);

        // the evaluation copy taken before stands for the same object
        assert!(eval_copy.shares_made_kind_with(&s));
        assert!(torus_of(&eval_copy).base().base().gw_nurb().is_some());

        // the copy constructor copies the made object
        let copy = s.clone();
        assert!(!copy.shares_made_kind_with(&s));
        assert!(torus_of(&copy).base().base().gw_nurb().is_some());
    }

    #[test]
    fn an_edit_detaches_the_made_kind_from_evaluation_copies() {
        let k = Kernel::default();
        let s = lazy_torus(&k);
        let mut edited = s.clone_for_evaluation().unwrap();
        s.kind_with_nurb(&k).unwrap();
        assert!(edited.shares_made_kind_with(&s));

        // the edit applies to the made object (it keeps its NURB) on a cell of its own
        let SurfaceKind::Torus(t) = edited.kind_mut() else {
            panic!("a torus");
        };
        assert!(t.base().base().gw_nurb().is_some());
        t.inside_out = false;
        assert!(!edited.shares_made_kind_with(&s));
        assert!(!edited.has_made_nurb());
        assert!(torus_of(&s).inside_out, "the original is untouched");
        assert!(s.has_made_nurb());
    }

    #[test]
    fn copy_makes_the_source_nurb_first() {
        let k = Kernel::default();
        let s = lazy_torus(&k);
        let copy = s.copy(&k).unwrap();
        assert!(s.has_made_nurb(), "the source keeps the NURB the copy made");
        assert!(torus_of(&copy).base().base().gw_nurb().is_some());
    }
}

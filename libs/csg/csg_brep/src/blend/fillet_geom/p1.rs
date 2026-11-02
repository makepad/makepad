//! Fillet geometry, part 1: the fillet vertex use, fillet vertex, fillet edge use,
//! fillet edge, fillet geometry and pseudo B-rep records with their accessors, and the
//! static helpers of `SmFilletGeom.cpp` (surface point drops, two-point surface/surface
//! intersection, seam handling, end-point/tangent interpolation, capping, rail/B-rep
//! edge matching, fillet-surface boundary repair).
//!
//! Upstream: `source/SMLib/inc/SmFilletGeom.h`, `source/SMLib/src/SmFilletGeom.cpp`.
//!
//! | Rust                                            | Upstream                                   |
//! |-------------------------------------------------|--------------------------------------------|
//! | [`FilletErrorType`], [`FilletCornerType`], [`FilStatus`], [`FilletVertexType`], [`BoundaryTrimmingType`], [`FilletEdgeType`], [`FilletGeomType`] | the enums of `SmFilletGeom.h` |
//! | [`FilletBrep`]                                  | `SmFilletBrep` (`SmFilletBrep()` CO-PORTED from `blend.fillet_geom_p6`; `~SmFilletBrep` is `Drop`) |
//! | [`FilletBrep::add_edge`], [`FilletBrep::remove_edge`] | `SmFilletBrep::AddEdge`, `RemoveEdge`  |
//! | [`CornerLookup`]                                | a vertex's / edge's `GetFilletCorner()` dereferenced (the executive's `m_vFilletCorners`, or the running corner's `this`) and the executive's `m_vExtendedSurfacesMap` |
//! | [`FilletVertexuse`], [`FilletVertexuse::new`]   | `SmFilletVertexuse` members, `SmFilletVertexuse()` (`~SmFilletVertexuse` is `Drop`) |
//! | [`FilletVertexuse::tsect_pnt`], [`FilletVertexuse::tsect_pnt_mut`], [`FilletVertexuse::uv_point`], [`FilletVertexuse::set_uv_point`], [`FilletVertexuse::set_tsect_pnt`] | `GetTsectPnt`, `GetUVPoint`, `SetUVPoint`, `SetTsectPnt` |
//! | [`FilletBrep::set_vertexuse_property`]          | `SmFilletVertexuse::SetProperty`           |
//! | [`fillet_vertexuse_is_kind_of`], [`FILLET_VERTEXUSE_TYPE`], [`FILLET_VERTEXUSE_CLASS_STRING`], [`FILLET_VERTEXUSE_TYPE_STRING`] | `SmFilletVertexuse::IsKindOf`, `GetType`, `GetClassString`, `GetTypeString` |
//! | [`FilletVertex`], [`FilletVertex::new`]         | `SmFilletVertex` members, `SmFilletVertex(SmFilletCorner*)` (CO-PORTED from `blend.fillet_geom_p2`) |
//! | [`FilletBrep::delete_fillet_vertex`]            | `SmFilletVertex::~SmFilletVertex`          |
//! | [`FilletVertex::copy_point_class`], [`FilletVertex::mate`], [`FilletVertex::status`], [`FilletVertex::point_classification`], [`FilletVertex::point_classification_mut`], [`FilletVertex::original_t_param`], [`FilletVertex::original_uv`], [`FilletVertex::point_class`], [`FilletVertex::point_class_object`], [`FilletVertex::fillet_corner`], [`FilletVertex::fillet_vertex_type`], [`FilletVertex::is_processed`] | `CopyPointClass`, `GetMate`, `GetStatus`, `GetPointClassification`, `GetOriginalTParam`, `GetOriginalUV`, `GetPointClass`, `GetPointClassObject`, `GetFilletCorner`, `GetFilletVertexType`, `IsProcessed` |
//! | [`FilletBrep::vertex_point`]                    | `SmFilletVertex::GetPoint`                 |
//! | [`FilletVertex::set_mate`], [`FilletVertex::set_status`], [`FilletVertex::set_fillet_corner`], [`FilletVertex::set_fillet_vertex_type`], [`FilletVertex::set_original_t_param`], [`FilletVertex::set_original_uv`], [`FilletVertex::set_point_class`] | `SetMate`, `SetStatus`, `SetFilletCorner`, `SetFilletVertexType`, `SetOriginalTParam`, `SetOriginalUV`, `SetPointClass` |
//! | [`FilletBrep::calc_vert_on_vert`]               | `SmFilletVertex::CalcVertOnVert`           |
//! | [`FilletBrep::calc_cliff_rail_int_edge`]        | `SmFilletVertex::CalcCliffRailIntEdge`     |
//! | [`FilletBrep::update_split_face`]               | `SmFilletVertex::UpdateSplitFace`          |
//! | [`FilletBrep::vertex_fillet_geoms`]             | `SmFilletVertex::GetFilletGeoms`           |
//! | [`FilletBrep::vu_at_rail_end`]                  | `SmFilletVertex::GetVUAtRailEnd`           |
//! | [`fillet_vertex_is_kind_of`], [`FILLET_VERTEX_TYPE`], [`FILLET_VERTEX_CLASS_STRING`], [`FILLET_VERTEX_TYPE_STRING`] | `SmFilletVertex::IsKindOf`, `GetType`, `GetClassString`, `GetTypeString` |
//! | [`FilletEdgeuse`], [`FilletBrep::new_fillet_edgeuse`] | `SmFilletEdgeuse` members, `SmFilletEdgeuse(SmFilletBrep*)` (`~SmFilletEdgeuse` is `Drop`) |
//! | [`FilletEdgeuse::fillet_geom`], [`FilletBrep::set_edgeuse_fillet_geom`], [`FilletBrep::set_edgeuse_fillet_vertexuse`], [`FilletBrep::set_edgeuse_uv_curve`] | `GetFilletGeom`, `SetFilletGeom`, `SetFilletVertexuse`, `SetUVCurve` |
//! | [`fillet_edgeuse_is_kind_of`], [`FILLET_EDGEUSE_TYPE`], [`FILLET_EDGEUSE_CLASS_STRING`], [`FILLET_EDGEUSE_TYPE_STRING`] | `SmFilletEdgeuse::IsKindOf`, `GetType`, `GetClassString`, `GetTypeString` |
//! | [`FilletEdge`], [`FilletBrep::new_fillet_edge`] | `SmFilletEdge` members, `SmFilletEdge(SmFilletBrep*, SmFilletCorner*)` (CO-PORTED from `blend.fillet_geom_p2`) |
//! | [`FilletBrep::delete_fillet_edge`]              | `SmFilletEdge::~SmFilletEdge`              |
//! | [`FilletEdge::curve_class`], [`FilletEdge::fillet_brep_edge`], [`FilletEdge::fillet_brep_edges`], [`FilletEdge::fillet_corner`], [`FilletEdge::fillet_edge_type`], [`FilletEdge::name`], [`FilletEdge::original_edge`], [`FilletEdge::original_face`], [`FilletEdge::split_faces`], [`FilletEdge::status`] | `GetCurveClass`, `GetFilletBrepEdge`, `GetFilletBrepEdges`, `GetFilletCorner`, `GetFilletEdgeType`, `GetName`, `GetOriginalEdge`, `GetOriginalFace`, `GetSplitFaces`, `GetStatus` |
//! | [`FilletBrep::original_uv_curve`]               | `SmFilletEdge::GetOriginalUVCurve`         |
//! | [`FilletEdge::set_curve_class`], [`FilletEdge::set_fillet_edge_type`], [`FilletEdge::set_fillet_corner`], [`FilletEdge::set_original_edge`], [`FilletEdge::set_original_face`], [`FilletEdge::add_split_face`], [`FilletEdge::set_status`] | `SetCurveClass`, `SetFilletEdgeType`, `SetFilletCorner`, `SetOriginalEdge`, `SetOriginalFace`, `AddSplitFace`, `SetStatus` |
//! | [`FilletEdge::clear_fillet_brep_edges`]         | `SmFilletEdge::ClearFilletBrepEdges`       |
//! | [`FilletBrep::find_fillet_brep_edge`]           | `SmFilletEdge::FindFilletBrepEdge`         |
//! | [`fillet_edge_is_kind_of`], [`FILLET_EDGE_TYPE`], [`FILLET_EDGE_CLASS_STRING`], [`FILLET_EDGE_TYPE_STRING`] | `SmFilletEdge::IsKindOf`, `GetType`, `GetClassString`, `GetTypeString` |
//! | [`FilletGeom`], [`FilletGeom::new`]             | `SmFilletGeom` members, `SmFilletGeom(SmFilletSolver*, SmFilletGeomType)` (CO-PORTED from `blend.fillet_geom_p5`) |
//! | [`FilletGeom::add_referenced_fillet_edgeuse`], [`FilletGeom::add_side_fillet_edgeuse`] | `AddReferencedFilletEdgeuse`, `AddSideFilletEdgeuse` |
//! | [`FilletGeom::fillet_geom_type`], [`FilletGeom::fillet_solver`], [`FilletGeom::status`], [`FilletGeom::split_flag`], [`FilletGeom::fillet_edges`], [`FilletGeom::center_line_curve`], [`FilletGeom::center_line_geometry`], [`FilletGeom::fillet_surface`], [`FilletGeom::rail`] | `GetFilletGeomType`, `GetFilletSolver`, `GetStatus`, `GetSplitFlag`, `GetFilletEdges`, `GetCenterLineCurve`, `GetFilletSurface`, `GetRail` |
//! | [`FilletGeom::offset_surface`], [`FilletGeom::set_offset_surface`] | `GetOffsetSurface`, `SetOffsetSurface` |
//! | [`FilletSurfaceOwner`], [`FilletSurfaceStorage`] | the B-rep owning `m_pFilletSurface` once `MakeFaceBrep` transferred it to a face |
//! | [`FilletBrep::rail_vertexuse`]                  | `SmFilletGeom::GetRailVertexuse`           |
//! | [`FilletGeom::set_fillet_surface`], [`FilletGeom::set_fillet_geom_type`], [`FilletGeom::set_rail`] | `SetFilletSurface`, `SetFilletGeomType`, `SetRail` |
//! | [`FilletBrep::trim_geom`]                       | `SmFilletGeom::Trim`                       |
//! | [`evaluate_mate`]                               | `sm_EvaluateMate`                          |
//! | [`surface_drop_point`]                          | `sm_SurfaceDropPoint`                      |
//! | [`two_pnts_intersection`]                       | `sm_TwoPntsIntersection`                   |
//! | [`SurfaceHandle`] (private `CliffHandle` for `CalcCliffRailIntEdge`) | the surface object `SmSolution` names in a surface point solve |
//! | [`find_stepoff_uv`]                             | `sm_FindStepoffUV`                         |
//! | [`classify_face_point`]                         | `sm_ClassifyFacePoint`                     |
//! | [`interpolate_endpts_and_tangents`]             | `sm_InterpolateEndptsAndTangents`          |
//! | [`adjust_uv_on_seam`]                           | `sm_AdjustUVOnSeam`                        |
//! | [`create_cubic_rail_rail_interpolant`]          | `sm_CreateCubicRailRailInterpolant`        |
//! | [`cleanup_noise`], [`clean_up_noise`]           | `sm_CleanupNoise`, `smos_CleanUpNoise` (`SmMath.cpp`, no owning unit) |
//! | [`examine_n_fix_fillet_geoms`], [`examine_n_fix_fillet_geoms_with_solvers`] | `sm_ExamineNFixFilletGeoms` |
//! | [`join_verts_by_capping`]                       | `JoinVertsByCapping`                       |
//! | [`check_seam_out`], [`check_seam_on`]           | `sm_CheckSeamOut`, `sm_CheckSeamOn`        |
//! | [`is_order_correct`]                            | `sm_IsOrderCorrect`                        |
//! | [`set_surface_closure`]                         | `sm_SetSurfaceClosure`                     |
//! | [`find_mark_for_context2`]                      | `sm_FindMarkForContext2`                   |
//!
//! Deviations:
//! - **Object model.** Upstream's `SmFilletVertexuse`, `SmFilletVertex`,
//!   `SmFilletEdgeuse` and `SmFilletEdge` derive from the topology classes and live in
//!   the pseudo B-rep `SmFilletBrep` (an `SmBrep`). Here [`FilletBrep`] owns a
//!   [`topology::Brep`](crate::topology::Brep) holding the topology records of the pseudo B-rep and keeps the
//!   fillet-specific members in side tables keyed by the topology ids
//!   ([`FilletBrep::fillet_vertex`], ...); every pseudo-B-rep vertex, edge, edge use and
//!   vertex use made by the fillet code has an entry. `m_pEdgeListHead` is
//!   [`FilletBrep::edge_list`]. The `SmFilletGeom` objects referenced from the pseudo
//!   B-rep's edge uses (upstream: owned by the fillet solvers) are stored in the
//!   [`FilletBrep`] as well and addressed by [`FilletGeomId`]. `SmFilletSolver*`,
//!   `SmFilletCorner*` and `SmFilletGeom*` pointers are the opaque [`FilletSolverId`],
//!   [`FilletCornerId`] and [`FilletGeomId`] (identity only, never dereferenced here).
//!   `m_pOrigEdge`, `m_pOrigFace` and `m_apSplitFaces` are ids into the *target*
//!   B-rep, `m_pFilletBrepEdge1/2` ids into the *fillet* B-rep; callers pass those
//!   B-reps explicitly.
//! - **Destructors.** The topology arenas are append-only, so `~SmFilletVertex` and
//!   `~SmFilletEdge` ([`FilletBrep::delete_fillet_vertex`],
//!   [`FilletBrep::delete_fillet_edge`]) unlink the deleted uses from their owners'
//!   lists (`SmTopology::~SmTopology`), drop the fillet side-table entries, and for an
//!   edge drop its curve and remove it from [`FilletBrep::edge_list`]; the unlinked
//!   topology records stay in the arenas, unreferenced. As upstream, a deleted vertex's
//!   edge uses keep their (dangling) vertex use id.
//! - **Constructors.** `SmFilletEdgeuse(SmFilletBrep*)` needs the pseudo B-rep's
//!   infinite region and its first shell (`SmBrep::GetInfiniteRegion`,
//!   `SmBrep::MakeShellVertexTopology`, `SmRegion::GetShells`): `GetShells` is
//!   `topology.shell`'s, with no attribute filter as upstream's default;
//!   `GetInfiniteRegion` is `topology.brep_p1`'s accessor. `SmFilletBrep()` runs
//!   `SmBrep()`, which creates that region: [`FilletBrep::new`] makes it as
//!   `topology.brep_p3`'s constructor does ([`Brep::new_with_infinite_region`]).
//!   It has no kernel, so the rest of `SmBrep()` (the context's zone tolerance and size
//!   settings, and the `SM_NO_CONSTRUCTION` notification) is not applied to the pseudo
//!   B-rep; it keeps `Brep::new`'s tolerance. `MakeShellVertexTopology` is
//!   `topology.brep_p2`'s; the rest of the edge use constructor (the shell-owned edge
//!   use with no fillet geometry) is ported behind them. `SmFilletGeom(pFilletSolver, eType)` copies the
//!   solver's offset surfaces with `SmOffsetSurface::Copy` (the kernel stands for the
//!   solver's creation context; its status is ignored upstream, so a failed copy leaves
//!   `NULL`, and only [`SolidError::Unported`] propagates). That constructor leaves `m_dOrientations` and `m_bCapped`
//!   uninitialised; they start as `0.0` and `false` here.
//! - **Transferred fillet surface.** After `MakeFaceBrep` the surface belongs to a face of
//!   the intermediate fillet B-rep and upstream's `GetFilletSurface()` keeps returning the
//!   pointer. [`FilletGeom::fillet_surface`] / [`FilletGeom::fillet_surface_mut`] take
//!   that B-rep ([`FilletSurfaceOwner`]) and resolve the face's surface in it; callers
//!   that run before any face is made pass `None`, and a transferred surface without
//!   (or with another) owner is refused with `InvalidInput`.
//! - **Arguments.** Out-params become return values; `SmContext` arguments are the
//!   [`Kernel`]; `int iDebugLevel` arguments and debug drawing are dropped.
//!   `sm_TwoPntsIntersection` takes the start and end *points* of its `SmVertex`
//!   arguments. The `SmFilletSolver` of `sm_FindStepoffUV` is the co-ported
//!   [`ConstantRadiusAssistedFs`] (its edge uses and `GetThisApproxTol3d`);
//!   [`FilletGeom::new`] and [`FilletGeom::offset_surface`] take the solver base
//!   ([`FilletSolver`]) they copy from or fall back to explicitly.
//!   `SmSurfParamType` closures are [`SurfParamType`]. `sm_IsOrderCorrect` indexes a
//!   slice of [`TsectPnt`] values (upstream: an array of pointers) and fails on an index
//!   out of range. `sm_FindMarkForContext2` searches a slice of [`MarkLock`] for a
//!   context index and returns the position of the first match.
//!   `sm_ExamineNFixFilletGeoms` takes the two geometries' solvers (upstream:
//!   `GetFilletSolver()`, an opaque id here) as co-ported [`ConstantRadiusAssistedFs`]
//!   values; a geometry whose adjusting path needs a solver it does not have is a
//!   null-pointer error (upstream dereferences it). Upstream's `pAdjustFS` is any
//!   `SmFilletSolver` and only base members are read (`GetSurface(k)`, `m_pSurfaces`);
//!   the signature commits to the one solver kind; it could take the
//!   [`crate::blend::fillet_solver::p1::FilletSolver`] base that every solver of
//!   `blend.fillet_standard_solver_p1` and `blend.fillet_custom_solver` now holds.
//! - **Surface cache flags.** `sm_SurfaceDropPoint` and `sm_FindStepoffUV` temporarily
//!   turn off the surface cache's point test (`m_bPointTestEnabled`) and turn on its
//!   boundary-curve processing around `GlobalPointSolve`, so every solution is kept
//!   unclassified. The Rust surface cache has no such switch yet: for a standalone
//!   surface the flags change nothing (the point test only runs for a face's surface,
//!   `m_bProcessBoundaryCurves` defaults to `TRUE`), and for a face-owned surface
//!   `Surface::global_point_solve` returns `Unported(query.face_p2)` at the point test
//!   itself (`surface::surface::p4`, where the switch is noted for that porter: once it
//!   exists, these two callers must disable the test or their solutions will be
//!   classified against the trim boundaries, unlike upstream). `sm_SurfaceDropPoint`'s
//!   own cache fetch (`smsurf_GetSurfaceCache` + `NER`) is ported: a surface whose
//!   cache cannot be built (its Bezier decomposition fails, as for UB-166's u direction)
//!   returns `SM_ERR_NULL_POINTER` before any solve; the solve then fetches the cache
//!   again (there is no cache manager yet), which costs time, not results.
//! - **Statuses.** Ignored upstream statuses (`Unitize` in
//!   `sm_InterpolateEndptsAndTangents`, `Trim` and `Intersect` in `SmFilletGeom::Trim`)
//!   are ignored here too (`Unitize` of a zero vector yields `(0, 0, 1)`, a failed
//!   interval intersection the "inside out" interval), except that an
//!   [`SolidError::Unported`] is always propagated. Null dereferences upstream
//!   (missing rails, curves, uses) are [`SolidError::Upstream`] with
//!   `SM_ERR_NULL_POINTER`; a point classification object of the wrong kind where
//!   upstream casts blindly is [`SolidError::InvalidInput`].
//! - **Record writes.** `SmVertex::SetPoint` (in `CalcVertOnVert`,
//!   `CalcCliffRailIntEdge`) and `pRail->SetCurve(pIsoCrv, TRUE)` (in
//!   `sm_ExamineNFixFilletGeoms`) notify `SM_NO_CHANGE_GEOMETRY`; the pseudo B-rep has
//!   no notification environment, so the port applies the resets that notification
//!   performs directly: the cached gaps of the vertex's uses, and for the edge the UV
//!   trim curve, drop-curve record and cached edge/face gap of each edge use (its
//!   mate's gap too) and the cached gaps of their vertex uses. The examine routine
//!   then drops the new rail onto the solver's offset surface with its offset distance
//!   at `0` over its `GetNaturalUVDomain`: `SmSurface::DropCurve`
//!   (`intersect.surface_drop_lib_p3`, the general drop on a copy of the offset surface)
//!   then runs; a failed drop skips the rail and a single UV curve becomes the mate
//!   edge use's UV curve through `SetUVCurve` with its defaults.
//! - **Difftest coverage.** The shim builds plain `SmBSplineSurface` / `SmBSplineCurve`
//!   geometry, except for [`evaluate_mate`], which is also compared on a real `SmPlane`
//!   (with and without its NURB) and a canonical `SmSurfOfRevolution` of a line (an
//!   `SmCylinder` / `SmCone` cannot be a [`Surface`] yet: the cone kind's dispatch gap
//!   in `surface.cone_p1`); analytic-kind paths inside the other surface callees are
//!   never compared.
//! - **Tolerance classes.** A `double` handed to an `SmZoneTol3d` / `SmApproxTol3d`
//!   parameter is converted exactly there ([`zone_tol3d`], [`approx_tol3d`]:
//!   `USE_DEFAULT` substitution, then `fabs`): `sm_SurfaceDropPoint` and
//!   `sm_FindStepoffUV` (`GlobalPointSolve`), `sm_ClassifyFacePoint` (the face
//!   tolerance returned as an `SmZoneTol3d`), `JoinVertsByCapping` (`LiftCurve`) and the
//!   examine path (`DropCurve`). `sm_TwoPntsIntersection` hands its tolerance to
//!   `PointBasedSurfaceIntersect` through `SM_CAST_APPROXTOL3D_PTR` (a pointer cast, no
//!   conversion), so it stays raw there.
//! - **Unported callees.** `SmSurface::GlobalCurveIntersect` (in
//!   [`FilletBrep::calc_cliff_rail_int_edge`]) stops at the unwired
//!   `SmSurfaceDropCurve::DoTrace` call of `intersect.surface_drop_lib_p1` (its stub
//!   carries the label `intersect.surface_drop_curve`);
//!   `SmFace::CreateUVTrimCurves` (`topology.face_p3`, reached through the wired
//!   `SmFace::CalculateUVDomainFromUVTrimCurves` of `topology.face_p1`) and the
//!   unported callees of `SmFace::Point3DClassify` (wired to `query::face::p1`) in
//!   `sm_ClassifyFacePoint`;
//!   `SmTemporaryRadiusChange` (`blend.fillet_executive_p2`) inside the wired
//!   `SmFilletSolver::RailRailIntersect` of `sm_CreateCubicRailRailInterpolant` (the
//!   cubic built from its result is ported behind it; the solvers are `SmFilletSolver*`
//!   upstream, here `&mut` [`FilletSolver`]s because its virtual callees may change
//!   them). Every stub replaces only its call.
//! - **Bounds.** `sm_ExamineNFixFilletGeoms` copies at most the smaller control net
//!   row; indices are checked. No input-driven recursion.

use std::collections::BTreeMap;

use crate::blend::fillet_classification::FilletClassObject;
use crate::blend::fillet_corner::p3::{FilletCorner, FilletExecutiveData};
use crate::blend::fillet_custom_solver::{ConstantRadiusAssistedFs, OffsetSurface};
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::context::MarkLock;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionArray, SolutionObject};
use crate::geom::tol::{approx_tol3d, zone_tol3d, USE_DEFAULT};
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p1::{BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::nurbs::math::CPoint;
use crate::surface::surface::p1::get_surface_cache;
use crate::surface::surface::p2::PointIntersection;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edge::{self as edge_unit, set_interval};
use crate::topology::edgeuse::p1::{self as eu1, face as edgeuse_face, mate, primary_edgeuse};
use crate::topology::edgeuse::p2::{evaluate_binormal_step_off, StepOffOptions};
use crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves;
use crate::topology::face::p2 as face2;
use crate::topology::vertex::vertexuses;
use crate::topology::vertexuse as vertexuse_unit;
use crate::topology::{
    Brep, EdgeId, Edgeuse, EdgeuseId, EdgeuseOwner, FaceId, RegionId, ShellId, VertexId,
    VertexuseId, VertexuseOwner,
};

const OP_EVALUATE_MATE: &str = "blend::fillet_geom::p1::evaluate_mate";
const OP_SURFACE_DROP_POINT: &str = "blend::fillet_geom::p1::surface_drop_point";
const OP_TWO_PNTS: &str = "blend::fillet_geom::p1::two_pnts_intersection";
const OP_STEPOFF: &str = "blend::fillet_geom::p1::find_stepoff_uv";
const OP_CLASSIFY: &str = "blend::fillet_geom::p1::classify_face_point";
const OP_EXAMINE: &str = "blend::fillet_geom::p1::examine_n_fix_fillet_geoms";
const OP_ORDER: &str = "blend::fillet_geom::p1::is_order_correct";
const OP_VERTEX: &str = "blend::fillet_geom::p1::FilletBrep::fillet_vertex";
const OP_EDGE: &str = "blend::fillet_geom::p1::FilletBrep::fillet_edge";
const OP_EDGEUSE: &str = "blend::fillet_geom::p1::FilletBrep::fillet_edgeuse";
const OP_VERTEXUSE: &str = "blend::fillet_geom::p1::FilletBrep::fillet_vertexuse";
const OP_GEOM: &str = "blend::fillet_geom::p1::FilletBrep::geom";
const OP_VERT_ON_VERT: &str = "blend::fillet_geom::p1::FilletBrep::calc_vert_on_vert";
const OP_CLIFF: &str = "blend::fillet_geom::p1::FilletBrep::calc_cliff_rail_int_edge";
const OP_FIND_EDGE: &str = "blend::fillet_geom::p1::FilletBrep::find_fillet_brep_edge";
const OP_RAIL_VU: &str = "blend::fillet_geom::p1::FilletBrep::rail_vertexuse";
const OP_TRIM: &str = "blend::fillet_geom::p1::FilletBrep::trim_geom";
const OP_OFFSET: &str = "blend::fillet_geom::p1::FilletGeom::offset_surface";
const OP_SET_OFFSET: &str = "blend::fillet_geom::p1::FilletGeom::set_offset_surface";

/// `SM_ERR_BAD_INTERSECTIONS`.
pub const SM_ERR_BAD_INTERSECTIONS: i32 = 1027;
fn upstream(operation: &'static str, code: i32) -> SolidError {
    SolidError::Upstream {
        operation,
        code,
        entity: None,
    }
}

/// `SM_DEG2RAD`.
fn deg2rad(deg: f64) -> f64 {
    deg * (PI / 180.0)
}

// ---- enums --------------------------------------------------------------------------

/// `SmFilletErrorType`: where a fillet failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilletErrorType {
    /// `SM_FILERR_OK`.
    #[default]
    Ok = 0,
    /// `SM_FILERR_EXEC_UNINITIALIZED`.
    ExecUninitialized = 1,
    /// `SM_FILERR_INVALID_INPUT`.
    InvalidInput = 2,
    /// `SM_FILERR_VERTEX_PROBLEM`.
    VertexProblem = 3,
    /// `SM_FILERR_EDGE_PROBLEM`.
    EdgeProblem = 4,
    /// `SM_FILERR_BAD_INT`.
    BadInt = 5,
    /// `SM_FILERR_BAD_TRIM`.
    BadTrim = 6,
    /// `SM_FILERR_BAD_MERGE`.
    BadMerge = 7,
    /// Not an upstream code (the pin's enum ends at `SM_FILERR_BAD_MERGE`). The "blend does not
    /// fit its faces" refusal of the source opt-ins (the `fixes.fillet_refuse_*` flags that
    /// refuse a blend larger than its faces). Only those opt-ins record it; with them off the port
    /// reports pin codes only.
    NoRoom = 8,
    /// `SM_FILERR_CORNER_UNSUPPORTED` (source change, source fork@a73cf5e): a
    /// filleted vertex no `SmFilletCorner` type handles. This port keeps its existing codes
    /// 0..=8 (pinned 0..=7 plus `NoRoom` = 8) and gives the new code 9; the fork's enum has
    /// `SM_FILERR_NOT_EXACT` = 9 and its
    /// `SM_FILERR_CORNER_UNSUPPORTED` = 10, but this port defines no `NOT_EXACT` code. It is
    /// not the C API's `SMLIB_ERR_CORNER_UNSUPPORTED` (-1111), which is a capi status and has
    /// no mapping here. Only `fixes.fillet_vertex_error_diagnostics` records it (#1469); with
    /// it off the port reports pin codes only.
    CornerUnsupported = 9,
}

/// `SmFilletCornerType`: the kind of corner where fillets meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilletCornerType {
    /// `SM_FCR_UNKNOWN`.
    #[default]
    Unknown = 0,
    /// `SM_FCR_OPEN`.
    Open = 1,
    /// `SM_FCR_DEGENERATE`.
    Degenerate = 2,
    /// `SM_FCR_1_x_1`.
    OneByOne = 3,
    /// `SM_FCR_2_x_2`.
    TwoByTwo = 4,
    /// `SM_FCR_3_x_2_MIXED`.
    ThreeByTwoMixed = 5,
    /// `SM_FCR_4_x_3`.
    FourByThree = 6,
    /// `SM_FCR_N_x_1_CLOSED`.
    NByOneClosed = 7,
    /// `SM_FCR_N_x_1`.
    NByOne = 8,
    /// `SM_FCR_N_x_2`.
    NByTwo = 9,
    /// `SM_FCR_N_x_N`.
    NByN = 10,
    /// `SM_FCR_N_x_N_CONVEX`.
    NByNConvex = 11,
    /// `SM_FCR_N_x_N_CONCAVE`.
    NByNConcave = 12,
}

/// `SmFilStatus`: the state of a fillet computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilStatus {
    /// `SM_FIL_UNPROCESSED`.
    #[default]
    Unprocessed = 0,
    /// `SM_FIL_PROCESSED`.
    Processed = 1,
    /// `SM_FIL_FAILURE`.
    Failure = 2,
    /// `SM_FIL_SURF_INT_FAILURE`.
    SurfIntFailure = 3,
    /// `SM_FE_END_PNT_DROP_FAILURE`.
    EndPntDropFailure = 4,
    /// `SM_FE_TO_BE_SQUEEZED`.
    ToBeSqueezed = 5,
    /// `SM_FE_CUBIC_RAIL_RAIL_INTERPOLATE`.
    CubicRailRailInterpolate = 6,
    /// `SM_FV_NO_INT_RAIL_EU`: unable to intersect the fillet rail curve with a side
    /// edge.
    NoIntRailEu = 7,
    /// `SM_FV_SOLVER_NOT_CONVERGE`.
    SolverNotConverge = 8,
    /// `SM_FV_TO_BE_DELETED`.
    ToBeDeleted = 9,
    /// `SM_FS_SURF_TRACING_FAILURE`.
    SurfTracingFailure = 10,
    /// `SM_FS_SELF_INT_HANDLING_FAILURE`.
    SelfIntHandlingFailure = 11,
    /// `SM_FS_SURF_SKINNING_FAILURE`.
    SurfSkinningFailure = 12,
}

impl FilStatus {
    /// The status with upstream's enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Unprocessed,
            1 => Self::Processed,
            2 => Self::Failure,
            3 => Self::SurfIntFailure,
            4 => Self::EndPntDropFailure,
            5 => Self::ToBeSqueezed,
            6 => Self::CubicRailRailInterpolate,
            7 => Self::NoIntRailEu,
            8 => Self::SolverNotConverge,
            9 => Self::ToBeDeleted,
            10 => Self::SurfTracingFailure,
            11 => Self::SelfIntHandlingFailure,
            12 => Self::SurfSkinningFailure,
            _ => return None,
        })
    }
}

/// `SmFilletVertexType`: how a fillet vertex's position is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilletVertexType {
    /// `SM_FV_FILLET_X_EDGEUSE`.
    FilletXEdgeuse = 0,
    /// `SM_FV_FILLET_X_FILLET`.
    FilletXFillet = 1,
    /// `SM_FV_FILLET_X2_FILLETS`.
    FilletX2Fillets = 2,
    /// `SM_FV_MATE`.
    Mate = 3,
    /// `SM_FV_ON_VERTEX`.
    OnVertex = 4,
    /// `SM_FV_ON_CROSS_SECTION`.
    OnCrossSection = 5,
    /// `SM_FV_CLIFF_RAIL_X_EDGE`.
    CliffRailXEdge = 6,
    /// `SM_FV_RAIL_END`.
    RailEnd = 7,
    /// `SM_FV_RAIL_X_RAIL`.
    RailXRail = 8,
    /// `SM_FV_RAIL_X_EDGEUSE`.
    RailXEdgeuse = 9,
    /// `SM_FV_RAIL_X_VERTEX`.
    RailXVertex = 10,
    /// `SM_FV_RAIL_X_EXTENDED_EDGEUSE`.
    RailXExtendedEdgeuse = 11,
    /// `SM_FV_SETBACK`.
    Setback = 12,
    /// `SM_FV_UNKNOWN`.
    #[default]
    Unknown = 13,
}

impl FilletVertexType {
    /// The type with upstream's enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::FilletXEdgeuse,
            1 => Self::FilletXFillet,
            2 => Self::FilletX2Fillets,
            3 => Self::Mate,
            4 => Self::OnVertex,
            5 => Self::OnCrossSection,
            6 => Self::CliffRailXEdge,
            7 => Self::RailEnd,
            8 => Self::RailXRail,
            9 => Self::RailXEdgeuse,
            10 => Self::RailXVertex,
            11 => Self::RailXExtendedEdgeuse,
            12 => Self::Setback,
            13 => Self::Unknown,
            _ => return None,
        })
    }
}

/// How a fillet surface is trimmed (`SmBoundaryTrimmingType`); discriminants are the
/// upstream values. `blend.fillet_standard_solver_p1` (PR #168) re-exports it at
/// `blend::fillet_standard_solver::p1::BoundaryTrimmingType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoundaryTrimmingType {
    /// `SM_BT_NONE`.
    #[default]
    None = 0,
    /// `SM_BT_MINIMAL`.
    Minimal = 1,
    /// `SM_BT_MAXIMAL`.
    Maximal = 2,
    /// `SM_BT_BEVEL`.
    Bevel = 3,
    /// `SM_BT_BLEND`.
    Blend = 4,
}

/// `SmFilletEdgeType`: what a fillet edge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilletEdgeType {
    /// `SM_FE_BLENDING_RAIL`.
    BlendingRail = 0,
    /// `SM_FE_CROSS_SECTION`.
    CrossSection = 1,
    /// `SM_FE_CLIFF_RAIL`.
    CliffRail = 2,
    /// `SM_FE_FILLET_END`.
    FilletEnd = 3,
    /// `SM_FE_FILLET_X_FILLET`.
    FilletXFillet = 4,
    /// `SM_FE_FILLET_X_SIDE_FACE`.
    FilletXSideFace = 5,
    /// `SM_FE_ON_EDGE`.
    OnEdge = 6,
    /// `SM_FE_ON_EXTEND_EDGE`.
    OnExtendEdge = 7,
    /// `SM_FE_PRECOMPUTED`.
    Precomputed = 8,
    /// `SM_FE_RAIL`.
    Rail = 9,
    /// `SM_FE_RAIL_EXTENSION`.
    RailExtension = 10,
    /// `SM_FE_RAIL_RAIL_INTERPOLATION`.
    RailRailInterpolation = 11,
    /// `SM_FE_SETBACK_RAIL`.
    SetbackRail = 12,
    /// `SM_FE_SPLIT_FACE`.
    SplitFace = 13,
    /// `SM_FE_UNKNOWN`.
    #[default]
    Unknown = 14,
}

impl FilletEdgeType {
    /// The type with upstream's enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::BlendingRail,
            1 => Self::CrossSection,
            2 => Self::CliffRail,
            3 => Self::FilletEnd,
            4 => Self::FilletXFillet,
            5 => Self::FilletXSideFace,
            6 => Self::OnEdge,
            7 => Self::OnExtendEdge,
            8 => Self::Precomputed,
            9 => Self::Rail,
            10 => Self::RailExtension,
            11 => Self::RailRailInterpolation,
            12 => Self::SetbackRail,
            13 => Self::SplitFace,
            14 => Self::Unknown,
            _ => return None,
        })
    }
}

/// `SmFilletGeomType`: what a fillet geometry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilletGeomType {
    /// `SM_FG_DEFAULT`.
    #[default]
    Default = 0,
    /// `SM_FG_BLENDS`.
    Blends = 1,
    /// `SM_FG_CLIFF_ROLLOVER`.
    CliffRollover = 2,
    /// `SM_FG_TANGENT_ROLLOVER`.
    TangentRollover = 3,
    /// `SM_FG_CLIFF_SIDE_PATCH`.
    CliffSidePatch = 4,
    /// `SM_FG_GAP_FILLER`.
    GapFiller = 5,
    /// `SM_FG_CORNER_FILLET`.
    CornerFillet = 6,
}

impl FilletGeomType {
    /// The type with upstream's enum value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Default,
            1 => Self::Blends,
            2 => Self::CliffRollover,
            3 => Self::TangentRollover,
            4 => Self::CliffSidePatch,
            5 => Self::GapFiller,
            6 => Self::CornerFillet,
            _ => return None,
        })
    }
}

// ---- opaque identities ---------------------------------------------------------------

/// The identity of an `SmFilletSolver*` (unit `blend.fillet_solver_p1`): a back
/// pointer that is never dereferenced here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FilletSolverId(pub u32);

/// The identity of an `SmFilletCorner*` (unit `blend.fillet_corner_p1`): a back
/// pointer that is never dereferenced here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FilletCornerId(pub u32);

const OP_CORNER: &str = "blend::fillet_geom::p1::CornerLookup::corner";

/// How the fillet geometry calculators reach the `SmFilletCorner` a [`FilletCornerId`]
/// names (upstream's `GetFilletCorner()` pointer, dereferenced) and the executive
/// members a corner reads (`GetExtendedSurface`).
///
/// [`CornerLookup::default`] is a caller without an executive (a standalone entry
/// point): an empty corner table and an empty extended-surface map.
#[derive(Debug, Clone, Copy, Default)]
pub struct CornerLookup<'c> {
    /// The executive's corner table (`m_vFilletCorners`), indexed by the id (`id.0`);
    /// `None`: the caller has none.
    pub table: Option<&'c [FilletCorner]>,
    /// The corner whose method is running and its id (`SmFilletCorner::CalcCornerVertGeom`
    /// / `CalcCornerEdgeGeom` on its own vertices and edges, while it is borrowed out of
    /// any table): an id equal to it resolves to it before the table.
    pub this: Option<(Option<FilletCornerId>, &'c FilletCorner)>,
    /// The executive's members the corners read; `None`: the caller has none.
    pub exec: Option<&'c FilletExecutiveData>,
}

impl<'c> CornerLookup<'c> {
    /// The executive's corner table `corners`, without executive data.
    #[must_use]
    pub const fn table(corners: &'c [FilletCorner]) -> Self {
        Self {
            table: Some(corners),
            this: None,
            exec: None,
        }
    }

    /// Only the running corner `corner` (with its id `id`) and the executive data it
    /// reads: another corner's id is `SM_ERR_NULL_POINTER` (upstream's corner methods
    /// only reach their own vertices and edges).
    #[must_use]
    pub const fn of_corner(
        id: Option<FilletCornerId>,
        corner: &'c FilletCorner,
        exec: &'c FilletExecutiveData,
    ) -> Self {
        Self {
            table: None,
            this: Some((id, corner)),
            exec: Some(exec),
        }
    }

    /// Lookup backed by [`FilletExecutiveData::corners`] (`SmFilletExecutive` routines).
    #[must_use]
    pub const fn of_executive(exec: &'c FilletExecutiveData) -> Self {
        Self {
            table: None,
            this: None,
            exec: Some(exec),
        }
    }

    /// The corner `id` (`GetFilletCorner()`, dereferenced).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for an id neither the running corner nor in the table (a
    /// caller without a corner store holds an empty table).
    pub fn corner(&self, id: FilletCornerId) -> Result<&'c FilletCorner> {
        if let Some((this_id, this)) = self.this {
            if this_id == Some(id) {
                return Ok(this);
            }
        }
        if let Some(exec) = self.exec {
            if let Some(c) = exec.corners.get(&id) {
                return Ok(c);
            }
        }
        // upstream dereferences the corner pointer; one this caller cannot resolve is
        // the NULL its NER would return for
        self.table
            .and_then(|table| usize::try_from(id.0).ok().and_then(|i| table.get(i)))
            .ok_or_else(|| SolidError::null_pointer(OP_CORNER))
    }

    /// `pCorner->GetExtendedSurface(pOriginalFace)`
    /// ([`FilletCorner::extended_surface`]): the extended surface the executive holds
    /// for `face`, `None` when it has none.
    ///
    /// # Errors
    ///
    /// None: a caller without executive data holds an empty map (`None` for every
    /// face).
    pub fn extended_surface(
        &self,
        corner: &FilletCorner,
        face: FaceId,
    ) -> Result<Option<&'c Surface>> {
        match self.exec {
            Some(exec) => Ok(corner.extended_surface(exec, face)),
            // a caller without an executive holds an empty m_vExtendedSurfacesMap
            None => Ok(None),
        }
    }
}

/// A fillet geometry stored in a [`FilletBrep`] (an `SmFilletGeom*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FilletGeomId(pub u32);

// ---- type codes ----------------------------------------------------------------------

/// `SmFilletEdgeuse_TYPE` (`TOPO_BASE_TYPE + 31`).
pub const FILLET_EDGEUSE_TYPE: i64 = 16031;
/// `SmFilletVertexuse_TYPE` (`TOPO_BASE_TYPE + 32`).
pub const FILLET_VERTEXUSE_TYPE: i64 = 16032;
/// `SmFilletEdge_TYPE` (`TOPO_BASE_TYPE + 33`).
pub const FILLET_EDGE_TYPE: i64 = 16033;
/// `SmFilletVertex_TYPE` (`TOPO_BASE_TYPE + 34`).
pub const FILLET_VERTEX_TYPE: i64 = 16034;
/// `SmFilletGeom_TYPE` (`TOPO_BASE_TYPE + 35`).
pub const FILLET_GEOM_TYPE: i64 = 16035;
/// `SmFilletBrep_TYPE` (`CONT_BASE_TYPE + 94`).
pub const FILLET_BREP_TYPE: i64 = 12094;

/// `SmFilletEdgeuse::GetClassString`.
pub const FILLET_EDGEUSE_CLASS_STRING: &str = "SmFilletEdgeuse";
/// `SmFilletEdgeuse::GetTypeString`.
pub const FILLET_EDGEUSE_TYPE_STRING: &str = "SmFilletEdgeuse_TYPE";
/// `SmFilletVertexuse::GetClassString`.
pub const FILLET_VERTEXUSE_CLASS_STRING: &str = "SmFilletVertexuse";
/// `SmFilletVertexuse::GetTypeString`.
pub const FILLET_VERTEXUSE_TYPE_STRING: &str = "SmFilletVertexuse_TYPE";
/// `SmFilletEdge::GetClassString`.
pub const FILLET_EDGE_CLASS_STRING: &str = "SmFilletEdge";
/// `SmFilletEdge::GetTypeString`.
pub const FILLET_EDGE_TYPE_STRING: &str = "SmFilletEdge_TYPE";
/// `SmFilletVertex::GetClassString`.
pub const FILLET_VERTEX_CLASS_STRING: &str = "SmFilletVertex";
/// `SmFilletVertex::GetTypeString`.
pub const FILLET_VERTEX_TYPE_STRING: &str = "SmFilletVertex_TYPE";

/// `SmFilletVertexuse::IsKindOf`: [`FILLET_VERTEXUSE_TYPE`], then the `SmVertexuse`
/// chain.
#[must_use]
pub const fn fillet_vertexuse_is_kind_of(t: i64) -> bool {
    t == FILLET_VERTEXUSE_TYPE || crate::topology::vertexuse::is_kind_of(t)
}

/// `SmFilletVertex::IsKindOf`: [`FILLET_VERTEX_TYPE`], then the `SmVertex` chain.
#[must_use]
pub const fn fillet_vertex_is_kind_of(t: i64) -> bool {
    t == FILLET_VERTEX_TYPE || crate::topology::vertex::is_kind_of(t)
}

/// `SmFilletEdgeuse::IsKindOf`: [`FILLET_EDGEUSE_TYPE`], then the `SmEdgeuse` chain.
#[must_use]
pub const fn fillet_edgeuse_is_kind_of(t: i64) -> bool {
    t == FILLET_EDGEUSE_TYPE || eu1::is_kind_of(t)
}

/// `SmFilletEdge::IsKindOf`: [`FILLET_EDGE_TYPE`], then the `SmEdge` chain.
#[must_use]
pub const fn fillet_edge_is_kind_of(t: i64) -> bool {
    t == FILLET_EDGE_TYPE || edge_unit::is_kind_of(t)
}

// ---- records -------------------------------------------------------------------------

/// The fillet members of a pseudo-B-rep vertex use (`SmFilletVertexuse`): the UV
/// parameter of the use on the fillet surface and the fillet solver's result.
#[derive(Debug, Clone, PartialEq)]
pub struct FilletVertexuse {
    /// `m_vUVPoint`: the UV parameter on the fillet surface.
    pub uv_point: Point2,
    /// `m_vTsectPnt`: the computed result of the fillet solver.
    pub tsect_pnt: TsectPnt,
}

impl Default for FilletVertexuse {
    fn default() -> Self {
        Self::new()
    }
}

impl FilletVertexuse {
    /// `SmFilletVertexuse()`: both surfaces' UV positions and derivatives of the
    /// intersection point set to `(-SM_BIG_DOUBLE, -SM_BIG_DOUBLE)`; the UV point is
    /// `SmPoint2d()`'s `(SM_UNDEF_DOUBLE, SM_UNDEF_DOUBLE)`.
    #[must_use]
    pub fn new() -> Self {
        let mut tsect_pnt = TsectPnt::new();
        for srf in [SrfIndex::First, SrfIndex::Second] {
            *tsect_pnt.uv_pos_mut(srf) = Point2::new(-BIG_DOUBLE, -BIG_DOUBLE);
            *tsect_pnt.uv_deriv_mut(srf) = Vec2::new(-BIG_DOUBLE, -BIG_DOUBLE);
        }
        Self {
            uv_point: Point2::UNDEF,
            tsect_pnt,
        }
    }

    /// `GetTsectPnt`.
    #[must_use]
    pub const fn tsect_pnt(&self) -> &TsectPnt {
        &self.tsect_pnt
    }

    /// `GetTsectPnt` (the reference upstream returns, for writing).
    pub fn tsect_pnt_mut(&mut self) -> &mut TsectPnt {
        &mut self.tsect_pnt
    }

    /// `GetUVPoint`.
    #[must_use]
    pub const fn uv_point(&self) -> Point2 {
        self.uv_point
    }

    /// `SetUVPoint`.
    pub fn set_uv_point(&mut self, uv_point: Point2) {
        self.uv_point = uv_point;
    }

    /// `SetTsectPnt`.
    pub fn set_tsect_pnt(&mut self, tsect_pnt: &TsectPnt) {
        self.tsect_pnt = tsect_pnt.clone();
    }
}

/// The fillet members of a pseudo-B-rep vertex (`SmFilletVertex`).
#[derive(Debug, Clone, PartialEq)]
pub struct FilletVertex {
    /// `m_eType`.
    pub vertex_type: FilletVertexType,
    /// `m_vPointClass`: the map to the original (target) B-rep.
    pub point_class: PointClassification<EntityRef>,
    /// `m_pMate[2]`: the matching vertices on the other rail curves.
    pub mates: [Option<VertexId>; 2],
    /// `m_eStatus`.
    pub status: FilStatus,
    /// `m_cpCorner`: the corner this vertex belongs to.
    pub corner: Option<FilletCornerId>,
}

impl FilletVertex {
    /// `SmFilletVertex(SmFilletCorner*)` (CO-PORTED from `blend.fillet_geom_p2`): type
    /// unknown, an empty classification with the system zone tolerance
    /// (`SmTol::GetZoneTol3d()`), no mates, unprocessed.
    #[must_use]
    pub fn new(k: &Kernel, corner: Option<FilletCornerId>) -> Self {
        Self {
            vertex_type: FilletVertexType::Unknown,
            point_class: PointClassification::new(zone_tol3d(k, USE_DEFAULT)),
            mates: [None, None],
            status: FilStatus::Unprocessed,
            corner,
        }
    }

    /// `CopyPointClass`.
    pub fn copy_point_class(&mut self, pc: &PointClassification<EntityRef>) {
        self.point_class = *pc;
    }

    /// `GetMate(lIndex)`; `None` for an index past the two mates (upstream reads out
    /// of bounds).
    #[must_use]
    pub fn mate(&self, index: usize) -> Option<VertexId> {
        self.mates.get(index).copied().flatten()
    }

    /// `GetStatus`.
    #[must_use]
    pub const fn status(&self) -> FilStatus {
        self.status
    }

    /// `GetPointClassification`.
    #[must_use]
    pub const fn point_classification(&self) -> &PointClassification<EntityRef> {
        &self.point_class
    }

    /// `GetPointClassification` (the reference upstream returns, for writing).
    pub fn point_classification_mut(&mut self) -> &mut PointClassification<EntityRef> {
        &mut self.point_class
    }

    /// `GetOriginalTParam`.
    #[must_use]
    pub const fn original_t_param(&self) -> f64 {
        self.point_class.t_param()
    }

    /// `GetOriginalUV`.
    #[must_use]
    pub const fn original_uv(&self) -> Point2 {
        self.point_class.uv_param()
    }

    /// `GetPointClass`.
    #[must_use]
    pub const fn point_class(&self) -> PointClassType {
        self.point_class.point_class()
    }

    /// `GetPointClassObject`: the target B-rep object the classification maps to.
    #[must_use]
    pub const fn point_class_object(&self) -> Option<EntityRef> {
        self.point_class.object()
    }

    /// `GetFilletCorner`.
    #[must_use]
    pub const fn fillet_corner(&self) -> Option<FilletCornerId> {
        self.corner
    }

    /// `GetFilletVertexType`.
    #[must_use]
    pub const fn fillet_vertex_type(&self) -> FilletVertexType {
        self.vertex_type
    }

    /// `IsProcessed`.
    #[must_use]
    pub fn is_processed(&self) -> bool {
        self.status == FilStatus::Processed
    }

    /// `SetMate(lIndex, pMate)`; an index past the two mates is ignored (upstream
    /// writes out of bounds).
    pub fn set_mate(&mut self, index: usize, mate: Option<VertexId>) {
        if let Some(slot) = self.mates.get_mut(index) {
            *slot = mate;
        }
    }

    /// `SetStatus`.
    pub fn set_status(&mut self, status: FilStatus) {
        self.status = status;
    }

    /// `SetFilletCorner`.
    pub fn set_fillet_corner(&mut self, corner: Option<FilletCornerId>) {
        self.corner = corner;
    }

    /// `SetFilletVertexType`.
    pub fn set_fillet_vertex_type(&mut self, vertex_type: FilletVertexType) {
        self.vertex_type = vertex_type;
    }

    /// `SetOriginalTParam` (`SetTParam(dT)` without snapping).
    pub fn set_original_t_param(&mut self, t: f64) {
        self.point_class.set_t_param(t, false);
    }

    /// `SetOriginalUV`.
    pub fn set_original_uv(&mut self, uv: Point2) {
        self.point_class.set_uv_param(uv);
    }

    /// `SetPointClass`.
    pub fn set_point_class(&mut self, point_class: PointClassType, object: Option<EntityRef>) {
        self.point_class.set_class_object(point_class, object);
    }
}

/// The fillet members of a pseudo-B-rep edge use (`SmFilletEdgeuse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FilletEdgeuse {
    /// `m_pFilletGeom`: the fillet geometry owning this use.
    pub fillet_geom: Option<FilletGeomId>,
}

impl FilletEdgeuse {
    /// `GetFilletGeom`.
    #[must_use]
    pub const fn fillet_geom(&self) -> Option<FilletGeomId> {
        self.fillet_geom
    }
}

/// The fillet members of a pseudo-B-rep edge (`SmFilletEdge`).
#[derive(Debug, Clone, PartialEq)]
pub struct FilletEdge {
    /// `m_eType`.
    pub edge_type: FilletEdgeType,
    /// `m_dDeviation`: deviation of the curve.
    pub deviation: f64,
    /// `m_pOrigEdge`: the target B-rep edge this curve is on.
    pub orig_edge: Option<EdgeId>,
    /// `m_pOrigFace`: the target B-rep face this curve attaches to.
    pub orig_face: Option<FaceId>,
    /// `m_apSplitFaces`: the new target B-rep faces when `orig_face` was split by this
    /// edge.
    pub split_faces: Vec<FaceId>,
    /// `m_pCurveClass`: the classification of the rail.
    pub curve_class: Option<CurveClassification<FilletClassObject>>,
    /// `m_cpCorner`: the fillet corner that made this edge.
    pub corner: Option<FilletCornerId>,
    /// `m_eStatus`.
    pub status: FilStatus,
    /// `m_pFilletBrepEdge1`: the fillet B-rep edge matching this edge geometrically.
    pub fillet_brep_edge1: Option<EdgeId>,
    /// `m_pFilletBrepEdge2`: a second fillet B-rep edge, when this edge was split.
    pub fillet_brep_edge2: Option<EdgeId>,
}

impl FilletEdge {
    /// The members `SmFilletEdge(SmFilletBrep*, SmFilletCorner*)` initialises
    /// (CO-PORTED from `blend.fillet_geom_p2`); the constructor itself is
    /// [`FilletBrep::new_fillet_edge`].
    #[must_use]
    pub const fn with_corner(corner: Option<FilletCornerId>) -> Self {
        Self {
            edge_type: FilletEdgeType::Unknown,
            deviation: 0.0,
            orig_edge: None,
            orig_face: None,
            split_faces: Vec::new(),
            curve_class: None,
            corner,
            status: FilStatus::Unprocessed,
            fillet_brep_edge1: None,
            fillet_brep_edge2: None,
        }
    }

    /// `GetCurveClass`.
    #[must_use]
    pub const fn curve_class(&self) -> Option<&CurveClassification<FilletClassObject>> {
        self.curve_class.as_ref()
    }

    /// `GetFilletBrepEdge`.
    #[must_use]
    pub const fn fillet_brep_edge(&self) -> Option<EdgeId> {
        self.fillet_brep_edge1
    }

    /// `GetFilletBrepEdges`: the non-null fillet B-rep edges, first then second.
    #[must_use]
    pub fn fillet_brep_edges(&self) -> Vec<EdgeId> {
        let mut edges = Vec::new();
        if let Some(e) = self.fillet_brep_edge1 {
            edges.push(e);
        }
        if let Some(e) = self.fillet_brep_edge2 {
            edges.push(e);
        }
        edges
    }

    /// `GetFilletCorner`.
    #[must_use]
    pub const fn fillet_corner(&self) -> Option<FilletCornerId> {
        self.corner
    }

    /// `GetFilletEdgeType`.
    #[must_use]
    pub const fn fillet_edge_type(&self) -> FilletEdgeType {
        self.edge_type
    }

    /// `GetName`: the type's name, or `None` for the types `GetName` leaves the buffer
    /// untouched for.
    #[must_use]
    pub const fn name(&self) -> Option<&'static str> {
        Some(match self.edge_type {
            FilletEdgeType::CrossSection => "SM_FE_CROSS_SECTION",
            FilletEdgeType::FilletXFillet => "SM_FE_FILLET_X_FILLET",
            FilletEdgeType::FilletXSideFace => "SM_FE_FILLET_X_SIDE_FACE",
            FilletEdgeType::RailRailInterpolation => "SM_FE_RAIL_RAIL_INTERPOLATION",
            FilletEdgeType::OnEdge => "SM_FE_ON_EDGE",
            FilletEdgeType::OnExtendEdge => "SM_FE_ON_EXTEND_EDGE",
            FilletEdgeType::FilletEnd => "SM_FE_FILLET_END",
            FilletEdgeType::SetbackRail => "SM_FE_SETBACK_RAIL",
            FilletEdgeType::RailExtension => "SM_FE_RAIL_EXTENSION",
            FilletEdgeType::Rail => "SM_FE_RAIL",
            _ => return None,
        })
    }

    /// `GetOriginalEdge`.
    #[must_use]
    pub const fn original_edge(&self) -> Option<EdgeId> {
        self.orig_edge
    }

    /// `GetOriginalFace`.
    #[must_use]
    pub const fn original_face(&self) -> Option<FaceId> {
        self.orig_face
    }

    /// `GetSplitFaces`.
    #[must_use]
    pub fn split_faces(&self) -> &[FaceId] {
        &self.split_faces
    }

    /// `GetStatus`.
    #[must_use]
    pub const fn status(&self) -> FilStatus {
        self.status
    }

    /// `SetCurveClass`.
    pub fn set_curve_class(&mut self, curve_class: Option<CurveClassification<FilletClassObject>>) {
        self.curve_class = curve_class;
    }

    /// Install an indexed classification with its explicit legacy namespace.
    ///
    /// # Errors
    /// Handle-conversion errors; no existing record is replaced on failure.
    pub fn set_indexed_curve_class(
        &mut self,
        class: Option<&CurveClassification<EntityRef>>,
    ) -> Result<()> {
        self.curve_class = class
            .map(|class| class.map_objects(&mut |o| Ok(FilletClassObject::Legacy(o))))
            .transpose()?;
        Ok(())
    }

    /// `SetFilletEdgeType`.
    pub fn set_fillet_edge_type(&mut self, edge_type: FilletEdgeType) {
        self.edge_type = edge_type;
    }

    /// `SetFilletCorner`.
    pub fn set_fillet_corner(&mut self, corner: Option<FilletCornerId>) {
        self.corner = corner;
    }

    /// `SetOriginalEdge`.
    pub fn set_original_edge(&mut self, edge: Option<EdgeId>) {
        self.orig_edge = edge;
    }

    /// `SetOriginalFace`.
    pub fn set_original_face(&mut self, face: Option<FaceId>) {
        self.orig_face = face;
    }

    /// `AddSplitFace` (`AddUnique`).
    pub fn add_split_face(&mut self, face: FaceId) {
        if !self.split_faces.contains(&face) {
            self.split_faces.push(face);
        }
    }

    /// `SetStatus`.
    pub fn set_status(&mut self, status: FilStatus) {
        self.status = status;
    }

    /// `ClearFilletBrepEdges`: clears each fillet B-rep edge pointer that is in
    /// `edges` (called when those edges were deleted).
    pub fn clear_fillet_brep_edges(&mut self, edges: &[EdgeId]) {
        for &e in edges {
            if self.fillet_brep_edge1 == Some(e) {
                self.fillet_brep_edge1 = None;
            }
            if self.fillet_brep_edge2 == Some(e) {
                self.fillet_brep_edge2 = None;
            }
        }
    }
}

/// The one physical storage location of a fillet surface.
#[derive(Debug, Clone)]
pub enum FilletSurfaceStorage {
    /// The geometry still owns the original surface before face installation.
    Owned(Box<Surface>),
    /// The actual face owns the original surface after installation.
    ManagedFace {
        /// The issued identity of the owning body (not an arena-offset namespace).
        body: crate::geom::object::ObjectRef,
        /// The face containing the actual surface, including a failed partial face.
        face: FaceId,
    },
}

/// The intermediate fillet B-rep that owns a transferred fillet surface, with the
/// identity `MakeFaceBrep` recorded for it in [`FilletSurfaceStorage::ManagedFace`]
/// (upstream reaches the face's surface through the retained pointer).
#[derive(Debug, Clone, Copy)]
pub struct FilletSurfaceOwner<B> {
    /// The issued identity of `body` (`key_of(EntityRef::Brep(..))` at installation).
    pub identity: crate::geom::object::ObjectRef,
    /// The body (`&Brep` or `&mut Brep`).
    pub body: B,
}

const OP_FILLET_SURFACE: &str = "blend::fillet_geom::fillet_surface";

/// The owning body of a transferred surface recorded as `body`, or an error when the
/// caller passed none or another body.
fn managed_owner<B>(
    owner: Option<FilletSurfaceOwner<B>>,
    body: crate::geom::object::ObjectRef,
) -> Result<B> {
    let owner = owner.ok_or_else(|| {
        SolidError::invalid_input(
            OP_FILLET_SURFACE,
            "transferred fillet surface needs its owning body",
        )
    })?;
    if owner.identity != body {
        return Err(SolidError::invalid_input(
            OP_FILLET_SURFACE,
            "fillet surface names a foreign managed body",
        ));
    }
    Ok(owner.body)
}

/// The geometry of one fillet surface (`SmFilletGeom`): two rails, the optional centre
/// line, the fillet surface and the records of the topology built on it.
#[derive(Debug, Clone)]
pub struct FilletGeom {
    /// `m_eType`.
    pub geom_type: FilletGeomType,
    /// `m_pFilletSolver`: the solver defining the implicit geometry.
    pub solver: Option<FilletSolverId>,
    /// `m_vRails[2]`: the 3D rails (pseudo-B-rep edges).
    pub rails: [Option<EdgeId>; 2],
    /// `m_vOtherRails1`: rail edges appended to rail 0.
    pub other_rails1: Vec<EdgeId>,
    /// `m_vOtherRails2`: rail edges appended to rail 1.
    pub other_rails2: Vec<EdgeId>,
    /// `m_pCenterLineCurve`.
    pub center_line_curve: Option<Curve>,
    /// `m_pFilletSurface` (`SM_FILLETSURF_TYPE` is `SmSurface`).
    pub fillet_surface: Option<FilletSurfaceStorage>,
    /// `m_pOffSurfs[2]`: this geometry's own offset surfaces (may differ from the
    /// solver's).
    pub off_surfs: [Option<OffsetSurface>; 2],
    /// `m_dOrientations[2]`: offset directions, `+-1.0`.
    pub orientations: [f64; 2],
    /// `m_dDeviations[2]`: deviation of each 3D rail from its surface.
    pub deviations: [f64; 2],
    /// `m_bCapped[2]`: whether each end required capping.
    pub capped: [bool; 2],
    /// `m_bNeedSplit`.
    pub need_split: bool,
    /// `m_eStatus`.
    pub status: FilStatus,
    /// `m_vEdges`: the edges this geometry owns (other than rails).
    pub edges: Vec<EdgeId>,
    /// `m_vVertices`: the vertices this geometry owns.
    pub vertices: Vec<VertexId>,
    /// `m_vRefEUs`: every referenced edge use.
    pub ref_eus: Vec<EdgeuseId>,
    /// `m_vSideEUs`: the side curves' edge uses (excluding rails).
    pub side_eus: Vec<EdgeuseId>,
}

impl FilletGeom {
    /// `SmFilletGeom(pFilletSolver, eType)` (CO-PORTED from `blend.fillet_geom_p5`):
    /// no rails, curves or surface, unprocessed; with a solver, copies of its offset
    /// surfaces. The solver is its `SmFilletSolver` base (the constructor reads only
    /// `GetSurface(i)`), so any solver class passes its base.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from copying a solver offset surface
    /// (`SmOffsetSurface::Copy`); any other failed copy leaves that slot `None`.
    pub fn new(
        k: &Kernel,
        solver: Option<(FilletSolverId, &FilletSolver)>,
        geom_type: FilletGeomType,
    ) -> Result<Self> {
        let mut off_surfs = [None, None];
        for (slot, i) in off_surfs.iter_mut().zip(0..2usize) {
            if let Some((_, fs)) = solver {
                *slot = match fs.surface(i)?.map(|slot| &slot.surface) {
                    // SmOffsetSurface::Copy() returns an SmSurface*, not SmOffsetSurface*;
                    // its status is ignored, so a failed copy leaves NULL
                    Some(off_srf) => match offset_surface_copy(k, off_srf) {
                        Ok(new_surf) => Some(new_surf),
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => None,
                    },
                    None => None,
                };
            }
        }
        Ok(Self {
            geom_type,
            solver: solver.map(|(id, _)| id),
            rails: [None, None],
            other_rails1: Vec::new(),
            other_rails2: Vec::new(),
            center_line_curve: None,
            fillet_surface: None,
            off_surfs,
            orientations: [0.0, 0.0],
            deviations: [0.0, 0.0],
            capped: [false, false],
            need_split: false,
            status: FilStatus::Unprocessed,
            edges: Vec::new(),
            vertices: Vec::new(),
            ref_eus: Vec::new(),
            side_eus: Vec::new(),
        })
    }

    /// `AddReferencedFilletEdgeuse` (`AddUnique`).
    pub fn add_referenced_fillet_edgeuse(&mut self, eu: EdgeuseId) {
        if !self.ref_eus.contains(&eu) {
            self.ref_eus.push(eu);
        }
    }

    /// `AddSideFilletEdgeuse` (`AddUnique`).
    pub fn add_side_fillet_edgeuse(&mut self, eu: EdgeuseId) {
        if !self.side_eus.contains(&eu) {
            self.side_eus.push(eu);
        }
    }

    /// `GetFilletGeomType`.
    #[must_use]
    pub const fn fillet_geom_type(&self) -> FilletGeomType {
        self.geom_type
    }

    /// `GetFilletSolver`.
    #[must_use]
    pub const fn fillet_solver(&self) -> Option<FilletSolverId> {
        self.solver
    }

    /// `GetStatus`.
    #[must_use]
    pub const fn status(&self) -> FilStatus {
        self.status
    }

    /// `GetSplitFlag`.
    #[must_use]
    pub const fn split_flag(&self) -> bool {
        self.need_split
    }

    /// `GetFilletEdges`: appends the owned edges to `edges`.
    pub fn fillet_edges(&self, edges: &mut Vec<EdgeId>) {
        edges.extend_from_slice(&self.edges);
    }

    /// The B-spline base of the actual retained centerline, for base-only callers.
    /// Virtual operations must use [`Self::center_line_geometry`] to retain analytic state.
    #[must_use]
    pub fn center_line_curve(&self) -> Option<&BSplineCurve> {
        use crate::curve::curve::CurveKind;
        match self.center_line_curve.as_ref()?.kind() {
            CurveKind::BSpline(curve) => Some(curve),
            CurveKind::Line(curve) => Some(curve.base()),
            CurveKind::Ellipse(curve) => Some(curve.base()),
            CurveKind::Hyperbola(curve) => Some(curve.base()),
            CurveKind::Parabola(curve) => Some(curve.base()),
            _ => None,
        }
    }

    /// `GetCenterLineCurve`: the actual object, including its analytic derived type.
    #[must_use]
    pub const fn center_line_geometry(&self) -> Option<&Curve> {
        self.center_line_curve.as_ref()
    }

    /// `GetFilletSurface() != NULL`: whether the geometry has a fillet surface, owned
    /// here or already transferred into a face (the pointer upstream keeps either way).
    #[must_use]
    pub const fn has_fillet_surface(&self) -> bool {
        self.fillet_surface.is_some()
    }

    /// `GetFilletSurface()`: the fillet surface, owned here or, after `MakeFaceBrep`
    /// transferred it, the surface of its face in the intermediate fillet B-rep.
    ///
    /// `owner` is that B-rep with the identity `MakeFaceBrep` recorded for it. Callers
    /// that run before any face is made pass `None` (no owning body exists yet).
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] for a transferred surface when `owner` is `None` or
    /// names another body; the face's own lookup errors, and `SM_ERR_NULL_POINTER` for a
    /// face without a surface.
    pub fn fillet_surface<'a>(
        &'a self,
        owner: Option<FilletSurfaceOwner<&'a Brep>>,
    ) -> Result<Option<&'a Surface>> {
        match self.fillet_surface.as_ref() {
            None => Ok(None),
            Some(FilletSurfaceStorage::Owned(surface)) => Ok(Some(surface)),
            Some(FilletSurfaceStorage::ManagedFace { body, face }) => {
                let owner = managed_owner(owner, *body)?;
                Ok(Some(owner.face(*face)?.surface.as_ref().ok_or_else(
                    || SolidError::null_pointer(OP_FILLET_SURFACE),
                )?))
            }
        }
    }

    /// `GetFilletSurface()` for writing: the surface owned here or, once transferred,
    /// the surface of its face in `owner` (see [`Self::fillet_surface`]).
    ///
    /// # Errors
    /// As [`Self::fillet_surface`].
    pub fn fillet_surface_mut<'a>(
        &'a mut self,
        owner: Option<FilletSurfaceOwner<&'a mut Brep>>,
    ) -> Result<Option<&'a mut Surface>> {
        match self.fillet_surface.as_mut() {
            None => Ok(None),
            Some(FilletSurfaceStorage::Owned(surface)) => Ok(Some(surface)),
            Some(FilletSurfaceStorage::ManagedFace { body, face }) => {
                let owner = managed_owner(owner, *body)?;
                Ok(Some(owner.face_mut(*face)?.surface.as_mut().ok_or_else(
                    || SolidError::null_pointer(OP_FILLET_SURFACE),
                )?))
            }
        }
    }

    /// `GetRail(lRailIndex)`; `None` for an index past the two rails (upstream reads
    /// out of bounds).
    #[must_use]
    pub fn rail(&self, rail_index: usize) -> Option<EdgeId> {
        self.rails.get(rail_index).copied().flatten()
    }

    /// `GetOffsetSurface(lRailIndex)`: this geometry's offset surface for the rail,
    /// else the solver's (`SmFilletSolver::GetSurface`, read from the co-ported
    /// `solver`, the geometry's [`FilletSolver`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) when the fallback is needed and
    /// `solver` is `None` (upstream dereferences `NULL`); [`SolidError::InvalidInput`]
    /// for a rail index past the two rails (upstream reads out of bounds).
    pub fn offset_surface<'a>(
        &'a self,
        rail_index: usize,
        solver: Option<&'a FilletSolver>,
    ) -> Result<Option<&'a OffsetSurface>> {
        let own = self
            .off_surfs
            .get(rail_index)
            .ok_or_else(|| SolidError::invalid_input(OP_OFFSET, "rail index past 1"))?;
        if let Some(s) = own {
            return Ok(Some(s));
        }
        let fs = solver.ok_or_else(|| SolidError::null_pointer(OP_OFFSET))?;
        Ok(fs.surface(rail_index)?.map(|slot| &slot.surface))
    }

    /// `SetOffsetSurface(lWhichSrf, pNewOffSrf)`: replaces (deleting) the rail's offset
    /// surface.
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR_INVALID_INPUT`) for a rail index past 1.
    pub fn set_offset_surface(
        &mut self,
        which: usize,
        new_off_srf: Option<OffsetSurface>,
    ) -> Result<()> {
        let slot = self.off_surfs.get_mut(which).ok_or_else(|| {
            SolidError::invalid_input(OP_SET_OFFSET, "offset surface index above 1")
        })?;
        *slot = new_off_srf;
        Ok(())
    }

    /// `SetFilletSurface`.
    pub fn set_fillet_surface(&mut self, surface: Option<Surface>) {
        self.fillet_surface = surface.map(|surface| FilletSurfaceStorage::Owned(Box::new(surface)));
    }

    /// `SetFilletGeomType`.
    pub fn set_fillet_geom_type(&mut self, geom_type: FilletGeomType) {
        self.geom_type = geom_type;
    }

    /// `SetRail(lIndex, pRailE)`; an index past the two rails is ignored (upstream
    /// writes out of bounds).
    pub fn set_rail(&mut self, index: usize, rail: Option<EdgeId>) {
        if let Some(slot) = self.rails.get_mut(index) {
            *slot = rail;
        }
    }
}

// ---- the pseudo B-rep ----------------------------------------------------------------

/// The pseudo B-rep holding the fillet edges and vertices (`SmFilletBrep`), with the
/// fillet members of its topology records and the fillet geometries referenced from
/// them.
#[derive(Debug, Clone)]
pub struct FilletBrep {
    /// The topology records (`SmBrep`).
    brep: Brep,
    /// `m_pEdgeListHead`'s list, in list order.
    edge_list: Vec<EdgeId>,
    vertices: BTreeMap<VertexId, FilletVertex>,
    edges: BTreeMap<EdgeId, FilletEdge>,
    edgeuses: BTreeMap<EdgeuseId, FilletEdgeuse>,
    vertexuses: BTreeMap<VertexuseId, FilletVertexuse>,
    geoms: Vec<FilletGeom>,
    /// `m_bEditingEnabled`.
    editing_enabled: bool,
}

impl Default for FilletBrep {
    /// [`FilletBrep::new`] (editing enabled, as `SmFilletBrep()` leaves it).
    fn default() -> Self {
        Self::new()
    }
}

impl FilletBrep {
    /// `SmFilletBrep()` (CO-PORTED from `blend.fillet_geom_p6`): an empty B-rep with
    /// editing enabled and `SmBrep()`'s infinite void region
    /// ([`Brep::new_with_infinite_region`]; see the module's Constructors).
    #[must_use]
    pub fn new() -> Self {
        // (cannot fail on an empty B-rep; were it to, the region stays missing and
        // `new_fillet_edgeuse` refuses it as the null upstream never has)
        let brep = Brep::new_with_infinite_region().unwrap_or_else(|_| Brep::new());
        Self {
            brep,
            edge_list: Vec::new(),
            vertices: BTreeMap::new(),
            edges: BTreeMap::new(),
            edgeuses: BTreeMap::new(),
            vertexuses: BTreeMap::new(),
            geoms: Vec::new(),
            editing_enabled: true,
        }
    }

    /// `m_bEditingEnabled`.
    #[must_use]
    pub const fn editing_enabled(&self) -> bool {
        self.editing_enabled
    }

    /// The topology records.
    #[must_use]
    pub const fn brep(&self) -> &Brep {
        &self.brep
    }

    /// The topology records, for the friend writes of construction (the fillet corner
    /// and solver units build the pseudo B-rep's topology).
    pub fn brep_mut(&mut self) -> &mut Brep {
        &mut self.brep
    }

    /// `m_pEdgeListHead`'s edges in list order.
    #[must_use]
    pub fn edge_list(&self) -> &[EdgeId] {
        &self.edge_list
    }

    /// The fillet vertices in id order.
    pub fn fillet_vertices(&self) -> impl Iterator<Item = (VertexId, &FilletVertex)> + '_ {
        self.vertices.iter().map(|(&id, v)| (id, v))
    }

    /// The fillet edge uses in id order.
    pub fn fillet_edgeuses(&self) -> impl Iterator<Item = (EdgeuseId, &FilletEdgeuse)> + '_ {
        self.edgeuses.iter().map(|(&id, e)| (id, e))
    }

    /// The fillet vertex uses in id order.
    pub fn fillet_vertexuses(&self) -> impl Iterator<Item = (VertexuseId, &FilletVertexuse)> + '_ {
        self.vertexuses.iter().map(|(&id, v)| (id, v))
    }

    /// The fillet geometries in id order.
    #[must_use]
    pub fn geoms(&self) -> &[FilletGeom] {
        &self.geoms
    }

    /// Records the fillet members of the pseudo-B-rep vertex `v` (a friend write of
    /// construction: `new (pPseudoBrep) SmFilletVertex(..)`).
    pub fn insert_fillet_vertex(&mut self, v: VertexId, record: FilletVertex) {
        self.vertices.insert(v, record);
    }

    /// Records the fillet members of the pseudo-B-rep edge `e` without adding it to
    /// [`FilletBrep::edge_list`] (a friend write of construction).
    pub fn insert_fillet_edge(&mut self, e: EdgeId, record: FilletEdge) {
        self.edges.insert(e, record);
    }

    /// Records the fillet members of the pseudo-B-rep edge use `eu` (a friend write of
    /// construction).
    pub fn insert_fillet_edgeuse(&mut self, eu: EdgeuseId, record: FilletEdgeuse) {
        self.edgeuses.insert(eu, record);
    }

    /// Records the fillet members of the pseudo-B-rep vertex use `vu` (a friend write
    /// of construction).
    pub fn insert_fillet_vertexuse(&mut self, vu: VertexuseId, record: FilletVertexuse) {
        self.vertexuses.insert(vu, record);
    }

    /// Stores a fillet geometry and returns its id.
    pub fn add_geom(&mut self, geom: FilletGeom) -> FilletGeomId {
        let id = FilletGeomId(self.geoms.len() as u32);
        self.geoms.push(geom);
        id
    }

    /// `SmFilletEdge(SmFilletBrep*, SmFilletCorner*)` (CO-PORTED from
    /// `blend.fillet_geom_p2`): a new pseudo-B-rep edge between `start` and `end`
    /// (`SmEdge()`: no curve, no uses), its fillet members, added to the edge list
    /// (`pPseudoBrep->AddEdge(this)`).
    ///
    /// # Errors
    ///
    /// Those of [`Brep::add_edge`].
    pub fn new_fillet_edge(
        &mut self,
        start: VertexId,
        end: VertexId,
        corner: Option<FilletCornerId>,
    ) -> Result<EdgeId> {
        let e = self.brep.add_edge(start, end)?;
        // SmEdge(): m_sZoneTol3d(0.0)
        self.brep.edge_mut(e)?.tolerance = 0.0;
        self.edges.insert(e, FilletEdge::with_corner(corner));
        self.add_edge(e);
        Ok(e)
    }

    /// `SmFilletEdgeuse(SmFilletBrep*)`: a new edge use connected to the first shell of
    /// the pseudo B-rep's infinite region (made with `MakeShellVertexTopology` when the
    /// region has none).
    ///
    /// # Errors
    ///
    /// The errors of `SmBrep::MakeShellVertexTopology` when the region has no shell;
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) without an infinite region
    /// (upstream dereferences it; `SmBrep()` always makes one); [`SolidError::Upstream`]
    /// (`SM_ERR_NULL_POINTER`) when the region still has no shell (upstream reads
    /// `sShells[0]` of an empty array).
    pub fn new_fillet_edgeuse(&mut self) -> Result<EdgeuseId> {
        // upstream dereferences m_pInfiniteRegion unchecked: SmFilletBrep() runs SmBrep(),
        // which always creates the infinite region, so it is never NULL there
        let Some(reg) = infinite_region(&self.brep) else {
            return Err(SolidError::null_pointer(OP_EDGEUSE));
        };
        let mut shells = region_shells(&self.brep, reg)?;
        if shells.is_empty() {
            // Just an artifact vertex
            let (_shell, _new_v) = make_shell_vertex_topology(&mut self.brep, reg)?;
            shells = region_shells(&self.brep, reg)?;
        }
        let shell = *shells
            .first()
            .ok_or_else(|| SolidError::null_pointer(OP_EDGEUSE))?;
        // SmEdgeuse(), then m_pSorLU = sShells[0], m_tEdgeuseType = SmShell_TYPE
        let eu = self.brep.add_edgeuse_record(Edgeuse {
            owner: EdgeuseOwner::Shell(Some(shell)),
            ..Edgeuse::default()
        })?;
        // m_pFilletGeom(NULL)
        self.edgeuses.insert(eu, FilletEdgeuse::default());
        Ok(eu)
    }

    /// `SmFilletBrep::AddEdge`: appends the edge to the edge list (`PostInsert`).
    pub fn add_edge(&mut self, e: EdgeId) {
        self.edge_list.push(e);
    }

    /// `SmFilletBrep::RemoveEdge`: unlinks the edge from the edge list (the record is
    /// kept).
    pub fn remove_edge(&mut self, e: EdgeId) {
        self.edge_list.retain(|&x| x != e);
    }

    /// The fillet members of vertex `v`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `v` is not a fillet vertex.
    pub fn fillet_vertex(&self, v: VertexId) -> Result<&FilletVertex> {
        self.vertices.get(&v).ok_or(SolidError::NotFound {
            operation: OP_VERTEX,
            entity: EntityRef::Vertex(v),
        })
    }

    /// The fillet members of vertex `v`, for writing.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `v` is not a fillet vertex.
    pub fn fillet_vertex_mut(&mut self, v: VertexId) -> Result<&mut FilletVertex> {
        self.vertices.get_mut(&v).ok_or(SolidError::NotFound {
            operation: OP_VERTEX,
            entity: EntityRef::Vertex(v),
        })
    }

    /// The fillet members of edge `e`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `e` is not a fillet edge.
    pub fn fillet_edge(&self, e: EdgeId) -> Result<&FilletEdge> {
        self.edges.get(&e).ok_or(SolidError::NotFound {
            operation: OP_EDGE,
            entity: EntityRef::Edge(e),
        })
    }

    /// The fillet members of edge `e`, for writing.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `e` is not a fillet edge.
    pub fn fillet_edge_mut(&mut self, e: EdgeId) -> Result<&mut FilletEdge> {
        self.edges.get_mut(&e).ok_or(SolidError::NotFound {
            operation: OP_EDGE,
            entity: EntityRef::Edge(e),
        })
    }

    /// Lend actual pseudo topology and its attached rail classification separately.
    ///
    /// # Errors
    /// Missing rail/classification, or the callback's error with partial writes retained.
    pub fn with_rail_classification<T>(
        &mut self,
        rail: EdgeId,
        call: impl FnOnce(&Brep, &mut CurveClassification<FilletClassObject>) -> Result<T>,
    ) -> Result<T> {
        let record = self.edges.get_mut(&rail).ok_or_else(|| {
            SolidError::invalid_input(
                "blend::fillet_geom::with_rail_classification",
                "fillet edge is not live",
            )
        })?;
        let class = record.curve_class.as_mut().ok_or_else(|| {
            SolidError::null_pointer("blend::fillet_geom::with_rail_classification")
        })?;
        call(&self.brep, class)
    }

    /// The fillet members of edge use `eu`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `eu` is not a fillet edge use.
    pub fn fillet_edgeuse(&self, eu: EdgeuseId) -> Result<&FilletEdgeuse> {
        self.edgeuses.get(&eu).ok_or(SolidError::NotFound {
            operation: OP_EDGEUSE,
            entity: EntityRef::Edgeuse(eu),
        })
    }

    /// The fillet members of vertex use `vu`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `vu` is not a fillet vertex use.
    pub fn fillet_vertexuse(&self, vu: VertexuseId) -> Result<&FilletVertexuse> {
        self.vertexuses.get(&vu).ok_or(SolidError::NotFound {
            operation: OP_VERTEXUSE,
            entity: EntityRef::Vertexuse(vu),
        })
    }

    /// The fillet members of vertex use `vu`, for writing.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `vu` is not a fillet vertex use.
    pub fn fillet_vertexuse_mut(&mut self, vu: VertexuseId) -> Result<&mut FilletVertexuse> {
        self.vertexuses.get_mut(&vu).ok_or(SolidError::NotFound {
            operation: OP_VERTEXUSE,
            entity: EntityRef::Vertexuse(vu),
        })
    }

    /// The fillet geometry `g`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown id.
    pub fn geom(&self, g: FilletGeomId) -> Result<&FilletGeom> {
        self.geoms
            .get(g.0 as usize)
            .ok_or_else(|| SolidError::invalid_input(OP_GEOM, "unknown fillet geometry"))
    }

    /// The fillet geometry `g`, for writing.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown id.
    pub fn geom_mut(&mut self, g: FilletGeomId) -> Result<&mut FilletGeom> {
        self.geoms
            .get_mut(g.0 as usize)
            .ok_or_else(|| SolidError::invalid_input(OP_GEOM, "unknown fillet geometry"))
    }

    // ---- SmFilletVertexuse ----------------------------------------------------------

    /// `SmFilletVertexuse::SetProperty(pEU)`: the vertex use becomes the use at the
    /// start of edge use `eu` (`m_tVertexuseType = SmEdgeuse_TYPE`, `m_pSorLUorEU`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling vertex use id.
    pub fn set_vertexuse_property(&mut self, vu: VertexuseId, eu: EdgeuseId) -> Result<()> {
        self.brep.vertexuse_mut(vu)?.owner = VertexuseOwner::Edgeuse(Some(eu));
        Ok(())
    }

    // ---- SmFilletVertex -------------------------------------------------------------

    /// `SmFilletVertex::GetPoint` (`m_vPoint`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn vertex_point(&self, v: VertexId) -> Result<Point3> {
        Ok(self.brep.vertex(v)?.point)
    }

    /// `SmFilletVertex::~SmFilletVertex`: deletes the vertex's vertex uses (unlinking
    /// them from the vertex, `SmTopology::~SmTopology`) and clears the mate pointers
    /// pointing back at it; then drops the fillet members (the `SmVertex` record stays
    /// in the arena, unreferenced).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `v` is not a fillet vertex.
    pub fn delete_fillet_vertex(&mut self, v: VertexId) -> Result<()> {
        let record = self.fillet_vertex(v)?.clone();

        // delete all vertexuses
        let uses = vertexuses(&self.brep, v)?;
        for vu in uses {
            self.vertexuses.remove(&vu);
            // ~SmTopology: m_pListOwner->Remove(this)
            self.brep.vertex_mut(v)?.vertexuses.retain(|&x| x != vu);
        }

        // clear mate pointers
        for mate in record.mates.into_iter().flatten() {
            if let Some(m) = self.vertices.get_mut(&mate) {
                if m.mates.first().copied().flatten() == Some(v) {
                    m.set_mate(0, None);
                } else if m.mates.get(1).copied().flatten() == Some(v) {
                    m.set_mate(1, None);
                }
            }
        }

        self.vertices.remove(&v);
        Ok(())
    }

    /// `SmFilletVertex::CalcVertOnVert`: a vertex of type `SM_FV_ON_VERTEX` takes the
    /// position of the target B-rep vertex its classification maps to and is
    /// processed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the classification object is not a vertex
    /// (upstream casts blindly); [`SolidError::NotFound`] for dangling ids.
    pub fn calc_vert_on_vert(&mut self, v: VertexId, target: &Brep) -> Result<()> {
        let point = match self.fillet_vertex(v)?.point_class_object() {
            Some(EntityRef::Vertex(tv)) => target.vertex(tv)?.point,
            Some(_) => {
                return Err(SolidError::invalid_input(
                    OP_VERT_ON_VERT,
                    "classification object is not a vertex",
                ))
            }
            None => return Err(SolidError::null_pointer(OP_VERT_ON_VERT)),
        };
        self.set_vertex_point(v, point)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmVertex::SetPoint`: `Notify(SM_NO_CHANGE_GEOMETRY)` resets the cached gaps of
    /// every vertex use of the vertex, then the point is stored.
    pub(crate) fn set_vertex_point(&mut self, v: VertexId, point: Point3) -> Result<()> {
        for vu in self.brep.vertex(v)?.vertexuses.clone() {
            let vu = self.brep.vertexuse_mut(vu)?;
            vu.vertex_edge_gap.reset();
            vu.vertex_face_gap.reset();
        }
        self.brep.vertex_mut(v)?.point = point;
        Ok(())
    }

    /// `SmFilletVertex::CalcCliffRailIntEdge`: sets the vertex to the intersection of
    /// the fillet surface with the target B-rep curve of the edge use its
    /// classification maps to, within `this_approx_tol_3d`, and marks it processed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the classification object is not an edge use
    /// (upstream casts blindly); [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for
    /// an edge without a curve; `SM_ERR` when no intersection is found; those of
    /// [`Surface::global_curve_intersect`].
    pub fn calc_cliff_rail_int_edge(
        &mut self,
        k: &Kernel,
        v: VertexId,
        target: &Brep,
        fillet_surface: &Surface,
        this_approx_tol_3d: f64,
    ) -> Result<()> {
        // get origBrep object mapping to this FilletVertex - known to be an Edgeuse
        let eu = match self.fillet_vertex(v)?.point_class_object() {
            Some(EntityRef::Edgeuse(eu)) => eu,
            Some(_) => {
                return Err(SolidError::invalid_input(
                    OP_CLIFF,
                    "classification object is not an edge use",
                ))
            }
            None => return Err(SolidError::null_pointer(OP_CLIFF)),
        };
        let edge = target
            .edgeuse(eu)?
            .edge
            .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
        let curve_3d = target
            .edge(edge)?
            .curve
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
        let ivl = curve_3d.natural_interval();

        // get FilletSurface/OrigBrepCurve intersection marking this FilletVertex Position3d
        let solutions = fillet_surface.global_curve_intersect(
            k,
            [
                CliffHandle::Curve(curve_3d),
                CliffHandle::Surface(fillet_surface),
            ],
            &fillet_surface.natural_uv_domain_with_nurb(k)?,
            curve_3d,
            &ivl,
            this_approx_tol_3d,
        )?;
        let first = solutions
            .get(0)
            .ok_or_else(|| SolidError::sm_err(OP_CLIFF))?;

        // side effect: Set SmFilletVertex::m_vPoint = XSectPoint(FilletSurface, Orig BrepCurve)
        let t = first
            .start
            .parameters
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP_CLIFF))?;
        let pnt = curve_3d.evaluate_point(k, t)?;
        self.set_vertex_point(v, pnt)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmFilletVertex::UpdateSplitFace`: when the target face `orig_face` was split
    /// into it and `new_face`, moves this vertex's classification to `new_face` if the
    /// vertex now lies in it ([`classify_face_point`] returns `2`).
    ///
    /// # Errors
    ///
    /// Those of [`classify_face_point`] (deferred: `topology.face_p3` and the unported
    /// callees of `SmFace::Point3DClassify`);
    /// [`SolidError::NotFound`] for dangling ids.
    pub fn update_split_face(
        &mut self,
        k: &Kernel,
        v: VertexId,
        target: &mut Brep,
        orig_face: FaceId,
        new_face: FaceId,
    ) -> Result<()> {
        let pos = self.vertex_point(v)?;
        let fv = self.fillet_vertex_mut(v)?;
        let pc = &mut fv.point_class;
        let mut which_face: i32 = -1; // Unset. values: 0: neither Face; 1: 1st Face; 2: 2nd Face; 3: both.

        if pc.face_object() == Some(EntityRef::Face(orig_face)) {
            which_face = classify_face_point(k, target, pos, orig_face, new_face)?;
            if which_face == 2 {
                pc.set_class_object(PointClassType::Face, Some(EntityRef::Face(new_face)));
            }
        }

        // It also has another Face pointer:
        if pc.face() == Some(EntityRef::Face(orig_face)) {
            if which_face < 0 {
                which_face = classify_face_point(k, target, pos, orig_face, new_face)?;
            }
            if which_face == 2 {
                pc.set_face(Some(EntityRef::Face(new_face)));
            }
        }

        Ok(())
    }

    /// `SmFilletVertex::GetFilletGeoms`: the fillet geometries of the `SM_FE_RAIL`
    /// edges connected to the vertex, in vertex use order.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for dangling ids or a use that is not a fillet record;
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a vertex use without an
    /// edge use or an edge use without an edge.
    pub fn vertex_fillet_geoms(&self, v: VertexId) -> Result<Vec<FilletGeomId>> {
        let mut geoms = Vec::new();
        for vu in vertexuses(&self.brep, v)? {
            let eu = eu1::vertexuse_edgeuse(&self.brep, vu)?
                .ok_or_else(|| SolidError::null_pointer(OP_VERTEX))?;
            let e = self
                .brep
                .edgeuse(eu)?
                .edge
                .ok_or_else(|| SolidError::null_pointer(OP_VERTEX))?;

            // skip edges not of type SM_FE_RAIL
            if self.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::Rail {
                continue;
            }

            // accumulate all the FilletEdgeuse->FilletGeoms
            if let Some(g) = self.fillet_edgeuse(eu)?.fillet_geom() {
                geoms.push(g);
            }
        }
        Ok(geoms)
    }

    /// `SmFilletVertex::GetVUAtRailEnd`: the first vertex use of `v` attached to an
    /// `SM_FE_RAIL` or `SM_FE_CLIFF_RAIL` edge, skipping `opt_exclude_vu`, uses whose
    /// edge use's fillet geometry is not `opt_fillet_geom` (when given) and uses on
    /// rails other than `opt_fillet_rail` (when given); `None` when there is none.
    ///
    /// # Errors
    ///
    /// As [`FilletBrep::vertex_fillet_geoms`].
    pub fn vu_at_rail_end(
        &self,
        v: VertexId,
        opt_fillet_geom: Option<FilletGeomId>,
        opt_exclude_vu: Option<VertexuseId>,
        opt_fillet_rail: Option<EdgeId>,
    ) -> Result<Option<VertexuseId>> {
        for vu in vertexuses(&self.brep, v)? {
            let eu = eu1::vertexuse_edgeuse(&self.brep, vu)?
                .ok_or_else(|| SolidError::null_pointer(OP_VERTEX))?;
            let e = self
                .brep
                .edgeuse(eu)?
                .edge
                .ok_or_else(|| SolidError::null_pointer(OP_VERTEX))?;
            let edge_type = self.fillet_edge(e)?.fillet_edge_type();

            // when edge is a RAIL or CLIFF_RAIL type
            if edge_type == FilletEdgeType::Rail || edge_type == FilletEdgeType::CliffRail {
                // skip excluded filletVertices
                if opt_exclude_vu == Some(vu) {
                    continue;
                }

                // skip excluded filletGeoms
                let fg = self.fillet_edgeuse(eu)?.fillet_geom();
                if opt_fillet_geom.is_some() && fg != opt_fillet_geom {
                    continue;
                }

                // skip excluded fillet Rails
                if opt_fillet_rail.is_some_and(|rail| rail != e) {
                    continue;
                }

                return Ok(Some(vu));
            }
        }
        Ok(None) // Could not find it!
    }

    // ---- SmFilletEdgeuse ------------------------------------------------------------

    /// `SmFilletEdgeuse::SetFilletGeom`: sets the back pointer, adding the use to the
    /// geometry's referenced edge uses.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `eu` is not a fillet edge use;
    /// [`SolidError::InvalidInput`] for an unknown geometry.
    pub fn set_edgeuse_fillet_geom(
        &mut self,
        eu: EdgeuseId,
        fillet_geom: Option<FilletGeomId>,
    ) -> Result<()> {
        if !self.edgeuses.contains_key(&eu) {
            return Err(SolidError::NotFound {
                operation: OP_EDGEUSE,
                entity: EntityRef::Edgeuse(eu),
            });
        }
        if let Some(g) = fillet_geom {
            self.geom_mut(g)?.add_referenced_fillet_edgeuse(eu);
        }
        if let Some(rec) = self.edgeuses.get_mut(&eu) {
            rec.fillet_geom = fillet_geom;
        }
        Ok(())
    }

    /// `SmFilletEdgeuse::SetFilletVertexuse` (`m_pVU`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge use id.
    pub fn set_edgeuse_fillet_vertexuse(
        &mut self,
        eu: EdgeuseId,
        vu: Option<VertexuseId>,
    ) -> Result<()> {
        self.brep.edgeuse_mut(eu)?.vertexuse = vu;
        Ok(())
    }

    /// `SmFilletEdgeuse::SetUVCurve(pUVTrimCurve, bDeleteOldCurve, bDbgWarnLeaks)`:
    /// replaces the edge use's UV trim curve (an old curve that is not deleted is
    /// leaked upstream and only warned about; here it is always dropped).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling edge use id.
    pub fn set_edgeuse_uv_curve(
        &mut self,
        eu: EdgeuseId,
        uv_trim_curve: Option<Curve>,
        delete_old_curve: bool,
        dbg_warn_leaks: bool,
    ) -> Result<()> {
        let _ = (delete_old_curve, dbg_warn_leaks);
        self.brep.edgeuse_mut(eu)?.uv_trim_curve = uv_trim_curve;
        Ok(())
    }

    // ---- SmFilletEdge ---------------------------------------------------------------

    /// `SmFilletEdge::~SmFilletEdge`: deletes the curve classification, every edge use
    /// of the edge and its vertex use (unlinked from the vertex, `SmVertex::Remove`),
    /// then (`~SmEdge`, `~SmTopology`) the edge's curve and its place in the edge
    /// list; the fillet members are dropped (the topology records stay in the arena,
    /// unreferenced).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] when `e` is not a fillet edge or an id dangles;
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for an edge use without a
    /// vertex use or a vertex use without a vertex.
    pub fn delete_fillet_edge(&mut self, e: EdgeId) -> Result<()> {
        self.fillet_edge(e)?;

        // delete all connected edgeuses and the edgeuses' connected vertexuses
        let uses = edge_unit::edgeuses(&self.brep, e, None, None)?;
        for eu in uses {
            let vu = self
                .brep
                .edgeuse(eu)?
                .vertexuse
                .ok_or_else(|| SolidError::null_pointer(OP_EDGE))?;
            let v = self
                .brep
                .vertexuse(vu)?
                .vertex
                .ok_or_else(|| SolidError::null_pointer(OP_EDGE))?;
            self.brep.vertex_mut(v)?.vertexuses.retain(|&x| x != vu);
            // delete pEU (~SmTopology: unlinked from the edge's list)
            self.brep.edge_mut(e)?.edgeuses.retain(|&x| x != eu);
            self.edgeuses.remove(&eu);
            // delete pVU
            self.vertexuses.remove(&vu);
        }

        // ~SmEdge: the owned curve; ~SmTopology: the edge list
        self.brep.edge_mut(e)?.curve = None;
        self.edge_list.retain(|&x| x != e);
        self.edges.remove(&e);
        Ok(())
    }

    /// `pFilletEdge->GetBrep()->DeleteEdge(pFilletEdge)` on this pseudo-B-rep: the wire
    /// edge is deleted by `SmBrep::DeleteEdge` ([`crate::topology::brep::p4::delete_edge`],
    /// which unlinks its edge uses and vertex uses, keeps a vertex use as a shell
    /// vertex use where its vertex would be left without any, and splits the shell as
    /// needed); `~SmFilletEdge` then finds no edge uses left. The fillet records of the
    /// edge, of its edge uses and of the vertex uses `DeleteEdge` deleted go with them.
    ///
    /// # Errors
    /// `SM_ERR` when editing is disabled or the edge is not a wire edge
    /// (`SmBrep::DeleteEdge`'s checks); those of the deletion.
    pub fn delete_wire_fillet_edge(&mut self, k: &mut Kernel, e: EdgeId) -> Result<()> {
        // check state - m_bEditingEnabled must be on
        if !self.editing_enabled {
            return Err(SolidError::sm_err(OP_EDGE));
        }
        self.fillet_edge(e)?;
        let uses = edge_unit::edgeuses(&self.brep, e, None, None)?;
        let mut vus = Vec::new();
        for &eu in &uses {
            if let Some(vu) = self.brep.edgeuse(eu)?.vertexuse {
                vus.push(vu);
            }
        }
        // The pseudo-B-rep's m_bEditingEnabled is this record's flag (checked above).
        let inner_editing = self.brep.editing_enabled();
        self.brep.set_editing_enabled(true);
        let deleted = crate::topology::brep::p4::with_local_env(|d| {
            crate::topology::brep::p4::delete_edge(k, &mut self.brep, d, e, None)
        });
        self.brep.set_editing_enabled(inner_editing);
        deleted?;
        for eu in uses {
            self.edgeuses.remove(&eu);
        }
        for vu in vus {
            if self.brep.vertexuse(vu).is_err() {
                self.vertexuses.remove(&vu);
            }
        }
        self.edge_list.retain(|&x| x != e);
        self.edges.remove(&e);
        Ok(())
    }

    /// `SmFilletEdge::GetOriginalUVCurve`: the UV trim curve of the primary edge use's
    /// mate.
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for an edge without a primary
    /// edge use or a primary use without a mate.
    pub fn original_uv_curve(&self, e: EdgeId) -> Result<Option<&Curve>> {
        let primary =
            primary_edgeuse(&self.brep, e)?.ok_or_else(|| SolidError::null_pointer(OP_EDGE))?;
        let mate_eu =
            mate(&self.brep, primary)?.ok_or_else(|| SolidError::null_pointer(OP_EDGE))?;
        Ok(self.brep.edgeuse(mate_eu)?.uv_trim_curve.as_ref())
    }

    /// `SmFilletEdge::FindFilletBrepEdge`: matches this fillet edge's curve to an edge
    /// of `fillet_brep` among `edges` (the fillet B-rep's edges) and records it as the
    /// fillet B-rep edge. Without `find_multiple_edges`, the first edge whose midpoint
    /// is within the edge tolerance of the curve midpoint (exactly, else the closest
    /// such); with it, the first two edges whose midpoints lie on the curve within the
    /// edge tolerance (exactly, else the two closest).
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for an edge without a curve,
    /// `SM_ERR` when no edge matches; those of the curve evaluations and point solve.
    pub fn find_fillet_brep_edge(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        fillet_brep: &Brep,
        edges: &[EdgeId],
        find_multiple_edges: bool,
    ) -> Result<()> {
        self.fillet_edge(e)?;

        // get this Fillet Edge's curve
        let curve_3d = self
            .brep
            .edge(e)?
            .curve
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP_FIND_EDGE))?;
        if curve_3d.is_degenerate(k, EFF_ZERO, None)? {
            return Ok(());
        }

        // let sPnt = curve's midPoint
        let ivl = curve_3d.natural_interval();
        let pnt = curve_3d.evaluate_point(k, ivl.evaluate(0.5))?;

        // These values are for checking tolerance slop.
        let mut best_edge1: Option<EdgeId> = None;
        let mut best_edge2: Option<EdgeId> = None;
        let mut best_dist1 = BIG_DOUBLE;
        let mut best_dist2 = BIG_DOUBLE;
        let scaled_zero = 10.0 * EFF_ZERO * (1.0 + pnt.max_dimension());

        let record = self.edges.get_mut(&e).ok_or(SolidError::NotFound {
            operation: OP_EDGE,
            entity: EntityRef::Edge(e),
        })?;

        // First just look for a matching midpoint.
        for &pe in edges {
            let edge = fillet_brep.edge(pe)?;
            let edge_curve = edge
                .curve
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP_FIND_EDGE))?;

            // let sEdgePnt = edge midPoint
            let edge_pnt = edge_curve.evaluate_point(k, edge.interval.evaluate(0.5))?;

            if find_multiple_edges {
                // Drop the point onto p3DCurve
                let solutions = curve_3d.global_point_solve::<NoObject>(
                    k,
                    &ivl,
                    SolverOperation::Intersect,
                    edge_pnt,
                    edge.tolerance,
                    SolutionRequested::Single,
                    &GlobalPointSolveOptions::default(),
                )?;

                // when EdgePnt is within tolerance of FilletEdge->Curve
                if let Some(first) = solutions.get(0) {
                    let dist = first.start.solution_value;
                    if dist < scaled_zero {
                        if record.fillet_brep_edge1.is_none() {
                            record.fillet_brep_edge1 = Some(pe);
                        } else if record.fillet_brep_edge2.is_none() {
                            record.fillet_brep_edge2 = Some(pe);
                            break; // Found two good ones, all done.
                        }
                        // else: "Unexpected branch" (debug warning only)
                    } else {
                        // Close, but not a great result.
                        if dist < best_dist1 {
                            best_dist2 = best_dist1;
                            best_dist1 = dist;
                            best_edge2 = best_edge1;
                            best_edge1 = Some(pe);
                        } else if dist < best_dist2 {
                            best_dist2 = dist;
                            best_edge2 = Some(pe);
                        }
                    }
                }
            } else {
                // when distance between midpoints is less than tolerance
                let dist = edge_pnt.distance(pnt);
                if dist < edge.tolerance {
                    if dist < scaled_zero {
                        // Remember that this edge maps to this fillet edge.
                        record.fillet_brep_edge1 = Some(pe);
                        return Ok(());
                    }

                    // Close, but not a great result.
                    if dist < best_dist1 {
                        best_edge1 = Some(pe);
                        best_dist1 = dist;
                    }
                }
            }
        }

        // every filletEdge should map to a Brep edge
        if record.fillet_brep_edge1.is_some() {
            return Ok(());
        }

        // Didn't set it.  See if we have a close approach.
        record.fillet_brep_edge1 = best_edge1;
        if find_multiple_edges {
            record.fillet_brep_edge2 = best_edge2;
        }

        // If that got it, we're done.
        if record.fillet_brep_edge1.is_some() {
            return Ok(());
        }

        Err(SolidError::sm_err(OP_FIND_EDGE))
    }

    // ---- SmFilletGeom ---------------------------------------------------------------

    /// `SmFilletGeom::GetRailVertexuse(lRailIndex, lEndIndex)`: the vertex use of the
    /// rail's edge use at the given end (the primary edge use's, or its mate's when the
    /// primary use is reversed; the other mate for the end `1`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index past 1; [`SolidError::Upstream`]
    /// (`SM_ERR_NULL_POINTER`) for a missing rail, primary edge use or mate (upstream
    /// asserts); [`SolidError::NotFound`] for dangling ids.
    pub fn rail_vertexuse(
        &self,
        g: FilletGeomId,
        rail_index: usize,
        end_index: usize,
    ) -> Result<Option<VertexuseId>> {
        if rail_index >= 2 || end_index >= 2 {
            return Err(SolidError::invalid_input(OP_RAIL_VU, "index past 1"));
        }
        let rail = self
            .geom(g)?
            .rail(rail_index)
            .ok_or_else(|| SolidError::null_pointer(OP_RAIL_VU))?;
        let mut eu = primary_edgeuse(&self.brep, rail)?
            .ok_or_else(|| SolidError::null_pointer(OP_RAIL_VU))?;
        if self.brep.edgeuse(eu)?.orientation == OrientType::Opposite {
            eu = mate(&self.brep, eu)?.ok_or_else(|| SolidError::null_pointer(OP_RAIL_VU))?;
        }
        if end_index != 0 {
            eu = mate(&self.brep, eu)?.ok_or_else(|| SolidError::null_pointer(OP_RAIL_VU))?;
        }
        Ok(self.brep.edgeuse(eu)?.vertexuse)
    }

    /// `SmFilletGeom::Trim(rTrimInterval)`: trims the rails and other rails (curve and
    /// edge interval), the centre line and the fillet surface's U domain to
    /// `trim_interval` (the curve trims may snap it to existing knots).
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a missing rail;
    /// [`SolidError::Unported`] from a trim; [`SolidError::NotFound`] for dangling ids.
    pub fn trim_geom(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        trim_interval: &mut Extent1d,
    ) -> Result<()> {
        let geom = self.geom(g)?;
        let mut rails: Vec<EdgeId> = Vec::new();
        for rail in geom.rails {
            rails.push(rail.ok_or_else(|| SolidError::null_pointer(OP_TRIM))?);
        }
        rails.extend_from_slice(&geom.other_rails1);
        rails.extend_from_slice(&geom.other_rails2);

        // Rails and OtherRails:
        for rail in rails {
            let edge = self.brep.edge_mut(rail)?;
            if let Some(curve) = edge.curve.as_mut() {
                // may snap sIvl by tol to existing knots
                ignore_non_unported(curve.trim(k, trim_interval, TrimMode::Notify))?;
                let temp = edge
                    .interval
                    .intersect(trim_interval)
                    .unwrap_or_else(|_| Extent1d::new());
                set_interval(&mut self.brep, rail, &temp)?;
            }
        }

        let geom = self.geom_mut(g)?;

        // Spine curve:
        if let Some(center) = geom.center_line_curve.as_mut() {
            ignore_non_unported(center.trim(k, trim_interval, TrimMode::Notify))?;
        }

        // Fillet Surface:
        if let Some(surface) = geom.fillet_surface_mut(None)? {
            let mut domain = surface.natural_uv_domain_with_nurb(k)?;
            // Trimming is in the u-direction.
            let u_domain = domain.u_interval();
            let temp = u_domain
                .intersect(trim_interval)
                .unwrap_or_else(|_| Extent1d::new());
            domain.set_u_interval(&temp);

            // trim to domain
            ignore_non_unported(surface.trim_with_domain(k, &mut domain))?;
        }

        Ok(())
    }
}

/// Ignores a status upstream does not check, but never an [`SolidError::Unported`].
fn ignore_non_unported<T>(r: Result<T>) -> Result<()> {
    match r {
        Err(e) if e.is_unported() => Err(e),
        _ => Ok(()),
    }
}

/// The solution handles of `SmFilletVertex::CalcCliffRailIntEdge`'s intersection:
/// upstream's solutions point at the target curve and the fillet surface, whose
/// natural domains the solver reads back (`SmSolution::GetParameterInterval`).
#[derive(Clone, Copy)]
pub(crate) enum CliffHandle<'a> {
    Curve(&'a Curve),
    Surface(&'a Surface),
}

impl PartialEq for CliffHandle<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Curve(a), Self::Curve(b)) => std::ptr::eq(*a, *b),
            (Self::Surface(a), Self::Surface(b)) => std::ptr::eq(*a, *b),
            _ => false,
        }
    }
}

impl SolutionObject for CliffHandle<'_> {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(_) => ObjectKind::Curve,
            Self::Surface(_) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(c) => Some(c.natural_interval()),
            Self::Surface(_) => None,
        }
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        match self {
            Self::Curve(_) => None,
            // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
            Self::Surface(s) => Some(s.natural_uv_domain_if_built()),
        }
    }
}

/// The surface the point solves of `sm_SurfaceDropPoint` (and so of
/// `sm_TwoPntsIntersection`) name: upstream's solutions point at `pSurface`
/// (`SmSurface::GlobalPointSolve`, `this`, SmFilletGeom.cpp:161), whose natural UV domain
/// `SmSolution::GetParameterInterval` reads back. `SmGlobalSolver::IdenticalSolutions`
/// (SmGlobalSolver.cpp:1910-1931) merges two solutions whose parameters differ by less
/// than `1e-4` of that domain's sides, so a seam point dropped onto a closed surface keeps
/// exactly the solutions upstream keeps (`sm_TwoPntsIntersection` branches on
/// `GetSize() == 2`, SmFilletGeom.cpp:286). Compares by identity, like a pointer.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceHandle<'a>(pub &'a Surface);

impl PartialEq for SurfaceHandle<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl SolutionObject for SurfaceHandle<'_> {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        Some(self.0.natural_uv_domain_if_built())
    }
}

/// The curve object a curve solve's solutions name (upstream the curve itself, `this`
/// of `SmCurve::GlobalPointSolve` / `GlobalCurveIntersect`, or the analytic replacement
/// its curve cache may make of it), with the natural interval
/// `SmSolution::GetParameterInterval` reads back from it. `SmGlobalSolver::IdenticalSolutions`
/// (SmGlobalSolver.cpp:1910-1931) merges two solutions whose parameters differ by less
/// than `1e-4` of that interval, so the solutions must name the curve, as upstream's do.
///
/// `id` tells the objects of one solve apart (two curves, and their analytic
/// replacements). Assumption: an analytic replacement carries the same interval as its
/// curve (`ivl` of the handle for it is the curve's own). That holds only if the curve
/// cache's analytic replacement keeps the B-spline's parameterisation; upstream reads the
/// replacement's own `GetNaturalInterval()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveObjectHandle {
    /// Which object of the solve this is.
    pub id: u8,
    /// Its natural parameter interval.
    pub ivl: Extent1d,
}

impl SolutionObject for CurveObjectHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.ivl)
    }
}

// ---- static helpers ---------------------------------------------------------------------

/// The UV point of a surface point-solve solution.
fn solution_uv<O: SolutionObject>(solutions: &SolutionArray<O>, index: usize) -> Point2 {
    solutions.get(index).map_or(Point2::new(0.0, 0.0), |s| {
        Point2::new(
            s.start.parameters.first().copied().unwrap_or(0.0),
            s.start.parameters.get(1).copied().unwrap_or(0.0),
        )
    })
}

/// `sm_EvaluateMate`: the point of a `SM_FV_MATE` vertex on `face`'s surface at `uv`,
/// using the surface's extension when `uv` is outside its natural domain.
///
/// # Errors
///
/// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a face without a surface or one
/// that is not a B-spline surface (the `SM_CAST_PTR` upstream); those of the
/// evaluations.
pub fn evaluate_mate(k: &Kernel, brep: &Brep, face: FaceId, uv: Point2) -> Result<Point3> {
    let surface =
        face2::surface(brep, face)?.ok_or_else(|| SolidError::null_pointer(OP_EVALUATE_MATE))?;
    // SM_CAST_PTR(SmBSplineSurface, ..) and NER before the domain test: B-spline surfaces
    // and the kinds deriving from them (planes, surfaces of revolution, cones, spheres,
    // tori); the other kinds fail the cast
    if !matches!(
        surface.kind(),
        SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
    ) {
        return Err(SolidError::null_pointer(OP_EVALUATE_MATE));
    }
    let natural_uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    if natural_uv_domain.contains_point(uv, 0.0) {
        surface.evaluate_point(k, uv)
    } else {
        match surface.kind_with_nurb(k)? {
            SurfaceKind::BSpline(s) => s.evaluate_point_on_extended_surface(uv),
            SurfaceKind::Plane(p) => p.base_for_query().evaluate_point_on_extended_surface(uv),
            SurfaceKind::SurfOfRevolution(s) => s.base().evaluate_point_on_extended_surface(uv),
            SurfaceKind::SurfOfExtrusion(s) => s.base().evaluate_point_on_extended_surface(uv),
            SurfaceKind::Cone(c) => c.rev().base().evaluate_point_on_extended_surface(uv),
            SurfaceKind::Sphere(s) => s.base().base().evaluate_point_on_extended_surface(uv),
            SurfaceKind::Torus(s) => s.base().base().evaluate_point_on_extended_surface(uv),
            _ => Err(SolidError::null_pointer(OP_EVALUATE_MATE)),
        }
    }
}

/// `sm_SurfaceDropPoint`: drops a point known to be on the surface onto it
/// (`GlobalPointSolve` with `SM_SO_INTERSECT`, every solution kept) with a tolerance
/// starting at `approx_tol / 100` and multiplied by 10 up to four times until a solution
/// is found.
///
/// # Errors
///
/// [`SolidError::Upstream`] (`SM_ERR`) when no tolerance finds a solution; those of the
/// point solve.
///
/// `handle` names `surface` in the solutions (upstream's solutions point at it).
pub fn surface_drop_point<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    handle: Option<O>,
    pnt: Point3,
    approx_tol: f64,
) -> Result<SolutionArray<O>> {
    // the surface cache the solves below use; a surface whose cache cannot be built
    // (its Bezier decomposition fails, UB-166's u direction) is a null pointer here
    if let Err(e) = get_surface_cache(k, surface, handle) {
        return Err(if e.is_unported() {
            e
        } else {
            SolidError::null_pointer(OP_SURFACE_DROP_POINT)
        });
    }
    let domain = surface.natural_uv_domain_with_nurb(k)?;

    // for a sequence of intersection tolerance distances
    let mut dropping_tol = approx_tol / 100.0;
    for _ in 0..5 {
        // intersect the point with the surface; the double converts to the solve's
        // SmZoneTol3d parameter (USE_DEFAULT -> zone tolerance, then fabs)
        let solutions = surface.global_point_solve::<O>(
            k,
            handle,
            &domain,
            SolverOperation::Intersect,
            pnt,
            zone_tol3d(k, dropping_tol),
            None,
            SolutionRequested::All,
        )?;
        if !solutions.is_empty() {
            return Ok(solutions);
        }
        dropping_tol *= 10.0;
    }

    // Unable to drop point
    Err(SolidError::sm_err(OP_SURFACE_DROP_POINT))
}

/// Whether the surface is closed in `param` over `domain` (`IsClosed`).
fn is_closed(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    param: SurfParamType,
) -> Result<bool> {
    Ok(surface.is_closed(k, domain, param, None, false)?.0)
}

/// `sm_TwoPntsIntersection`: the surface/surface intersection curve through a start
/// point (and optionally an end point) known to be on both surfaces, headed along
/// `direction`. The points are dropped onto both surfaces (on a closed surface, the
/// seam copy on the side `direction` points to), then `PointBasedSurfaceIntersect` traces
/// the curve at `tol_3d` with a 30 degree angle tolerance.
///
/// The drops' solutions name their surface ([`SurfaceHandle`]), as upstream's do
/// (`sm_SurfaceDropPoint`, SmFilletGeom.cpp:161): `SmGlobalSolver::IdenticalSolutions`
/// then merges seam near-duplicates within `1e-4` of the surface's UV domain, and the
/// `solutions.len() == 2` seam test (cpp:286) counts what upstream counts (review of
/// #264; with the object-less handle the tolerance was only `100 * EFF_ZERO * (1 + |s|)`).
///
/// # Errors
///
/// Those of the point drops ([`surface_drop_point`]) and of
/// [`Surface::point_based_surface_intersect`]; [`SolidError::InvalidInput`] for a zero
/// direction. `direction` is unitised in place before the intersection (upstream
/// `pDirection->Unitize()`).
pub fn two_pnts_intersection(
    k: &Kernel,
    start_pnt: Point3,
    opt_end_pnt: Option<Point3>,
    direction: &mut Vec3,
    opt_end_direction: Option<Vec3>,
    surf1: &Surface,
    surf2: &Surface,
    tol_3d: f64,
) -> Result<PointIntersection> {
    // surface size locals
    let domain1 = surf1.natural_uv_domain_with_nurb(k)?;
    let domain2 = surf2.natural_uv_domain_with_nurb(k)?;

    let ssi_tol = tol_3d; // use caller-specified tol for SSI.
    let angle_tol = deg2rad(30.0);
    let dropping_tol = ssi_tol;

    // Drop start point onto surf1 - watch for closed surfaces
    let mut surface1_points: Vec<Point2> = Vec::new();
    let mut surface2_points: Vec<Point2> = Vec::new();
    // (the solutions name the surface they were dropped onto, as upstream's do)
    let solutions = drop_or_bad(surface_drop_point(
        k,
        surf1,
        Some(SurfaceHandle(surf1)),
        start_pnt,
        dropping_tol,
    ))?;
    let mut index11 = 0;
    // Please note that it is possible that this point lies on the seam of a closed surface.
    if solutions.len() == 2 {
        // Closed surface
        let uv = solution_uv(&solutions, 0);
        let uv_dir = drop_vector(k, surf1, uv, *direction)?;
        if (is_closed(k, surf1, &domain1, SurfParamType::U)? && uv_dir.x < 0.0)
            || (is_closed(k, surf1, &domain1, SurfParamType::V)? && uv_dir.y < 0.0)
        {
            index11 = 1;
        }
    }
    let uv11 = solution_uv(&solutions, index11);

    // Drop start point onto surf2 - watch for closed surfaces
    let solutions = drop_or_bad(surface_drop_point(
        k,
        surf2,
        Some(SurfaceHandle(surf2)),
        start_pnt,
        dropping_tol,
    ))?;
    let mut index21 = 0;
    if solutions.len() == 2 {
        // Closed surface
        let uv = solution_uv(&solutions, 0);
        let uv_dir = drop_vector(k, surf2, uv, *direction)?;
        if (is_closed(k, surf2, &domain2, SurfParamType::U)? && uv_dir.x < 0.0)
            || (is_closed(k, surf2, &domain2, SurfParamType::V)? && uv_dir.y < 0.0)
        {
            index21 = 1;
        }
    }
    let uv21 = solution_uv(&solutions, index21);

    // add start points into surfacePoint arrays
    surface1_points.push(uv11);
    surface2_points.push(uv21);

    // when given a stop point - drop it onto the surfaces
    if let Some(end_pnt) = opt_end_pnt {
        // Drop end point onto surf1 - watch for closed surfaces
        let solutions =
            surface_drop_point(k, surf1, Some(SurfaceHandle(surf1)), end_pnt, dropping_tol)?;
        let mut index12 = 0;
        if solutions.len() == 2 {
            // Closed surface
            for ii in 0..2 {
                let uv = solution_uv(&solutions, ii);
                let uv_dir = drop_vector(k, surf1, uv, *direction)?;
                if uv_dir.dot(uv - uv11) < 0.0 {
                    index12 = 1;
                    break;
                }
            }
        }
        let uv12 = solution_uv(&solutions, index12);

        // Drop end point onto surf2 - watch for closed surfaces
        let solutions =
            surface_drop_point(k, surf2, Some(SurfaceHandle(surf2)), end_pnt, dropping_tol)?;
        let mut index22 = 0;
        if solutions.len() == 2 {
            // Closed surface
            for ii in 0..2 {
                let uv = solution_uv(&solutions, ii);
                let uv_dir = drop_vector(k, surf2, uv, *direction)?;
                if uv_dir.dot(uv - uv21) < 0.0 {
                    index22 = 1;
                }
            }
        }
        let uv22 = solution_uv(&solutions, index22);

        // add dropped endPoints to SurfacePoint arrays
        surface1_points.push(uv12);
        surface2_points.push(uv22);
    }

    // SER(pDirection->Unitize()): a zero direction is stored as (0,0,1) before the
    // error is returned
    *direction = match direction.unitize() {
        Ok(unit) => unit,
        Err(e) => {
            *direction = Vec3::Z;
            return Err(e);
        }
    };

    surf1.point_based_surface_intersect(
        k,
        &domain1,
        surf2,
        &domain2,
        &surface1_points,
        &surface2_points,
        [false, false],
        [Some(*direction), opt_end_direction],
        Some(ssi_tol),
        Some(angle_tol),
    )
}

/// The start-point drops of `sm_TwoPntsIntersection`: an empty success is
/// `SM_ERR_BAD_INTERSECTIONS`, any other status passes through.
fn drop_or_bad<O: SolutionObject>(r: Result<SolutionArray<O>>) -> Result<SolutionArray<O>> {
    let solutions = r?;
    if solutions.is_empty() {
        return Err(upstream(OP_TWO_PNTS, SM_ERR_BAD_INTERSECTIONS));
    }
    Ok(solutions)
}

/// `DropVectors(uv, TRUE, TRUE, 1, &direction, &uv_dir)`.
fn drop_vector(k: &Kernel, surface: &Surface, uv: Point2, direction: Vec3) -> Result<Vec2> {
    let dropped = drop_vectors_at(k, surface, uv, Side::Left, Side::Left, &[direction])?;
    dropped
        .first()
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP_TWO_PNTS))
}

/// `sm_FindStepoffUV`: the guess UVs for `PointOnPlaneSolve`: for each of the solver's
/// two edge uses, the point stepped off the edge along the binormal by
/// `|stepoff_dist|` at `curve_param`, dropped (closest point) onto the edge use's face
/// surface within the face's UV domain.
///
/// # Errors
///
/// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a solver without both edge
/// uses, an edge use without a face or a face without a surface; `SM_ERR` when a drop
/// finds nothing; those of the step-off evaluation and point solve.
pub fn find_stepoff_uv(
    k: &Kernel,
    brep: &mut Brep,
    solver: &ConstantRadiusAssistedFs,
    curve_param: f64,
    stepoff_dist: f64,
) -> Result<[Point2; 2]> {
    let mut uvs = [Point2::new(0.0, 0.0); 2];
    for (slot, edgeuse) in uvs.iter_mut().zip(solver.solver.edgeuses) {
        let eu = edgeuse.ok_or_else(|| SolidError::null_pointer(OP_STEPOFF))?;
        let face = edgeuse_face(brep, eu)?.ok_or_else(|| SolidError::null_pointer(OP_STEPOFF))?;
        face2::surface(brep, face)?.ok_or_else(|| SolidError::null_pointer(OP_STEPOFF))?;
        // Step off the edge along the binormal direction
        let step = evaluate_binormal_step_off(
            k,
            brep,
            eu,
            curve_param,
            stepoff_dist.abs(),
            &StepOffOptions::default(),
        )?;
        // The step-off needs the B-rep mutably, so the face's surface (checked above,
        // in upstream's order) is borrowed again after it.
        let surface =
            face2::surface(brep, face)?.ok_or_else(|| SolidError::null_pointer(OP_STEPOFF))?;
        let tol = solver.solver.approx_tol_3d;
        // Make the surface think that it is a standalone surface.
        let solutions = surface.global_point_solve::<NoObject>(
            k,
            Some(NoObject),
            &face2::uv_domain(brep, face)?,
            SolverOperation::Minimize,
            step.point,
            zone_tol3d(k, tol),
            None,
            SolutionRequested::Single,
        )?;
        if solutions.is_empty() {
            return Err(SolidError::sm_err(OP_STEPOFF));
        }
        *slot = solution_uv(&solutions, 0);
    }
    Ok(uvs)
}

/// `CalculateUVDomainFromUVTrimCurves(rNewFaceDomain)` with upstream's defaults
/// (`bLimitDomain = TRUE`, no loop gaps) and its status ignored, as upstream does: on
/// any error but an unported callee the input domain stands.
fn uv_domain_from_uv_trim_curves(
    k: &Kernel,
    brep: &mut Brep,
    face: FaceId,
    domain: Extent2d,
) -> Result<Extent2d> {
    match calculate_uv_domain_from_uv_trim_curves(k, brep, face, true, false, false) {
        Ok(d) => Ok(d.domain),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(domain),
    }
}

/// `SmFace::Point3DClassify` ([`crate::query::face::p1::point_3d_classify`]): the
/// classification's point class and whether the call returned `SM_SUCCESS` (upstream
/// compares the status and does not propagate it). `zone_tol_3d` is
/// `SmTol::GetZoneTol3d(pOrigFace)` for both faces; the flags are upstream's `TRUE`
/// (check boundaries first), `FALSE` (target loops only), `NULL` (target loops) and
/// `TRUE` (okay to work in UV space). A failed call keeps the class written before the
/// failure; an unported callee propagates.
#[allow(clippy::too_many_arguments)]
fn point_3d_classify(
    k: &Kernel,
    brep: &mut Brep,
    face: FaceId,
    pos: Point3,
    zone_tol_3d: f64,
    check_boundaries_first: bool,
    guess_uv: Option<Point2>,
    target_loops_only: bool,
    target_loops: Option<&[crate::topology::LoopId]>,
    ok_in_uv_space: bool,
) -> Result<(PointClassType, bool)> {
    let mut pc = PointClassification::<EntityRef>::new(zone_tol_3d);
    let r = crate::query::face::p1::point_3d_classify(
        k,
        brep,
        face,
        pos,
        zone_tol_3d,
        check_boundaries_first,
        &mut pc,
        crate::query::face::p1::Point3dClassifyOptions {
            guess_uv,
            tgt_loops_only: target_loops_only,
            tgt_loops: target_loops,
            uv_space_okay: ok_in_uv_space,
        },
    );
    match r {
        Ok(()) => Ok((pc.point_class(), true)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok((pc.point_class(), false)),
    }
}

/// `sm_ClassifyFacePoint`: which of two faces that were one face before a split
/// contains `pos`: `0` neither, `1` `orig_face` only, `2` `new_face` only, `3` both.
/// Both surfaces get a normalised point drop (success only inside the face's trimmed UV
/// domain); when that does not decide (both or neither drop, gaps within a factor 20),
/// each face's `Point3DClassify` does.
///
/// # Errors
///
/// [`SolidError::Unported`]: `SmFace::CreateUVTrimCurves` (`topology.face_p3`, through
/// `SmFace::CalculateUVDomainFromUVTrimCurves`), the unported callees of
/// `SmFace::Point3DClassify` ([`crate::query::face::p1::point_3d_classify`]);
/// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a face without a surface.
pub fn classify_face_point(
    k: &Kernel,
    brep: &mut Brep,
    pos: Point3,
    orig_face: FaceId,
    new_face: FaceId,
) -> Result<i32> {
    let face_domain1 = face2::uv_domain(brep, orig_face)?;
    let face_domain1 = uv_domain_from_uv_trim_curves(k, brep, orig_face, face_domain1)?;

    // Use Normalize: it should be over the Face it's in, and don't bother finding the
    // closest approach if it's not.
    let (stat1, mut success1, uv1, gap1) = normalized_drop(k, brep, orig_face, pos, &face_domain1)?;
    if success1 && !face_domain1.contains_point(uv1, 0.0) {
        success1 = false;
    }

    let face_domain2 = face2::uv_domain(brep, new_face)?;
    let face_domain2 = uv_domain_from_uv_trim_curves(k, brep, new_face, face_domain2)?;

    let (stat2, mut success2, uv2, gap2) = normalized_drop(k, brep, new_face, pos, &face_domain2)?;
    if success2 && !face_domain2.contains_point(uv2, 0.0) {
        success2 = false;
    }

    // Best situation: the point drops to one Face and not the other.
    if stat1 && !stat2 {
        return Ok(1);
    }
    if success1 && !success2 {
        return Ok(1);
    }
    if stat2 && !stat1 {
        return Ok(2);
    }
    if success2 && !success1 {
        return Ok(2);
    }

    // If both succeeded, check gaps.
    if success1 {
        if gap2 > 0.0 && gap1 > gap2 * 20.0 {
            return Ok(2);
        }
        if gap1 > 0.0 && gap2 > gap1 * 20.0 {
            return Ok(1);
        }
    }

    // If we're still here, we'll have to do the more expensive point-in-Face test.
    let guess_uv = if success1 { Some(uv1) } else { None };
    // SmTol::GetZoneTol3d(pOrigFace): the face's tolerance returned as an SmZoneTol3d,
    // used for both faces
    let tol = zone_tol3d(k, face2::tolerance(brep, orig_face)?);
    let (class1, stat1) = point_3d_classify(
        k, brep, orig_face, pos, tol, true, guess_uv, false, None, true,
    )?;
    let success1 = class1 != PointClassType::Unknown;

    let guess_uv = if success2 { Some(uv2) } else { None };
    let (class2, stat2) = point_3d_classify(
        k, brep, new_face, pos, tol, true, guess_uv, false, None, true,
    )?;
    let success2 = class2 != PointClassType::Unknown;

    if stat1 && !stat2 {
        return Ok(1);
    }
    if success1 && !success2 {
        return Ok(1);
    }
    if stat2 && !stat1 {
        return Ok(2);
    }
    if success2 && !success1 {
        return Ok(2);
    }

    // Here again, bSuccess1 == bSuccess2.
    if success1 {
        return Ok(3);
    }

    // Point is on the surface, and within both Face domains, but it's not actually
    // within the boundaries of either Face. Let the caller deal with it.
    Ok(0)
}

/// `pFace->GetSurface()->DropPoint(pos, domain, NULL, success, uv, gap, multi,
/// SM_SO_NORMALIZE)`: whether the call succeeded, and its outputs.
fn normalized_drop(
    k: &Kernel,
    brep: &Brep,
    face: FaceId,
    pos: Point3,
    domain: &Extent2d,
) -> Result<(bool, bool, Point2, f64)> {
    let surface =
        face2::surface(brep, face)?.ok_or_else(|| SolidError::null_pointer(OP_CLASSIFY))?;
    let opts = DropPointOptions {
        guess_uv: None,
        operation: SolverOperation::Normalize,
        tgt_seam_side: None,
        want_solutions: false,
    };
    match surface.drop_point::<NoObject>(k, pos, domain, &opts) {
        Ok(d) => Ok((true, d.success, d.result_uv, d.gap)),
        Err(e) if e.is_unported() => Err(e),
        // DropPoint sets dGap = SM_BIG_DOUBLE before failing
        Err(_) => Ok((false, false, Point2::new(0.0, 0.0), BIG_DOUBLE)),
    }
}

/// `sm_InterpolateEndptsAndTangents`: a curve of dimension `curve_dim` (2 or 3; else 3)
/// from two end points and tangents: a line when the tangents are colinear with the
/// chord, else a conic (a circular arc, or a parabola) through the tangent lines'
/// intersection when it lies between the points and not too close to either, else a
/// cubic Hermite with tangent lengths scaled by the intersection distances. The tangents
/// are unitised, then scaled for the Hermite case, in place.
///
/// # Errors
///
/// Those of the line/line closest point, the angle computation and the curve creation.
pub fn interpolate_endpts_and_tangents(
    start_pnt: Point3,
    end_pnt: Point3,
    start_vec: &mut Vec3,
    end_vec: &mut Vec3,
    curve_dim: u64,
) -> Result<BSplineCurve> {
    let curve_dim = if (2..=3).contains(&curve_dim) {
        curve_dim
    } else {
        3
    };

    // Unitize tangents  // Don't quit just because of zero vectors.
    *start_vec = start_vec.unitize_or_z();
    *end_vec = end_vec.unitize_or_z();

    // Note: IsColinearWith() returns TRUE if the points are coincident.
    let dist_between_pts = start_pnt.distance(end_pnt);

    if dist_between_pts > EFF_ZERO && start_vec.is_colinear_with(*end_vec, start_pnt, end_pnt) {
        // Will create a line first
        return BSplineCurve::create_line_segment(curve_dim, start_pnt, end_pnt);
    }

    // Will attempt to make a conic
    let (t1, t2) = line_line_closest_point(start_pnt, *start_vec, end_pnt, *end_vec)?;

    // If the intersection is between the points, the intersection parameters will
    // have opposite signs.
    let mut can_make_conic = t1 * t2 < -EFF_ZERO;

    // Also don't make conic if it hits right next to one point.
    can_make_conic &= t1.abs() > 0.1 * dist_between_pts;
    can_make_conic &= t2.abs() > 0.1 * dist_between_pts;

    let knots = [0.0, 1.0];
    if can_make_conic {
        // Make conic
        let mid_pnt = start_pnt + *start_vec * t1;
        let cntrl_poly = [start_pnt, mid_pnt, end_pnt];
        let knot_mult = [3usize, 3];
        let mut weights = [1.0, 1.0, 1.0];
        if (t1 + t2).abs() < EFF_ZERO_SQRT {
            // Make circular arc
            let vec = end_pnt - start_pnt;
            let angle = vec.angle_between(*start_vec)?;
            weights[1] = angle.cos();
            BSplineCurve::create_canonical(&CanonicalData {
                dim: curve_dim,
                degree: 2,
                control_points: &cntrl_poly,
                form: BSplineCurveForm::Unspecified,
                knot_multiplicities: &knot_mult,
                knots: &knots,
                knot_type: KnotType::Unspecified,
                weights: Some(&weights),
                trim: None,
            })
        } else {
            // Make parabola: no weights.
            BSplineCurve::create_canonical(&CanonicalData {
                dim: curve_dim,
                degree: 2,
                control_points: &cntrl_poly,
                form: BSplineCurveForm::Unspecified,
                knot_multiplicities: &knot_mult,
                knots: &knots,
                knot_type: KnotType::Unspecified,
                weights: None,
                trim: None,
            })
        }
    } else {
        // Can not make conic curve, create cubic Hermite instead
        // First, Adjust the lengths of direction vectors
        let mut t1 = t1.abs();
        let mut t2 = t2.abs();
        // Don't allow zero-length vectors (coincident control points)
        if t1 < t2 / 10.0 {
            t1 = t2 / 10.0;
        }
        if t2 < t1 / 10.0 {
            t2 = t1 / 10.0;
        }
        let sum = t1 + t2;
        *start_vec *= dist_between_pts * t1 / sum;
        *end_vec *= dist_between_pts * t2 / sum;
        let herm = HermiteCurve::new(start_pnt, *start_vec, end_pnt, *end_vec, curve_dim);
        let cntrl_poly = herm.bezier_points();
        let knot_mult = [4usize, 4];
        BSplineCurve::create_canonical(&CanonicalData {
            dim: curve_dim,
            degree: 3,
            control_points: &cntrl_poly,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &knot_mult,
            knots: &knots,
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        })
    }
}

/// `sm_AdjustUVOnSeam`: when `uv` lies on a seam of a closed surface (within a
/// thousandth of the domain length), moves it to the other side of the seam if the
/// surface derivative across the seam there points away from `ref_direction` (angle
/// above 90 degrees).
///
/// # Errors
///
/// Those of the derivative evaluation and the angle computations.
pub fn adjust_uv_on_seam(
    k: &Kernel,
    surf: &Surface,
    ref_direction: Vec3,
    uv: &mut Point2,
) -> Result<()> {
    let domain = surf.natural_uv_domain_with_nurb(k)?;
    let min = domain.min();
    let max = domain.max();

    let (_pnt, mut du, mut dv) = surf.evaluate_1st_derivatives(k, *uv, true, true)?;

    if is_closed(k, surf, &domain, SurfParamType::U)? {
        let tolerance_2d = domain.x_length() / 1000.0;
        if (uv.x - min.x).abs() < tolerance_2d {
            let angle = du.angle_between(ref_direction)?;
            if angle > PI / 2.0 {
                uv.x += domain.x_length();
            }
        } else if (uv.x - max.x).abs() < tolerance_2d {
            du = -du; // Backwards from top of domain.
            let angle = du.angle_between(ref_direction)?;
            if angle > PI / 2.0 {
                uv.x -= domain.x_length();
            }
        }
    }

    if is_closed(k, surf, &domain, SurfParamType::V)? {
        let tolerance_2d = domain.y_length() / 1000.0;
        if (uv.y - min.y).abs() < tolerance_2d {
            let angle = dv.angle_between(ref_direction)?;
            if angle > PI / 2.0 {
                uv.y += domain.y_length();
            }
        } else if (uv.y - max.y).abs() < tolerance_2d {
            dv = -dv; // Backwards from top of domain.
            let angle = dv.angle_between(ref_direction)?;
            if angle > PI / 2.0 {
                uv.y -= domain.y_length();
            }
        }
    }

    Ok(())
}

/// `sm_CreateCubicRailRailInterpolant`: a 2D (parameter space) cubic through the
/// intersection of two solvers' rails on `face`, from `start_pnt` with `start_tangent`
/// to `end_pnt` with `end_tangent`; `bulge_factor` in `[0, 1]` sets the tangent lengths
/// (bigger is sharper).
///
/// # Errors
///
/// Those of `SmFilletSolver::FindIndexOfRailOnFace` and `RailRailIntersect`
/// ([`SolidError::Unported`] at its `SmTemporaryRadiusChange`,
/// `blend.fillet_executive_p2`); [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a
/// rail index past the solver's two edge uses, a missing edge use or vertex use (all
/// dereferenced unchecked upstream); `SM_ERR` when the rails do not intersect; those of
/// the angle computations and [`BSplineCurve::create_canonical`].
pub fn create_cubic_rail_rail_interpolant(
    k: &Kernel,
    brep: &mut Brep,
    fil_solver1: &mut dyn FilletSolverOps,
    fil_solver2: &mut dyn FilletSolverOps,
    face: FaceId,
    corner_vert: VertexId,
    start_pnt: Point3,
    start_tangent: Vec3,
    end_pnt: Point3,
    end_tangent: Vec3,
    bulge_factor: f64,
) -> Result<BSplineCurve> {
    const OP: &str = "blend::fillet_geom::p1::create_cubic_rail_rail_interpolant";
    let solver_edgeuse = |fs: &FilletSolver, i: usize| {
        fs.edgeuses
            .get(i)
            .copied()
            .flatten()
            .ok_or_else(|| SolidError::null_pointer(OP))
    };
    // First, calculate the intersection of two rails
    let rail_index = fil_solver1
        .solver()
        .find_index_of_rail_on_face(brep, Some(face))?;
    let other_rail_index = fil_solver2
        .solver()
        .find_index_of_rail_on_face(brep, Some(face))?;
    // Derive guess uv point here
    let mut eu = solver_edgeuse(fil_solver1.solver(), rail_index)?;
    let eu_vertex = |brep: &Brep, eu: EdgeuseId| -> Result<(VertexuseId, Option<VertexId>)> {
        let vu = eu1::vertexuse(brep, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
        Ok((vu, vertexuse_unit::vertex(brep, vu)?))
    };
    if eu_vertex(brep, eu)?.1 != Some(corner_vert) {
        eu = solver_edgeuse(fil_solver2.solver(), other_rail_index)?;
    }
    let (vu, _) = eu_vertex(brep, eu)?;
    // ComputeUVPoint(sUVGuess) with its default bOKToMakeUVTrimCurve = TRUE; its status
    // is ignored, so a failure leaves SmPoint2d()'s (SM_UNDEF_DOUBLE, SM_UNDEF_DOUBLE)
    let uv_guess = match vertexuse_unit::compute_uv_point(k, brep, vu, true) {
        Ok(uv) => uv,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => Point2::new(UNDEF_DOUBLE, UNDEF_DOUBLE),
    };

    let mut tsect_pnt1 = TsectPnt::default();
    let mut tsect_pnt2 = TsectPnt::default();
    let found_intersection = FilletSolver::rail_rail_intersect(
        fil_solver1,
        fil_solver2,
        k,
        Some(&mut *brep),
        uv_guess,
        rail_index,
        other_rail_index,
        &mut tsect_pnt1,
        &mut tsect_pnt2,
    )?;
    if !found_intersection {
        return Err(SolidError::sm_err(OP));
    }
    let rail_srf = match rail_index {
        0 => SrfIndex::First,
        1 => SrfIndex::Second,
        _ => return Err(SolidError::null_pointer(OP)),
    };
    // SmPoint2d to SmPoint3d: z = 0
    let uv_pnt = Point3::from(tsect_pnt1.uv_pos(rail_srf));

    let vec = uv_pnt - start_pnt;
    let len = bulge_factor * vec.length();
    let angle = start_tangent.angle_between(vec)?;
    let len1 = (len / angle.cos()).abs();
    let p1 = start_pnt + start_tangent * len1;
    let vec = uv_pnt - end_pnt;
    let len = bulge_factor * vec.length();

    let angle = end_tangent.angle_between(vec)?;
    let len1 = (len / angle.cos()).abs();
    let p2 = end_pnt + end_tangent * len1;

    let cntrl_poly = [start_pnt, p1, uv_pnt, p2, end_pnt];
    let knots = [0.0, 0.5, 1.0];
    let knot_mult = [4, 1, 4];
    BSplineCurve::create_canonical(&CanonicalData {
        dim: 2,
        degree: 2,
        control_points: &cntrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots: &knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })
}

/// `smos_CleanUpNoise`: removes floating-point noise up to four decimal places
/// (`1.1300000000007` becomes `1.13`); a value with no such noise is returned
/// unchanged.
#[must_use]
pub fn clean_up_noise(x: f64) -> f64 {
    const NUM_DECIMAL_PLACES: u32 = 4;
    // let   x = dRound + dDecimals
    // where  dRound    = largest integer smaller than x
    // and    dDecimals = the decimals in x.
    let mut tol_scaled = EFF_ZERO * (1.0 + x.abs());
    let mut round = x.floor();
    let mut decimals = x - round;
    let mut incr = 1.0;
    for _ in 1..=NUM_DECIMAL_PLACES {
        // if dDecimals are equal to 1.0
        if 1.0 - decimals < tol_scaled {
            // return integer value (dRound + 1)
            round += incr;
            return round;
        }

        // when dDecimals are equal to 0.0
        if decimals < tol_scaled {
            // return integer value (dRound)
            return round;
        }

        // left shift (base 10) dDecimals so that its largest numeral is now an integer
        incr *= 0.1;
        decimals *= 10.0;
        tol_scaled *= 10.0;
        let one_digit = decimals.floor();
        decimals -= one_digit;
        round += incr * one_digit;
    }
    x // No change
}

/// `sm_CleanupNoise`: [`clean_up_noise`] on both coordinates.
#[must_use]
pub fn cleanup_noise(uv: Point2) -> Point2 {
    Point2::new(clean_up_noise(uv.x), clean_up_noise(uv.y))
}

/// `sm_ExamineNFixFilletGeoms`: at an Nx2 tangent corner where the rails of two
/// adjacent fillets (`geom1` at its start or end, `geom2` likewise) fail to meet at
/// the fillet vertices `start_fv` and `end_fv`, copies the boundary control-point row
/// of the fillet surface that does meet them (within `dist_tol`) onto the one that
/// does not (reversed when the surfaces run in opposite directions; end points only
/// when the rows differ in length) and rebuilds the adjusted surface's rails as its
/// iso-curves. Only cubic-or-higher B-spline fillet surfaces are examined; nothing is
/// done when neither surface needs adjusting.
///
/// # Errors
///
/// `solvers` are the two geometries' fillet solvers (`GetFilletSolver()`), used on the
/// adjusting path for the offset surface each rebuilt rail is dropped onto. The same
/// geometry may be passed twice (`SmFilletEdge::CalcCrossSection` does so for a closed
/// loop).
///
/// # Errors
///
/// [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`) for a geometry without a fillet
/// surface or rails, or, on the adjusting path, without a solver or a solver without
/// the rail's offset surface (upstream dereferences them); `SM_ERR` when both surfaces
/// need adjusting; those of the rail drop onto the solver's offset surface
/// (`SmSurface::DropCurve`, unit `intersect.surface_drop_lib_p3`; a failed drop only
/// skips the rail, [`SolidError::Unported`] propagates).
pub fn examine_n_fix_fillet_geoms(
    k: &Kernel,
    fb: &mut FilletBrep,
    geom1: FilletGeomId,
    at_start_of_geom1: bool,
    geom2: FilletGeomId,
    at_start_of_geom2: bool,
    start_fv: VertexId,
    end_fv: VertexId,
    dist_tol: f64,
    solvers: [Option<&mut ConstantRadiusAssistedFs>; 2],
) -> Result<()> {
    let [solver1, solver2] = solvers;
    examine_n_fix_fillet_geoms_with_solvers(
        k,
        fb,
        geom1,
        at_start_of_geom1,
        geom2,
        at_start_of_geom2,
        start_fv,
        end_fv,
        dist_tol,
        [solver1.map(|s| &s.solver), solver2.map(|s| &s.solver)],
    )
}

/// [`examine_n_fix_fillet_geoms`] with the geometries' `SmFilletSolver` bases
/// (`GetFilletSolver()`, resolved by the caller; the same solver may be passed twice).
/// The rebuilt rail is dropped onto a copy of the solver's offset surface with its
/// offset distance set to `0` (upstream sets it on the solver's surface and restores it
/// after the drop, which itself runs on a copy here).
///
/// # Errors
///
/// As [`examine_n_fix_fillet_geoms`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn examine_n_fix_fillet_geoms_with_solvers(
    k: &Kernel,
    fb: &mut FilletBrep,
    geom1: FilletGeomId,
    at_start_of_geom1: bool,
    geom2: FilletGeomId,
    at_start_of_geom2: bool,
    start_fv: VertexId,
    end_fv: VertexId,
    dist_tol: f64,
    solvers: [Option<&FilletSolver>; 2],
) -> Result<()> {
    let start_pnt = fb.vertex_point(start_fv)?;
    let end_pnt = fb.vertex_point(end_fv)?;

    let geoms = [geom1, geom2];
    let at_start_of_geom = [at_start_of_geom1, at_start_of_geom2];

    // check state - only high degree bspline surfaces can be examined
    {
        let surface1 = fb
            .geom(geom1)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let surface2 = fb
            .geom(geom2)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        if surface1.degree_with_nurb(k, SurfParamType::U)? < 3
            || surface2.degree_with_nurb(k, SurfParamType::U)? < 3
            || !surface1.is_kind_of(BSplineSurface::TYPE)?
            || !surface2.is_kind_of(BSplineSurface::TYPE)?
        {
            // Can only fix non-analytic nurb fillets
            return Ok(());
        }
    }

    // Determine which fillet surface need be adjusted
    let mut adjust_surf = [true, true];
    let mut orient = [OrientType::Same, OrientType::Same];

    // iter each Surface
    for (((&geom, &at_start), adjust), orient_i) in geoms
        .iter()
        .zip(&at_start_of_geom)
        .zip(adjust_surf.iter_mut())
        .zip(orient.iter_mut())
    {
        let surface = fb
            .geom(geom)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        let (uv1, uv2) = if at_start {
            (domain.evaluate(0.0, 0.0), domain.evaluate(0.0, 1.0))
        } else {
            (domain.evaluate(1.0, 0.0), domain.evaluate(1.0, 1.0))
        };

        let pnt1 = surface.evaluate_point(k, uv1)?;
        let pnt2 = surface.evaluate_point(k, uv2)?;
        let mut start_dist = start_pnt.distance(pnt1);
        let dist2 = start_pnt.distance(pnt2);
        let end_dist;
        if start_dist < dist2 {
            end_dist = end_pnt.distance(pnt2);
        } else {
            start_dist = dist2;
            end_dist = end_pnt.distance(pnt1);
            *orient_i = OrientType::Opposite;
        }

        if start_dist < dist_tol && end_dist < dist_tol {
            *adjust = false;
        }
    }

    let [adjust0, adjust1] = adjust_surf;
    if !adjust0 && !adjust1 {
        // Nothing need to be adjusted
        return Ok(());
    }
    if adjust0 && adjust1 {
        // Can not adjust both surfaces
        return Err(SolidError::sm_err(OP_EXAMINE));
    }

    // Copy one row of control points from the 'good' fillet to the 'bad' fillet.
    let (to_geom, to_at_start, from_geom, from_at_start) = if adjust0 {
        (geom1, at_start_of_geom1, geom2, at_start_of_geom2)
    } else {
        (geom2, at_start_of_geom2, geom1, at_start_of_geom1)
    };
    let copy_end_points_only;
    let from_row: Vec<CPoint>;
    {
        let from_surface = fb
            .geom_mut(from_geom)?
            .fillet_surface_mut(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let Some(bs) = b_spline_net_mut(from_surface) else {
            return Ok(());
        };
        let gw = bs.get_or_create_gw_nurb()?;
        let row_index = if from_at_start { 0 } else { gw.n() };
        from_row = gw.row(row_index)?.to_vec();
    }
    let from_count = from_row.len().saturating_sub(1);
    {
        let to_surface = fb
            .geom_mut(to_geom)?
            .fillet_surface_mut(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let Some(bs) = b_spline_net_mut(to_surface) else {
            return Ok(());
        };
        let gw = bs.get_or_create_gw_nurb()?;
        let to_count = gw.m();
        copy_end_points_only = from_count != to_count;
        let row_index = if to_at_start { 0 } else { gw.n() };

        let from_at = |kk: usize| -> Result<CPoint> {
            from_row
                .get(kk)
                .copied()
                .ok_or_else(|| SolidError::invalid_input(OP_EXAMINE, "control net row too short"))
        };
        let [orient0, orient1] = orient;
        if orient0 == orient1 {
            if copy_end_points_only {
                *gw.cpt_mut(row_index, 0)? = from_at(0)?;
                *gw.cpt_mut(row_index, to_count)? = from_at(from_count)?;
            } else {
                for kk in 0..=to_count {
                    *gw.cpt_mut(row_index, kk)? = from_at(kk)?;
                }
            }
        } else if copy_end_points_only {
            *gw.cpt_mut(row_index, 0)? = from_at(from_count)?;
            *gw.cpt_mut(row_index, to_count)? = from_at(0)?;
        } else {
            for kk in 0..=to_count {
                *gw.cpt_mut(row_index, kk)? = from_at(to_count - kk)?;
            }
        }
    }

    // Recreate rail curves
    let adjust_fg = to_geom;
    // pAdjustFG->GetFilletSolver()
    let [solver1, solver2] = solvers;
    let adjust_solver = if adjust0 { solver1 } else { solver2 };
    let (surf_domain, rails) = {
        let g = fb.geom(adjust_fg)?;
        let surface = g
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        (surface.natural_uv_domain_with_nurb(k)?, g.rails)
    };

    for (kk, rail) in rails.into_iter().enumerate() {
        let rail = rail.ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let iso_parameter = if kk == 1 {
            surf_domain.max().y
        } else {
            surf_domain.min().y
        };

        // Produce iso-curve for rail
        let iso = {
            let surface = fb
                .geom(adjust_fg)?
                .fillet_surface(None)?
                .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
            surface.create_iso_parametric_curve(
                k,
                SurfParamType::V,
                iso_parameter,
                0.0,
                None,
                false,
            )?
        };
        // pRail->SetCurve(pIsoCrv, TRUE): the old curve is deleted and the change
        // notification clears the edge uses' UV trim curves and cached gaps
        set_rail_curve(fb.brep_mut(), rail, iso.curve)?;

        // Project the new curve onto the surface of the original brep
        drop_rail_onto_offset_surface(k, fb, adjust_solver, kk, rail, dist_tol)?;
    }

    Ok(())
}

/// The B-spline net of a fillet surface that `IsKindOf(SmBSplineSurface_TYPE)`: a
/// B-spline surface, or the base net of a surface of revolution; a plane never gets
/// here (its degree 1 fails the degree check).
fn b_spline_net_mut(surface: &mut Surface) -> Option<&mut BSplineSurface> {
    match surface.kind_mut() {
        SurfaceKind::BSpline(bs) => Some(bs),
        SurfaceKind::SurfOfRevolution(s) => Some(s.base_mut()),
        SurfaceKind::SurfOfExtrusion(s) => Some(s.base_mut()),
        _ => None,
    }
}

/// `SmEdge::SetCurve(pCurve, TRUE)` on a pseudo-B-rep rail: the `SM_NO_CHANGE_GEOMETRY`
/// notification clears each edge use's UV trim curve and drop-curve record and resets
/// its cached edge/face gap (its mate's too) and the cached gaps of the edge's vertex
/// uses; then the curve replaces the old one.
pub(crate) fn set_rail_curve(brep: &mut Brep, rail: EdgeId, curve: Curve) -> Result<()> {
    for eu in edge_unit::edgeuses(brep, rail, None, None)? {
        let mate_eu = mate(brep, eu)?;
        let rec = brep.edgeuse_mut(eu)?;
        rec.uv_trim_curve = None;
        rec.drop_curve_fail = None;
        rec.max_edge_face_gap.reset();
        let vu = rec.vertexuse;
        if let Some(m) = mate_eu {
            brep.edgeuse_mut(m)?.max_edge_face_gap.reset();
        }
        if let Some(vu) = vu {
            let vu = brep.vertexuse_mut(vu)?;
            vu.vertex_edge_gap.reset();
            vu.vertex_face_gap.reset();
        }
    }
    brep.edge_mut(rail)?.curve = Some(curve);
    Ok(())
}

/// The tail of `sm_ExamineNFixFilletGeoms` for one rebuilt rail: drops the rail's new
/// curve onto the solver's offset surface for that rail with its offset distance set to
/// `0` and, when the drop yields one UV curve, makes it the rail's mate edge use's UV
/// curve.
///
/// `adjust_solver` is the adjusted geometry's `GetFilletSolver()`; upstream dereferences
/// it (and `GetSurface(k)` with `NER`) without a null check, so a missing solver or
/// offset surface is `SM_ERR_NULL_POINTER`.
fn drop_rail_onto_offset_surface(
    k: &Kernel,
    fb: &mut FilletBrep,
    adjust_solver: Option<&FilletSolver>,
    rail_index: usize,
    rail: EdgeId,
    dist_tol: f64,
) -> Result<()> {
    let solver = adjust_solver.ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
    // SmFilletSolver::GetSurface(k); NER(pOffsetSurf). The offset distance is set to
    // 0 on a copy (upstream sets it on the solver's surface and restores it below).
    let mut offset_surf = solver
        .surface(rail_index)?
        .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?
        .surface
        .clone();
    let offset_surf = &mut offset_surf;
    offset_surf.set_offset_distance(0.0);

    // Drop the 3d curve onto the surface
    let iso_crv = fb
        .brep()
        .edge(rail)?
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
    let domain = offset_natural_uv_domain(offset_surf);
    let drop = offset_surface_drop_curve(
        k,
        offset_surf,
        &domain,
        iso_crv,
        &iso_crv.natural_interval(),
        // the double converts to DropCurve's SmApproxTol3d parameter
        approx_tol3d(k, dist_tol),
    );
    let uv_curves = match drop {
        // the sampled max distance and the deviation are not used
        Ok((_max_dist_to_surf, _deviation, uv_curves)) => uv_curves,
        Err(e) if e.is_unported() => return Err(e),
        // if (eStatus != SM_SUCCESS) continue
        Err(_) => return Ok(()),
    };

    // Will replace the original UV-curve when we have only one result
    if let Ok([uv_curve]) = <[Curve; 1]>::try_from(uv_curves) {
        // upstream dereferences the primary edge use and its mate unchecked
        let prim_eu = primary_edgeuse(fb.brep(), rail)?
            .ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        let mate_eu =
            mate(fb.brep(), prim_eu)?.ok_or_else(|| SolidError::null_pointer(OP_EXAMINE))?;
        // SetUVCurve(pUVCurve) with its defaults (bDeleteOldCurve = FALSE, bDbgWarnLeaks =
        // TRUE); upstream then deletes the curve it replaced
        fb.set_edgeuse_uv_curve(mate_eu, Some(uv_curve), false, true)?;
    }
    Ok(())
}

/// `SmBrep::GetInfiniteRegion` ([`Brep::infinite_region`], unit `topology.brep_p1`):
/// `m_pInfiniteRegion`, which may be NULL.
fn infinite_region(brep: &Brep) -> Option<RegionId> {
    brep.infinite_region()
}

/// `SmRegion::GetShells(rShells)` ([`Region::get_shells`](crate::topology::Region::get_shells))
/// with upstream's default `pOptAttributeId = NULL`: no attribute filter, so the
/// attribute lookup is never called.
fn region_shells(brep: &Brep, region: RegionId) -> Result<Vec<ShellId>> {
    brep.region(region)?.get_shells(None, |_, _| Ok(false))
}

/// `SmBrep::MakeShellVertexTopology(pRegion, rpShell, rpVertex)`
/// ([`topology::brep::p2::make_shell_vertex_topology`](crate::topology::brep::p2::make_shell_vertex_topology)):
/// the new shell and vertex.
fn make_shell_vertex_topology(brep: &mut Brep, region: RegionId) -> Result<(ShellId, VertexId)> {
    crate::topology::brep::p2::make_shell_vertex_topology(brep, Some(region))
}

/// `SmOffsetSurface::Copy(crContext, rpNewSurf)` ([`OffsetSurface::copy`](crate::surface::offset_surface::OffsetSurface::copy);
/// the solver's creation context is the kernel). A solver's offset surface does not own
/// its base (`SmOffsetSurface(d, rSurface, FALSE, ..)`), so the copy shares the base
/// pointer and keeps its [`SurfaceId`](crate::blend::fillet_custom_solver::SurfaceId).
fn offset_surface_copy(k: &Kernel, surface: &OffsetSurface) -> Result<OffsetSurface> {
    Ok(OffsetSurface {
        surface: surface.surface.copy(k)?,
        base_id: surface.base_id,
    })
}

/// `SmOffsetSurface::GetNaturalUVDomain`
/// ([`OffsetSurface::natural_uv_domain`](crate::surface::offset_surface::OffsetSurface::natural_uv_domain)).
fn offset_natural_uv_domain(surface: &OffsetSurface) -> Extent2d {
    surface.surface.natural_uv_domain()
}

/// `SmSurface::DropCurve(crContext, crDropDomain, cr3DCurve, crInterval, dDistTol,
/// rdMaxDistToSurf, rdDeviation, rUVCurves)` on an offset surface (unit
/// `intersect.surface_drop_lib_p3`): the out-parameters `(rdMaxDistToSurf,
/// rdDeviation, rUVCurves)`.
fn offset_surface_drop_curve(
    k: &Kernel,
    surface: &OffsetSurface,
    drop_domain: &Extent2d,
    curve_3d: &Curve,
    interval: &Extent1d,
    dist_tol: f64,
) -> Result<(f64, f64, Vec<Curve>)> {
    // `SmOffsetSurface` does not override `DropCurve`: the general drop
    // (`bKeepAllDropCurves = TRUE`, no fail record) runs on the offset surface as a
    // surface; the port's drop takes a `Surface`, so it runs on a copy of the offset.
    let as_surface = Surface::from(surface.surface.copy(k)?);
    let dropped = as_surface.drop_curve::<NoObject>(
        k,
        drop_domain,
        curve_3d,
        interval,
        dist_tol,
        true,
        None,
        true,
    )?;
    Ok((
        dropped.max_drop_to_surf,
        dropped.max_approx_dev,
        dropped.uv_curves,
    ))
}

/// `JoinVertsByCapping`: when the end of `uv_curve1` (at its max parameter, or min when
/// `orient1` is opposite) and the end of `uv_curve2` (likewise with `orient2`) do not
/// coincide on the fillet surface, fills the gap with up to three iso-parameter UV
/// lines (to the U domain boundary on curve 1's side, along it, and back to curve 2),
/// each lifted to a 3D curve at `this_approx_tol_3d`; the new UV curves, 3D curves and
/// `SM_OT_SAME` orientations are appended to the three lists.
///
/// # Errors
///
/// Those of the curve evaluations, the line creation and [`Surface::lift_curve`].
pub fn join_verts_by_capping(
    k: &Kernel,
    this_approx_tol_3d: f64,
    surface: &Surface,
    uv_curve1: &Curve,
    orient1: OrientType,
    uv_curve2: &Curve,
    orient2: OrientType,
    curves_3d: &mut Vec<Curve>,
    uv_curves: &mut Vec<Curve>,
    orients: &mut Vec<OrientType>,
) -> Result<()> {
    // locals
    let ivl1 = uv_curve1.natural_interval();
    let ivl2 = uv_curve2.natural_interval();
    let t_val1 = if orient1 == OrientType::Opposite {
        ivl1.min()
    } else {
        ivl1.max()
    };
    let t_val2 = if orient2 == OrientType::Opposite {
        ivl2.min()
    } else {
        ivl2.max()
    };
    let domain = surface.natural_uv_domain_with_nurb(k)?;

    // let sUV1 = UVTrimCurve1 UV endPoint
    let pnt = uv_curve1.evaluate_point(k, t_val1)?;
    let uv1 = Point2::new(pnt.x, pnt.y);

    // let sUV4 = UVTrimCurve2 UV endPoint
    let pnt = uv_curve2.evaluate_point(k, t_val2)?;
    let uv4 = Point2::new(pnt.x, pnt.y);

    // let sUV2 = UVTrimCurve1->EndPoint moved to Surface V_Dir domain boundary
    // let sUV3 = UVTrimCurve2->EndPoint moved to Surface V_Dir domain boundary
    let uv2 = Point2::new(
        if orient1 == OrientType::Opposite {
            domain.u_min()
        } else {
            domain.u_max()
        },
        uv1.y,
    );
    let uv3 = Point2::new(uv2.x, uv4.y);

    for (a, b) in [(uv1, uv2), (uv2, uv3), (uv3, uv4)] {
        // when the gap is more than tol, make a curve to fill it
        if a.distance(b) > EFF_ZERO_SQRT {
            // create a isoParameter UV Line
            let uv_curve = Curve::from(BSplineCurve::create_line_segment(
                2,
                Point3::new(a.x, a.y, 0.0),
                Point3::new(b.x, b.y, 0.0),
            )?);

            // let p3DCurve = SurfaceProjection of new UV Line
            let lifted = surface.lift_curve(
                k,
                &domain,
                &uv_curve,
                &uv_curve.natural_interval(),
                // the double converts to LiftCurve's SmApproxTol3d parameter
                approx_tol3d(k, this_approx_tol_3d),
                false,
            )?;

            // add the curves to the output arrays
            uv_curves.push(uv_curve);
            curves_3d.push(lifted.curve);
            orients.push(OrientType::Same);
        }
    }

    Ok(())
}

/// Whether the UV point carries the "unset" marker `-SM_BIG_DOUBLE` (or below).
fn is_unset_uv(uv: Point2) -> bool {
    uv.x <= -BIG_DOUBLE || uv.y <= -BIG_DOUBLE
}

/// `sm_CheckSeamOut`: when `uv` is outside the surface's natural domain (by more than
/// `tol`) in a closed direction of `closure`, moves it in by one period and returns
/// `true`; an unset `uv` or no closure returns `false`.
#[must_use]
pub fn check_seam_out(surf: &Surface, uv: &mut Point2, tol: f64, closure: SurfParamType) -> bool {
    if closure == SurfParamType::Neither {
        return false;
    }
    if is_unset_uv(*uv) {
        return false;
    }

    // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
    let dom = surf.natural_uv_domain_if_built();

    if dom.contains_point(*uv, tol) {
        return false;
    }

    let mut ret = false;

    if closure == SurfParamType::U || closure == SurfParamType::Both {
        let dom_u = dom.u_interval();
        if uv.x < dom_u.min() - tol {
            uv.x += dom_u.length();
            ret = true;
        }
        if uv.x > dom_u.max() + tol {
            uv.x -= dom_u.length();
            ret = true;
        }
    }

    if closure == SurfParamType::V || closure == SurfParamType::Both {
        let dom_v = dom.v_interval();
        if uv.y < dom_v.min() - tol {
            uv.y += dom_v.length();
            ret = true;
        }
        if uv.y > dom_v.max() + tol {
            uv.y -= dom_v.length();
            ret = true;
        }
    }

    ret
}

/// `sm_CheckSeamOn`: when `uv` is on a domain boundary (within `tol`) that is a seam
/// (a closed direction of `closure`), moves it to the other side of the seam and
/// returns `true`; an unset `uv` or no closure returns `false`.
#[must_use]
pub fn check_seam_on(surf: &Surface, uv: &mut Point2, tol: f64, closure: SurfParamType) -> bool {
    if closure == SurfParamType::Neither {
        return false;
    }
    if is_unset_uv(*uv) {
        return false;
    }

    // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
    let dom = surf.natural_uv_domain_if_built();

    let mut ret = false;

    if closure == SurfParamType::U || closure == SurfParamType::Both {
        let dom_u = dom.u_interval();
        if (uv.x - dom_u.min()).abs() < tol {
            uv.x = dom_u.max();
            ret = true;
        } else if (uv.x - dom_u.max()).abs() < tol {
            uv.x = dom_u.min();
            ret = true;
        }
    }

    if closure == SurfParamType::V || closure == SurfParamType::Both {
        let dom_v = dom.v_interval();
        if (uv.y - dom_v.min()).abs() < tol {
            uv.y = dom_v.max();
            ret = true;
        } else if (uv.y - dom_v.max()).abs() < tol {
            uv.y = dom_v.min();
            ret = true;
        }
    }

    ret
}

/// `sm_IsOrderCorrect`: whether the intersection point at `idx1` points towards the one
/// at `idx2` (the 3D tangent at `idx1` has a positive dot product with the chord).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an index past the slice.
pub fn is_order_correct(tsect_pnts: &[TsectPnt], idx1: usize, idx2: usize) -> Result<bool> {
    let tsp1 = tsect_pnts
        .get(idx1)
        .ok_or_else(|| SolidError::invalid_input(OP_ORDER, "index 1 out of range"))?;
    let tsp2 = tsect_pnts
        .get(idx2)
        .ok_or_else(|| SolidError::invalid_input(OP_ORDER, "index 2 out of range"))?;

    let diff = tsp2.crv_pos() - tsp1.crv_pos();
    let tan = tsp1.crv_deriv();

    Ok(diff.dot(tan) > 0.0)
}

/// `sm_SetSurfaceClosure`: the surface's closure over `domain`: `U`, `V`, `Both` or
/// `Neither`.
///
/// # Errors
///
/// Those of the closure tests.
pub fn set_surface_closure(k: &Kernel, surf: &Surface, domain: &Extent2d) -> Result<SurfParamType> {
    let mut ret = SurfParamType::Neither;

    if is_closed(k, surf, domain, SurfParamType::U)? {
        ret = SurfParamType::U;
    }
    if is_closed(k, surf, domain, SurfParamType::V)? {
        ret = if ret == SurfParamType::Neither {
            SurfParamType::V
        } else {
            SurfParamType::Both
        };
    }

    Ok(ret)
}

/// `sm_FindMarkForContext2`: the position in `mark_locks` of the first lock bound to
/// the context `context` (`None` is upstream's `NULL` context, which matches a lock
/// bound to `NULL`), or `None`.
#[must_use]
pub fn find_mark_for_context2(mark_locks: &[MarkLock], context: Option<usize>) -> Option<usize> {
    mark_locks.iter().position(|lock| lock.context() == context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;
    use crate::nurbs::srf_basic::Surface as NurbSurface;
    use crate::topology::{Edgeuse, Vertexuse};

    fn bilinear_patch() -> Surface {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 2.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(3.0, 0.0, 0.0, NOW),
                CPoint::new(3.0, 2.0, 0.5, NOW),
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
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    #[test]
    fn noise_cleanup_rounds_near_decimals() {
        assert_eq!(clean_up_noise(1.0 + 1e-13), 1.0);
        assert_eq!(clean_up_noise(2.0 - 1e-13), 2.0);
        // the digits are re-assembled as 1 + 0.1 + 0.01 * 3 (as upstream)
        assert!((clean_up_noise(1.13 + 1e-13) - 1.13).abs() < 1e-15);
        assert_eq!(clean_up_noise(0.5 + 1e-14), 0.5);
        assert_eq!(clean_up_noise(0.123456789), 0.123456789);
        assert_eq!(clean_up_noise(-0.5), -0.5);
        let uv = cleanup_noise(Point2::new(1.0 + 1e-13, 2.5));
        assert_eq!(uv, Point2::new(1.0, 2.5));
    }

    #[test]
    fn seam_checks_move_points_by_a_period() {
        let s = bilinear_patch();
        let mut uv = Point2::new(1.2, 0.5);
        assert!(check_seam_out(&s, &mut uv, 1e-9, SurfParamType::U));
        assert_eq!(uv, Point2::new(1.2 - 1.0, 0.5));
        let mut uv = Point2::new(0.5, 0.5);
        assert!(!check_seam_out(&s, &mut uv, 1e-9, SurfParamType::Both));
        assert!(!check_seam_out(&s, &mut uv, 1e-9, SurfParamType::Neither));
        let mut unset = Point2::new(-BIG_DOUBLE, 0.5);
        assert!(!check_seam_out(&s, &mut unset, 1e-9, SurfParamType::U));

        let mut uv = Point2::new(0.0, 0.3);
        assert!(check_seam_on(&s, &mut uv, 1e-9, SurfParamType::U));
        assert_eq!(uv, Point2::new(1.0, 0.3));
        let mut uv = Point2::new(0.3, 1.0);
        assert!(check_seam_on(&s, &mut uv, 1e-9, SurfParamType::V));
        assert_eq!(uv, Point2::new(0.3, 0.0));
        assert!(!check_seam_on(&s, &mut uv, 1e-9, SurfParamType::Neither));
    }

    #[test]
    fn surface_closure_of_an_open_patch_is_neither() {
        let k = &Kernel::default();
        let s = bilinear_patch();
        let closure = set_surface_closure(k, &s, &s.natural_uv_domain_if_built()).unwrap();
        assert_eq!(closure, SurfParamType::Neither);
    }

    #[test]
    fn order_check_uses_chord_and_tangent() {
        let mut a = TsectPnt::new();
        *a.crv_pos_mut() = Point3::new(0.0, 0.0, 0.0);
        *a.crv_deriv_mut() = Vec3::new(1.0, 0.0, 0.0);
        let mut b = TsectPnt::new();
        *b.crv_pos_mut() = Point3::new(2.0, 0.0, 0.0);
        *b.crv_deriv_mut() = Vec3::new(1.0, 0.0, 0.0);
        let pts = vec![a, b];
        assert!(is_order_correct(&pts, 0, 1).unwrap());
        assert!(!is_order_correct(&pts, 1, 0).unwrap());
        assert!(is_order_correct(&pts, 0, 2).is_err());
    }

    #[test]
    fn interpolation_picks_line_conic_and_hermite() {
        // colinear: a line
        let mut sv = Vec3::new(1.0, 0.0, 0.0);
        let mut ev = Vec3::new(1.0, 0.0, 0.0);
        let c = interpolate_endpts_and_tangents(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            &mut sv,
            &mut ev,
            3,
        )
        .unwrap();
        assert_eq!(c.degree().unwrap(), 1);

        // tangents meeting between the points at 90 degrees: a circular arc
        let mut sv = Vec3::new(1.0, 0.0, 0.0);
        let mut ev = Vec3::new(0.0, 1.0, 0.0);
        let c = interpolate_endpts_and_tangents(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            &mut sv,
            &mut ev,
            3,
        )
        .unwrap();
        assert_eq!(c.degree().unwrap(), 2);
        let mid = c.evaluate_point(0.5).unwrap();
        let r = (mid - Point3::new(0.0, 1.0, 0.0)).length();
        assert!((r - 1.0).abs() < 1e-12);

        // tangent lines meeting behind the start point: a cubic Hermite
        let mut sv = Vec3::new(0.0, 1.0, 0.0);
        let mut ev = Vec3::new(1.0, 1.0, 0.0);
        let c = interpolate_endpts_and_tangents(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            &mut sv,
            &mut ev,
            2,
        )
        .unwrap();
        assert_eq!(c.degree().unwrap(), 3);
        assert_eq!(c.dim(), 2);
    }

    #[test]
    fn capping_fills_the_gap_with_iso_lines() {
        let k = Kernel::default();
        let s = bilinear_patch();
        let c1 = Curve::from(
            BSplineCurve::create_line_segment(
                2,
                Point3::new(0.2, 0.2, 0.0),
                Point3::new(0.6, 0.3, 0.0),
            )
            .unwrap(),
        );
        let c2 = Curve::from(
            BSplineCurve::create_line_segment(
                2,
                Point3::new(0.7, 0.9, 0.0),
                Point3::new(0.8, 0.8, 0.0),
            )
            .unwrap(),
        );
        let mut c3 = Vec::new();
        let mut uv = Vec::new();
        let mut or = Vec::new();
        // the cap lines are iso-parametric and lifted through sm_ExtractIsoCurve
        join_verts_by_capping(
            &k,
            1e-6,
            &s,
            &c1,
            OrientType::Same,
            &c2,
            OrientType::Same,
            &mut c3,
            &mut uv,
            &mut or,
        )
        .unwrap();
        assert_eq!(uv.len(), 3);
        assert_eq!(c3.len(), 3);
        assert_eq!(or, vec![OrientType::Same; 3]);
    }

    #[test]
    fn pseudo_brep_records_and_destructors() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        assert!(fb.editing_enabled());
        let v0 = fb
            .brep_mut()
            .add_vertex(Point3::new(0.0, 0.0, 0.0))
            .unwrap();
        let v1 = fb
            .brep_mut()
            .add_vertex(Point3::new(1.0, 0.0, 0.0))
            .unwrap();
        fb.insert_fillet_vertex(v0, FilletVertex::new(&k, None));
        fb.insert_fillet_vertex(v1, FilletVertex::new(&k, Some(FilletCornerId(3))));
        let e = fb.new_fillet_edge(v0, v1, None).unwrap();
        assert_eq!(fb.edge_list(), &[e]);
        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::Rail);
        assert_eq!(fb.fillet_edge(e).unwrap().name(), Some("SM_FE_RAIL"));

        // two edge uses with their vertex uses, as MakeFilletEdge builds them
        let eu1 = fb
            .brep_mut()
            .add_edgeuse_record(Edgeuse::default())
            .unwrap();
        let eu2 = fb
            .brep_mut()
            .add_edgeuse_record(Edgeuse::default())
            .unwrap();
        let vu1 = fb
            .brep_mut()
            .add_vertexuse_record(Vertexuse::default())
            .unwrap();
        let vu2 = fb
            .brep_mut()
            .add_vertexuse_record(Vertexuse::default())
            .unwrap();
        for (eu, vu, v, o) in [
            (eu1, vu1, v0, OrientType::Same),
            (eu2, vu2, v1, OrientType::Opposite),
        ] {
            let b = fb.brep_mut();
            b.edgeuse_mut(eu).unwrap().edge = Some(e);
            b.edgeuse_mut(eu).unwrap().orientation = o;
            b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
            b.vertexuse_mut(vu).unwrap().vertex = Some(v);
            b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
            b.vertex_mut(v).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
            fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
        }
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap());
        fb.geom_mut(g).unwrap().set_rail(0, Some(e));
        fb.set_edgeuse_fillet_geom(eu1, Some(g)).unwrap();
        assert_eq!(fb.geom(g).unwrap().ref_eus, vec![eu1]);
        assert_eq!(fb.vertex_fillet_geoms(v0).unwrap(), vec![g]);
        assert_eq!(
            fb.vertex_fillet_geoms(v1).unwrap(),
            Vec::<FilletGeomId>::new()
        );
        assert_eq!(
            fb.vu_at_rail_end(v0, Some(g), None, None).unwrap(),
            Some(vu1)
        );
        assert_eq!(fb.vu_at_rail_end(v0, None, Some(vu1), None).unwrap(), None);
        assert_eq!(fb.rail_vertexuse(g, 0, 0).unwrap(), Some(vu1));
        assert_eq!(fb.rail_vertexuse(g, 0, 1).unwrap(), Some(vu2));
        assert!(fb.rail_vertexuse(g, 1, 0).is_err());

        // mates
        fb.fillet_vertex_mut(v0).unwrap().set_mate(1, Some(v1));
        fb.fillet_vertex_mut(v1).unwrap().set_mate(0, Some(v0));
        fb.delete_fillet_vertex(v0).unwrap();
        assert!(fb.fillet_vertex(v0).is_err());
        assert_eq!(fb.fillet_vertex(v1).unwrap().mate(0), None);
        assert!(fb.brep().vertex(v0).unwrap().vertexuses.is_empty());

        fb.delete_fillet_edge(e).unwrap();
        assert!(fb.edge_list().is_empty());
        assert!(fb.fillet_edge(e).is_err());
        assert!(fb.brep().vertex(v1).unwrap().vertexuses.is_empty());
        // SmFilletBrep() made the infinite region: the edge use is owned by its first
        // shell (an artifact shell vertex when the region had none)
        let eu = fb.new_fillet_edgeuse().unwrap();
        let reg = fb.brep().infinite_region().unwrap();
        let shell = *fb
            .brep()
            .region(reg)
            .unwrap()
            .get_shells(None, |_, _| Ok(false))
            .unwrap()
            .first()
            .unwrap();
        assert_eq!(
            fb.brep().edgeuse(eu).unwrap().owner,
            EdgeuseOwner::Shell(Some(shell))
        );
    }

    #[test]
    fn kind_of_chains_and_type_codes() {
        assert!(fillet_vertex_is_kind_of(FILLET_VERTEX_TYPE));
        assert!(fillet_vertex_is_kind_of(crate::topology::vertex::TYPE));
        assert!(fillet_vertex_is_kind_of(crate::topology::topology::TYPE));
        assert!(!fillet_vertex_is_kind_of(FILLET_EDGE_TYPE));
        assert!(fillet_edge_is_kind_of(edge_unit::TYPE));
        assert!(fillet_edgeuse_is_kind_of(eu1::TYPE));
        assert!(fillet_vertexuse_is_kind_of(
            crate::topology::vertexuse::TYPE
        ));
        assert!(!fillet_vertexuse_is_kind_of(FILLET_BREP_TYPE));
        assert_eq!(FilStatus::from_code(7), Some(FilStatus::NoIntRailEu));
        assert_eq!(FilletEdgeType::from_code(99), None);
    }
    /// A cubic-by-linear B-spline strip: rows `i = 0..4` at `x = x0 + i`, columns at
    /// `y = 0` and `y = 2`, `z = 0`.
    fn cubic_strip(x0: f64) -> Surface {
        let net = (0..4)
            .map(|i| {
                let x = x0 + f64::from(i);
                vec![CPoint::new(x, 0.0, 0.0, NOW), CPoint::new(x, 2.0, 0.0, NOW)]
            })
            .collect();
        let nurb = NurbSurface::new(
            net,
            3,
            1,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    /// Two fillet geometries at an Nx2 corner: geometry 0's strip starts at `x = 0.5`
    /// (its rails are two edges with one edge use and vertex use each), geometry 1's
    /// ends at `x = 3`, where the corner vertices sit. Returns the B-rep, the
    /// geometries, the corner vertices and the edge use / vertex use of rail 0.
    fn corner_for_examine(
        k: &Kernel,
    ) -> (
        FilletBrep,
        [FilletGeomId; 2],
        [VertexId; 2],
        EdgeuseId,
        VertexuseId,
    ) {
        let mut fb = FilletBrep::new();
        let add_v = |fb: &mut FilletBrep, p: Point3| {
            let v = fb.brep_mut().add_vertex(p).unwrap();
            fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
            v
        };
        let start_fv = add_v(&mut fb, Point3::new(3.0, 0.0, 0.0));
        let end_fv = add_v(&mut fb, Point3::new(3.0, 2.0, 0.0));
        let mut rails = [None, None];
        let mut rail0_uses = None;
        for rail in &mut rails {
            let va = add_v(&mut fb, Point3::new(0.5, 0.0, 0.0));
            let vb = add_v(&mut fb, Point3::new(3.5, 0.0, 0.0));
            let e = fb.new_fillet_edge(va, vb, None).unwrap();
            let eu = fb
                .brep_mut()
                .add_edgeuse_record(Edgeuse::default())
                .unwrap();
            let vu = fb
                .brep_mut()
                .add_vertexuse_record(Vertexuse::default())
                .unwrap();
            let b = fb.brep_mut();
            b.edgeuse_mut(eu).unwrap().edge = Some(e);
            b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
            b.vertexuse_mut(vu).unwrap().vertex = Some(va);
            b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
            b.vertex_mut(va).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
            fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
            *rail = Some(e);
            rail0_uses.get_or_insert((eu, vu));
        }
        let mut g0 = FilletGeom::new(k, None, FilletGeomType::Blends).unwrap();
        g0.set_fillet_surface(Some(cubic_strip(0.5)));
        g0.rails = rails;
        let mut g1 = FilletGeom::new(k, None, FilletGeomType::Blends).unwrap();
        g1.set_fillet_surface(Some(cubic_strip(0.0)));
        let g0 = fb.add_geom(g0);
        let g1 = fb.add_geom(g1);
        let (eu, vu) = rail0_uses.unwrap();
        (fb, [g0, g1], [start_fv, end_fv], eu, vu)
    }

    #[test]
    fn examine_copies_the_good_row_rebuilds_the_rail_and_drops_it() {
        use crate::blend::fillet_custom_solver::SurfaceId;
        let k = Kernel::default();
        let (mut fb, [g0, g1], [start_fv, end_fv], eu, vu) = corner_for_examine(&k);
        // stale cached state the rail's SetCurve notification must clear
        {
            let mut sv = Vec3::new(1.0, 0.0, 0.0);
            let mut ev = Vec3::new(1.0, 0.0, 0.0);
            let line = interpolate_endpts_and_tangents(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                &mut sv,
                &mut ev,
                2,
            )
            .unwrap();
            let b = fb.brep_mut();
            let rec = b.edgeuse_mut(eu).unwrap();
            rec.uv_trim_curve = Some(Curve::from(line));
            rec.max_edge_face_gap.edgeuse = Some(eu);
            rec.max_edge_face_gap.gap_3d = 0.25;
            let vrec = b.vertexuse_mut(vu).unwrap();
            vrec.vertex_edge_gap.vertexuse = Some(vu);
            vrec.vertex_face_gap.vertexuse = Some(vu);
        }
        let mut solver = ConstantRadiusAssistedFs::from_parts(
            1e-6,
            1e-3,
            1e-6,
            [0.25, 0.25],
            [
                Some(
                    OffsetSurface::new(&k, 0.25, cubic_strip(0.0), SurfaceId::Free(1), 0).unwrap(),
                ),
                None,
            ],
            [1.0, -1.0],
            [None, None],
        );
        let err = examine_n_fix_fillet_geoms(
            &k,
            &mut fb,
            g0,
            true,
            g1,
            false,
            start_fv,
            end_fv,
            1e-6,
            [Some(&mut solver), None],
        )
        .unwrap_err();
        // the real drop runs on rail 0 (a failed drop skips the rail); rail 1 has no
        // solver offset surface, which upstream dereferences without a check
        assert!(
            matches!(err, SolidError::Upstream { code: 1006, .. }),
            "{err:?}"
        );
        // the offset distance was zeroed for the drop and restored
        assert_eq!(
            solver.solver.surfaces[0]
                .as_ref()
                .unwrap()
                .surface
                .offset_distance(),
            0.25
        );
        // geometry 0's start row is now geometry 1's end row; its other rows are its own
        let near = |a: Point3, b: Point3| a.distance(b) < 1e-12;
        let s0 = fb.geom(g0).unwrap().fillet_surface(None).unwrap().unwrap();
        let at = |u: f64, v: f64| s0.evaluate_point(&k, Point2::new(u, v)).unwrap();
        assert!(near(at(0.0, 0.0), Point3::new(3.0, 0.0, 0.0)));
        assert!(near(at(0.0, 1.0), Point3::new(3.0, 2.0, 0.0)));
        assert!(near(at(1.0, 0.0), Point3::new(3.5, 0.0, 0.0)));
        // rail 0 is the v = 0 iso-curve of the adjusted surface; rail 1's iso-curve is
        // set before its drop hits the missing offset surface
        let rails = fb.geom(g0).unwrap().rails;
        let rail0 = fb.brep().edge(rails[0].unwrap()).unwrap();
        let curve = rail0.curve.as_ref().unwrap();
        let ivl = curve.natural_interval();
        assert!(near(
            curve.evaluate_point(&k, ivl.min()).unwrap(),
            Point3::new(3.0, 0.0, 0.0)
        ));
        assert!(near(
            curve.evaluate_point(&k, ivl.max()).unwrap(),
            Point3::new(3.5, 0.0, 0.0)
        ));
        assert!(fb.brep().edge(rails[1].unwrap()).unwrap().curve.is_some());
        // the SetCurve notification cleared the cached state of rail 0's uses
        let rec = fb.brep().edgeuse(eu).unwrap();
        assert!(rec.uv_trim_curve.is_none());
        assert!(rec.drop_curve_fail.is_none());
        assert!(!rec.max_edge_face_gap.is_init());
        let vrec = fb.brep().vertexuse(vu).unwrap();
        assert!(!vrec.vertex_edge_gap.is_init());
        assert!(!vrec.vertex_face_gap.is_init());

        // without the solver the same path is upstream's null dereference
        let (mut fb, [g0, g1], [start_fv, end_fv], _, _) = corner_for_examine(&k);
        let err = examine_n_fix_fillet_geoms(
            &k,
            &mut fb,
            g0,
            true,
            g1,
            false,
            start_fv,
            end_fv,
            1e-6,
            [None, None],
        )
        .unwrap_err();
        assert!(
            matches!(err, SolidError::Upstream { code: 1006, .. }),
            "{err:?}"
        );
        // and with the good surface meeting the corner nothing is adjusted
        let (mut fb, [_, g1], [start_fv, end_fv], _, _) = corner_for_examine(&k);
        examine_n_fix_fillet_geoms(
            &k,
            &mut fb,
            g1,
            false,
            g1,
            false,
            start_fv,
            end_fv,
            1e-6,
            [None, None],
        )
        .unwrap();
    }

    #[test]
    fn transferred_fillet_surface_resolves_in_its_owning_body() {
        use crate::geom::attribute::ObjectKey;
        use crate::geom::object::ObjectRef;
        let k = Kernel::default();
        let mut body = Brep::new_with_infinite_region().unwrap();
        let shell = ShellId::from_index(0);
        let face = body
            .add_face_record(crate::topology::Face {
                surface: Some(bilinear_patch()),
                ..crate::topology::Face::new(shell)
            })
            .unwrap();
        let identity = ObjectRef {
            type_code: 1,
            key: ObjectKey(7),
        };
        let mut g = FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap();
        g.set_fillet_surface(Some(bilinear_patch()));
        // Before MakeFaceBrep the geometry owns the surface; no body is needed.
        assert!(g.fillet_surface(None).unwrap().is_some());
        g.fillet_surface = Some(FilletSurfaceStorage::ManagedFace {
            body: identity,
            face,
        });
        let installed = g
            .fillet_surface(Some(FilletSurfaceOwner {
                identity,
                body: &body,
            }))
            .unwrap()
            .unwrap();
        assert!(core::ptr::eq(
            installed,
            body.face(face).unwrap().surface.as_ref().unwrap()
        ));
        for owner in [
            None,
            Some(FilletSurfaceOwner {
                identity: ObjectRef {
                    type_code: 1,
                    key: ObjectKey(8),
                },
                body: &body,
            }),
        ] {
            assert!(matches!(
                g.fillet_surface(owner),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        // Writes go to the face's surface.
        let flat = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        *g.fillet_surface_mut(Some(FilletSurfaceOwner {
            identity,
            body: &mut body,
        }))
        .unwrap()
        .unwrap() = flat;
        assert!(matches!(
            g.fillet_surface_mut(None),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            body.face(face).unwrap().surface.as_ref().unwrap().kind(),
            SurfaceKind::Plane(_)
        ));
        // A face without a surface is upstream's NULL.
        body.face_mut(face).unwrap().surface = None;
        assert!(matches!(
            g.fillet_surface(Some(FilletSurfaceOwner {
                identity,
                body: &body
            })),
            Err(SolidError::Upstream { .. })
        ));
    }
}

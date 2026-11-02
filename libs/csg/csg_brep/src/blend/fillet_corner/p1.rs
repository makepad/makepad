//! Fillet corners, part 1: the file-static helpers of `SmFilletCorner.cpp` (edge
//! tangency at a vertex, the face shared by two edge uses, face tangency across an edge,
//! face-corner concavity, the planar and surface-of-revolution corner patches and the
//! variable-radius corner radius) and the `SmFilletCorner` methods this unit owns, as
//! methods of [`FilletCorner`] (the corner record of
//! [`crate::blend::fillet_corner::p3`]): data access, the predicates and setters, the
//! tangent-solver lookups, the extended-surface registry, the corner edge
//! re-initialisation and squeeze, the concave rail-rail test and the Nx1 degeneracy
//! check, the corner faces in the fillet B-rep, the constant-radius corner fillet and the
//! insertion of the corner edges into the target B-rep.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp`, `source/SMLib/inc/SmFilletCorner.h`.
//!
//! | Rust                                         | Upstream                                    |
//! |----------------------------------------------|---------------------------------------------|
//! | [`test_g1_continuity`]                       | `sm_TestG1Continuity`                       |
//! | [`face_by_two_edgeuses`]                     | `sm_GetFaceByTwoEdgeuses`                   |
//! | [`check_tangential_edgeuse`]                 | `sm_CheckTangentialEdgeuse`                 |
//! | [`check_face_complimentary_corner_concavity`] | `sm_CheckFaceComplimentaryCornerConcavity` |
//! | [`create_srf_of_revolution`]                 | `sm_CreateSrfOfRevolution`                  |
//! | [`create_planar_surface`]                    | `sm_CreatePlanarSurface`                    |
//! | [`fillet_radius`]                            | `sm_GetFilletRadius`                        |
//! | [`check_seam_out`] (re-export)               | `sm_CheckSeamOut`                           |
//! | [`set_surface_closure`] (re-export)          | `sm_SetSurfaceClosure`                      |
//! | [`FilletCorner::this_approx_tol_3d`]         | `GetThisApproxTol3d` (and the `SmFilletCorner` branch of `SmTol::GetApproxTol3d(const SmObject*)`) |
//! | [`FilletCorner::fillet_edges`], [`FilletCorner::fillet_vertices`] | `GetFilletEdges`, `GetFilletVertices` |
//! | [`FilletCorner::filleted_vertex`], [`FilletCorner::set_back_dist`], [`FilletCorner::status`] | `GetFilletedVertex`, `GetSetBackDist`, `GetStatus` |
//! | [`FilletCorner::type_code`], [`FilletCorner::type_string`], [`FilletCorner::is_kind_of`] | `GetType`, `GetTypeString`, `IsKindOf` (`SM_COMMON`) |
//! | [`FilletCorner::add_fillet_vertex`]          | `AddFilletVertex`                           |
//! | [`FilletCorner::set_bevel`], [`FilletCorner::set_calc_as_const_rad`], [`FilletCorner::set_chamfer`] | `SetBevel`, `SetCalcAsConstRad`, `SmFilletConvexNxNCorner::SetChamfer` |
//! | [`FilletCorner::is_bevel_corner`], [`FilletCorner::is_blending_corner`], [`FilletCorner::is_degenerate_corner`], [`FilletCorner::is_extended_surface_patch`], [`FilletCorner::is_calc_as_const_rad`] | `IsBevelCorner`, `IsBlendingCorner`, `IsDegenerateCorner`, `IsExtendedSurfacePatch`, `IsCalcAsConstRad` |
//! | [`FilletCorner::is_tangent_corner`]          | `IsTangentCorner` (base, `SmFilletNx1ClosedCorner`, `SmFilletNx2Corner`) |
//! | [`FilletCorner::tangent_fillet_solver`]      | `GetTangentFilletSolver` (base, `SmFillet1x1Corner`, `SmFillet2x2Corner`) |
//! | [`FilletCorner::fillet_solvers`]             | `GetFilletSolvers`                          |
//! | [`FilletCorner::extended_surface`], [`FilletCorner::add_extended_original_surface`] | `GetExtendedSurface`, `AddExtendedOriginalSurface` |
//! | [`FilletCorner::re_initialize_corner_edges`] | `ReInitializeCornerEdges`                   |
//! | [`FilletCorner::squeeze_edge`]               | `SqueezeEdge`                               |
//! | [`FilletCorner::test_rail_rail_interpolation_curve`] | `SmFilletConcaveNxNCorner::TestRailRailInterpolationCurve` |
//! | [`check_nx1_degeneracy`]                     | `SmFilletNx1Corner::CheckNx1Degeneracy` (static) |
//! | [`FilletCorner::calc_corner_vert_geom`]      | `CalcCornerVertGeom`                        |
//! | [`FilletCorner::calc_corner_edge_geom`]      | `CalcCornerEdgeGeom`                        |
//! | [`FilletCorner::check_and_fix_degeneracy`]   | `CheckAndFixDegeneracy`                     |
//! | [`FilletCorner::create_end_patch`]           | `CreateEndPatch`                            |
//! | [`FilletCorner::make_face_brep`], [`FilletCorner::make_face_brep_with`] | `MakeFaceBrep` (the new faces recorded in [`FilletCorner::surface_faces`]) |
//! | [`FilletCorner::make_constant_radius_fillet`], [`FilletCorner::make_constant_radius_fillet_with`] | `MakeConstantRadiusFillet` |
//! | [`FilletCorner::adjust_vertex_law_intervals`], [`LawIntervalSolvers`] | the law-interval half of `SmFilletCorner::CalcCornerVertGeom` |
//! | [`FilletCorner::insert_intersection_topology_with_context`] | `InsertIntersectionTopology` |
//! | `PlainConstantRadiusFs` (private)            | the `SmConstantRadiusFS` virtuals (not the assisted solver's) |
//! | `p3::FilletCorner::adjust_topology` / `adjust_tangent_corner` (`_` arms) | `SmFilletCorner::AdjustTopology`, `SmFillet4x3Corner::AdjustTopology`, `SmFilletNxNCorner::AdjustTopology`, `SmFilletCorner::AdjustTangentCorner` (all `return SM_SUCCESS`) |
//!
//! Deviations:
//! - **Shared helpers.** `sm_CheckSeamOut` and `sm_SetSurfaceClosure` are textually the
//!   same in `SmFilletCorner.cpp` and `SmFilletGeom.cpp` (each file has its own static
//!   copy): both are the ports in [`crate::blend::fillet_geom::p1`], re-exported here.
//! - **Out-params.** Results are return values: [`test_g1_continuity`] returns the
//!   flag and the optional angle (`pdAngDeg`, only set for non-degenerate tangents);
//!   [`fillet_radius`] returns `(radius, angle span)`. The `iDebugLevel` arguments
//!   (graphics only) are dropped.
//! - **Ignored statuses.** Where upstream ignores a callee's status (`SE`, or no check:
//!   `SmCurve::Evaluate`, `SmEdgeuse::EvaluateBinormal`, `SmVector3d::AngleBetween`,
//!   `Unitize`, `AdjustSTEPUVDomain`), the port continues with the values upstream's
//!   outputs keep: `SmVector3d` members start as `SM_UNDEF_DOUBLE`, `AngleBetween`
//!   reports `0`, `Unitize` stores `(0, 0, 1)`. Only [`SolidError::Unported`] propagates.
//!   `SmVector3d::CCWAngleBetween` (ignored in `sm_GetFilletRadius`) cannot fail there:
//!   its projected vectors pass a `100 * SM_EFF_ZERO_SQ` length test before
//!   `AngleBetween`, whose zero test is the smaller `SM_EFF_ZERO_SQ`.
//! - **Undefined inputs.** Where upstream dereferences null (an edge without a curve or
//!   edge uses, an edge use without a radial, vertex use or edge) or indexes past the
//!   curve array of `sm_CreateSrfOfRevolution`, the port returns
//!   [`SolidError::InvalidInput`]. A curve of `sm_CreateSrfOfRevolution` that is not an
//!   `SmBSplineCurve` (the array's element type) is [`SolidError::InvalidInput`].
//! - **Surfaces.** `SM_FILLETSURF_TYPE` is `SmSurface` (`SM_FILLET_SURFS` is defined):
//!   the new surfaces are [`Surface`] values (an `SmPlane` stays a plane).
//! - **Methods.** `GetContext` and `GetFilletExecutive` have no port: the [`Kernel`] and
//!   the executive's [`FilletExecutiveData`] are passed to the methods that need them
//!   (`m_pExecutive`'s `m_vExtendedSurfacesMap` is
//!   [`FilletExecutiveData::extended_surfaces_map`]). The virtual methods dispatch on
//!   [`FilletCorner::kind`]; the `SmFilletNx1ClosedCorner` / `SmFilletNx2Corner`
//!   overrides of `GetTangentFilletSolver` belong to unit `blend.fillet_corner_p2` and
//!   are [`SolidError::Unported`] there. `SqueezeEdge` removes the deleted edge and
//!   vertex from the corner's lists at once instead of leaving the `NULL` slots that
//!   `CalcCornerEdgeGeom` compresses. `ReInitializeCornerEdges` applies
//!   `SmEdge::SetCurve(NULL, TRUE)`'s notification effects to the pseudo B-rep's
//!   records directly (no spatial cache: the pseudo B-rep has none).
//! - **Corner geometry steps.** `CalcCornerVertGeom` / `CalcCornerEdgeGeom` call
//!   `SmFilletVertex::CalcCornerVertGeom` / `SmFilletEdge::CalcCornerEdgeGeom`
//!   ([`FilletBrep::calc_corner_vert_geom`] /
//!   [`FilletBrep::calc_corner_edge_geom`]) per vertex / edge, with the
//!   [`CornerEnv`]'s kernel, target B-rep and solvers. The vertex and edge calculators
//!   that read the vertex's or edge's corner get this corner and the executive data
//!   ([`CornerLookup::of_corner`]: upstream's corners only reach their own vertices and
//!   edges). The variable-radius law-interval adjustment of `CalcCornerVertGeom` is
//!   ported; resolving a fillet geometry's solver to its variable-radius type
//!   (`OffsetRadiiCanChange`, the `SmVariableRadiusFS` cast) needs the executive's
//!   solver store and is `Unported`
//!   (`blend.fillet_executive_p1`, #260). `CalcCornerEdgeGeom` iterates over a snapshot
//!   of `m_vEdges` so a squeeze in the loop sees the same slots as upstream's `NULL`
//!   entries, compressed at the end. `m_pExecutive->m_bDoGlobalMerge` and
//!   `m_vTopoEdges` (`RemoveFromTopoEdges`, CO-PORTED from `blend.fillet_executive_p1`)
//!   and `m_sErrorInfo` ([`FilletExecutiveData::error`]) are fields of
//!   [`FilletExecutiveData`]. `m_pBrep->DeleteEdge` of a failed corner edge is the
//!   pseudo-B-rep's wire-edge deletion ([`FilletBrep::delete_wire_fillet_edge`]),
//!   handed in by the caller of [`FilletCorner::calc_corner_edge_geom_with`].
//! - **`MakeFaceBrep`** builds into [`CornerEnv::fillet`] (`m_pExecutive->m_pFilletBrep`;
//!   `None` is the `NULL` upstream dereferences) through `SmBrep::MakeFaceWithCurves`
//!   with that B-rep's notification environment; the face gets a clone of the corner
//!   surface (upstream hands it the pointer `m_vSurfaces` keeps). Fewer than three corner
//!   edges with one surface (`m_vEdges[2]`) and an edge without curve or vertex are
//!   [`SolidError::InvalidInput`].
//! - **`MakeConstantRadiusFillet`** builds its temporary `SmConstantRadiusFS` from the two
//!   fillet surfaces ([`ConstantRadiusFs::from_surfaces`], identities
//!   [`SurfaceId::Free`] of their fillet geometries) with the identity one past
//!   [`CornerEnv::solvers`], and recalculates its geometry through
//!   `blend.fillet_geom_p3`'s `ReCalcFilletGeom` with the solver's own virtuals
//!   (`PlainConstantRadiusFs`: [`ConstantRadiusFs`] stands for the assisted solver).
//!   Upstream deletes the solver on return (not the fillet surface it returns); here
//!   the solver is dropped and its geometry stays unreferenced in the pseudo B-rep's
//!   table, as do the two rails whose curves were moved. An unknown surface generator
//!   type (no solver is made, `pNewFS->SetFilletExecutive` on `NULL`) and the
//!   `m_vEdges[9999]` read of a corner without a measurable edge are
//!   [`SolidError::InvalidInput`]; the `FIX_LINEAR_CASE` block is not compiled upstream.
//! - **`InsertIntersectionTopology`** runs on the fillet executive, which the caller
//!   supplies as a [`CornerInsertionContext`] (the live session's insertion context:
//!   its topology intersector, edge attribute, marker attributes and topology-change
//!   bookkeeping). There is no executive-free form: the indexed entry over
//!   [`CornerEnv`] (which carries no topology intersector) is retired, as in
//!   `blend.fillet_geom_p4`.
//!
//! No loops depend on input size beyond the given arrays; there is no recursion.

use std::collections::BTreeMap;

use crate::blend::fillet_corner::p2::is_b_spline_kind;
use crate::blend::fillet_corner::p3::{
    CornerEnv, CornerPatchEnv, ExecutiveErrorObject, FilletCorner, FilletCornerKind,
    FilletExecutiveData,
};
use crate::blend::fillet_custom_solver::{InitialValues, JacobianLoad, OffsetSurface, SurfaceId};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilStatus, FilletBrep, FilletEdgeType, FilletErrorType, FilletGeomId,
    FilletGeomType, FilletSolverId, FilletVertexType,
};
use crate::blend::fillet_geom::p4::set_edge_curve;
use crate::blend::fillet_solver::p1::{
    FilletExecutiveRef, FilletSolver, FilletSolverOps, FilletSolverType, FilletSurfaceGenerator,
    FilletSurfaceGeneratorType,
};
use crate::blend::fillet_standard_solver::p1::{
    constant_radius_is_kind_of, ConstantRadiusFs, VariableRadiusFs, CONSTANT_RADIUS_FS_TYPE,
};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::{p2 as curve2, Curve, CurveKind};
use crate::curve::line::Line;
use crate::error::EntityRef;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::ExtentNd;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::matrix::Matrix;
use crate::geom::vector2d::Point2;
use crate::intersect::point_class::PointClassType;
use crate::intersect::surface_intersector::p1::{IntersectionPointType, SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::brep::{BrepFaceServices, FaceCaches};
use crate::surface::plane::Plane;
use crate::surface::surface::Surface;
use crate::topology::brep::p7::{make_face_with_curves, FaceCurvesInput};
use crate::topology::brep_cache::CacheObject;
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};
use crate::topology::{
    edge, vertex, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, ObjectHandle, VertexId, VertexuseId,
};

pub use crate::blend::fillet_geom::p1::{check_seam_out, set_surface_closure};

const OP_G1: &str = "blend::fillet_corner::p1::test_g1_continuity";
const OP_FACE: &str = "blend::fillet_corner::p1::face_by_two_edgeuses";
const OP_TANGENTIAL: &str = "blend::fillet_corner::p1::check_tangential_edgeuse";
const OP_CONCAVITY: &str = "blend::fillet_corner::p1::check_face_complimentary_corner_concavity";
const OP_REVOLUTION: &str = "blend::fillet_corner::p1::create_srf_of_revolution";
const OP_PLANAR: &str = "blend::fillet_corner::p1::create_planar_surface";

/// The reason of the inputs upstream dereferences null on.
pub(super) const NULL_DEREF: &str = "upstream dereferences null";

/// The angle limit of `sm_CheckFaceComplimentaryCornerConcavity` ("Here we test against
/// 2 degrees").
const CONCAVITY_ANGLE_LIMIT_DEG: f64 = 2.0;

/// A status upstream ignores: `Unported` propagates, anything else gives `fallback`.
fn ignored<T>(r: Result<T>, fallback: T) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(fallback),
    }
}

/// `pCrv->Evaluate(t, 1, TRUE, aPV)` with the status ignored: position and tangent,
/// `SM_UNDEF_DOUBLE` vectors when the evaluation fails.
fn evaluate_tangent(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 2]> {
    let pv = ignored(curve.evaluate(k, t, 1, true, true).map(Some), None)?;
    Ok(match pv.as_deref() {
        Some([p, d, ..]) => [*p, *d],
        _ => [Vec3::UNDEF, Vec3::UNDEF],
    })
}

/// `pEU->EvaluateBinormal(t, FALSE, pnt, bin, pOptTangent, pOptNormal)` with the
/// status ignored (`SE`): binormal, tangent and face use normal, `SM_UNDEF_DOUBLE`
/// vectors when it fails.
fn evaluate_binormal(k: &Kernel, b: &mut Brep, eu: EdgeuseId, t: f64) -> Result<[Vec3; 3]> {
    let r = eu3::evaluate_binormal(k, b, eu, t, false, &eu3::BinormalOptions::default());
    let r = ignored(r.map(Some), None)?;
    Ok(match r {
        Some(e) => [e.binormal, e.tangent, e.face_normal],
        None => [Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF],
    })
}

/// `pEU->GetRadial()`, dereferenced by every caller.
fn radial(b: &Brep, eu: EdgeuseId, op: &'static str) -> Result<EdgeuseId> {
    eu1::radial(b, eu)?.ok_or_else(|| SolidError::invalid_input(op, NULL_DEREF))
}

/// `pEU->GetEdge()`, dereferenced by every caller.
fn edge_of(b: &Brep, eu: EdgeuseId, op: &'static str) -> Result<EdgeId> {
    eu1::edge(b, eu)?.ok_or_else(|| SolidError::invalid_input(op, NULL_DEREF))
}

/// `sm_TestG1Continuity`: whether the end tangents of `edge1` and `edge2` at `vert`
/// are within `ang_tol_deg` degrees of parallel (either sense). Returns the flag and
/// the angle in degrees (`pdAngDeg`, set only when both tangents are longer than
/// `SM_EFF_ZERO`). `false` when any of the three is missing.
///
/// Edge 1 is evaluated at the end of its interval at `vert` (its start when `vert` is
/// its start vertex); edge 2 at the start of its interval when `vert` is its start
/// vertex and it is not edge 1, else at the end.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without a curve (upstream dereferences
/// null); [`SolidError::Unported`] from the curve evaluations.
pub fn test_g1_continuity(
    k: &Kernel,
    b: &Brep,
    vert: Option<VertexId>,
    edge1: Option<EdgeId>,
    edge2: Option<EdgeId>,
    ang_tol_deg: f64,
) -> Result<(bool, Option<f64>)> {
    // no work - not given 3 valid pointers
    let (Some(vert), Some(edge1), Some(edge2)) = (vert, edge1, edge2) else {
        return Ok((false, None));
    };

    let crv1 =
        edge::curve(b, edge1)?.ok_or_else(|| SolidError::invalid_input(OP_G1, NULL_DEREF))?;
    let ivl1 = edge::interval(b, edge1)?;
    let param1 = if edge::start_vertex(b, edge1)? == Some(vert) {
        ivl1.min()
    } else {
        ivl1.max()
    };

    // get Edge1 tangent at vertex
    let pv1 = evaluate_tangent(k, crv1, param1)?;

    let crv2 =
        edge::curve(b, edge2)?.ok_or_else(|| SolidError::invalid_input(OP_G1, NULL_DEREF))?;
    let ivl2 = edge::interval(b, edge2)?;
    let param2 = if edge2 == edge1 || edge::start_vertex(b, edge2)? != Some(vert) {
        ivl2.max()
    } else {
        ivl2.min()
    };

    // get Edge2 tangent at vertex
    let pv2 = evaluate_tangent(k, crv2, param2)?;

    if pv1[1].length_squared() > EFF_ZERO_SQ && pv2[1].length_squared() > EFF_ZERO_SQ {
        // get angle (degrees) between Edge1 and Edge2 endTangents (SE: 0 on failure)
        let mut ang = ignored(pv1[1].angle_between(pv2[1]), 0.0)?;
        if ang > PI / 2.0 {
            ang = PI - ang;
        }
        let ang_deg = rad_to_deg(ang);

        // check for in tolerance angle
        return Ok((ang_deg < ang_tol_deg, Some(ang_deg)));
    }

    // endTangents are not within tolerance
    Ok((false, None))
}

/// `sm_GetFaceByTwoEdgeuses`: the first face shared by the sectors of `eu1` and `eu2`
/// (each edge use's face and its radial's face, in that order), skipping the face of
/// `eu3` when given. `None` when either edge use is missing or no face is shared.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without a radial (upstream
/// dereferences null).
pub fn face_by_two_edgeuses(
    b: &Brep,
    eu1: Option<EdgeuseId>,
    eu2: Option<EdgeuseId>,
    eu3: Option<EdgeuseId>,
) -> Result<Option<FaceId>> {
    let (Some(eu1), Some(eu2)) = (eu1, eu2) else {
        return Ok(None);
    };

    // let sFaces1 = two faces attached to pEU1->Sector
    let faces1 = [eu1::face(b, eu1)?, eu1::face(b, radial(b, eu1, OP_FACE)?)?];
    // let sFaces2 = two faces attached to pEU2->Sector
    let faces2 = [eu1::face(b, eu2)?, eu1::face(b, radial(b, eu2, OP_FACE)?)?];
    let skip = match eu3 {
        Some(eu3) => Some(eu1::face(b, eu3)?),
        None => None,
    };

    for face1 in faces1 {
        // when asked skip pEU3->Face
        if skip == Some(face1) {
            continue;
        }
        for face2 in faces2 {
            // return 1st found common face
            if face1 == face2 {
                return Ok(face2);
            }
        }
    }
    Ok(None)
}

/// `sm_CheckTangentialEdgeuse`: whether the face use normals of `eu` and its radial
/// at the middle of the edge's interval are parallel within `ang_tol_deg` degrees
/// (the edge separates two tangential faces).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without an edge or radial (upstream
/// dereferences null); [`SolidError::Unported`] from `EvaluateBinormal`.
pub fn check_tangential_edgeuse(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    ang_tol_deg: f64,
) -> Result<bool> {
    let e = edge_of(b, eu, OP_TANGENTIAL)?;
    let t = edge::interval(b, e)?.evaluate(0.5);

    let [_, _, norm1] = evaluate_binormal(k, b, eu, t)?;
    let rad = radial(b, eu, OP_TANGENTIAL)?;
    let [_, _, norm2] = evaluate_binormal(k, b, rad, t)?;

    // Edge is adjacent to two tangential faces when the normals are parallel
    Ok(norm1.is_parallel_to(norm2, ang_tol_deg))
}

/// The binormal and the corner-outward tangent of `eu` at `corner` (the shared steps
/// of `sm_CheckFaceComplimentaryCornerConcavity` for each edge use).
fn corner_binormal(
    k: &Kernel,
    b: &mut Brep,
    corner: VertexId,
    eu: EdgeuseId,
) -> Result<(Vec3, Vec3)> {
    let vu = eu1::vertexuse(b, eu)?
        .ok_or_else(|| SolidError::invalid_input(OP_CONCAVITY, NULL_DEREF))?;
    let vert = vertexuse::vertex(b, vu)?;
    let orient = eu1::orientation(b, eu)?;

    // get parameter on pEU corresponding to CornerVert
    let at_corner = vert == Some(corner);
    let t = if (at_corner && orient == OrientType::Opposite)
        || (!at_corner && orient == OrientType::Same)
    {
        1.0
    } else {
        0.0
    };

    // get pEU tangent (pointing from corner into edge) and binormal at cornerVertex
    let e = edge_of(b, eu, OP_CONCAVITY)?;
    let ivl = edge::interval(b, e)?;
    let [bin, mut tangent, _] = evaluate_binormal(k, b, eu, ivl.evaluate(t))?;
    if !at_corner {
        // our corner vertex is the end of the edge, so flip the tangent to point away
        tangent = -tangent;
    }
    Ok((bin, tangent))
}

/// `sm_CheckFaceComplimentaryCornerConcavity`: whether the face corner at `corner`
/// bounded by `test_eu1` and `test_eu2` (both pointing out of the corner) is concave
/// (its inside angle runs from 180 degrees plus tolerance to 360). With `ref_eu` (an
/// edge use lying between them, which must separate tangential faces within 2 degrees,
/// else `false`) the face is `ref_eu`'s; without it the face the two edge uses share.
/// Each test edge use is replaced by its radial when the radial lies on that face; the
/// corner is concave when the dot product of each unit binormal with the other edge
/// use's unit tangent is below `-2 * PI / 180` (upstream compares it to the angle limit
/// in radians).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without a radial, vertex use or edge
/// (upstream dereferences null); [`SolidError::Unported`] from `EvaluateBinormal`.
pub fn check_face_complimentary_corner_concavity(
    k: &Kernel,
    b: &mut Brep,
    corner: VertexId,
    test_eu1: EdgeuseId,
    test_eu2: EdgeuseId,
    ref_eu: Option<EdgeuseId>,
) -> Result<bool> {
    // If pOptRefEU is given, it needs to be a tangential edge
    if let Some(ref_eu) = ref_eu {
        if !check_tangential_edgeuse(k, b, ref_eu, CONCAVITY_ANGLE_LIMIT_DEG)? {
            return Ok(false);
        }
    }

    // get face spanning the edgeuse->Edges or any old face connected to the pOptRefEU->edge
    let common_face = match ref_eu {
        Some(ref_eu) => eu1::face(b, ref_eu)?,
        None => face_by_two_edgeuses(b, Some(test_eu1), Some(test_eu2), None)?,
    };

    // let pEU2 and pEU1 be Edgeuses to common face from their respective edges
    let mut eu_a = test_eu1;
    let mut eu_b = test_eu2;
    let radial2 = radial(b, test_eu2, OP_CONCAVITY)?;
    if common_face == eu1::face(b, radial2)? {
        eu_b = radial2;
    }
    let radial1 = radial(b, test_eu1, OP_CONCAVITY)?;
    if common_face == eu1::face(b, radial1)? {
        eu_a = radial1;
    }

    let (bin1, tangent1) = corner_binormal(k, b, corner, eu_a)?;
    let (bin2, tangent2) = corner_binormal(k, b, corner, eu_b)?;

    // Check if binormal of each edgeuse is pointing opposite way of the tangent of the
    // other edgeuse (Unitize statuses ignored)
    let bin1 = bin1.unitize_or_z();
    let bin2 = bin2.unitize_or_z();
    let tangent1 = tangent1.unitize_or_z();
    let tangent2 = tangent2.unitize_or_z();

    let val1 = bin1.dot(tangent2);
    let val2 = bin2.dot(tangent1);

    // SM_DEG2RAD(-dAngleLimitDegrees)
    let limit_rad = (-CONCAVITY_ANGLE_LIMIT_DEG) * (PI / 180.0);
    Ok(val1 < limit_rad && val2 < limit_rad)
}

/// The `SmBSplineCurve` part of `curve` (`SmTArray<SmBSplineCurve*>` elements: the
/// analytic curves derive from it upstream).
fn bspline_part(curve: &Curve) -> Result<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Ok(c),
        CurveKind::Line(c) => Ok(&c.base),
        CurveKind::Ellipse(c) => Ok(c.base()),
        CurveKind::Hyperbola(c) => Ok(c.base()),
        CurveKind::Parabola(c) => Ok(c.base()),
        _ => Err(SolidError::invalid_input(
            OP_REVOLUTION,
            "boundary curve is not an SmBSplineCurve",
        )),
    }
}

/// `sm_CreateSrfOfRevolution`: the surface of revolution filling the counter-clockwise
/// loop `curves` (end to end), when `curves[major]` is an arc (`IsArc` with 5 samples
/// and tolerance `tol_3d`) whose axis and angle span define the revolution. With four
/// curves the opposite curve must also be an arc on a colinear axis with an angle span
/// within 2 degrees. The curve before the major arc is revolved by 1.1 times the arc's
/// span; the curve after it must lie on the result (`SimpleCoincidenceChecker` within
/// `tol_3d`).
///
/// # Errors
///
/// `SM_ERR` when a required curve is not an arc, the axes or spans disagree, or the
/// next curve does not lie on the surface; [`SolidError::InvalidInput`] for an empty
/// array, an index outside it (upstream reads past the array) or a curve that is not
/// an `SmBSplineCurve`; those of `CreateSurfOfRevolution` and the coincidence check.
pub fn create_srf_of_revolution(
    k: &Kernel,
    curves: &[Curve],
    major: usize,
    tol_3d: f64,
) -> Result<Surface> {
    let total = curves.len();
    let out_of_range = || SolidError::invalid_input(OP_REVOLUTION, "curve index outside the array");
    let major_arc = curves.get(major).ok_or_else(out_of_range)?;

    // no work - can't make SurfOfRevolution when MajorArc is not an Arc
    let Some((frame, _rad, start_ang, end_ang)) = major_arc.is_arc(k, 5, tol_3d)? else {
        return Err(SolidError::sm_err(OP_REVOLUTION));
    };
    let origin = frame.origin();
    let ref_z_axis = frame.z_axis();
    let mut angle_span = end_ang - start_ang;

    // when given exactly 4 curves - make sure OppositeCurve->AxisOfRevolution ==
    // MajorCurve->AxisOfRevolution and have similar ArcAngle lengths
    if total == 4 {
        let other_arc = curves.get((major + 2) % 4).ok_or_else(out_of_range)?;
        let Some((frame1, _rad1, start_ang1, end_ang1)) = other_arc.is_arc(k, 5, tol_3d)? else {
            return Err(SolidError::sm_err(OP_REVOLUTION));
        };
        let origin1 = frame1.origin();
        let ref_z_axis1 = frame1.z_axis();
        let angle_span1 = end_ang1 - start_ang1;

        // Quit: if two Arcs don't share Colinear AxesOfRotation and similar ArcAngle lengths
        if !ref_z_axis.is_colinear_with(ref_z_axis1, origin, origin1)
            || (angle_span - angle_span1).abs() > 2.0
        {
            return Err(SolidError::sm_err(OP_REVOLUTION));
        }
    }

    // Increase angle span by 10% to make sure it's big enough
    angle_span *= 1.1;

    // Let SurfOfRevolution GenCurve = Curve CW from MajorArcCurve
    let gen_index = (major + total - 1)
        .checked_rem(total)
        .ok_or_else(out_of_range)?;
    let gen = curves.get(gen_index).ok_or_else(out_of_range)?;
    let gen_curve = bspline_part(gen)?;
    // (an SmLine generator: CreateSurfOfRevolution reads its line vector, and gives a
    // generator parallel to the axis an arc length sweep parameterisation)
    let gen_line_vector = match gen.kind() {
        CurveKind::Line(l) => Some(l.line_vector()),
        _ => None,
    };

    // Sweep GenCurve about MajorArcCurve AxisOfRevolution to make the fillet surface
    let surface = Surface::from(BSplineSurface::create_surf_of_revolution_with_line_vector(
        k,
        gen_curve,
        gen_line_vector,
        origin,
        ref_z_axis,
        angle_span,
        false,
        None,
    )?);

    // let OtherCurve = Curve opposite to GenCurve
    let other_curve = curves.get((major + 1) % total).ok_or_else(out_of_range)?;

    // Check for OtherCurve/SurfOfRevolution coincidence
    let ivl = other_curve.natural_interval();
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let found = surface.simple_coincidence_checker::<ObjectHandle>(
        k,
        &uv_domain,
        other_curve,
        &ivl,
        tol_3d,
        false,
        [None, None],
    )?;

    // when SurfOfRevolution is not coincident with OtherCurve - don't use it
    if found.is_none() {
        return Err(SolidError::sm_err(OP_REVOLUTION));
    }
    Ok(surface)
}

/// `sm_CreatePlanarSurface`: the planar corner patch through `plane_pnts`. Three points
/// give an `SmPlane` through their centroid with normal `(p1 - p0) x (p2 - p0)` and the
/// STEP domain `[-d, d]^2`, `d = |p1 - p0| + |p2 - p0|` (status ignored); four give the
/// bilinear B-spline surface with corners `p0, p1, p2, p3` in order around it.
/// `tol_3d` is unused upstream.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` (`Unitize`) for three collinear points; `SM_ERR` for any
/// other point count ("Not yet implemented"); those of `CreateBilinearSurface`.
pub fn create_planar_surface(k: &Kernel, plane_pnts: &[Point3], tol_3d: f64) -> Result<Surface> {
    let _ = tol_3d; // SM_REF1

    match plane_pnts {
        // when 3 points are given - we can always build an exactly interpolating plane
        [p0, p1, p2] => {
            // Make regular plane - centered on Point Centroid and oriented by given points
            let origin = (*p0 + *p1 + *p2) / 3.0;
            let x_axis = *p1 - *p0;
            let y_axis = *p2 - *p0;
            let z_axis = x_axis.cross(y_axis);

            // signal error for degenerate point set
            let z_axis = z_axis.unitize()?;

            // create infinite plane
            let mut plane = Surface::from(Plane::from_normal(origin, z_axis));

            // Adjust the newPlane->UVDomain to fit the given points (status ignored)
            let size = x_axis.length() + y_axis.length();
            let ext = Extent2d::from_points(Point2::new(-size, -size), Point2::new(size, size));
            ignored(plane.adjust_step_uv_domain(k, &ext), ())?;
            Ok(plane)
        }
        // Make NonRational Bilinear BSplineSurface interpolating given corners
        [p0, p1, p2, p3] => Ok(Surface::from(BSplineSurface::create_bilinear_surface(
            *p0, *p1, *p3, *p2,
        )?)),
        // other cases not yet implemented
        _ => Err(SolidError::sm_err(OP_PLANAR)),
    }
}

/// `sm_GetFilletRadius`: the radius and angle span (radians) of the circular arc
/// tangent to the two surfaces at `surf_pnt` / `surf_pnt2` with normals `surf_normal` /
/// `surf_normal2` (flipped when `offset_orientation > 0`): the counter-clockwise angle
/// between the normals about their cross product (folded to `PI - angle` above `PI`)
/// and `radius = |p2 - p1| / 2 / sin(angle / 2)`.
///
/// # Errors
///
/// None in practice (see the module docs on `CCWAngleBetween`).
pub fn fillet_radius(
    surf_pnt: Point3,
    surf_normal: Vec3,
    surf_pnt2: Point3,
    surf_normal2: Vec3,
    offset_orientation: f64,
) -> Result<(f64, f64)> {
    let (mut normal1, mut normal2) = (surf_normal, surf_normal2);
    if offset_orientation > 0.0 {
        normal1 = -normal1;
        normal2 = -normal2;
    }
    // Unitize status ignored
    let axis = normal1.cross(normal2).unitize_or_z();
    let mut ang_rad = axis.ccw_angle_between(normal1, normal2)?;
    if ang_rad > PI {
        ang_rad = PI - ang_rad;
    }
    let leng = surf_pnt.distance(surf_pnt2);
    let radius = leng / 2.0 / (ang_rad / 2.0).sin();
    Ok((radius, ang_rad))
}

// ---- SmFilletCorner methods: data access, predicates and the small helpers ---------------

const OP_SOLVERS: &str = "blend::fillet_corner::p1::fillet_solvers";
const OP_REINIT: &str = "blend::fillet_corner::p1::re_initialize_corner_edges";
const OP_SQUEEZE: &str = "blend::fillet_corner::p1::squeeze_edge";
const OP_RAIL_RAIL: &str = "blend::fillet_corner::p1::test_rail_rail_interpolation_curve";
const OP_NX1: &str = "blend::fillet_corner::p1::check_nx1_degeneracy";

/// `SmFilletCorner_TYPE` (`TOPO_BASE_TYPE + 36`).
pub const FILLET_CORNER_TYPE: i64 = 16036;
/// `SmFilletCorner::GetTypeString` (`SM_COMMON`).
pub const FILLET_CORNER_TYPE_STRING: &str = "SmFilletCorner_TYPE";

impl FilletCorner {
    /// `GetThisApproxTol3d` (its `SM_ASSERT_TOL` is debug-only). Also the
    /// `SmFilletCorner` branch of `SmTol::GetApproxTol3d(const SmObject*)`.
    #[must_use]
    pub const fn this_approx_tol_3d(&self) -> f64 {
        self.approx_tol_3d
    }

    /// `GetFilletEdges`: appends `m_vEdges` to `edges`.
    pub fn fillet_edges(&self, edges: &mut Vec<EdgeId>) {
        edges.extend_from_slice(&self.edges);
    }

    /// `GetFilletVertices`: appends `m_vVertices` to `verts`.
    pub fn fillet_vertices(&self, verts: &mut Vec<VertexId>) {
        verts.extend_from_slice(&self.vertices);
    }

    /// `GetFilletedVertex` (`m_cpVertex`).
    #[must_use]
    pub const fn filleted_vertex(&self) -> Option<VertexId> {
        self.vertex
    }

    /// `GetSetBackDist`.
    #[must_use]
    pub const fn set_back_dist(&self) -> f64 {
        self.set_back_dist
    }

    /// `GetStatus`.
    #[must_use]
    pub const fn status(&self) -> FilStatus {
        self.status
    }

    /// `GetType` (`SM_COMMON`): [`FILLET_CORNER_TYPE`] for every derived class.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        FILLET_CORNER_TYPE
    }

    /// `GetTypeString` (`SM_COMMON`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        FILLET_CORNER_TYPE_STRING
    }

    /// `IsKindOf`: an `SmFilletCorner` or what `SmObject::IsKindOf` accepts.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == FILLET_CORNER_TYPE || crate::geom::object::is_kind_of(t)
    }

    /// `AddFilletVertex`.
    pub fn add_fillet_vertex(&mut self, v: VertexId) {
        self.vertices.push(v);
    }

    /// `SetBevel`.
    pub fn set_bevel(&mut self, is_bevel: bool) {
        self.bevel = is_bevel;
    }

    /// `SetCalcAsConstRad`.
    pub fn set_calc_as_const_rad(&mut self, is_const: bool) {
        self.calc_as_const_rad = is_const;
    }

    /// `SmFilletConvexNxNCorner::SetChamfer` (the member is `SmFilletCorner`'s; the
    /// setter is only declared on the convex `NxN` corner upstream).
    pub fn set_chamfer(&mut self, is_chamfer: bool) {
        self.chamfer = is_chamfer;
    }

    /// `IsBevelCorner`.
    #[must_use]
    pub const fn is_bevel_corner(&self) -> bool {
        self.bevel
    }

    /// `IsBlendingCorner`.
    #[must_use]
    pub const fn is_blending_corner(&self) -> bool {
        self.blending
    }

    /// `IsDegenerateCorner`.
    #[must_use]
    pub const fn is_degenerate_corner(&self) -> bool {
        self.degenerate
    }

    /// `IsExtendedSurfacePatch`.
    #[must_use]
    pub const fn is_extended_surface_patch(&self) -> bool {
        self.extended_surface_patch
    }

    /// `IsCalcAsConstRad`.
    #[must_use]
    pub const fn is_calc_as_const_rad(&self) -> bool {
        self.calc_as_const_rad
    }

    /// `IsTangentCorner` (virtual): `m_bG1Case` for the closed Nx1 and the Nx2 corner,
    /// `FALSE` for every other kind.
    #[must_use]
    pub const fn is_tangent_corner(&self) -> bool {
        match self.kind {
            FilletCornerKind::NByOneClosed | FilletCornerKind::NByTwo => self.g1_case,
            _ => false,
        }
    }

    /// `GetTangentFilletSolver(pFS)` (virtual): the solver of the other filleted edge
    /// use of a 2x2 corner, the solver itself for a 1x1 corner, `None` (`SM_ASSERT_ERR`)
    /// when the corner has another number of filleted edge uses or `fs` is not one of
    /// their solvers; `None` for the base class (`SM_REF1(pFS)`). The closed Nx1 and Nx2
    /// overrides (unit `blend.fillet_corner_p2`) also test `sm_TestG1Continuity` on the
    /// filleted edges of `b`, the B-rep being filleted, which the base and the 1x1 / 2x2
    /// overrides do not read.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a 1x1, 2x2, closed Nx1 or Nx2 corner without
    /// `m_pSolverEUs` (upstream dereferences it); those of
    /// [`FilletCorner::tangent_fillet_solver_nx1_closed`] and
    /// [`FilletCorner::tangent_fillet_solver_nx2`].
    pub fn tangent_fillet_solver(
        &self,
        k: &Kernel,
        b: &Brep,
        exec: &FilletExecutiveData,
        fs: FilletSolverId,
    ) -> Result<Option<FilletSolverId>> {
        self.tangent_fillet_solver_with_lookup(k, b, fs, |eu| exec.fillet_solver_of_edgeuse(eu))
    }

    /// Borrowed-lookup form of [`FilletCorner::tangent_fillet_solver`] for live
    /// executive adapters whose solver identities are generic handles.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a 1x1, 2x2, closed Nx1 or Nx2 corner without
    /// `m_pSolverEUs`; those of
    /// [`FilletCorner::tangent_fillet_solver_nx1_closed_with_lookup`] and
    /// [`FilletCorner::tangent_fillet_solver_nx2_with_lookup`].
    pub fn tangent_fillet_solver_with_lookup<S: Copy + Eq>(
        &self,
        k: &Kernel,
        b: &Brep,
        fs: S,
        mut solver_of: impl FnMut(EdgeuseId) -> Option<S>,
    ) -> Result<Option<S>> {
        const OP: &str = "blend::fillet_corner::p1::tangent_fillet_solver";
        match self.kind {
            FilletCornerKind::OneByOne => {
                let eus = self
                    .solver_eus
                    .as_deref()
                    .ok_or_else(|| SolidError::invalid_input(OP, NULL_DEREF))?;
                let [eu] = eus else {
                    return Ok(None);
                };
                let my_fs = solver_of(*eu);
                if my_fs != Some(fs) {
                    return Ok(None);
                }
                Ok(my_fs)
            }
            FilletCornerKind::TwoByTwo => {
                let eus = self
                    .solver_eus
                    .as_deref()
                    .ok_or_else(|| SolidError::invalid_input(OP, NULL_DEREF))?;
                let [eu0, eu1_] = eus else {
                    return Ok(None);
                };
                let fs1 = solver_of(*eu0);
                let fs2 = solver_of(*eu1_);
                if fs1 == Some(fs) {
                    return Ok(fs2);
                }
                if fs2 == Some(fs) {
                    return Ok(fs1);
                }
                Ok(None)
            }
            FilletCornerKind::NByOneClosed => {
                self.tangent_fillet_solver_nx1_closed_with_lookup(k, b, fs, solver_of)
            }
            FilletCornerKind::NByTwo => {
                self.tangent_fillet_solver_nx2_with_lookup(k, b, fs, solver_of)
            }
            _ => Ok(None),
        }
    }

    /// `GetFilletSolvers`: one entry per filleted edge use, each the solver of the
    /// FIRST filleted edge use (`(*m_pSolverEUs)[0]` inside the loop, as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without `m_pSolverEUs` (upstream dereferences it).
    pub fn fillet_solvers(
        &self,
        exec: &FilletExecutiveData,
    ) -> Result<Vec<Option<FilletSolverId>>> {
        self.fillet_solvers_with_lookup(|edgeuse| Ok(exec.fillet_solver_of_edgeuse(edgeuse)))
    }

    /// Resolve the actual corner solvers through a borrowed executive lookup.
    /// The native repeated-first-edgeuse lookup is retained.
    ///
    /// # Errors
    /// Missing corner edge uses or actual solver lookup failures.
    pub fn fillet_solvers_with_lookup(
        &self,
        mut solver_of: impl FnMut(EdgeuseId) -> Result<Option<FilletSolverId>>,
    ) -> Result<Vec<Option<FilletSolverId>>> {
        let eus = self
            .solver_eus
            .as_deref()
            .ok_or_else(|| SolidError::invalid_input(OP_SOLVERS, NULL_DEREF))?;
        let mut solvers = Vec::new();
        for _ in eus {
            let first = eus
                .first()
                .ok_or_else(|| SolidError::invalid_input(OP_SOLVERS, "empty solver edge uses"))?;
            solvers.push(solver_of(*first)?);
        }
        Ok(solvers)
    }

    /// `GetExtendedSurface(pOriginalFace)`: the extended surface registered in the
    /// executive for a face of the original B-rep, `None` when there is none.
    #[must_use]
    pub fn extended_surface<'a>(
        &self,
        exec: &'a FilletExecutiveData,
        original_face: FaceId,
    ) -> Option<&'a Surface> {
        exec.extended_surfaces_map.get(&original_face)
    }

    /// `AddExtendedOriginalSurface(pOriginalFace, pExtSurface)`: registers the pair in
    /// the executive's `m_vExtendedSurfacesMap`. When the face already has one, the
    /// surface with the larger natural domain is kept (the new one is dropped when its
    /// domain is contained in the old one's within `SM_EFF_ZERO`).
    pub fn add_extended_original_surface(
        &self,
        exec: &mut FilletExecutiveData,
        original_face: FaceId,
        ext_surface: Surface,
    ) {
        Self::add_extended_original_surface_with_map(
            &mut exec.extended_surfaces_map,
            original_face,
            ext_surface,
        );
    }

    /// Register an actual extended surface in the executive's owned face map.
    /// The same native domain-containment rule is used by both executive adapters.
    /// Returns whether `ext_surface` was kept (upstream callers compare
    /// `GetExtendedSurface` with the pointer they added).
    pub fn add_extended_original_surface_with_map(
        map: &mut BTreeMap<FaceId, Surface>,
        original_face: FaceId,
        ext_surface: Surface,
    ) -> bool {
        if let Some(old_surface) = map.get(&original_face) {
            // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
            let new_domain = ext_surface.natural_uv_domain_if_built();
            let old_domain = old_surface.natural_uv_domain_if_built();
            if new_domain.is_contained_by(&old_domain, EFF_ZERO) {
                // Old domain was bigger, delete the new ext surface.
                return false;
            }
            // else: the old surface is deleted (replaced below)
        }
        map.insert(original_face, ext_surface);
        true
    }

    /// `ReInitializeCornerEdges`: `SetCurve(NULL, TRUE)` on every corner fillet edge
    /// of the pseudo B-rep: the curve is dropped and, as the geometry-change
    /// notification does, the UV trim curves of its edge uses are deleted and the
    /// cached gaps of its edge uses, their mates and its vertex uses reset. An edge
    /// without a curve is left alone (`SmEdge::SetCurve` returns at once).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] / `SM_ERR_NULL_POINTER` where the notification
    /// dereferences null (an edge use without mate or vertex use, a null vertex use);
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn re_initialize_corner_edges(&self, pseudo: &mut FilletBrep) -> Result<()> {
        let b = pseudo.brep_mut();
        for &e in &self.edges {
            if b.edge(e)?.curve.is_none() {
                continue;
            }
            // side effect: delete all UVTrimCurves and the cached gaps (SmEdge::Notify,
            // SM_NO_CHANGE_GEOMETRY)
            for eu in edge::edgeuses(b, e, None, None)? {
                eu1::set_uv_trim_curve(b, eu, eu1::UvTrimCurveArg::Null, 0.0, true, None)?;
                let m = eu1::mate(b, eu)?.ok_or_else(|| {
                    SolidError::invalid_input(OP_REINIT, "Notify: edge use without mate")
                })?;
                b.edgeuse_mut(eu)?.max_edge_face_gap.reset();
                b.edgeuse_mut(m)?.max_edge_face_gap.reset();
            }
            for vu in edge::vertexuses(b, e, None)? {
                let vu = vu.ok_or_else(|| SolidError::null_pointer(OP_REINIT))?;
                let r = b.vertexuse_mut(vu)?;
                r.vertex_edge_gap.reset();
                r.vertex_face_gap.reset();
            }
            b.edge_mut(e)?.curve = None;
        }
        Ok(())
    }

    /// `SqueezeEdge(pEdgeToSqueeze)`: collapses a fillet edge of the pseudo B-rep onto
    /// its surviving vertex. The vertex marked `SM_FV_TO_BE_DELETED` is deleted: the
    /// vertex uses of its rail edges move to the survivor, its other edges (corner
    /// edges of this corner) are deleted, then the vertex itself.
    ///
    /// Deviation: upstream leaves `NULL` slots in `m_vEdges` / `m_vVertices` that
    /// `CalcCornerEdgeGeom` compresses afterwards; here the entries are removed at
    /// once (the relative order is the same).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when neither vertex is to be deleted or an edge / the vertex is not
    /// this corner's; [`SolidError::InvalidInput`] where upstream dereferences null;
    /// those of the deletions.
    pub fn squeeze_edge(&mut self, pseudo: &mut FilletBrep, edge_to_squeeze: EdgeId) -> Result<()> {
        let null = || SolidError::invalid_input(OP_SQUEEZE, NULL_DEREF);
        // Get the vertex that will be deleted
        let mut surviving = edge::vertex(pseudo.brep(), edge_to_squeeze)?.ok_or_else(null)?;
        let mut delete = edge::other_vertex(pseudo.brep(), edge_to_squeeze, Some(surviving))?
            .ok_or_else(null)?;
        if pseudo.fillet_vertex(delete)?.status != FilStatus::ToBeDeleted {
            core::mem::swap(&mut surviving, &mut delete);
        }
        if pseudo.fillet_vertex(delete)?.status != FilStatus::ToBeDeleted {
            return Err(SolidError::sm_err(OP_SQUEEZE));
        }

        let vus = vertex::vertexuses(pseudo.brep(), delete)?;
        for vu in vus {
            let eu = eu1::vertexuse_edgeuse(pseudo.brep(), vu)?.ok_or_else(null)?;
            let fe = eu1::edge(pseudo.brep(), eu)?.ok_or_else(null)?;
            if pseudo.fillet_edge(fe)?.edge_type == FilletEdgeType::Rail {
                // pDeleteFV->Remove(pVU); pSurvivingFV->PostInsert(pVU); the edge record's
                // vertex end this vertex use stands for (the ends are derived from the
                // vertex uses upstream) follows: the primary edge use's vertex is the start
                // end when the primary is oriented SM_OT_SAME, else the end
                let b = pseudo.brep_mut();
                b.vertex_mut(delete)?.vertexuses.retain(|&x| x != vu);
                b.vertexuse_mut(vu)?.vertex = Some(surviving);
                b.vertex_mut(surviving)?.vertexuses.push(vu);
                let prim = eu1::primary_edgeuse(b, fe)?.ok_or_else(null)?;
                let same = eu1::orientation(b, prim)? == OrientType::Same;
                let slot = usize::from((eu == prim) != same);
                if let Some(end) = b.edge_mut(fe)?.vertices.get_mut(slot) {
                    *end = surviving;
                }
            } else {
                let found = self
                    .edges
                    .iter()
                    .position(|&x| x == fe)
                    .ok_or_else(|| SolidError::sm_err(OP_SQUEEZE))?;
                self.edges.remove(found);
                pseudo.delete_fillet_edge(fe)?;
            }
        }
        let found = self
            .vertices
            .iter()
            .position(|&x| x == delete)
            .ok_or_else(|| SolidError::sm_err(OP_SQUEEZE))?;
        self.vertices.remove(found);
        pseudo.delete_fillet_vertex(delete)?;

        // all done
        Ok(())
    }

    /// `SmFilletConcaveNxNCorner::TestRailRailInterpolationCurve(pCurve)`: whether the
    /// corner vertex lies on the concave side of `curve` at its closest point (the
    /// vector from the closest point to the vertex opposes the curvature vector).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without `m_cpVertex` (upstream dereferences it);
    /// `SM_ERR` when the closest-point solve does not give exactly one solution; those
    /// of the solve, the evaluation and the unitisations (zero curvature or a vertex on
    /// the curve).
    pub fn test_rail_rail_interpolation_curve(
        &self,
        k: &Kernel,
        target: &Brep,
        curve: &Curve,
    ) -> Result<bool> {
        // Check to see if the original corner 'intersects' the curve
        // Drop corner vertex onto pCurve
        let vertex = self
            .vertex
            .ok_or_else(|| SolidError::invalid_input(OP_RAIL_RAIL, NULL_DEREF))?;
        let corner_pnt = target.vertex(vertex)?.point;
        let ivl = curve.natural_interval();
        let solutions = curve.global_point_solve::<ObjectHandle>(
            k,
            &ivl,
            SolverOperation::Minimize,
            corner_pnt,
            self.approx_tol_3d,
            SolutionRequested::Single,
            &GlobalPointSolveOptions::default(),
        )?;
        if solutions.len() != 1 {
            return Err(SolidError::sm_err(OP_RAIL_RAIL));
        }
        let t = solutions
            .get(0)
            .and_then(|s| s.start.parameters.first().copied())
            .ok_or_else(|| SolidError::sm_err(OP_RAIL_RAIL))?;
        // Find the 2nd deriv. of the curve at dT
        let geom = curve2::evaluate_geometric(k, curve, t, 2, true)?;
        let (Some(pos), Some(curvature)) = (geom.first(), geom.get(2)) else {
            return Err(SolidError::sm_err(OP_RAIL_RAIL));
        };
        let v = corner_pnt - *pos;
        let curvature = curvature.unitize()?;
        let v = v.unitize()?;
        Ok(v.dot(curvature) < 0.0)
    }
}

/// `SmFilletNx1Corner::CheckNx1Degeneracy(pCornerVert, pFilEdgeuse)` (static): whether
/// the two side edge uses next to the filleted edge use at the corner vertex (its
/// clockwise neighbour and its radial's counter-clockwise neighbour; the mate of the
/// first when they coincide, the 2x1 case) have parallel face use normals at the
/// corner end of their edges (within `SM_EFF_ZERO_SQRT`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (a missing neighbour,
/// radial, mate, vertex use or edge); those of the binormal evaluations
/// (`SmEdgeuse::EvaluateBinormal`, `SER`).
pub fn check_nx1_degeneracy(
    k: &Kernel,
    b: &mut Brep,
    corner_vert: VertexId,
    fil_edgeuse: EdgeuseId,
) -> Result<bool> {
    check_nx1_degeneracy_status(k, b, corner_vert, fil_edgeuse)?
}

/// [`check_nx1_degeneracy`] with the two outcomes upstream distinguishes: the outer
/// error is what the harness cannot run (a null dereference, a dangling id, an unported
/// callee), the inner one the status the routine returns through its `SmBoolean` when a
/// side edge use's binormal evaluation fails (`SER(pSideEU->EvaluateBinormal(..))`,
/// which `Create`'s `if (CheckNx1Degeneracy(...))` reads as true).
///
/// # Errors
///
/// As [`check_nx1_degeneracy`]'s guards.
pub fn check_nx1_degeneracy_status(
    k: &Kernel,
    b: &mut Brep,
    corner_vert: VertexId,
    fil_edgeuse: EdgeuseId,
) -> Result<Result<bool, SolidError>> {
    let null = || SolidError::invalid_input(OP_NX1, NULL_DEREF);
    // Get adjacent edgeuses of filleted edgeuse
    let side_eu1 = eu1::cw_edgeuse(b, fil_edgeuse)?.ok_or_else(null)?;
    let radial = eu1::radial(b, fil_edgeuse)?.ok_or_else(null)?;
    let mut side_eu2 = eu1::ccw_edgeuse(b, radial)?.ok_or_else(null)?;
    if side_eu1 == side_eu2 {
        // likely 2x1 case
        side_eu2 = eu1::mate(b, side_eu1)?.ok_or_else(null)?;
    }

    // the parameter of each side edge at the corner end
    let corner_param = |b: &Brep, eu: EdgeuseId| -> Result<f64> {
        let vu = eu1::vertexuse(b, eu)?.ok_or_else(null)?;
        let vert = b.vertexuse(vu)?.vertex;
        let e = eu1::edge(b, eu)?.ok_or_else(null)?;
        let ivl = edge::interval(b, e)?;
        let orientation = eu1::orientation(b, eu)?;
        let mut t = ivl.max();
        if (vert == Some(corner_vert) && orientation == OrientType::Same)
            || (vert != Some(corner_vert) && orientation == OrientType::Opposite)
        {
            t = ivl.min();
        }
        Ok(t)
    };
    let t1 = corner_param(b, side_eu1)?;
    let t2 = corner_param(b, side_eu2)?;
    let options = eu3::BinormalOptions::default();
    // SER(..): the evaluation's status is returned through the SmBoolean; a null
    // dereference, a dangling id or an unported callee inside it is not a status
    let binormal = |b: &mut Brep, eu: EdgeuseId, t: f64| -> Result<Result<Vec3, SolidError>> {
        match eu3::evaluate_binormal(k, b, eu, t, false, &options) {
            Ok(e) => Ok(Ok(e.face_normal)),
            Err(e @ (SolidError::Unported { .. } | SolidError::NotFound { .. })) => Err(e),
            Err(
                e @ (SolidError::InvalidInput { reason, .. }
                | SolidError::PortInvariant { reason, .. }),
            ) if reason == NULL_DEREF => Err(e),
            Err(e) => Ok(Err(e)),
        }
    };
    let faceuse_normal1 = match binormal(b, side_eu1, t1)? {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };
    let faceuse_normal2 = match binormal(b, side_eu2, t2)? {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };
    // Check if faceuse normals are parallel to each other. With
    // `fillet_knife_edge_rounds` (WR-570, fork b7ec55b) anti-parallel normals are a cusp
    // (the faces folded back on each other, a knife edge): the ball touches them back
    // from the vertex and the fillet ends there as at any other corner
    if k.fixes.fillet_knife_edge_rounds {
        // (`bAntiParallel` starts FALSE; `IsParallelTo` leaves it for a zero vector)
        let (parallel, anti) = faceuse_normal1.parallel_sense(faceuse_normal2, EFF_ZERO_SQRT);
        return Ok(Ok(parallel && !anti.unwrap_or(false)));
    }
    Ok(Ok(
        faceuse_normal1.is_parallel_to(faceuse_normal2, EFF_ZERO_SQRT)
    ))
}

// ---- SmFilletCorner methods: the corner geometry steps -----------------------------------

const OP_VERT_GEOM: &str = "blend::fillet_corner::p1::calc_corner_vert_geom";
const OP_EDGE_GEOM: &str = "blend::fillet_corner::p1::calc_corner_edge_geom";
const OP_DEGENERACY: &str = "blend::fillet_corner::p1::check_and_fix_degeneracy";
const OP_END_PATCH: &str = "blend::fillet_corner::p1::create_end_patch";
const OP_MAKE_FACE: &str = "blend::fillet_corner::p1::make_face_brep";
const OP_CONST_RAD: &str = "blend::fillet_corner::p1::make_constant_radius_fillet";
const OP_INSERT: &str = "blend::fillet_corner::p1::insert_intersection_topology";

/// The solver store a corner's law-interval adjustment resolves fillet geometries'
/// solvers through.
pub trait LawIntervalSolvers {
    /// `pThisFS->OffsetRadiiCanChange()` then `SM_CAST_PTR(SmVariableRadiusFS, pThisFS)`:
    /// the variable-radius solver behind `id` when its offset radii can change, else
    /// `None` (the native loop's two `continue`s).
    ///
    /// # Errors
    /// An unresolved solver identity or those of `OffsetRadiiCanChange`.
    fn variable_radius_mut(&mut self, id: FilletSolverId) -> Result<Option<&mut VariableRadiusFs>>;
}

impl LawIntervalSolvers for [FilletSolver] {
    fn variable_radius_mut(
        &mut self,
        _id: FilletSolverId,
    ) -> Result<Option<&mut VariableRadiusFs>> {
        // A table of base solvers: SmFilletSolver::OffsetRadiiCanChange() is TRUE and
        // the SM_CAST_PTR(SmVariableRadiusFS, ..) of a base solver is NULL.
        Ok(None)
    }
}

/// `pFrom->Remove(pVU); pTo->PostInsert(pVU)` for a vertex use of the pseudo B-rep; the
/// edge record's vertex ends (derived from the vertex uses upstream) follow.
fn move_vertexuse(
    pseudo: &mut FilletBrep,
    vu: VertexuseId,
    from: VertexId,
    to: VertexId,
) -> Result<()> {
    let b = pseudo.brep_mut();
    b.vertex_mut(from)?.vertexuses.retain(|&x| x != vu);
    b.vertexuse_mut(vu)?.vertex = Some(to);
    b.vertex_mut(to)?.vertexuses.push(vu);
    if let Some(e) =
        eu1::vertexuse_edgeuse(b, vu)?.and_then(|eu| b.edgeuse(eu).ok().and_then(|r| r.edge))
    {
        for end in &mut b.edge_mut(e)?.vertices {
            if *end == from {
                *end = to;
            }
        }
    }
    Ok(())
}

impl FilletCorner {
    /// `CalcCornerVertGeom`: `SmFilletVertex::CalcCornerVertGeom` on every corner fillet
    /// vertex (any failure is `SM_ERR`), then, for a corner calculated as a constant-radius
    /// blend, the variable-radius solvers' law intervals are shortened to the edge
    /// parameters the newly processed vertices found.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: those of the vertex calculations, and
    /// the solver store of the law-interval adjustment (`blend.fillet_executive_p1`,
    /// which resolves a fillet geometry's solver to its variable-radius type); `SM_ERR`
    /// for a vertex calculation that fails;
    /// [`SolidError::InvalidInput`] for a vertex without rail-end vertex use, a fillet
    /// geometry without solver or a solver without first edge use (upstream dereferences
    /// them); those of the interval arithmetic.
    pub fn calc_corner_vert_geom(
        &mut self,
        env: &mut CornerEnv<'_>,
        solvers: &mut [FilletSolver],
    ) -> Result<()> {
        let mut context = (env, solvers);
        self.calc_corner_vert_geom_with(
            &mut context,
            |(env, _), fv| Ok(env.pseudo.fillet_vertex(fv)?.is_processed()),
            |(env, _), fv| {
                env.pseudo.calc_corner_vert_geom(
                    env.k,
                    fv,
                    CornerLookup::of_corner(env.corner, self, env.exec),
                    env.target,
                    env.solvers,
                )
            },
            |(env, solvers), corner, processed| {
                corner.adjust_vertex_law_intervals(env.pseudo, env.target, *solvers, processed)
            },
        )
    }

    /// Calculate corner vertices in native order using the caller's live objects.
    ///
    /// The processed flag is sampled immediately before each calculation. Ordinary
    /// calculation errors become `SM_ERR`; genuine missing dependencies propagate.
    /// Law adjustment runs afterward only for a corner calculated as constant radius.
    /// The explicit context lets both phases borrow one authoritative object store.
    ///
    /// # Errors
    /// Processed-state lookup and law-adjustment errors, `SM_ERR` for a failed
    /// calculation, or its genuine [`SolidError::Unported`] dependency.
    pub fn calc_corner_vert_geom_with<C>(
        &self,
        context: &mut C,
        mut is_processed: impl FnMut(&C, VertexId) -> Result<bool>,
        mut calculate: impl FnMut(&mut C, VertexId) -> Result<()>,
        adjust_law_intervals: impl FnOnce(&mut C, &Self, &[bool]) -> Result<()>,
    ) -> Result<()> {
        let mut already_processed = Vec::new();
        for &fv in &self.vertices {
            already_processed.push(is_processed(context, fv)?);
            match calculate(context, fv) {
                Ok(()) => {}
                Err(e @ SolidError::Unported { .. }) => return Err(e),
                Err(_) => return Err(SolidError::sm_err(OP_VERT_GEOM)),
            }
        }
        if !self.is_calc_as_const_rad() {
            return Ok(());
        }
        adjust_law_intervals(context, self, &already_processed)
    }

    /// The law-interval half of `SmFilletCorner::CalcCornerVertGeom`, run for a corner
    /// calculated as constant radius: every unprocessed vertex's adjacent
    /// variable-radius solvers narrow their law interval to the vertex's edge
    /// parameter.
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] for a vertex without rail-end vertex use or a
    /// fillet geometry without solver (upstream dereferences them);
    /// `SM_ERR_NULL_POINTER` for a missing vertex, intersection parameter or solver
    /// edge use; those of the solver store and the interval arithmetic.
    pub fn adjust_vertex_law_intervals<S: LawIntervalSolvers + ?Sized>(
        &self,
        pseudo: &FilletBrep,
        target: &Brep,
        solvers: &mut S,
        already_processed: &[bool],
    ) -> Result<()> {
        let num_verts = self.vertices.len();
        // Get the corner vertex, for determining Start or End.
        let corner_vertex = self.filleted_vertex();

        for ii in 0..num_verts {
            if already_processed.get(ii).copied().unwrap_or(false) {
                continue; // Would happen with a Mate, for example.
            }
            let fv = *self
                .vertices
                .get(ii)
                .ok_or_else(|| SolidError::null_pointer(OP_VERT_GEOM))?;

            let fgs = pseudo.vertex_fillet_geoms(fv)?;
            for fg in fgs {
                let fvu = pseudo
                    .vu_at_rail_end(fv, Some(fg), None, None)?
                    .ok_or_else(|| SolidError::invalid_input(OP_VERT_GEOM, NULL_DEREF))?;
                let edge_t = pseudo
                    .fillet_vertexuse(fvu)?
                    .tsect_pnt()
                    .user_doubles
                    .first()
                    .copied()
                    .ok_or_else(|| SolidError::null_pointer(OP_VERT_GEOM))?;

                let solver_id = pseudo
                    .geom(fg)?
                    .solver
                    .ok_or_else(|| SolidError::invalid_input(OP_VERT_GEOM, NULL_DEREF))?;

                // pThisFS->OffsetRadiiCanChange(), SM_CAST_PTR(SmVariableRadiusFS, pThisFS)
                let Some(vrfs) = solvers.variable_radius_mut(solver_id)? else {
                    continue;
                };

                let curr_ivl = vrfs.fillet_law_interval();
                let mut new_ivl = curr_ivl;

                // See which end (the solver's edge uses belong to the filleted B-rep).
                let eu = vrfs
                    .curve_based
                    .solver
                    .edgeuses
                    .first()
                    .copied()
                    .flatten()
                    .ok_or_else(|| SolidError::null_pointer(OP_VERT_GEOM))?;
                let filleted_edge = eu1::edge(target, eu)?
                    .ok_or_else(|| SolidError::invalid_input(OP_VERT_GEOM, NULL_DEREF))?;
                let at_fillet_start = edge::start_vertex(target, filleted_edge)? == corner_vertex;
                let at_fillet_end = edge::end_vertex(target, filleted_edge)? == corner_vertex;

                // A closed fillet edge needs the whole FilletLaw [B679]
                if at_fillet_start && at_fillet_end {
                    break;
                }

                let at_law_start = if vrfs.fillet_law_orientation() {
                    !at_fillet_start
                } else {
                    at_fillet_start
                };

                // See whether this end of this FS should be adjusted: it should not be
                // adjusted if it's interior to the fillet radius law function.
                let edge_map = vrfs.fillet_law().edge_map;
                let law_param = if at_law_start {
                    edge_map.min()
                } else {
                    edge_map.max()
                };
                // The entire EdgeMap is always parameterizated 0 to 1.
                if law_param > EFF_ZERO && law_param < 1.0 - EFF_ZERO {
                    continue;
                }

                // Set sNewIvl to a single point, at the other end, prior to adding new value.
                if at_fillet_start {
                    new_ivl.set_min_max(new_ivl.max(), new_ivl.max())?;
                } else {
                    new_ivl.set_min_max(new_ivl.min(), new_ivl.min())?;
                }

                new_ivl.add_value(edge_t);

                new_ivl = new_ivl.intersect(&curr_ivl)?; // Use the shortest interval.  [B663]

                // Check whether anything changes.
                let curr_law_ivl = vrfs.fillet_law_interval();
                if new_ivl != curr_law_ivl {
                    vrfs.set_fillet_law_interval(new_ivl);
                }
            } // end for each FilletGeom connected to this FilletVertex
        } // end for each FilletVertex

        Ok(())
    }

    /// The start of the fork's `SmFilletNx1Corner::CalcCornerEdgeGeom` (not upstream; opt-in
    /// `fixes.fillet_nx1_end_rib_global_merge`, WR-303, source issue #841;
    /// source fork `967f7ab`). At a corner of at most three edges the one corner
    /// edge is the end rib, from one rail's end on its side edge to the other's. When
    /// one rail missed its side edge (`SM_FV_NO_INT_RAIL_EU`) because that edge ends
    /// within the fillet's width, there is no end rib: it is deleted and the caller sets
    /// the executive's global merge, which cuts the round off where it meets the body.
    /// Not when the side edge also lies on the fillet's other side surface: it continues
    /// the filleted edge between the fillet's own two surfaces, no end exists there, and
    /// the end rib fails as before. Returns whether the rib was deleted.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` for a missing edge use, face, surface or curve (upstream
    /// dereferences them); those of the surface coincidence test and the deletion.
    pub fn drop_end_rib_for_global_merge(
        &mut self,
        k: &mut Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
    ) -> Result<bool> {
        const OP: &str = "blend::fillet_corner::p1::drop_end_rib_for_global_merge";
        let [end_rib] = self.edges[..] else {
            return Ok(false);
        };
        if !k.fixes.fillet_nx1_end_rib_global_merge
            || !matches!(self.kind, FilletCornerKind::NByOne | FilletCornerKind::Open)
            || pseudo.fillet_edge(end_rib)?.edge_type != FilletEdgeType::FilletXSideFace
        {
            return Ok(false);
        }
        let start_fv =
            edge::vertex(pseudo.brep(), end_rib)?.ok_or_else(|| SolidError::null_pointer(OP))?;
        let end_fv = edge::other_vertex(pseudo.brep(), end_rib, Some(start_fv))?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let missed = |miss: VertexId, other: VertexId| -> Result<bool> {
            Ok(
                pseudo.fillet_vertex(miss)?.status() == FilStatus::NoIntRailEu
                    && pseudo.fillet_vertex(other)?.is_processed(),
            )
        };
        let miss_fv = if missed(start_fv, end_fv)? {
            start_fv
        } else if missed(end_fv, start_fv)? {
            end_fv
        } else {
            return Ok(false);
        };
        let fv = pseudo.fillet_vertex(miss_fv)?;
        let Some(EntityRef::Edgeuse(side_eu)) = fv.point_class_object() else {
            return Ok(false);
        };
        if fv.point_class() != PointClassType::Edgeuse {
            return Ok(false);
        }

        // The side edge the rail missed, and the fillet's side face it does not bound.
        let fil_eu = self
            .solver_eus
            .as_ref()
            .and_then(|eus| eus.first().copied())
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let radial = |eu| -> Result<EdgeuseId> {
            eu1::radial(target, eu)?.ok_or_else(|| SolidError::null_pointer(OP))
        };
        let face_of = |eu| -> Result<FaceId> {
            eu1::face(target, eu)?.ok_or_else(|| SolidError::null_pointer(OP))
        };
        let side_faces = [face_of(side_eu)?, face_of(radial(side_eu)?)?];
        let mut other_face = face_of(fil_eu)?;
        if side_faces.contains(&other_face) {
            other_face = face_of(radial(fil_eu)?)?;
        }
        let surface = crate::topology::face::p2::surface(target, other_face)?
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .clone_for_evaluation()?;
        let side_edge = eu1::edge(target, side_eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
        let curve = edge::curve(target, side_edge)?
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .clone_for_evaluation()?;
        let interval = edge::interval(target, side_edge)?;
        // the face's own surface: its trimmed-surface services come from the B-rep
        let mut services = BrepFaceServices::with_caches(target, FaceCaches::new());
        let runs_on = surface
            .simple_coincidence_checker_with_context(
                k,
                &surface.natural_uv_domain_with_nurb(k)?,
                &curve,
                &interval,
                self.approx_tol_3d,
                false,
                [
                    Some(CacheObject::Curve(side_edge)),
                    Some(CacheObject::Surface(other_face)),
                ],
                None,
                &mut services,
            )?
            .is_some();
        if runs_on {
            return Ok(false);
        }
        pseudo.delete_wire_fillet_edge(k, end_rib)?;
        self.edges.clear();
        Ok(true)
    }

    /// `CalcCornerEdgeGeom`: `SmFilletEdge::CalcCornerEdgeGeom` on every corner fillet edge
    /// (not the rails). A failed edge is deleted when the executive does a global merge and
    /// the edge is a fillet-side-face edge, else `SM_ERR`; an edge to be squeezed is squeezed
    /// ([`FilletCorner::squeeze_edge`]); every other edge takes its curve's natural interval
    /// and becomes processed (a cubic rail-rail interpolation keeps its status). The slots
    /// the deletions leave are compressed at the end.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: those of the edge calculations; those of the deletion of
    /// a failed edge (`SmBrep::DeleteEdge` on the pseudo-B-rep); `SM_ERR` for a
    /// failed edge the merge cannot absorb; `SM_ERR_NULL_POINTER` for a processed edge
    /// without curve; those of the squeeze.
    pub fn calc_corner_edge_geom(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        if self.drop_end_rib_for_global_merge(env.k, env.target, env.pseudo)? {
            env.exec.do_global_merge = true;
        }
        self.calc_corner_edge_geom_with(
            env,
            |env, corner, edge| {
                let mut local_error = std::mem::take(&mut env.exec.error);
                let res = env.pseudo.calc_corner_edge_geom_with_error_info(
                    env.k,
                    edge,
                    CornerLookup::of_corner(env.corner, corner, env.exec),
                    env.solvers,
                    env.target,
                    Some(&mut local_error),
                );
                env.exec.error = local_error;
                res
            },
            |env| env.exec.do_global_merge,
            |env| &mut *env.pseudo,
            |env, edge| env.pseudo.delete_wire_fillet_edge(env.k, edge),
        )
    }

    /// Calculate corner edges through live operations, preserving native slot,
    /// squeeze, deletion and status-update order.
    ///
    /// # Errors
    /// Actual calculator failures or the existing squeeze/deletion dependencies.
    pub fn calc_corner_edge_geom_with<C>(
        &mut self,
        context: &mut C,
        mut calculate: impl FnMut(&mut C, &Self, EdgeId) -> Result<()>,
        global_merge: impl Fn(&C) -> bool,
        mut pseudo: impl for<'p> FnMut(&'p mut C) -> &'p mut FilletBrep,
        mut delete_edge: impl FnMut(&mut C, EdgeId) -> Result<()>,
    ) -> Result<()> {
        // for every filletEdge (not a rail) ordered around this corner
        let mut slots: Vec<Option<EdgeId>> = self.edges.iter().copied().map(Some).collect();
        for ii in 0..slots.len() {
            // skip deleted edges (possibly from degenerate corners)
            let Some(curr_edge) = slots.get(ii).copied().flatten() else {
                continue;
            };

            // Branch on SmFilletEdge->m_eType to pass call to
            // the appropriate edgeBuilding function
            let calc = calculate(context, self, curr_edge);
            if let Err(e @ SolidError::Unported { .. }) = calc {
                return Err(e);
            }
            if calc.is_err() {
                // CalcCornerEdgeGeom() failed

                // Error out if m_bDoGlobalMerge is FALSE or the type of edge is not
                // qualify for global merge afterwards.
                if !global_merge(context)
                    || pseudo(context).fillet_edge(curr_edge)?.edge_type
                        != FilletEdgeType::FilletXSideFace
                {
                    return Err(SolidError::sm_err(OP_EDGE_GEOM));
                }
                // pCurrEdge->GetBrep()->DeleteEdge(pCurrEdge): the pseudo-B-rep's
                delete_edge(context, curr_edge)?;
                if let Some(slot) = slots.get_mut(ii) {
                    *slot = None;
                }
            } else {
                // pCurrEdge->CalcCornerEdgeGeom() succeeded
                if pseudo(context).fillet_edge(curr_edge)?.status == FilStatus::ToBeSqueezed {
                    // squeeze and delete it
                    self.edges = slots.iter().copied().flatten().collect();
                    self.squeeze_edge(pseudo(context), curr_edge)?;
                    for slot in &mut slots {
                        if slot.is_some_and(|e| !self.edges.contains(&e)) {
                            *slot = None;
                        }
                    }
                    continue;
                }

                // set currEdge->Interval = currEdge->Curve->NaturalInterval
                let ivl = pseudo(context)
                    .brep()
                    .edge(curr_edge)?
                    .curve
                    .as_ref()
                    .map(Curve::natural_interval)
                    .ok_or_else(|| SolidError::null_pointer(OP_EDGE_GEOM))?;
                edge::set_interval(pseudo(context).brep_mut(), curr_edge, &ivl)?;

                // increment status to SM_FIL_PROCESSED for all but
                // SM_FE_CUBIC_RAIL_RAIL_INTERPOLATE cases
                let fe = pseudo(context).fillet_edge_mut(curr_edge)?;
                if fe.status != FilStatus::CubicRailRailInterpolate {
                    fe.set_status(FilStatus::Processed);
                }
            } // end pCurrEdge->CalcCornerEdgeGeom() succeeded branch
        } // end iter every filletCorner->Edge (not rails)

        // remove all NULL entries from m_vEdges and m_vVertices
        self.edges = slots.into_iter().flatten().collect();

        // all done
        Ok(())
    }

    /// Whether every corner vertex is processed and within the corner tolerance.
    /// This is the read-only prefix of `CheckAndFixDegeneracy`, before topology edits.
    ///
    /// # Errors
    /// Actual pseudo-vertex record or point lookup failures.
    pub fn vertices_are_degenerate(
        &self,
        pseudo: &FilletBrep,
        corner_point: Point3,
    ) -> Result<bool> {
        // for every corner filletVertex
        for &fv in &self.vertices {
            // low work - corners with unprocessed vertices are not degenerate
            if !pseudo.fillet_vertex(fv)?.is_processed() {
                return Ok(false);
            }

            // get corner filletVertex point
            let pnt = pseudo.vertex_point(fv)?;

            // when filletVertex point is more than tolerance from origVertex point
            if pnt.distance(corner_point) > self.approx_tol_3d {
                // the corner is not degenerat
                return Ok(false);
            }
        } // end iter all corner filletVertices

        Ok(true)
    }

    /// `CheckAndFixDegeneracy`: a corner whose processed fillet vertices all lie within
    /// the corner's approximation tolerance of the filleted vertex is degenerate: its
    /// corner edges are deleted (and dropped from the executive's topology edges), its
    /// fillet vertices are merged into the first one (their vertex uses move there, with
    /// their intersection points marked singular), which is classified onto the filleted
    /// vertex and gets its UV positions on each rail's face from that vertex's vertex uses.
    /// Returns whether the corner was degenerate (`m_bDegenerate` is set then).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without filleted vertex, without a first fillet vertex,
    /// for a fillet vertex use without edge use or edge, a fillet geometry without solver
    /// (or a solver id past `solvers`) and an original vertex use without face use
    /// (upstream dereferences them); `SM_ERR_NULL_POINTER` for a rail without original
    /// face or a rail edge use without fillet geometry; those of the UV projections, the
    /// rail-index lookup and the deletions.
    pub fn check_and_fix_degeneracy(
        &mut self,
        env: &mut CornerEnv<'_>,
        solvers: &[FilletSolver],
    ) -> Result<bool> {
        let CornerEnv {
            k,
            target,
            pseudo,
            exec,
            ..
        } = env;
        self.check_and_fix_degeneracy_with(
            k,
            target,
            pseudo,
            |e| exec.remove_from_topo_edges(e),
            |id| {
                solvers.get(id.0 as usize).ok_or_else(|| {
                    SolidError::invalid_input(OP_DEGENERACY, "solver id past the list")
                })
            },
        )
    }

    /// [`Self::check_and_fix_degeneracy`] over the caller's executive state: `remove_topo_edge`
    /// is `m_pExecutive->RemoveFromTopoEdges` of a deleted corner edge, `solver` resolves a
    /// fillet geometry's solver (`GetFilletSolver()`).
    ///
    /// # Errors
    /// As [`Self::check_and_fix_degeneracy`], with `solver`'s resolution failures.
    pub fn check_and_fix_degeneracy_with<'s>(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        mut remove_topo_edge: impl FnMut(EdgeId),
        solver: impl Fn(FilletSolverId) -> Result<&'s FilletSolver>,
    ) -> Result<bool> {
        let null = || SolidError::invalid_input(OP_DEGENERACY, NULL_DEREF);
        // get original vertex point location
        let corner_vertex = self.vertex.ok_or_else(null)?;
        let corner_pnt = target.vertex(corner_vertex)?.point;

        if !self.vertices_are_degenerate(pseudo, corner_pnt)? {
            return Ok(false);
        }

        // arrive here when all corner filletVertices are within tolerance
        // of origVertex point

        // delete all corner edges (not rails)
        for e in std::mem::take(&mut self.edges) {
            pseudo.remove_edge(e);
            remove_topo_edge(e);
            pseudo.delete_fillet_edge(e)?;
        }

        // Combine all vertices into the 1st corner FilletVertex

        // get the 1st corner FilletVertex
        let fv0 = *self.vertices.first().ok_or_else(null)?;

        // for every corner FilletVertex except the first
        for fv in self.vertices.iter().skip(1).copied().collect::<Vec<_>>() {
            // move every cornerVertex->Vertexuse to 1st FilletVertex
            for vu in vertex::vertexuses(pseudo.brep(), fv)? {
                // move vertexuse to 1st FilletVertex
                move_vertexuse(pseudo, vu, fv, fv0)?;

                // set associated TsectPnt type to SM_IP_SINGULARITY
                pseudo.fillet_vertexuse_mut(vu)?.tsect_pnt_mut().point_type =
                    IntersectionPointType::Singularity;
            }

            // delete the FilletVertex
            pseudo.delete_fillet_vertex(fv)?;
        } // end iter every corner filletVertex but the 1st

        // set m_vVertices to a list of 1 filletVertex
        self.vertices = vec![fv0];

        // set the filletVertex to classify to the orig Vertex
        {
            let rec = pseudo.fillet_vertex_mut(fv0)?;
            rec.set_point_class(
                PointClassType::Vertex,
                Some(EntityRef::Vertex(corner_vertex)),
            );
            rec.set_fillet_vertex_type(FilletVertexType::OnVertex);
        }

        // get vertexuses for both origVertex and the remaining corner filletVertex
        let orig_vus = vertex::vertexuses(target, corner_vertex)?;
        let fil_vus = vertex::vertexuses(pseudo.brep(), fv0)?;

        // for every corner FilletVertex->Vertexuse - set its TsectPnt->UVPoint position
        for fil_vu in fil_vus {
            // filletVertexuse locals: TsectPnt, rail->face, railIndex
            let fil_eu = eu1::vertexuse_edgeuse(pseudo.brep(), fil_vu)?.ok_or_else(null)?;
            let rail = eu1::edge(pseudo.brep(), fil_eu)?.ok_or_else(null)?;
            let face = pseudo
                .fillet_edge(rail)?
                .orig_face
                .ok_or_else(|| SolidError::null_pointer(OP_DEGENERACY))?;
            let fg = pseudo
                .fillet_edgeuse(fil_eu)?
                .fillet_geom
                .ok_or_else(|| SolidError::null_pointer(OP_DEGENERACY))?;
            let solver_id = pseudo.geom(fg)?.solver.ok_or_else(null)?;
            let fil_solver = solver(solver_id)?;
            let rail_index = fil_solver.find_index_of_rail_on_face(target, Some(face))?;

            // for every original vertex->Vertexuse
            for &vu in &orig_vus {
                // when filletVertexuse->Rail->Face == origVertexuse->Face
                let fu = vertexuse::faceuse(target, vu)?.ok_or_else(null)?;
                if eu1::faceuse_face(target, fu)? == Some(face) {
                    // refine the filletVertex->TsectPnt->UVpoint position (the
                    // projection's status is not checked: a failure leaves sUV's
                    // SM_UNDEF_DOUBLE members)
                    let uv = ignored(
                        vertexuse::compute_uv_point(k, target, vu, true).map(Some),
                        None,
                    )?
                    .unwrap_or(Point2::UNDEF);
                    let srf = if rail_index == 0 {
                        SrfIndex::First
                    } else {
                        SrfIndex::Second
                    };
                    *pseudo
                        .fillet_vertexuse_mut(fil_vu)?
                        .tsect_pnt_mut()
                        .uv_pos_mut(srf) = uv;
                    break;
                }
            } // end iter every original vertex->Vertexuse
        } // end iter every corner FilletVertex->Vertexuse updating UVPoint values

        // inform the public - corner is degenerate
        self.degenerate = true;
        Ok(true)
    }

    /// `CreateEndPatch`: for the first corner fillet edge of type `SM_FE_FILLET_X_SIDE_FACE`,
    /// a copy (with analytics) of its original face's extended surface, or of the face's
    /// surface, is added to `m_vSurfaces` and the corner face is built (`MakeFaceBrep`; a
    /// failure there is noted as a vertex-problem fillet error, `NoteFilletError`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for such an edge without original face;
    /// [`SolidError::InvalidInput`] for a face without surface or a `MakeFaceBrep` null
    /// dereference; `SM_ERR` after noting a vertex-problem fillet error when `MakeFaceBrep`
    /// fails; those of the copy.
    pub fn create_end_patch(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        // for every filletCorner->FilletEdge
        for curr_edge in self.edges.clone() {
            // skip non SM_FE_FILLET_X_SIDE_FACE FilletEdges
            let fe = env.pseudo.fillet_edge(curr_edge)?;
            if fe.edge_type != FilletEdgeType::FilletXSideFace {
                continue;
            }

            // Get FilletEdge->OriginalFace->ExtendedSurface or Surface when needed
            let face = fe
                .orig_face
                .ok_or_else(|| SolidError::null_pointer(OP_END_PATCH))?;
            let surface = match self.extended_surface(env.exec, face) {
                Some(s) => s.clone(),
                None => env
                    .target
                    .face(face)?
                    .surface
                    .clone()
                    .ok_or_else(|| SolidError::invalid_input(OP_END_PATCH, NULL_DEREF))?,
            };

            // Add a copy of this surface to the FilletCorner->m_vSurfaces array
            // Copy pSurface, when possible as an analytic surface
            let new_surface = surface.copy_and_add_analytics(env.k)?;
            self.surfaces.push(new_surface);

            // Create Face (and Edges and Verts) in m_pFilletBrep from m_vSurfaces with call m_pFilletBrep->MakeFaceWithCurves()
            match self.make_face_brep(env) {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(
                    e @ SolidError::InvalidInput {
                        reason: NULL_DEREF, ..
                    },
                ) => return Err(e),
                Err(_) => {
                    // when MakeFaceBrep() failed - Note FilletError
                    env.exec.error.note_corner_error(
                        FilletErrorType::VertexProblem,
                        self.corner_type(),
                        FilStatus::Failure,
                        Some(self.kind.name()),
                        self.filleted_vertex()
                            .map(ExecutiveErrorObject::TargetVertex),
                        [None, None], // For now, we don't generally have just two surfaces.
                        [None, None], // For now, we don't generally have just two edgeuses.
                        Some("Fillet Corner Error: Corner End Face Creation Error"),
                    );
                    // FILEXEC_CORNER_ERR(this, "Corner End Face Creation Error"): a message, then SM_ERR
                    return Err(SolidError::sm_err(OP_END_PATCH));
                }
            }

            break;
        } // end iter every filletEdge searching for one attached to face to extend

        // all done
        Ok(())
    }

    /// [`FilletCorner::create_end_patch`] against any [`CornerPatchEnv`]:
    /// `extended_surface` resolves the executive's extended surface of an original
    /// face, and the environment's face-error note is the caller's
    /// `FILEXEC_CORNER_ERR(this, "Corner End Face Creation Error")`.
    ///
    /// # Errors
    /// As [`FilletCorner::create_end_patch`].
    pub fn create_end_patch_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
        extended_surface: impl Fn(FaceId) -> Option<Surface>,
    ) -> Result<()> {
        // for every filletCorner->FilletEdge
        for curr_edge in self.edges.clone() {
            // skip non SM_FE_FILLET_X_SIDE_FACE FilletEdges
            let fe = env.pseudo.fillet_edge(curr_edge)?;
            if fe.edge_type != FilletEdgeType::FilletXSideFace {
                continue;
            }

            // Get FilletEdge->OriginalFace->ExtendedSurface or Surface when needed
            let face = fe
                .orig_face
                .ok_or_else(|| SolidError::null_pointer(OP_END_PATCH))?;
            let surface = match extended_surface(face) {
                Some(s) => s,
                None => env
                    .target
                    .face(face)?
                    .surface
                    .clone()
                    .ok_or_else(|| SolidError::invalid_input(OP_END_PATCH, NULL_DEREF))?,
            };

            // Copy pSurface, when possible as an analytic surface
            let new_surface = surface.copy_and_add_analytics(env.k)?;
            self.surfaces.push(new_surface);

            // Create Face (and Edges and Verts) in m_pFilletBrep from m_vSurfaces
            match self.make_face_brep_with(env) {
                Ok(()) => {}
                Err(e) if e.is_unported() => return Err(e),
                Err(
                    e @ SolidError::InvalidInput {
                        reason: NULL_DEREF, ..
                    },
                ) => return Err(e),
                Err(_) => {
                    // when MakeFaceBrep() failed - Note FilletError
                    (env.note_face_error)(self);
                    return Err(SolidError::sm_err(OP_END_PATCH));
                }
            }

            break;
        } // end iter every filletEdge searching for one attached to face to extend

        Ok(())
    }

    /// `MakeFaceBrep`: builds the corner's face(s) in the fillet B-rep
    /// ([`CornerEnv::fillet`], in its infinite region). One surface: a face on it bounded
    /// by copies of the corner edges' curves (all `SM_OT_SAME`), its orientation from
    /// the surface normal at the third edge's vertex against the cross product of the
    /// second edge's end tangent and the third edge's start tangent. Several surfaces
    /// (an N-sided patch): one face per surface bounded by its four natural iso
    /// boundaries, oriented the same way at the corner of the first two. No surface: no
    /// work.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the orientation point does not project onto the surface; those of
    /// the curve copies, evaluations, projections, iso curves and
    /// `SmBrep::MakeFaceWithCurves`; [`SolidError::InvalidInput`] where upstream
    /// dereferences `NULL` (no fillet B-rep, a corner edge without curve or vertex) or
    /// reads past `m_vEdges` (fewer than three edges with one surface).
    pub fn make_face_brep(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        env.with_patch_env(|env| self.make_face_brep_with(env))
    }

    /// [`FilletCorner::make_face_brep`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::make_face_brep`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_face_brep_with(&mut self, env: &mut CornerPatchEnv<'_, '_>) -> Result<()> {
        const OP: &str = OP_MAKE_FACE;
        let null = || SolidError::invalid_input(OP, NULL_DEREF);

        // no work - no surface for which to build a face
        let num_surfaces = self.surfaces.len();
        if num_surfaces == 0 {
            return Ok(());
        }

        // when face is defined by just 1 surface
        if num_surfaces == 1 {
            let surface = self.surfaces.first().ok_or_else(null)?.clone();
            let total_edges = self.edges.len();

            // set number edges in newFace's loop
            let curve_loops = vec![total_edges];

            // set s3Dcurves and sCurveOrients with every FilletCorner->FilletEdge
            let mut curves_3d = Vec::new();
            let mut curve_orients = Vec::new();
            for &curr_edge in &self.edges {
                let curve = env
                    .pseudo
                    .brep()
                    .edge(curr_edge)?
                    .curve
                    .as_ref()
                    .ok_or_else(null)?
                    .copy(env.k)?;
                curves_3d.push(Some(curve));
                curve_orients.push(OrientType::Same);
            }

            // Do a simple test to determine the loop orientation
            let edge_at = |i: usize| {
                self.edges
                    .get(i)
                    .copied()
                    .ok_or_else(|| SolidError::invalid_input(OP, NULL_DEREF))
            };
            let (e1, e2) = (edge_at(1)?, edge_at(2)?);
            // let patchNormal = crossProduct of two edge endTangents at one vertex
            let tangent1 = vertex::edge_end_tangent(env.k, env.pseudo.brep(), e1, false)?;
            let tangent2 = vertex::edge_end_tangent(env.k, env.pseudo.brep(), e2, true)?;
            let patch_normal = tangent1.cross(tangent2).unitize_or_z();

            // get Surface UVPoint for vertex location
            let v = edge::vertex(env.pseudo.brep(), e2)?.ok_or_else(null)?;
            let pnt = env.pseudo.vertex_point(v)?;
            let loop_orient = loop_orientation(env.k, &surface, pnt, patch_normal, OP)?;

            // Make a face bounded by m_vEdges with m_vSurfaces[0] shape in pFilletBrep's
            // infinite region.
            let input = FaceCurvesInput {
                loops: curve_loops,
                curves_3d: Some(curves_3d),
                uv_curves: None,
                orientations: curve_orients,
                loop_points: Vec::new(),
            };
            let face = make_face(env, input, surface, loop_orient, OP)?;
            self.surface_faces.extend(face);

            // all done
            return Ok(());
        }

        // arrive here when building an N-sided patch

        // for every m_Surfaces->Surface - build 1 face with natural edge boundaries
        for surface in self.surfaces.clone() {
            let uv_domain = surface.natural_uv_domain_with_nurb(env.k)?;

            // Add surface Natural IsoParameter boundaries to s3DCurves array
            let mut curves_3d = Vec::new();
            let mut curve_orients = Vec::new();
            for (param, iso, orient) in [
                (SurfParamType::U, uv_domain.min.x, OrientType::Same),
                (SurfParamType::V, uv_domain.max.y, OrientType::Same),
                (SurfParamType::U, uv_domain.max.x, OrientType::Opposite),
                (SurfParamType::V, uv_domain.min.y, OrientType::Opposite),
            ] {
                let iso_curve = surface
                    .create_iso_parametric_curve(env.k, param, iso, 0.0, None, false)?
                    .curve;
                curves_3d.push(iso_curve);
                curve_orients.push(orient);
            }

            // Each rectangular surface has 4 sides
            let curve_loops = vec![4];

            // Do a simple test to determine the loop orientation
            // let patchNormal = crossProduct of two edge endTangents at one vertex
            let [crv1, crv2, ..] = curves_3d.as_slice() else {
                return Err(null());
            };
            let pv1 = crv1.evaluate(env.k, crv1.natural_interval().max(), 1, true, true)?;
            let tangent1 = pv1.get(1).copied().ok_or_else(null)?;
            let pv1 = crv2.evaluate(env.k, crv2.natural_interval().min(), 1, true, true)?;
            let tangent2 = pv1.get(1).copied().ok_or_else(null)?;
            let patch_normal = tangent1.cross(tangent2).unitize_or_z();

            // get Surface UVPoint for vertex location
            let pnt = pv1.first().copied().ok_or_else(null)?;
            let loop_orient = loop_orientation(env.k, &surface, pnt, patch_normal, OP)?;

            // make a face bounded by m_vEdges with m_vSurfaces[0] shape in pFilletBrep's
            // infinite region.
            let input = FaceCurvesInput {
                loops: curve_loops,
                curves_3d: Some(curves_3d.into_iter().map(Some).collect()),
                uv_curves: None,
                orientations: curve_orients,
                loop_points: Vec::new(),
            };
            let face = make_face(env, input, surface, loop_orient, OP)?;
            self.surface_faces.extend(face);
        } // end iter every surface buiding a face

        Ok(())
    }
}

impl FilletCorner {
    /// `SmFilletCorner::MakeConstantRadiusFillet`: a three-sided corner of cross
    /// sections whose shortest edge is a circular arc becomes a constant-radius fillet
    /// (the arc's radius) between the fillet surfaces of the two other edges: its rails
    /// run from the vertex opposite the arc (the start of the edge before it) to the
    /// arc's two ends, its centre line from the opposite point's projection onto the
    /// arc's axis to the arc's centre. The rails' curves (the second reversed) and the
    /// UV trim curves of their mates replace those of the two other edges and their
    /// primary edge uses; the new fillet surface is returned.
    ///
    /// # Errors
    ///
    /// `SM_ERR` (unable to convert) for a corner that is not three-sided or has an edge
    /// that is not a cross section, a shortest edge that is not an arc, surface
    /// generators of different types or blend scales, a linear generator, a point that
    /// does not project exactly once onto a fillet surface, or a failed
    /// `ReCalcFilletGeom`; `SM_ERR_NULL_POINTER` for a missing fillet geometry, fillet
    /// surface or rail curve; those of the projections, the centre line, the evaluation
    /// and the reversals; [`SolidError::InvalidInput`] where upstream dereferences
    /// `NULL` (a vertex, curve, primary edge use, mate, vertex use, solver, surface
    /// generator, the solver of an unknown generator type, the new fillet surface) or
    /// indexes `m_vEdges[9999]` (no edge length below `SM_BIG_DOUBLE`).
    pub fn make_constant_radius_fillet(&self, env: &mut CornerEnv<'_>) -> Result<Surface> {
        env.with_patch_env(|env| self.make_constant_radius_fillet_with(env))
    }

    /// [`FilletCorner::make_constant_radius_fillet`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::make_constant_radius_fillet`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn make_constant_radius_fillet_with(
        &self,
        env: &mut CornerPatchEnv<'_, '_>,
    ) -> Result<Surface> {
        const OP: &str = OP_CONST_RAD;
        let sm_err = || SolidError::sm_err(OP); // Unable to convert
        let null = || SolidError::null_pointer(OP);
        let deref = || SolidError::invalid_input(OP, NULL_DEREF);

        // check state
        let total_edges = self.edges.len();
        if total_edges != 3 {
            return Err(sm_err()); // Unable to convert
        }

        // Find the shortest Edge.
        let vertex_of = |b: &Brep, e: EdgeId| edge::vertex(b, e)?.ok_or_else(deref);
        let other_of =
            |b: &Brep, e: EdgeId, v: VertexId| edge::other_vertex(b, e, Some(v))?.ok_or_else(deref);
        let mut found_index = 9999usize;
        let mut min_dist = BIG_DOUBLE;
        for (ii, &e) in self.edges.iter().enumerate() {
            if env.pseudo.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::CrossSection {
                return Err(sm_err()); // Unable to convert
            }
            let v = vertex_of(env.pseudo.brep(), e)?;
            let other_v = other_of(env.pseudo.brep(), e, v)?;
            let leng = env
                .pseudo
                .vertex_point(v)?
                .distance(env.pseudo.vertex_point(other_v)?);
            if leng < min_dist {
                min_dist = leng;
                found_index = ii;
            }
        }

        let edge_at = |i: usize| {
            self.edges
                .get(i)
                .copied()
                .ok_or_else(|| SolidError::invalid_input(OP, NULL_DEREF))
        };
        let base_e = edge_at(found_index)?; // Shortest Edge
        let tol = self.approx_tol_3d;
        // SM_CAST_PTR(SmBSplineCurve, pBaseE->GetCurve()), then the virtual IsArc
        let base_arc = env
            .pseudo
            .brep()
            .edge(base_e)?
            .curve
            .clone()
            .filter(is_b_spline_kind)
            .ok_or_else(deref)?;
        // Determine the radius of fillet
        let Some((ref_frame, fillet_rad, _, _)) = base_arc.is_arc(env.k, 5, tol)? else {
            return Err(sm_err()); // Unable to convert
        };

        let edge0 = edge_at((found_index + total_edges - 1) % total_edges)?;
        let edge1 = edge_at((found_index + 1) % total_edges)?;
        let prim_of = |pseudo: &FilletBrep, e: EdgeId| {
            eu1::primary_edgeuse(pseudo.brep(), e)?.ok_or_else(deref)
        };
        let prim_eu0 = prim_of(env.pseudo, edge0)?;
        let fg0 = env
            .pseudo
            .fillet_edgeuse(prim_eu0)?
            .fillet_geom()
            .ok_or_else(null)?;
        let surface0 = env
            .pseudo
            .geom(fg0)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(null)?;
        let prim_eu1 = prim_of(env.pseudo, edge1)?;
        let fg1 = env
            .pseudo
            .fillet_edgeuse(prim_eu1)?
            .fillet_geom()
            .ok_or_else(null)?;
        let surface1 = env
            .pseudo
            .geom(fg1)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(null)?;

        // Create a temorary center curve
        let line_end = ref_frame.origin();
        let opposite_v = vertex_of(env.pseudo.brep(), edge0)?;
        let opposite_pnt = env.pseudo.vertex_point(opposite_v)?;
        let z_axis = ref_frame.z_axis();
        let t = line_closest_point(line_end, z_axis, opposite_pnt);
        let line_start = line_end + z_axis * t;
        let center_curve = Curve::from(Line::create_line_segment(3, line_start, line_end, None)?);

        // Drop both ends of base curve onto two fillets
        let drop_uv = |k: &Kernel, surface: &Surface, p: Point3| -> Result<Point2> {
            let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
            let solutions = surface.global_point_solve::<ObjectHandle>(
                k,
                None,
                &uv_domain,
                SolverOperation::Intersect,
                p,
                tol,
                None,
                SolutionRequested::All,
            )?;
            let [sol] = solutions.as_slice() else {
                return Err(sm_err());
            };
            Ok(Point2::new(sol.start.param(0)?, sol.start.param(1)?))
        };
        let v = vertex_of(env.pseudo.brep(), base_e)?;
        let uv = drop_uv(env.k, &surface0, env.pseudo.vertex_point(v)?)?;
        let other_v = other_of(env.pseudo.brep(), base_e, v)?;
        let other_uv = drop_uv(env.k, &surface1, env.pseudo.vertex_point(other_v)?)?;

        let fsg_of =
            |env: &CornerPatchEnv<'_, '_>, g: FilletGeomId| -> Result<FilletSurfaceGenerator> {
                let id = env.pseudo.geom(g)?.fillet_solver().ok_or_else(deref)?;
                usize::try_from(id.0)
                    .ok()
                    .and_then(|i| env.solvers.get(i))
                    .ok_or_else(|| {
                        SolidError::invalid_input(OP, "fillet solver id not in the solver table")
                    })?
                    .fillet_surface_generator()
                    .copied()
                    .ok_or_else(deref)
            };
        let fsg0 = fsg_of(env, fg0)?;
        let fsg1 = fsg_of(env, fg1)?;
        let fsg_type = fsg0.fillet_surface_generator_type();
        let mut blend_scale = 1.5; // For use in SmBlendCurveCrossSectionFSG cases only
        let mut max_continuity = 0u32;
        if fsg_type != fsg1.fillet_surface_generator_type() {
            return Err(sm_err());
        }

        if fsg_type == FilletSurfaceGeneratorType::BlendCurve {
            blend_scale = fsg0.blend_scale().unwrap_or(blend_scale);
            if (blend_scale - fsg1.blend_scale().unwrap_or(blend_scale)).abs() > EFF_ZERO {
                return Err(sm_err()); // Unable to convert
            }
            let continuity = fsg1.continuity().unwrap_or(0);
            if continuity > max_continuity {
                max_continuity = continuity;
            }
        }

        // Make a constant radius fillet solver
        let fsg = match fsg_type {
            FilletSurfaceGeneratorType::Circular => {
                FilletSurfaceGenerator::circular(true, 1.0e-3, false, false)
            }
            FilletSurfaceGeneratorType::BlendCurve => {
                FilletSurfaceGenerator::blend_curve(blend_scale, max_continuity)
            }
            FilletSurfaceGeneratorType::Linear => return Err(sm_err()),
            // no solver is made: pNewFS->SetFilletExecutive on NULL
            FilletSurfaceGeneratorType::Unknown => return Err(deref()),
        };
        // the temporary solver's identity: past the solver table
        let new_fs_id = FilletSolverId(
            u32::try_from(env.solvers.len())
                .map_err(|_| SolidError::invalid_input(OP, "solver table size"))?,
        );
        let mut new_fs = PlainConstantRadiusFs(ConstantRadiusFs::from_surfaces(
            env.k,
            env.pseudo,
            new_fs_id,
            tol * 0.1,
            10.0 * PI / 180.0,
            2.0 * PI / 180.0,
            [fillet_rad, fillet_rad],
            [
                (surface0.clone(), SurfaceId::Free(fg0.0)),
                (surface1.clone(), SurfaceId::Free(fg1.0)),
            ],
            [true, true],
        )?);
        new_fs.0.solver.set_fillet_surface_generator(Some(fsg));
        new_fs
            .0
            .solver
            .set_fillet_executive(Some(FilletExecutiveRef));
        crate::blend::fillet_solver::curve_blend_sih::inherit_executive_handler(
            &mut new_fs.0.solver,
            env.solvers,
        );
        let new_fg = new_fs.0.solver.first_fillet_geom()?;

        // Create two rails, one is along the pInterpolateEdge, the other one is along
        // the pOppositeFillet but reversed
        let rail0 = env
            .pseudo
            .make_fillet_edge(Some(opposite_v), Some(v), None)?;
        env.pseudo.geom_mut(new_fg)?.set_rail(0, Some(rail0));
        let rail1 = env
            .pseudo
            .make_fillet_edge(Some(opposite_v), Some(other_v), None)?;
        env.pseudo.geom_mut(new_fg)?.set_rail(1, Some(rail1));

        // Prepare for computing the fillet
        let mut eu0 = prim_of(env.pseudo, rail0)?;
        let mut eu1 = prim_of(env.pseudo, rail1)?;

        // Drop sOppositePnt onto both surfaces
        let opposite_uv = drop_uv(env.k, &surface0, opposite_pnt)?;
        let opposite_other_uv = drop_uv(env.k, &surface1, opposite_pnt)?;

        for (end, (uv0, uv1)) in [(opposite_uv, opposite_other_uv), (uv, other_uv)]
            .into_iter()
            .enumerate()
        {
            if end == 1 {
                let mate = |b: &Brep, eu: EdgeuseId| eu1::mate(b, eu)?.ok_or_else(deref);
                eu0 = mate(env.pseudo.brep(), eu0)?;
                eu1 = mate(env.pseudo.brep(), eu1)?;
            }
            for eu in [eu0, eu1] {
                let vu = env.pseudo.brep().edgeuse(eu)?.vertexuse.ok_or_else(deref)?;
                let tp = env.pseudo.fillet_vertexuse_mut(vu)?.tsect_pnt_mut();
                *tp.uv_pos_mut(SrfIndex::First) = uv0;
                *tp.uv_pos_mut(SrfIndex::Second) = uv1;
            }
        }

        env.pseudo
            .geom_mut(new_fg)?
            .set_fillet_geom_type(FilletGeomType::CornerFillet);
        // Compute marching direction for solver
        let mid_pnt = base_arc.evaluate_point(env.k, base_arc.natural_interval().evaluate(0.5))?;
        let march_dir = mid_pnt - opposite_pnt;
        match env.pseudo.re_calc_fillet_geom(
            env.k,
            new_fg,
            &mut new_fs,
            env.target,
            false,
            Some(march_dir),
            Some(&center_curve),
        ) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(sm_err()), // Unable to convert
        }

        // EdgeUse current UVTrimCurves
        let rail_curve = |pseudo: &FilletBrep, e: EdgeId| -> Result<Curve> {
            pseudo.brep().edge(e)?.curve.clone().ok_or_else(null)
        };
        let rail0_curve = rail_curve(env.pseudo, rail0)?;
        let mut rail1_curve = rail_curve(env.pseudo, rail1)?;
        let eu0_uv_trim_curve = env.pseudo.brep().edgeuse(eu0)?.uv_trim_curve.clone();
        let eu1_uv_trim_curve = env.pseudo.brep().edgeuse(eu1)?.uv_trim_curve.clone();

        // Clear EU UVTrimCurve ptrs to prevent deleting them at the wrong time
        for eu in [prim_eu0, prim_eu1, eu0, eu1] {
            env.pseudo.set_edgeuse_uv_curve(eu, None, false, false)?;
        }

        // Reset all geometry of corresponding edges: retrieve 3d & uv curves from rails,
        // delete them and replace them by new ones

        // move pRail0->Curve to pEdge0 (delete current pEdge0->Curve)
        set_edge_curve(env.pseudo.brep_mut(), rail0, None)?;
        let crv_ivl = rail0_curve.natural_interval();
        set_edge_curve(env.pseudo.brep_mut(), edge0, Some(rail0_curve))?;
        env.pseudo.brep_mut().edge_mut(edge0)?.interval = crv_ivl;

        // move pRail1->Curve to pEdge1 (delete current pEdge1->Curve - reverse Rail1Curve
        // direction)
        set_edge_curve(env.pseudo.brep_mut(), rail1, None)?;
        let crv_ivl = rail1_curve.natural_interval();
        let crv_ivl = rail1_curve.reverse_parameterization(env.k, &crv_ivl)?;
        set_edge_curve(env.pseudo.brep_mut(), edge1, Some(rail1_curve))?;
        env.pseudo.brep_mut().edge_mut(edge1)?.interval = crv_ivl;

        // move pEU0->UVTrimCurve to pPrimEU0 (delete current pPrimEU0->UVTrimCurve)
        env.pseudo
            .set_edgeuse_uv_curve(prim_eu0, eu0_uv_trim_curve, true, true)?;

        // move pEU1->UVTrimCurve to pPrimEU1 (delete current pPrimEU1->UVTrimCurve -
        // reverse UVTrimCurve direction)
        let mut eu1_uv_trim_curve = eu1_uv_trim_curve;
        if let Some(c) = eu1_uv_trim_curve.as_mut() {
            let uv_crv_ivl = c.natural_interval();
            c.reverse_parameterization(env.k, &uv_crv_ivl)?;
        }
        env.pseudo
            .set_edgeuse_uv_curve(prim_eu1, eu1_uv_trim_curve, true, true)?;

        env.pseudo
            .geom(new_fg)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(deref)
    }
}

impl FilletCorner {
    /// `SmFilletCorner::InsertIntersectionTopology`: for a blending corner, registers
    /// the (empty) intersections of each face at the filleted vertex with each corner
    /// surface; then merges every fillet-B-rep edge of each corner edge on an original
    /// face (an edge of the executive's edge attribute with exactly one face) into
    /// that face of the target B-rep through the executive's topology intersector, and
    /// registers the (empty) intersection of the two faces' surfaces.
    ///
    /// `context` is the fillet executive (`GetFilletExecutive()`): its topology
    /// intersector (`RegisterExistingSSI`, `MergeCurveOnSurfaces`), its edge attribute
    /// (`m_pAttributeE`), the 780 marker attribute and `UpdateTopologyChanges`, with
    /// the curve and surface identities it retains.
    ///
    /// # Errors
    ///
    /// The first error of the context's operations, preserving earlier mutations;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (a blending
    /// corner without vertex).
    pub fn insert_intersection_topology_with_context(
        &self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        context: &mut impl CornerInsertionContext,
    ) -> Result<()> {
        context.begin_corner()?;
        if self.blending {
            let vertex = self
                .vertex
                .ok_or_else(|| SolidError::invalid_input(OP_INSERT, NULL_DEREF))?;
            for face in vertex::faces(target, vertex)? {
                for (jj, surface) in self.surfaces.iter().enumerate() {
                    let surface_face = self.surface_faces.get(jj).copied();
                    context.register_blending_intersection(target, face, surface, surface_face)?;
                }
            }
        }
        for &rail in &self.edges {
            let record = pseudo.fillet_edge(rail)?;
            let Some(face) = record.original_face() else {
                continue;
            };
            let mapped = record.fillet_brep_edges();
            for &edge in &mapped {
                let mut curve = context.capture_managed_curve(edge)?;
                if !context.attribute_edges()?.contains(&edge) {
                    continue;
                }
                let faces = context.managed_edge_faces(edge)?;
                let [managed_face] = faces.as_slice() else {
                    continue;
                };
                if mapped.len() == 1 {
                    curve = context.capture_pseudo_curve(pseudo, rail)?;
                }
                context.with_corner_face_scope(target, face, |context, target, marker| {
                    context.merge_corner_curve(pseudo, target, face, &curve, edge)?;
                    context.update_corner_topology(pseudo, target, marker, face)?;
                    let surface = context.capture_managed_surface(*managed_face)?;
                    context.register_bound_intersection(target, face, &surface)
                })?;
            }
        }
        Ok(())
    }
}

/// Retained dependencies of the native corner topology-insertion loop.
pub trait CornerInsertionContext {
    /// Actual curve authority captured before topology edits.
    type CurveBinding;
    /// Actual surface authority captured at its native getter position.
    type SurfaceBinding;
    /// Marker owned for one native face scope.
    type FaceMarker;
    /// Resolve the retained executive intersector.
    /// # Errors
    /// Missing actual insertion context.
    fn begin_corner(&mut self) -> Result<()>;
    /// Capture the managed edge's current curve before attribute filtering.
    /// # Errors
    /// Missing actual edge or curve authority.
    fn capture_managed_curve(&mut self, edge: EdgeId) -> Result<Option<Self::CurveBinding>>;
    /// Capture the source pseudo curve for a singleton mapped edge list.
    /// # Errors
    /// Missing actual curve authority.
    fn capture_pseudo_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        edge: EdgeId,
    ) -> Result<Option<Self::CurveBinding>>;
    /// Read the actual current edge-attribute users.
    /// # Errors
    /// Missing attribute lifecycle state.
    fn attribute_edges(&self) -> Result<Vec<EdgeId>>;
    /// Read the managed edge's faces in owning traversal order.
    /// # Errors
    /// Missing actual topology.
    fn managed_edge_faces(&self, edge: EdgeId) -> Result<Vec<FaceId>>;
    /// Hold marker 780 through merge, topology updates and SSI registration.
    /// # Errors
    /// Marker setup or the enclosed operation fails.
    fn with_corner_face_scope<T>(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        body: impl FnOnce(&mut Self, &mut Brep, &Self::FaceMarker) -> Result<T>,
    ) -> Result<T>;
    /// Resolve the edge tolerance and merge at the native call position.
    /// # Errors
    /// The actual merge fails; all partial writes remain visible.
    fn merge_corner_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        face: FaceId,
        curve: &Option<Self::CurveBinding>,
        managed_edge: EdgeId,
    ) -> Result<()>;
    /// Update records while the actual marker is still attached.
    /// # Errors
    /// The actual topology bookkeeping fails.
    fn update_corner_topology(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        marker: &Self::FaceMarker,
        face: FaceId,
    ) -> Result<()>;
    /// Read the managed face's surface after merge and topology updates.
    /// # Errors
    /// Missing actual surface authority.
    fn capture_managed_surface(&mut self, face: FaceId) -> Result<Option<Self::SurfaceBinding>>;
    /// Register the native empty surface intersection.
    /// # Errors
    /// Missing surface authority or the original registration error.
    fn register_bound_intersection(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        surface: &Option<Self::SurfaceBinding>,
    ) -> Result<()>;
    /// Register a blending corner's actual retained patch surface.
    /// # Errors
    /// Missing patch context or the original registration error.
    fn register_blending_intersection(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        surface: &Surface,
        surface_face: Option<FaceId>,
    ) -> Result<()>;
}

/// `SmConstantRadiusFS` itself (not the assisted solver [`ConstantRadiusFs`] stands for):
/// its overrides are the assisted solver's except `GetType`, `IsKindOf`,
/// `GetSolverType` and `PointOnPlaneSolve`, which stay `SmConstantRadiusFS`'s / the
/// base's.
struct PlainConstantRadiusFs(ConstantRadiusFs);

impl FilletSolverOps for PlainConstantRadiusFs {
    fn solver(&self) -> &FilletSolver {
        &self.0.solver
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.0.solver
    }

    /// `SmConstantRadiusFS::GetJacobianSize`.
    fn jacobian_size(&self) -> Result<usize> {
        <ConstantRadiusFs as FilletSolverOps>::jacobian_size(&self.0)
    }

    /// `SmConstantRadiusFS::SetupOffsetValues`.
    fn setup_offset_values(
        &mut self,
        tsect: &TsectPnt,
        offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        <ConstantRadiusFs as FilletSolverOps>::setup_offset_values(&mut self.0, tsect, offset_dist)
    }

    /// `SmConstantRadiusFS::OffsetRadiiCanChange`.
    fn offset_radii_can_change(&self) -> Result<bool> {
        <ConstantRadiusFs as FilletSolverOps>::offset_radii_can_change(&self.0)
    }

    /// `SmConstantRadiusFS::LoadJacobian`.
    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        <ConstantRadiusFs as FilletSolverOps>::load_jacobian(
            &mut self.0,
            k,
            num_equations,
            num_parameters,
            load,
            f,
            jacobian,
        )
    }

    /// `SmConstantRadiusFS::LoadInitialValues`.
    fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        surface1: &mut OffsetSurface,
        surface1_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        <ConstantRadiusFs as FilletSolverOps>::load_initial_values(
            &mut self.0,
            k,
            brep,
            do_surf2_calcs,
            rail_index,
            surface1,
            surface1_index,
            intervals,
            periodicities,
            guess_t,
        )
    }

    /// `SmConstantRadiusFS::GetType`.
    fn type_code(&self) -> Result<i64> {
        Ok(CONSTANT_RADIUS_FS_TYPE)
    }

    /// `SmConstantRadiusFS::IsKindOf`.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(constant_radius_is_kind_of(t))
    }

    /// `SmConstantRadiusFS::GetSolverType`.
    fn solver_type(&self) -> Result<FilletSolverType> {
        Ok(FilletSolverType::ConstRadius)
    }
}

/// `MakeFaceBrep`'s loop orientation: `SM_OT_OPPOSITE` when the surface normal at the
/// first projection of `pnt` (`GlobalPointSolve`, `SM_SO_MINIMIZE`, `SM_EFF_ZERO`, all
/// solutions) points against `patch_normal`, else `SM_OT_SAME`.
fn loop_orientation(
    k: &Kernel,
    surface: &Surface,
    pnt: Point3,
    patch_normal: Vec3,
    op: &'static str,
) -> Result<OrientType> {
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let solutions = surface.global_point_solve::<ObjectHandle>(
        k,
        None,
        &uv_domain,
        SolverOperation::Minimize,
        pnt,
        EFF_ZERO,
        None,
        SolutionRequested::All,
    )?;
    let first = solutions
        .as_slice()
        .first()
        .ok_or_else(|| SolidError::sm_err(op))?; // != 1?
    let uv = Point2::new(first.start.param(0)?, first.start.param(1)?);

    // get Surface Normal at vertex location
    let surf_normal = surface
        .evaluate_normal(k, uv, Side::Left, Side::Left)?
        .unitize_or_z();

    // when SurfaceNormal and PatchNormal are aligned eLoopOrient = SM_OT_SAME
    Ok(if surf_normal.dot(patch_normal) < -EFF_ZERO {
        OrientType::Opposite
    } else {
        OrientType::Same
    })
}

/// `pFilletBrep->MakeFaceWithCurves(NULL, ...)`: a face on `surface` (its natural
/// domain) in the fillet B-rep's infinite region; the new face.
fn make_face(
    env: &mut CornerPatchEnv<'_, '_>,
    input: FaceCurvesInput,
    surface: Surface,
    orientation: OrientType,
    op: &'static str,
) -> Result<Option<FaceId>> {
    let fillet = env
        .fillet
        .as_mut()
        .ok_or_else(|| SolidError::invalid_input(op, NULL_DEREF))?;
    let uv_domain = surface.natural_uv_domain_with_nurb(env.k)?;
    let region = fillet.brep.infinite_region();
    let key_of = fillet.notify.key_of;
    let face_ref = |f: FaceId| key_of(EntityRef::Face(f));
    let made = make_face_with_curves(
        env.k,
        fillet.brep,
        region,
        input,
        surface,
        &uv_domain,
        orientation,
        None,
        &face_ref,
        &mut fillet.notify,
    )?;
    Ok(made.face)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_failure_retains_completed_prefix_and_reads_live_merge_flag() {
        use crate::blend::fillet_geom::p1::FilletCornerId;
        use crate::geom::extent1d::Extent1d;
        let mut pseudo = FilletBrep::new();
        let start = pseudo.brep_mut().add_vertex(Point3::ZERO).unwrap();
        let end = pseudo
            .brep_mut()
            .add_vertex(Point3::new(1.0, 0.0, 0.0))
            .unwrap();
        let mut edges = Vec::new();
        for _ in 0..3 {
            let edge = pseudo
                .new_fillet_edge(start, end, Some(FilletCornerId(0)))
                .unwrap();
            pseudo
                .fillet_edge_mut(edge)
                .unwrap()
                .set_fillet_edge_type(FilletEdgeType::FilletXSideFace);
            let record = pseudo.brep_mut().edge_mut(edge).unwrap();
            record.curve = Some(Curve::from(
                BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::new(1.0, 0.0, 0.0))
                    .unwrap(),
            ));
            record.interval = Extent1d::from_min_max(5.0, 6.0);
            edges.push(edge);
        }
        let mut corner = FilletCorner::new(FilletCornerKind::NByOne, None, None, None, 1e-3, 0.1);
        corner.edges.clone_from(&edges);
        let mut context = (pseudo, Vec::new(), true);
        let result = corner.calc_corner_edge_geom_with(
            &mut context,
            |(_, visited, global_merge), _, edge| {
                visited.push(edge);
                if edge == edges[1] {
                    *global_merge = false;
                    return Err(SolidError::sm_err("defined edge failure"));
                }
                Ok(())
            },
            |(_, _, global_merge)| *global_merge,
            |(pseudo, _, _)| pseudo,
            |_, _| Ok(()),
        );
        assert!(matches!(
            result,
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert_eq!(context.1, edges[..2]);
        assert_eq!(
            context.0.fillet_edge(edges[0]).unwrap().status(),
            FilStatus::Processed
        );
        assert_ne!(
            context.0.fillet_edge(edges[1]).unwrap().status(),
            FilStatus::Processed
        );
        assert_ne!(
            context.0.fillet_edge(edges[2]).unwrap().status(),
            FilStatus::Processed
        );
        assert_eq!(
            context.0.brep().edge(edges[0]).unwrap().interval,
            Extent1d::from_min_max(0.0, 1.0)
        );
        assert_eq!(
            context.0.brep().edge(edges[1]).unwrap().interval,
            Extent1d::from_min_max(5.0, 6.0)
        );
        assert_eq!(corner.edges, edges);
    }

    #[test]
    fn vertex_geometry_failure_retains_completed_prefix_and_stops_law_adjustment() {
        use crate::blend::fillet_geom::p1::FilletVertex;

        let k = Kernel::default();
        let mut pseudo = FilletBrep::new();
        let first = pseudo.brep_mut().add_vertex(Point3::ZERO).unwrap();
        let second = pseudo.brep_mut().add_vertex(Point3::ZERO).unwrap();
        let third = pseudo.brep_mut().add_vertex(Point3::ZERO).unwrap();
        for vertex in [first, second, third] {
            pseudo.insert_fillet_vertex(vertex, FilletVertex::new(&k, None));
        }
        let mut corner = FilletCorner::new(FilletCornerKind::NByOne, None, None, None, 1e-3, 0.1);
        corner.vertices = vec![first, second, third];
        corner.set_calc_as_const_rad(true);
        let mut context = (pseudo, Vec::new(), false);
        let result = corner.calc_corner_vert_geom_with(
            &mut context,
            |(pseudo, _, _), vertex| Ok(pseudo.fillet_vertex(vertex)?.is_processed()),
            |(pseudo, visited, _), vertex| {
                visited.push(vertex);
                if vertex == second {
                    return Err(SolidError::null_pointer("defined calculator failure"));
                }
                pseudo
                    .fillet_vertex_mut(vertex)?
                    .set_status(FilStatus::Processed);
                Ok(())
            },
            |(_, _, adjusted), _, _| {
                *adjusted = true;
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert_eq!(context.1, [first, second]);
        assert!(context.0.fillet_vertex(first).unwrap().is_processed());
        assert!(!context.0.fillet_vertex(second).unwrap().is_processed());
        assert!(!context.0.fillet_vertex(third).unwrap().is_processed());
        assert!(!context.2);
    }

    /// `SqueezeEdge` moves one rail vertex use at a time: a closed rail at the vertex to
    /// delete keeps its other end there when the loop stops with `SM_ERR` at a non-rail
    /// edge that is not the corner's (reviewer's scenario: rail use, missing edge, rail
    /// use).
    #[test]
    fn squeeze_moves_one_end_of_a_closed_rail_per_vertex_use() {
        use crate::blend::fillet_geom::p1::{FilletEdge, FilletVertex};
        use crate::topology::{Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};

        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let b = fb.brep_mut();
        let d = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let kv = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let rail = b.add_edge(d, d).unwrap();
        let squeezed = b.add_edge(d, kv).unwrap();
        let add_eu = |b: &mut Brep, e: EdgeId, orientation: OrientType, at: VertexId| {
            let eu = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    owner: EdgeuseOwner::Unknown,
                    orientation,
                    ..Edgeuse::default()
                })
                .unwrap();
            let vu = b
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(at),
                    owner: VertexuseOwner::Edgeuse(Some(eu)),
                    ..Vertexuse::default()
                })
                .unwrap();
            b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            b.vertex_mut(at).unwrap().vertexuses.push(vu);
            vu
        };
        // the deleted vertex's uses in order: the rail's first use, the squeezed (non-rail,
        // not the corner's) edge's use, the rail's second use
        let r0_vu = add_eu(b, rail, OrientType::Same, d);
        let _s0_vu = add_eu(b, squeezed, OrientType::Same, d);
        let r1_vu = add_eu(b, rail, OrientType::Opposite, d);
        let _s1_vu = add_eu(b, squeezed, OrientType::Opposite, kv);
        let mut fv = FilletVertex::new(&k, None);
        fv.set_status(FilStatus::ToBeDeleted);
        fb.insert_fillet_vertex(d, fv);
        fb.insert_fillet_vertex(kv, FilletVertex::new(&k, None));
        let mut fe = FilletEdge::with_corner(None);
        fe.set_fillet_edge_type(FilletEdgeType::Rail);
        fb.insert_fillet_edge(rail, fe);
        fb.insert_fillet_edge(squeezed, FilletEdge::with_corner(None));
        fb.add_edge(rail);
        fb.add_edge(squeezed);

        let mut c = FilletCorner::new(FilletCornerKind::NByTwo, None, None, None, 1e-3, 0.1);
        c.vertices = vec![d, kv];
        // the corner's edge list lacks the squeezed edge: SM_ERR at its vertex use
        let r = c.squeeze_edge(&mut fb, squeezed);
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
        let b = fb.brep();
        assert_eq!(b.edge(rail).unwrap().vertices, [kv, d]);
        assert_eq!(b.vertexuse(r0_vu).unwrap().vertex, Some(kv));
        assert_eq!(b.vertexuse(r1_vu).unwrap().vertex, Some(d));
        assert_eq!(b.vertex(d).unwrap().vertexuses.len(), 2);
        assert_eq!(b.vertex(kv).unwrap().vertexuses.len(), 2);
    }
    /// `CheckAndFixDegeneracy` does not check `ComputeUVPoint`'s status: a degenerate
    /// corner whose rail's original face has no surface (the projection fails with
    /// `SM_ERR_NULL_POINTER`) still merges, keeps the UV position at `SM_UNDEF_DOUBLE`,
    /// is marked degenerate and returns `TRUE`.
    #[test]
    fn degeneracy_fix_ignores_a_failed_uv_projection() {
        use crate::blend::fillet_geom::p1::{
            FilletEdge, FilletEdgeuse, FilletGeom, FilletGeomType, FilletVertex, FilletVertexuse,
        };
        use crate::topology::{
            Edgeuse, EdgeuseOwner, Face, Faceuse, Loopuse, Vertexuse, VertexuseOwner,
        };

        let mut k = Kernel::default();
        let corner_pnt = Point3::new(1.0, 2.0, 3.0);

        // the filleted B-rep: the corner vertex, used by a loop edge use of a face without
        // surface
        let mut target = Brep::default();
        let shell = target.add_shell().unwrap();
        let face = target.add_face_record(Face::new(shell)).unwrap();
        let fu = target
            .add_faceuse_record(Faceuse {
                shell: Some(shell),
                face: Some(face),
                ..Faceuse::default()
            })
            .unwrap();
        target.face_mut(face).unwrap().faceuse = Some(fu);
        let lu = target
            .add_loopuse_record(Loopuse {
                faceuse: Some(fu),
                ..Loopuse::default()
            })
            .unwrap();
        let cv = target.add_vertex(corner_pnt).unwrap();
        let teu = target
            .add_edgeuse_record(Edgeuse {
                owner: EdgeuseOwner::Loopuse(Some(lu)),
                ..Edgeuse::default()
            })
            .unwrap();
        let tvu = target
            .add_vertexuse_record(Vertexuse {
                vertex: Some(cv),
                owner: VertexuseOwner::Edgeuse(Some(teu)),
                ..Vertexuse::default()
            })
            .unwrap();
        target.edgeuse_mut(teu).unwrap().vertexuse = Some(tvu);
        target.vertex_mut(cv).unwrap().vertexuses.push(tvu);

        // the pseudo B-rep: one processed corner vertex at the corner point, the end of a
        // rail on that face whose edge use's geometry has the solver
        let mut fb = FilletBrep::new();
        let g = {
            let mut g = FilletGeom::new(&k, None, FilletGeomType::Default).unwrap();
            g.solver = Some(FilletSolverId(0));
            fb.add_geom(g)
        };
        let b = fb.brep_mut();
        let fv0 = b.add_vertex(corner_pnt).unwrap();
        let far = b.add_vertex(Point3::new(5.0, 0.0, 0.0)).unwrap();
        let rail = b.add_edge(fv0, far).unwrap();
        let eu = b
            .add_edgeuse_record(Edgeuse {
                edge: Some(rail),
                ..Edgeuse::default()
            })
            .unwrap();
        let vu = b
            .add_vertexuse_record(Vertexuse {
                vertex: Some(fv0),
                owner: VertexuseOwner::Edgeuse(Some(eu)),
                ..Vertexuse::default()
            })
            .unwrap();
        b.edgeuse_mut(eu).unwrap().vertexuse = Some(vu);
        b.edge_mut(rail).unwrap().edgeuses.push(eu);
        b.vertex_mut(fv0).unwrap().vertexuses.push(vu);
        let mut fv = FilletVertex::new(&k, None);
        fv.set_status(FilStatus::Processed);
        fb.insert_fillet_vertex(fv0, fv);
        fb.insert_fillet_vertex(far, FilletVertex::new(&k, None));
        let mut fe = FilletEdge::with_corner(None);
        fe.set_fillet_edge_type(FilletEdgeType::Rail);
        fe.orig_face = Some(face);
        fb.insert_fillet_edge(rail, fe);
        fb.insert_fillet_edgeuse(
            eu,
            FilletEdgeuse {
                fillet_geom: Some(g),
            },
        );
        fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());

        let solvers = [FilletSolver::new(1e-3, 1e-3, 0.01)];
        let mut exec = FilletExecutiveData::default();
        let mut c = FilletCorner::new(FilletCornerKind::NByTwo, Some(cv), None, None, 1e-3, 0.1);
        c.vertices = vec![fv0];
        let r = {
            let mut env = CornerEnv {
                k: &mut k,
                target: &mut target,
                pseudo: &mut fb,
                exec: &mut exec,
                corner: None,
                solvers: &mut [],
                fillet: None,
            };
            c.check_and_fix_degeneracy(&mut env, &solvers)
        };
        assert!(r.unwrap());
        assert!(c.degenerate);
        assert_eq!(c.vertices, vec![fv0]);
        let tp = fb.fillet_vertexuse(vu).unwrap().tsect_pnt();
        assert_eq!(tp.uv_pos(SrfIndex::First), Point2::UNDEF);
        assert_eq!(
            fb.fillet_vertex(fv0).unwrap().fillet_vertex_type(),
            FilletVertexType::OnVertex
        );
    }
    use crate::error::SM_ERR;

    #[test]
    fn planar_surface_point_counts() {
        let k = Kernel::default();
        let p = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        assert!(create_planar_surface(&k, &p[..3], 0.0).is_ok());
        assert!(create_planar_surface(&k, &p, 0.0).is_ok());
        let e = create_planar_surface(&k, &p[..2], 0.0).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { code: SM_ERR, .. }));
        let line = [p[0], p[1], Point3::new(2.0, 0.0, 0.0)];
        assert!(create_planar_surface(&k, &line, 0.0).is_err());
    }

    #[test]
    fn fillet_radius_quarter_turn() {
        // normals at right angles, points sqrt(2) apart: radius 1, span PI/2
        let (r, a) = fillet_radius(
            Point3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            -1.0,
        )
        .unwrap();
        assert!((a - PI / 2.0).abs() < 1e-12);
        assert!((r - 1.0).abs() < 1e-12);
    }

    #[test]
    fn missing_inputs() {
        let b = Brep::default();
        let k = Kernel::default();
        assert_eq!(
            test_g1_continuity(&k, &b, None, None, None, 1.0).unwrap(),
            (false, None)
        );
        assert_eq!(face_by_two_edgeuses(&b, None, None, None).unwrap(), None);
        let k = Kernel::default();
        assert!(create_srf_of_revolution(&k, &[], 0, 0.0).is_err());
    }
}

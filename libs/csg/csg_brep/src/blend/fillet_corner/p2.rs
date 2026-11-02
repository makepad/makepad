//! Fillet corners, part 2: the concave edge uses of a corner, the topology adjustments
//! of the `Nx2` (concave), convex / concave `NxN` and closed `Nx1` (tangent) corners, the
//! analytic and variable-radius corner surfaces and the corner's error report.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp`, `source/SMLib/inc/SmFilletCorner.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`find_concave_edgeuses`]                         | `sm_FindConcaveEdgeuses`                         |
//! | [`FilletCorner::adjust_concave_corner_topology`]  | `SmFilletNx2Corner::AdjustConcaveCornerTopology` |
//! | [`FilletCorner::adjust_topology_convex_nxn`], [`FilletCorner::adjust_topology_convex_nxn_with`] | `SmFilletConvexNxNCorner::AdjustTopology` |
//! | [`FilletCorner::adjust_tangent_corner_nx1_closed`], [`FilletCorner::nx1_closed_tangent_split`] | `SmFilletNx1ClosedCorner::AdjustTangentCorner`  |
//! | [`FilletCorner::make_analytic_corner_surface`], [`FilletCorner::make_analytic_corner_surface_with`] | `SmFilletCorner::MakeAnalyticCornerSurface` |
//! | [`FilletCorner::make_variable_radius_fillet`], [`FilletCorner::make_variable_radius_fillet_with`] | `SmFilletCorner::MakeVariableRadiusFillet` |
//! | [`FilletCorner::record_fillet_error`]             | `SmFilletCorner::RecordFilletError`              |
//! | [`FilletCorner::update_split_face`]               | `SmFilletCorner::UpdateSplitFace`                |
//! | [`FilletCorner::tangent_fillet_solver_nx1_closed`] | `SmFilletNx1ClosedCorner::GetTangentFilletSolver` |
//! | [`FilletCorner::tangent_fillet_solver_nx2`]       | `SmFilletNx2Corner::GetTangentFilletSolver`      |
//!
//! The constructors of the derived classes this unit owns are CO-PORTED in
//! `blend.fillet_corner_p3` as `FilletCorner::new`.
//!
//! Deviations:
//! - **Out-param.** The concave edge uses (`rConcaveEUs`, appended to) are returned, as
//!   is the variable-radius fillet's surface (`rpSurface`); the `iDebugLevel` arguments
//!   (graphics only) are dropped.
//! - **Undefined input.** Upstream reads `(*pAllEUs)[GetSize() - 1]` unchecked, past the
//!   array when it is empty; an empty `all_eus` is [`SolidError::InvalidInput`]. Its
//!   callers pass a corner's `m_pAllEUs`, which `SmFilletCorner::Create` never leaves
//!   empty. `SmFilletConvexNxNCorner::AdjustTopology` writes past its two-element
//!   `sVec` when a corner vertex has more than two cross sections, and indexes
//!   `m_vEdges` / inserts at `9999` when no corner edge starts at the chosen vertex:
//!   both are [`SolidError::InvalidInput`].
//! - **Corner methods** are methods of `blend.fillet_corner_p3`'s [`FilletCorner`] taking
//!   its [`CornerEnv`] (that module's docs). Where upstream dereferences `NULL` or casts a
//!   classification object blindly, the port returns [`SolidError::InvalidInput`];
//!   `NER` is `SM_ERR_NULL_POINTER` and `SER` propagates. Evaluations whose status
//!   upstream ignores leave [`Point3::UNDEF`] / [`Vec3::UNDEF`] (the `SmVector3d`
//!   member initialisers); `SER(v.Unitize())` of a zero vector is the
//!   `SM_ERR_INVALID_INPUT` status, an ignored `Unitize` gives `(0, 0, 1)`. The new
//!   fillet vertices are created with their point (upstream `SetPoint`s them later; no
//!   vertex use is attached in between).
//! - **`MakeAnalyticCornerSurface`** returns `SmBoolean` but exits through `SER` / `NER`:
//!   the status it returns there is non-zero, which its caller reads as `TRUE` (an
//!   analytic surface was made, though none was added). The port returns `Ok(true)`
//!   for those exits (`plan/upstream-issues.md`). Its fits that fail softly upstream
//!   (`if (SM_SUCCESS != fit(..)) return FALSE;`) are `false`; only
//!   [`SolidError::Unported`] and [`SolidError::InvalidInput`] propagate from them.
//!   The fillet solvers are the base [`FilletSolver`]s of [`CornerEnv::solvers`]: a
//!   solver's `GetSolverType` is its [`FilletSolverOps::solver_type`].
//! - **`MakeVariableRadiusFillet`** builds its temporary `SmVariableRadiusFS`
//!   ([`VariableRadiusFs::from_surfaces`]) with the identity one past
//!   [`CornerEnv::solvers`]; the original surface is identified by its face, the
//!   opposite fillet's surface by its fillet geometry. Upstream deletes the solver (and
//!   with it the solver's fillet geometry, but not its fillet surface, which is
//!   returned) on return; here the solver is dropped and its geometry stays unreferenced
//!   in the pseudo B-rep's geometry table, as do the two rail edges whose curves were
//!   moved (upstream leaves those too). The `SmFilletLaw` is
//!   [`FilletLaw::bspline_elliptical`].
//! - **`RecordFilletError`** writes to the executive passed in (`GetFilletExecutive()`)
//!   with the caller's identities of the filleted vertex, surfaces and fillet vertices.
//!   Its callers in `blend.fillet_geom_p3` / `_p5` hold only a corner id, so they still
//!   return [`SolidError::Unported`] (`plan/findings.md`).
//! - **`UpdateSplitFace`** reads `m_vVertices` / `m_vEdges` directly (the inline
//!   `GetFilletVertices` / `GetFilletEdges` of unit `blend.fillet_corner_p1` append
//!   them). Its callees always return `SM_SUCCESS` upstream (ignored); their ports'
//!   errors (lookups, and the `NULL` dereferences of `SmFilletEdge::ShouldSwapFaces` and
//!   face classification) propagate. The faces are ids (upstream compares and
//!   dereferences the pointers); a fillet edge's corner is resolved in the caller's
//!   `corners` (upstream's `GetFilletCorner()` pointer).
//! - **`GetTangentFilletSolver`** overrides take the executive's edge-use-to-solver map
//!   ([`FilletExecutiveData`]) and the B-rep being filleted (for `sm_TestG1Continuity`)
//!   explicitly; `SM_ASSERT_ERR` (a debug assertion) is not reproduced, the `NULL` it
//!   returns is `None`.
//! - **Tangent corners.** `SmFilletNx1ClosedCorner::AdjustTangentCorner` shares the
//!   steps after its split with `SmFilletNx2Corner::AdjustTangentCorner`
//!   (`blend.fillet_corner_p3`): `CalcCornerVertGeom` with its status ignored, the
//!   seam moves of the rail ends' UV points and the two `ReCalcFilletGeom`s with the
//!   solver's geometry for the surfaces set to each half.
//! - **`MakeConstantRadiusFillet`** is unit `blend.fillet_corner_p1`'s
//!   [`FilletCorner::make_constant_radius_fillet`].
//!
//! Every loop runs over a given list once; there is no recursion.

use crate::blend::fillet_corner::p1::{
    check_face_complimentary_corner_concavity, test_g1_continuity,
};
use crate::blend::fillet_corner::p1::{
    create_planar_surface, create_srf_of_revolution, fillet_radius,
};
use crate::blend::fillet_corner::p3::{
    move_vertexuse, primary_eu, rail_tangent_unchecked, recalc_split_geoms, set_vertex_point,
    unchecked_point, CornerEnv, CornerPatchEnv, FilletCorner, FilletExecutiveData, TangentSplitEnv,
};
use crate::blend::fillet_custom_solver::{OffsetSurface, SurfaceId};
use crate::blend::fillet_executive::p3::{ErrorInfo, FilletExecutive};
use crate::blend::fillet_geom::p1::FilletErrorType;
use crate::blend::fillet_geom::p1::{
    check_seam_out, set_surface_closure, FilStatus, FilletBrep, FilletCornerId, FilletCornerType,
    FilletEdgeType, FilletGeomId, FilletGeomType, FilletSolverId, FilletVertex, FilletVertexType,
};
use crate::blend::fillet_geom::p2::{srf_srf_intersection, SrfSrfOptions};
use crate::blend::fillet_geom::p4::set_edge_curve;
use crate::blend::fillet_solver::p1::{
    FilletExecutiveRef, FilletSolver, FilletSolverOps, FilletSolverType, FilletSurfaceGenerator,
    FilletSurfaceGeneratorType,
};
use crate::blend::fillet_standard_solver::p1::{FilletLaw, VariableRadiusFs};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, EntityRef, Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::geom_utility::p1::{add_sorted, line_closest_point};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::SolutionObject;
use crate::geom::vector2d::Point2;
use crate::intersect::point_class::ClassObject;
use crate::intersect::point_class::PointClassType;
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::knots::Side;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};
use crate::topology::face::p2 as face_p2;
use crate::topology::ObjectHandle;
use crate::topology::{edge, vertex, Brep, EdgeId, EdgeuseId, FaceId, VertexId, VertexuseId};
use std::collections::BTreeMap;

const OP_CONCAVE: &str = "blend::fillet_corner::p2::find_concave_edgeuses";
const OP_CONVEX_TOPO: &str = "blend::fillet_corner::p2::adjust_topology_convex_nxn";
const OP_NX1_TANGENT: &str = "blend::fillet_corner::p2::adjust_tangent_corner_nx1_closed";
const OP_ANALYTIC: &str = "blend::fillet_corner::p2::make_analytic_corner_surface";
const OP_VAR_RAD: &str = "blend::fillet_corner::p2::make_variable_radius_fillet";

/// `SM_BIG_DOUBLE`.
const SM_BIG_DOUBLE: f64 = BIG_DOUBLE;
const OP_CONCAVE_TOPO: &str = "blend::fillet_corner::p2::adjust_concave_corner_topology";
const OP_TANGENT_SOLVER: &str = "blend::fillet_corner::p2::tangent_fillet_solver";

/// `sm_FindConcaveEdgeuses`: the first pair of consecutive edge uses of `all_eus`
/// (cyclically, starting with the last and the first) that bound a concave face corner
/// at `vertex` (`sm_CheckFaceComplimentaryCornerConcavity` without a reference edge use),
/// as `[previous, current]`; empty when no pair does.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an empty `all_eus` (upstream indexes past the
/// array); those of [`check_face_complimentary_corner_concavity`].
pub fn find_concave_edgeuses(
    k: &Kernel,
    b: &mut Brep,
    vertex: VertexId,
    all_eus: &[EdgeuseId],
) -> Result<Vec<EdgeuseId>> {
    let Some(&last) = all_eus.last() else {
        return Err(SolidError::invalid_input(
            OP_CONCAVE,
            "no edge uses (upstream indexes past the array)",
        ));
    };
    let mut concave = Vec::new();
    let mut prev = last;
    for &curr in all_eus {
        if check_face_complimentary_corner_concavity(k, b, vertex, prev, curr, None)? {
            concave.push(prev);
            concave.push(curr);
            return Ok(concave);
        }
        prev = curr;
    }
    Ok(concave)
}

impl FilletCorner {
    /// `SmFilletNx1ClosedCorner::GetTangentFilletSolver(pFS)`: the corner's one solver
    /// when it is `fs` and its edge meets itself with G1 continuity at the corner vertex
    /// (`sm_TestG1Continuity` within `m_dTangencyTolRadians`); `None` otherwise
    /// (`SM_ASSERT_ERR` when the corner does not have exactly one filleted edge use or
    /// `fs` is not its solver; not an error when it is not G1).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unset `m_pSolverEUs` (upstream dereferences
    /// it); those of [`test_g1_continuity`].
    pub fn tangent_fillet_solver_nx1_closed(
        &self,
        k: &Kernel,
        b: &Brep,
        exec: &FilletExecutiveData,
        fs: FilletSolverId,
    ) -> Result<Option<FilletSolverId>> {
        self.tangent_fillet_solver_nx1_closed_with_lookup(k, b, fs, |eu| {
            exec.fillet_solver_of_edgeuse(eu)
        })
    }

    /// Borrowed-lookup form of [`FilletCorner::tangent_fillet_solver_nx1_closed`] for
    /// live executive adapters whose solver identities are generic handles.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unset `m_pSolverEUs`; those of
    /// [`test_g1_continuity`].
    pub fn tangent_fillet_solver_nx1_closed_with_lookup<S: Copy + Eq>(
        &self,
        k: &Kernel,
        b: &Brep,
        fs: S,
        mut solver_of: impl FnMut(EdgeuseId) -> Option<S>,
    ) -> Result<Option<S>> {
        let solver_eus = self
            .solver_eus
            .as_deref()
            .ok_or_else(|| SolidError::invalid_input(OP_TANGENT_SOLVER, "m_pSolverEUs is NULL"))?;
        let [edge_use] = solver_eus else {
            return Ok(None); // SM_ASSERT_ERR
        };
        let fil_solver = solver_of(*edge_use);

        if fil_solver != Some(fs) {
            return Ok(None); // SM_ASSERT_ERR
        }

        // We have to test its continuity.
        let edge = eu1::edge(b, *edge_use)?;
        let tangency_tol_deg = rad_to_deg(self.tangency_tol_radians);
        if test_g1_continuity(k, b, self.vertex, edge, edge, tangency_tol_deg)?.0 {
            return Ok(fil_solver);
        }

        Ok(None) // not an error.
    }

    /// `SmFilletNx2Corner::GetTangentFilletSolver(pFS)`: when the corner's two filleted
    /// edges meet with G1 continuity at the corner vertex, the solver of the other one
    /// than `fs`; `None` otherwise (`SM_ASSERT_ERR` when the corner does not have
    /// exactly two filleted edge uses or `fs` is neither solver).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unset `m_pSolverEUs` (upstream dereferences
    /// it); those of [`test_g1_continuity`].
    pub fn tangent_fillet_solver_nx2(
        &self,
        k: &Kernel,
        b: &Brep,
        exec: &FilletExecutiveData,
        fs: FilletSolverId,
    ) -> Result<Option<FilletSolverId>> {
        self.tangent_fillet_solver_nx2_with_lookup(k, b, fs, |eu| exec.fillet_solver_of_edgeuse(eu))
    }

    /// Borrowed-lookup form of [`FilletCorner::tangent_fillet_solver_nx2`] for live
    /// executive adapters whose solver identities are generic handles.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unset `m_pSolverEUs`; those of
    /// [`test_g1_continuity`].
    pub fn tangent_fillet_solver_nx2_with_lookup<S: Copy + Eq>(
        &self,
        k: &Kernel,
        b: &Brep,
        fs: S,
        mut solver_of: impl FnMut(EdgeuseId) -> Option<S>,
    ) -> Result<Option<S>> {
        let solver_eus = self
            .solver_eus
            .as_deref()
            .ok_or_else(|| SolidError::invalid_input(OP_TANGENT_SOLVER, "m_pSolverEUs is NULL"))?;
        let [eu0, eu1_] = solver_eus else {
            return Ok(None); // SM_ASSERT_ERR
        };

        // We have to test its continuity.
        let edge0 = eu1::edge(b, *eu0)?;
        let edge1 = eu1::edge(b, *eu1_)?;

        let tangency_tol_deg = rad_to_deg(self.tangency_tol_radians);
        if !test_g1_continuity(k, b, self.vertex, edge0, edge1, tangency_tol_deg)?.0 {
            return Ok(None); // not an error.
        }

        let fs0 = solver_of(*eu0);
        let fs1 = solver_of(*eu1_);

        if fs0 == Some(fs) {
            return Ok(fs1);
        }
        if fs1 == Some(fs) {
            return Ok(fs0);
        }

        Ok(None) // SM_ASSERT_ERR
    }

    /// `SmFilletCorner::UpdateSplitFace`: the target face `orig_face` was split, making
    /// `split_face`; every fillet vertex of the corner (then every fillet edge) moves to
    /// the face it now lies in. `corners` resolves the fillet corner of a fillet edge
    /// (upstream's `SmFilletEdge::GetFilletCorner()` pointer), as
    /// [`FilletBrep::should_swap_faces`] does.
    ///
    /// # Errors
    ///
    /// Those of [`FilletBrep::update_split_face`] and
    /// [`FilletBrep::update_edge_split_face`].
    pub fn update_split_face(
        &self,
        env: &mut CornerEnv<'_>,
        orig_face: FaceId,
        split_face: FaceId,
        corners: &[FilletCorner],
    ) -> Result<()> {
        self.update_split_face_with_breps(
            env.k, env.pseudo, env.target, orig_face, split_face, corners,
        )
    }

    /// Shared split-face update with the actual topology borrows, without a solver table.
    /// # Errors
    /// Those of the existing vertex and edge split-face updates.
    pub fn update_split_face_with_breps(
        &self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        orig_face: FaceId,
        split_face: FaceId,
        corners: &[FilletCorner],
    ) -> Result<()> {
        for &v in &self.vertices {
            pseudo.update_split_face(k, v, target, orig_face, split_face)?;
        }
        for &e in &self.edges {
            pseudo.update_edge_split_face(
                k,
                e,
                Some(orig_face),
                Some(split_face),
                target,
                corners,
            )?;
        }
        Ok(())
    }

    /// `SmFilletNx2Corner::AdjustConcaveCornerTopology`: for a mixed-convexity corner
    /// whose two fillets do not meet on the side edge (their rails hit it more than
    /// `m_dThisApproxTol3d` apart), splits the `SM_FV_RAIL_X_EDGEUSE` vertex: two new
    /// such vertices where the rails hit the side edge, the old one becomes
    /// `SM_FV_FILLET_X_FILLET`, and three new fillet edges fill the gap on the extended
    /// side face.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when no `SM_FV_RAIL_X_EDGEUSE` vertex has two fillet
    /// geometries (or a cross-section edge use and its mate have none);
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` or casts a
    /// classification object that is not an edge use; [`SolidError::Unported`] (module
    /// docs).
    pub fn adjust_concave_corner_topology(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        self.adjust_concave_corner_topology_with(
            env.k,
            env.target,
            env.pseudo,
            env.corner,
            &mut env.exec.extended_surfaces_map,
        )
    }

    /// [`FilletCorner::adjust_concave_corner_topology`] against explicit objects: the
    /// executive's kernel, target and pseudo-B-rep, this corner's identity and the
    /// executive's extended-surface map (the legacy [`CornerEnv`] or a live session).
    ///
    /// # Errors
    ///
    /// As [`FilletCorner::adjust_concave_corner_topology`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn adjust_concave_corner_topology_with(
        &mut self,
        k: &Kernel,
        target: &Brep,
        pseudo: &mut FilletBrep,
        corner: Option<FilletCornerId>,
        extended_surfaces: &mut BTreeMap<FaceId, Surface>,
    ) -> Result<()> {
        // Find the fillet vertex to be adjusted, pAdjustVert.
        // It's the only SM_FV_RAIL_X_EDGEUSE.
        let mut found = None;
        for &v in &self.vertices {
            if pseudo.fillet_vertex(v)?.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
                continue;
            }
            if let [g1, g2] = pseudo.vertex_fillet_geoms(v)?[..] {
                found = Some((v, g1, g2));
                break;
            }
        }
        let (adjust_vert, fillet_geom1, fillet_geom2) =
            found.ok_or_else(|| SolidError::null_pointer(OP_CONCAVE_TOPO))?;

        let Some(EntityRef::Edgeuse(side_eu1)) =
            pseudo.fillet_vertex(adjust_vert)?.point_class_object()
        else {
            return Err(null_deref("classification object is not an edge use"));
        };
        let vu1 = pseudo
            .vu_at_rail_end(adjust_vert, Some(fillet_geom1), None, None)?
            .ok_or_else(|| null_deref("no rail vertex use"))?;
        let vu2 = pseudo
            .vu_at_rail_end(adjust_vert, Some(fillet_geom2), Some(vu1), None)?
            .ok_or_else(|| null_deref("no rail vertex use"))?;
        let edge_param1 = pseudo.fillet_vertexuse(vu1)?.tsect_pnt().curve_parameter;
        let edge_param2 = pseudo.fillet_vertexuse(vu2)?.tsect_pnt().curve_parameter;
        let side_edge =
            eu1::edge(target, side_eu1)?.ok_or_else(|| null_deref("side edge use without edge"))?;
        let side_edge_curve = target
            .edge(side_edge)?
            .curve
            .as_ref()
            .ok_or_else(|| null_deref("side edge without curve"))?;
        let pnt1 = unchecked_point(side_edge_curve.evaluate_point(k, edge_param1))?;
        let pnt2 = unchecked_point(side_edge_curve.evaluate_point(k, edge_param2))?;

        // See if we have coincident geometry, i.e. two rails intersect at this vertex.
        let gap_dist = pnt1.distance(pnt2);
        if gap_dist < self.approx_tol_3d {
            // No adjustments are needed
            return Ok(());
        }

        // See which of the two rail-side edge intersections is closer to the filleted
        // vertex, to decide Large/Small (read before the new vertices are made: nothing
        // here depends on them).
        let vert_pnt = target
            .vertex(
                self.vertex
                    .ok_or_else(|| null_deref("m_cpVertex is NULL"))?,
            )?
            .point;
        let near1 = pnt1.distance(vert_pnt) < pnt2.distance(vert_pnt);
        let (pnt_l, t_l, pnt_s, t_s) = if near1 {
            (pnt2, edge_param2, pnt1, edge_param1)
        } else {
            (pnt1, edge_param1, pnt2, edge_param2)
        };
        let (large_fg, small_fg, vu_on_large_fg, vu_on_small_fg) = if near1 {
            (fillet_geom2, fillet_geom1, vu2, vu1)
        } else {
            (fillet_geom1, fillet_geom2, vu1, vu2)
        };
        let side_face = if near1 {
            eu1::face(target, side_eu1)?
        } else {
            let radial = eu1::radial(target, side_eu1)?
                .ok_or_else(|| null_deref("side edge use without radial partner"))?;
            eu1::face(target, radial)?
        };

        // Create two new FilletVertices where each rail hits the side edge.
        let new_vert_l = new_rail_x_edgeuse_vertex_in(k, pseudo, corner, pnt_l, t_l)?; // "Large"
        let new_vert_s = new_rail_x_edgeuse_vertex_in(k, pseudo, corner, pnt_s, t_s)?; // "Small"
        self.vertices.push(new_vert_l);
        self.vertices.push(new_vert_s);

        let fv = pseudo.fillet_vertex_mut(adjust_vert)?;
        fv.set_fillet_vertex_type(FilletVertexType::FilletXFillet);
        fv.set_status(FilStatus::Unprocessed);

        move_vertexuse(pseudo, vu_on_large_fg, adjust_vert, new_vert_l)?;
        move_vertexuse(pseudo, vu_on_small_fg, adjust_vert, new_vert_s)?;

        let new_edge = pseudo.make_fillet_edge(Some(adjust_vert), Some(new_vert_l), corner)?;
        let fe = pseudo.fillet_edge_mut(new_edge)?;
        fe.set_fillet_edge_type(FilletEdgeType::FilletXSideFace);
        fe.set_original_face(side_face);
        let prim_eu = primary_eu(pseudo, new_edge, OP_CONCAVE_TOPO)?;
        pseudo.set_edgeuse_fillet_geom(prim_eu, Some(large_fg))?;

        // Need to extend the side surface
        let side_face_id = side_face.ok_or_else(|| null_deref("no side face"))?;
        let side_surface = target
            .face(side_face_id)?
            .surface
            .as_ref()
            .ok_or_else(|| null_deref("side face without surface"))?;
        let new_surface =
            side_surface.create_extended_surface(k, 10.0 * gap_dist, ContinuityType::G1)?;

        // Copy pNewSurface, when possible as an analytic surface.
        let new_analytic_surface = new_surface.copy_and_add_analytics(k)?;
        Self::add_extended_original_surface_with_map(
            extended_surfaces,
            side_face_id,
            new_analytic_surface,
        );

        let new_edge1 = pseudo.make_fillet_edge(Some(new_vert_l), Some(new_vert_s), corner)?;
        let fe1 = pseudo.fillet_edge_mut(new_edge1)?;
        fe1.set_fillet_edge_type(FilletEdgeType::OnEdge);
        fe1.set_original_edge(Some(side_edge));
        fe1.set_original_face(side_face);

        let new_edge2 = pseudo.make_fillet_edge(Some(new_vert_s), Some(adjust_vert), corner)?;
        let fe2 = pseudo.fillet_edge_mut(new_edge2)?;
        fe2.set_fillet_edge_type(FilletEdgeType::RailExtension);
        fe2.set_original_face(side_face);
        let prim_eu2 = primary_eu(pseudo, new_edge2, OP_CONCAVE_TOPO)?;
        pseudo.set_edgeuse_fillet_geom(prim_eu2, Some(small_fg))?;

        self.edges.push(new_edge);
        self.edges.push(new_edge1);
        self.edges.push(new_edge2);

        // pAdjustVert comes in with two mates, in the common face.
        // Find them -- they are at the other end of the two FilletEdges
        // of type SM_FE_CROSS_SECTION -- and reset them as mates of
        // the two FilletVertices we just created.
        let vus = pseudo.brep().vertex(adjust_vert)?.vertexuses.clone();
        for vu in vus {
            let eu = eu1::vertexuse_edgeuse(pseudo.brep(), vu)?
                .ok_or_else(|| null_deref("vertex use without edge use"))?;
            let fillet_e = pseudo
                .brep()
                .edgeuse(eu)?
                .edge
                .ok_or_else(|| null_deref("edge use without edge"))?;
            if pseudo.fillet_edge(fillet_e)?.fillet_edge_type() != FilletEdgeType::CrossSection {
                continue;
            }

            // FilletEdge of type SM_FE_CROSS_SECTION:
            let other_vert = edge::other_vertex(pseudo.brep(), fillet_e, Some(adjust_vert))?;
            let mut fg = pseudo.fillet_edgeuse(eu)?.fillet_geom();
            if fg.is_none() {
                let mate = eu1::mate(pseudo.brep(), eu)?
                    .ok_or_else(|| null_deref("edge use without mate"))?;
                fg = pseudo.fillet_edgeuse(mate)?.fillet_geom();
                if fg.is_none() {
                    return Err(SolidError::null_pointer(OP_CONCAVE_TOPO));
                }
            }
            // pOtherVert->GetMate(0) is read only when the geometry matches
            let has_mate = |pseudo: &FilletBrep| -> Result<bool> {
                let other =
                    other_vert.ok_or_else(|| null_deref("cross section without other vertex"))?;
                Ok(pseudo.fillet_vertex(other)?.mate(0).is_some())
            };
            let new_v = if fg == Some(small_fg) && has_mate(pseudo)? {
                Some(new_vert_s)
            } else if fg == Some(large_fg) && has_mate(pseudo)? {
                Some(new_vert_l)
            } else {
                None
            };
            if let (Some(new_v), Some(other)) = (new_v, other_vert) {
                pseudo.fillet_vertex_mut(new_v)?.set_mate(0, Some(other));
                pseudo.fillet_vertex_mut(other)?.set_mate(0, Some(new_v));
            }
        }
        let fv = pseudo.fillet_vertex_mut(adjust_vert)?;
        fv.set_mate(0, None);
        fv.set_mate(1, None);

        // Allow fillets to be extended at the corner
        self.blending = false;

        self.topology_adjusted = true;

        Ok(())
    }
}

impl FilletCorner {
    /// `SmFilletConvexNxNCorner::AdjustTopology` (also `SmFilletConcaveNxNCorner`'s): a
    /// three-sided corner patch that is too tight is stretched into a four-sided one.
    /// The `SM_FV_RAIL_X_RAIL` vertex whose two cross-section iso edges (estimated on
    /// its original face) are farthest apart, without crossing and of comparable length,
    /// is replaced by a rail-rail-interpolating edge between itself and a new
    /// `SM_FV_MATE` vertex, when that distance is at least a fifth of the shortest
    /// corner edge.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a corner vertex that is not `SM_FV_RAIL_X_RAIL`;
    /// `SM_ERR_NULL_POINTER` for a vertex without original face, a face without surface,
    /// a cross section or corner edge without fillet geometry, a filleted edge without
    /// curve or a new vertex without rail vertex use; those of the evaluations;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` or indexes past
    /// an array (module docs).
    pub fn adjust_topology_convex_nxn(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        let solvers = &*env.solvers;
        self.adjust_topology_convex_nxn_with(
            env.k,
            env.target,
            env.pseudo,
            env.corner,
            &|id: FilletSolverId| {
                usize::try_from(id.0)
                    .ok()
                    .and_then(|i| solvers.get(i))
                    .ok_or_else(|| {
                        invalid(OP_CONVEX_TOPO, "fillet solver id not in the solver table")
                    })
            },
        )
    }

    /// [`FilletCorner::adjust_topology_convex_nxn`] against explicit objects: the
    /// executive's kernel, target and pseudo-B-rep, this corner's identity and its
    /// solver store (the legacy [`CornerEnv`] or a live executive session).
    ///
    /// # Errors
    /// As [`FilletCorner::adjust_topology_convex_nxn`], plus the store's unresolved
    /// solver identities.
    #[allow(clippy::too_many_lines)]
    pub fn adjust_topology_convex_nxn_with<'s>(
        &mut self,
        k: &Kernel,
        target: &Brep,
        pseudo: &mut FilletBrep,
        corner: Option<FilletCornerId>,
        solver: &dyn Fn(FilletSolverId) -> Result<&'s FilletSolver>,
    ) -> Result<()> {
        const OP: &str = OP_CONVEX_TOPO;
        // Only non-chamfer & three-sided corners can be adjusted for now
        let total = self.vertices.len();
        if total > 3
        /* || m_bChamfer */
        {
            return Ok(());
        }

        let mut found_vert_index = None;
        let mut min_dist = SM_BIG_DOUBLE;
        let mut max_dist = 0.0;

        // Look for a vertex of the corner patch that can be 'stretched',
        // i.e. replacing the vertex by a rail-rail-interpolating edge
        for ii in 0..total {
            let curr_vert = vertex_at(&self.vertices, ii, OP)?;
            if pseudo.fillet_vertex(curr_vert)?.fillet_vertex_type() != FilletVertexType::RailXRail
            {
                return Err(SolidError::sm_err(OP)); // Unknown case
            }
            let next_vert = vertex_at(&self.vertices, (ii + 1) % total, OP)?;

            let vert_pnt = pseudo.vertex_point(curr_vert)?;
            let dist_to_next_vert = vert_pnt.distance(pseudo.vertex_point(next_vert)?);
            if min_dist > dist_to_next_vert {
                min_dist = dist_to_next_vert;
            }

            // Get the original face which pCurrVert is on
            let orig_face =
                point_class_face(pseudo, curr_vert).ok_or_else(|| SolidError::null_pointer(OP))?;

            let mut is_candidate = true;
            let vertexuses = pseudo.brep().vertex(curr_vert)?.vertexuses.clone();
            let mut pnt = [Point3::UNDEF; 4];
            let mut vec = [Vec3::UNDEF; 2];
            let mut count = 0usize;
            // Try to determine the estimated distance between two adjacent fillets
            // if iso-edges are used in the stretched corner patch.
            for vu in vertexuses {
                let eu = eu1::vertexuse_edgeuse(pseudo.brep(), vu)?
                    .ok_or_else(|| invalid(OP, "vertex use without edge use"))?;
                let e = pseudo
                    .brep()
                    .edgeuse(eu)?
                    .edge
                    .ok_or_else(|| invalid(OP, "edge use without edge"))?;
                if pseudo.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::CrossSection {
                    continue;
                }
                let fillet_geom = edgeuse_or_mate_geom(pseudo, eu, OP)?
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let fil_solver = geom_solver(pseudo, fillet_geom, OP)?;

                let other_v = edge::other_vertex(pseudo.brep(), e, Some(curr_vert))?
                    .ok_or_else(|| invalid(OP, "cross section without other vertex"))?;
                let rail_vu = pseudo
                    .vu_at_rail_end(other_v, Some(fillet_geom), None, None)?
                    .ok_or_else(|| invalid(OP, "no rail vertex use"))?;
                let rail_index =
                    solver(fil_solver)?.find_index_of_rail_on_face(target, Some(orig_face))?;
                let uv = pseudo
                    .fillet_vertexuse(rail_vu)?
                    .tsect_pnt()
                    .uv_pos(srf_index(rail_index));
                // (more than two cross sections: upstream writes past sVec)
                let (Some(p), true) = (pnt.get_mut(count), count < vec.len()) else {
                    return Err(invalid(
                        OP,
                        "more than two cross sections at a corner vertex",
                    ));
                };
                *p = face_surface(target, orig_face, OP)?.evaluate_point(k, uv)?;

                // The following is a simple way to determine if two iso-edges
                // are crossing each other.
                let v = *p - vert_pnt;
                if let Some(slot) = vec.get_mut(count) {
                    *slot = v;
                }
                let filleted_eu = solver(fil_solver)?
                    .edgeuse(0)?
                    .ok_or_else(|| invalid(OP, "fillet solver without edge use"))?;
                let filleted_edge = eu1::edge(target, filleted_eu)?
                    .ok_or_else(|| invalid(OP, "filleted edge use without edge"))?;
                let ivl = edge::interval(target, filleted_edge)?;
                let curve = target
                    .edge(filleted_edge)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let tangent = if edge::start_vertex(target, filleted_edge)? == self.vertex {
                    tangent_at(k, curve, ivl.min(), OP)?
                } else {
                    -tangent_at(k, curve, ivl.max(), OP)?
                };
                if tangent.dot(v) < -EFF_ZERO {
                    // Reject this vertex if crossing of the iso-edges happened
                    is_candidate = false;
                    break;
                }
                count += 1;
            }

            if !is_candidate {
                continue;
            }

            // SM_ASSERT(lCount == 2); (fewer leave sVec at its SM_UNDEF_DOUBLE members)
            let [vec0, vec1] = vec;
            let vec_len1 = vec0.length();
            let vec_len2 = vec1.length();
            if vec_len1 < 0.2 * vec_len2 || vec_len2 < 0.2 * vec_len1 {
                continue;
            }

            let [pnt0, pnt1, ..] = pnt;
            let dist = pnt0.distance(pnt1);
            if dist > max_dist {
                max_dist = dist;
                found_vert_index = Some(ii);
            }
        }

        // Determine if we have found a good candidate. Let's use
        // 1/5th of the shortest corner edge as the threshold
        let Some(found_vert_index) = found_vert_index else {
            // Do not adjust this corner
            return Ok(());
        };
        if max_dist < min_dist / 5.0 {
            // Do not adjust this corner
            return Ok(());
        }

        // Insert an interpolating edge to substitute the found vertex
        let adjust_vert = vertex_at(&self.vertices, found_vert_index, OP)?;
        // (a vertex without face: upstream calls GetSurface on NULL)
        let orig_face = point_class_face(pseudo, adjust_vert)
            .ok_or_else(|| invalid(OP, "corner vertex without original face"))?;
        let orig_surf = target
            .face(orig_face)?
            .surface
            .clone()
            .ok_or_else(|| SolidError::null_pointer(OP))?;

        // Determine where to inset the new edge
        let mut found_edge_index = None;
        let mut next_e = None;
        let mut prev_e = *total
            .checked_sub(1)
            .and_then(|i| self.edges.get(i))
            .ok_or_else(|| invalid(OP, "fewer corner edges than vertices"))?;
        for ii in 0..total {
            let e = *self
                .edges
                .get(ii)
                .ok_or_else(|| invalid(OP, "fewer corner edges than vertices"))?;
            next_e = Some(e);
            if edge::vertex(pseudo.brep(), e)? == Some(adjust_vert) {
                found_edge_index = Some(ii);
                break;
            }
            prev_e = e;
        }
        // SM_ASSERT(lFoundEdgeIndex != 9999); (upstream inserts at 9999 otherwise)
        let found_edge_index =
            found_edge_index.ok_or_else(|| invalid(OP, "no corner edge starts at the vertex"))?;
        let next_e = next_e.ok_or_else(|| invalid(OP, "no corner edge"))?;

        // Set pAdjustVert as the SM_FV_MATE type and compute its geometry
        let prev_eu = primary_eu(pseudo, prev_e, OP)?;
        let prev_fg = pseudo
            .fillet_edgeuse(prev_eu)?
            .fillet_geom()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let prev_v = vertex_at(&self.vertices, (found_vert_index + total - 1) % total, OP)?;
        let prev_vu = pseudo
            .vu_at_rail_end(prev_v, Some(prev_fg), None, None)?
            .ok_or_else(|| invalid(OP, "no rail vertex use"))?;
        let prev_fs = geom_solver(pseudo, prev_fg, OP)?;
        let rail_index1 = solver(prev_fs)?.find_index_of_rail_on_face(target, Some(orig_face))?;
        let tsect_pnt1 = pseudo.fillet_vertexuse(prev_vu)?.tsect_pnt().clone();
        let uv1 = tsect_pnt1.uv_pos(srf_index(rail_index1));
        let pnt1 = orig_surf.evaluate_point(k, uv1)?;
        pseudo.fillet_vertex_mut(adjust_vert)?.set_original_uv(uv1);
        set_vertex_point(pseudo, adjust_vert, pnt1)?;
        let fv = pseudo.fillet_vertex_mut(adjust_vert)?;
        fv.set_fillet_vertex_type(FilletVertexType::Mate);
        fv.set_mate(0, Some(prev_v));
        pseudo
            .fillet_vertex_mut(prev_v)?
            .set_mate(0, Some(adjust_vert));
        let vu1 = pseudo
            .vu_at_rail_end(adjust_vert, Some(prev_fg), None, None)?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        copy_uv_pos(pseudo, vu1, &tsect_pnt1)?;

        // Make a new vertex
        // Compute its geometry
        let next_eu = primary_eu(pseudo, next_e, OP)?;
        let next_fg = pseudo
            .fillet_edgeuse(next_eu)?
            .fillet_geom()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let next_v = vertex_at(&self.vertices, (found_vert_index + 1) % total, OP)?;
        let next_vu = pseudo
            .vu_at_rail_end(next_v, Some(next_fg), None, None)?
            .ok_or_else(|| invalid(OP, "no rail vertex use"))?;
        let next_fs = geom_solver(pseudo, next_fg, OP)?;
        let rail_index2 = solver(next_fs)?.find_index_of_rail_on_face(target, Some(orig_face))?;
        let tsect_pnt2 = pseudo.fillet_vertexuse(next_vu)?.tsect_pnt().clone();
        let uv2 = tsect_pnt2.uv_pos(srf_index(rail_index2));
        let pnt2 = orig_surf.evaluate_point(k, uv2)?;
        let new_vert = pseudo.brep_mut().add_vertex(pnt2)?;
        let mut fv = FilletVertex::new(k, corner);
        fv.set_original_uv(uv2);
        fv.set_point_class(PointClassType::Face, Some(EntityRef::Face(orig_face)));
        fv.set_fillet_vertex_type(FilletVertexType::Mate);
        fv.set_mate(0, Some(next_v));
        fv.set_status(FilStatus::Processed);
        pseudo.insert_fillet_vertex(new_vert, fv);
        pseudo
            .fillet_vertex_mut(next_v)?
            .set_mate(0, Some(new_vert));
        let vu2 = pseudo
            .vu_at_rail_end(adjust_vert, Some(next_fg), None, None)?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        copy_uv_pos(pseudo, vu2, &tsect_pnt2)?;

        // Move pVU2 from pAdjustVert to pNewVert
        move_vertexuse(pseudo, vu2, adjust_vert, new_vert)?;

        let mut vu3 = pseudo
            .brep()
            .edgeuse(next_eu)?
            .vertexuse
            .ok_or_else(|| invalid(OP, "edge use without vertex use"))?;
        if pseudo.brep().vertexuse(vu3)?.vertex != Some(adjust_vert) {
            let mate = eu1::mate(pseudo.brep(), next_eu)?
                .ok_or_else(|| invalid(OP, "edge use without mate"))?;
            vu3 = pseudo
                .brep()
                .edgeuse(mate)?
                .vertexuse
                .ok_or_else(|| invalid(OP, "edge use without vertex use"))?;
        }
        // Move pVU3 from pAdjustVert to pNewVert
        move_vertexuse(pseudo, vu3, adjust_vert, new_vert)?;

        self.vertices.insert(found_vert_index, new_vert);

        let new_edge = pseudo.make_fillet_edge(Some(adjust_vert), Some(new_vert), corner)?;
        let fe = pseudo.fillet_edge_mut(new_edge)?;
        fe.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
        fe.set_original_face(Some(orig_face));

        self.edges.insert(found_edge_index, new_edge);

        self.topology_adjusted = true;

        Ok(())
    }
}

impl FilletCorner {
    /// The opening of `SmFilletNx1ClosedCorner::AdjustTangentCorner`, up to its
    /// split: `None` when the corner needs no adjustment (not a 3x1 closed corner, its
    /// filleted edge not tangent to itself within 2 degrees, or the fillet surface
    /// meeting itself or leaving a gap); else the fillet geometry and the rail-end
    /// vertex use (second by intersection curve parameter) to split it at.
    ///
    /// # Errors
    ///
    /// As [`FilletCorner::adjust_tangent_corner_nx1_closed`] before its split.
    #[allow(clippy::too_many_lines)]
    pub fn nx1_closed_tangent_split(
        &self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &FilletBrep,
    ) -> Result<Option<(FilletGeomId, VertexuseId)>> {
        const OP: &str = OP_NX1_TANGENT;
        // First check that this is the appropriate type.
        // Must be Nx1 tangent corner, and currently N must be 3.
        let Some(&[solver_eu]) = self.solver_eus.as_deref() else {
            return Ok(None);
        };
        if self.all_eus.as_ref().is_none_or(|v| v.len() != 3) {
            return Ok(None);
        }

        // Check that the two filleted edges are tangent at this vertex.
        // (the statuses are ignored upstream: a failed evaluation leaves the derivative
        // default-constructed)
        let mut deriv = |t: f64| -> Result<Vec3> {
            match eu3::normalized_evaluate(k, target, solver_eu, t, false, true, true) {
                Ok(ev) => Ok(ev.first_deriv.unwrap_or(Vec3::UNDEF)),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(Vec3::UNDEF),
            }
        };
        let first_deriv1 = deriv(0.0)?;
        let first_deriv2 = deriv(1.0)?;

        let ang_tol_deg = 2.0; // cbi: some places use 5 degrees; sort that all out.
        if !first_deriv1.is_parallel_to(first_deriv2, ang_tol_deg) {
            return Ok(None);
        }

        // Ok, we are an Nx1-closed tangent corner.

        // Check whether the fillet surface meets itself smoothly: corner points will
        // match. There will be two FilletGeoms, but they are the same.
        let v0 = vertex_at(&self.vertices, 0, OP)?;
        let fillet_geoms = pseudo.vertex_fillet_geoms(v0)?;
        // SM_ASSERT(sFilletGeoms.GetSize() == 2); (upstream reads [0] unchecked)
        let orig_fg = *fillet_geoms
            .first()
            .ok_or_else(|| invalid(OP, "corner vertex without fillet geometry"))?;
        let fillet_srf = pseudo
            .geom(orig_fg)?
            .fillet_surface(None)?
            .ok_or_else(|| invalid(OP, "fillet geometry without fillet surface"))?;

        let srf_domain = fillet_srf.natural_uv_domain_with_nurb(k)?;
        // u runs the long way, v goes across the fillet. (statuses ignored)
        let srf_pt00 =
            unchecked_point(fillet_srf.evaluate_point(k, srf_domain.evaluate(0.0, 0.0)))?;
        let srf_pt10 =
            unchecked_point(fillet_srf.evaluate_point(k, srf_domain.evaluate(1.0, 0.0)))?;
        let srf_pt01 =
            unchecked_point(fillet_srf.evaluate_point(k, srf_domain.evaluate(0.0, 1.0)))?;
        let srf_pt11 =
            unchecked_point(fillet_srf.evaluate_point(k, srf_domain.evaluate(1.0, 1.0)))?;

        let dist0 = srf_pt00.distance(srf_pt10);
        let dist1 = srf_pt01.distance(srf_pt11);

        if dist0 < self.approx_tol_3d && dist1 < self.approx_tol_3d {
            return Ok(None);
        }

        // Ok, the fillet surface does not meet itself smoothly, we have to do something.
        // Either it overlaps, or there is a gap. Drop the fillet surface end point to the
        // start point plus its u-derivative. (The status is ignored: a failure keeps the
        // point and leaves the derivative default-constructed.)
        let (srf_pt00, su00) = match ignored(fillet_srf.evaluate_1st_derivatives(
            k,
            srf_domain.evaluate(0.0, 0.0),
            true,
            true,
        ))? {
            Some((p, su, _)) => (p, su),
            None => (srf_pt00, Vec3::UNDEF),
        };
        let t = line_closest_point(srf_pt00, su00, srf_pt10);

        if t <= 0.0 {
            return Ok(None);
        }

        // Overlap: we have to split.
        // SM_ASSERT(m_vVertices.GetSize() == 2);
        let mut fv = v0;
        // Looking for a vertex where a rail hits a side edge:
        if pseudo.fillet_vertex(fv)?.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
            fv = vertex_at(&self.vertices, 1, OP)?;
            if pseudo.fillet_vertex(fv)?.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
                return Err(SolidError::sm_err(OP));
            }
        }

        // The FG has four FVUs:
        let mut fvus = [None; 4];
        for (slot, (r, e)) in fvus.iter_mut().zip([(0, 0), (0, 1), (1, 0), (1, 1)]) {
            *slot = pseudo.rail_vertexuse(orig_fg, r, e)?;
        }

        // Sort these: their int-curve parameters should be set.
        let mut crv_params: Vec<f64> = Vec::with_capacity(4);
        let mut sort_indices: Vec<u64> = Vec::with_capacity(4);
        for (ii, fvu) in (0u64..).zip(fvus) {
            let fvu = fvu.ok_or_else(|| invalid(OP, "fillet geometry without rail vertex use"))?;
            let int_param = pseudo.fillet_vertexuse(fvu)?.tsect_pnt().curve_parameter;
            if int_param < -EFF_ZERO {
                // If this happens, we'll have to figure out something else.
                return Err(SolidError::sm_err(OP));
            }
            add_sorted(
                int_param,
                ii,
                true,
                &mut crv_params,
                Some(&mut sort_indices),
            );
        }

        // Split at the 2nd sorted parameter: that will make pOrigFG be the beginning
        // short piece of fillet, which is the rollover part.
        let this_fv_use = sort_indices
            .get(1)
            .and_then(|&i| usize::try_from(i).ok())
            .and_then(|i| fvus.get(i).copied().flatten())
            .ok_or_else(|| invalid(OP, "rail vertex use index"))?;
        Ok(Some((orig_fg, this_fv_use)))
    }

    /// `SmFilletNx1ClosedCorner::AdjustTangentCorner`: for a 3x1 closed corner whose
    /// filleted edge is tangent to itself at the vertex (within 2 degrees), checks that
    /// the fillet surface meets itself at the corner; where it overlaps, the fillet
    /// geometry is split at the second rail end (sorted by intersection curve parameter)
    /// into a short tangent roll-over part and the rest, the corner takes over the new
    /// edge, the rail ends' UV points are moved across the base surfaces' seams and both
    /// parts are recalculated. A failed split leaves the corner as it is.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when neither corner vertex is `SM_FV_RAIL_X_EDGEUSE` or a rail end has a
    /// negative intersection curve parameter; `SM_ERR_NULL_POINTER` for a geometry
    /// without solver or a rail without curve; [`SolidError::InvalidInput`] where
    /// upstream indexes past an array or dereferences `NULL` (no corner vertex, fillet
    /// geometry, fillet surface, rail, rail vertex use or vertex);
    /// [`SolidError::Unported`] (`blend.fillet_corner_p3` module docs).
    #[allow(clippy::too_many_lines)]
    pub fn adjust_tangent_corner_nx1_closed(
        &mut self,
        env: &mut TangentSplitEnv<'_>,
    ) -> Result<()> {
        const OP: &str = OP_NX1_TANGENT;
        let Some((orig_fg, this_fv_use)) =
            self.nx1_closed_tangent_split(env.k, env.target, env.pseudo)?
        else {
            return Ok(());
        };

        // Which end of the FilletGeom are we at? 0 for start, 1 for end.
        let which_end = 0usize;

        // It makes a difference to the split routine whether the original
        // FG, or the new one, is a tangent-rollover.
        env.pseudo
            .geom_mut(orig_fg)?
            .set_fillet_geom_type(FilletGeomType::TangentRollover);

        // pOrigFG->GetFilletSolver()->SplitFilletGeomAtSideEdge(..): upstream
        // dereferences a NULL solver
        let si = env
            .pseudo
            .geom(orig_fg)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let split = env.ops.split_fillet_geom_at_side_edge(
            env.k,
            env.target,
            env.pseudo,
            self,
            si,
            orig_fg,
            this_fv_use,
            which_end,
        );
        // If failure, the whole fillet could still succeed, so don't SER.
        let new_fg = match split {
            Ok(g) => g,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(()),
        };

        // The split fillet vertex, pThisFV, is now just a mate to the
        // corresponding FV on the other rail.
        let this_fv = env
            .pseudo
            .brep()
            .vertexuse(this_fv_use)?
            .vertex
            .ok_or_else(|| invalid(OP, "rail vertex use without vertex"))?;

        let fvr = env.pseudo.fillet_vertex_mut(this_fv)?;
        fvr.set_fillet_vertex_type(FilletVertexType::Mate);
        fvr.set_status(FilStatus::Unprocessed);

        // Grab the correct rail. (GetRailIndexOfVertex's status is ignored; it leaves the
        // index unset when it fails)
        let rail_idx = env.pseudo.rail_index_of_vertex(orig_fg, Some(this_fv))?;
        let this_rail = env
            .pseudo
            .geom(orig_fg)?
            .rail(rail_idx)
            .ok_or_else(|| invalid(OP, "fillet geometry without rail"))?;
        let other_rail = 1usize
            .checked_sub(rail_idx)
            .ok_or_else(|| invalid(OP, "rail index"))?;

        // Set it as mate of its opposite fillet vertex. (lWhichEnd is 0)
        let opposite_fvu = env
            .pseudo
            .rail_vertexuse(orig_fg, other_rail, 0)?
            .ok_or_else(|| invalid(OP, "fillet geometry without rail vertex use"))?;
        let opposite_fv = env.pseudo.brep().vertexuse(opposite_fvu)?.vertex;

        if let Some(opp) = opposite_fv {
            env.pseudo
                .fillet_vertex_mut(this_fv)?
                .set_mate(0, Some(opp));
            let o = env.pseudo.fillet_vertex_mut(opp)?;
            o.set_mate(0, Some(this_fv));
            // Also tell it that its geometry is kaput
            // (because pThisFV is on a different base surface).
            o.set_status(FilStatus::Unprocessed);
        }

        // Also, pThisFV is now in a face.
        let new_rail = env
            .pseudo
            .geom(new_fg)?
            .rail(rail_idx)
            .ok_or_else(|| invalid(OP, "fillet geometry without rail"))?;
        let new_base_face = env.pseudo.fillet_edge(new_rail)?.original_face();
        env.pseudo
            .fillet_vertex_mut(this_fv)?
            .set_point_class(PointClassType::Face, new_base_face.map(EntityRef::Face));

        // Edges are kaput too.
        for &e in &self.edges {
            env.pseudo
                .fillet_edge_mut(e)?
                .set_status(FilStatus::Unprocessed);
        }

        // Also: there are now two new FilletVertex's, which belong to this FilletCorner.
        // 'Same' is on the same side as pThisFV, 'Opp' as pOppositeFV; they are at the
        // high end of the original FG.
        let new_fv_same = env.pseudo.rail_vertex(orig_fg, rail_idx, 1)?;
        let new_fv_opp = env.pseudo.rail_vertex(orig_fg, other_rail, 1)?;

        // pNewFVSame should be all set, it's what pThisFV used to be.
        // But pNewFVOpp is now in a face.
        let opp_rail = env
            .pseudo
            .geom(orig_fg)?
            .rail(other_rail)
            .ok_or_else(|| invalid(OP, "fillet geometry without rail"))?;
        let opp_base_face = env.pseudo.fillet_edge(opp_rail)?.original_face();
        env.pseudo
            .fillet_vertex_mut(new_fv_opp)?
            .set_point_class(PointClassType::Face, opp_base_face.map(EntityRef::Face));

        // We also have to take responsibility for the new edge.
        let new_edges =
            vertex::common_edges(env.pseudo.brep(), new_fv_same, Some(new_fv_opp), None)?;
        // SM_ASSERT(aNewEdges.GetSize() == 1);
        if let Some(&this_fil_edge) = new_edges.first() {
            self.edges.push(this_fil_edge);
            env.pseudo
                .fillet_edge_mut(this_fil_edge)?
                .set_fillet_corner(env.corner);
        }

        // We have to update the fillet surfaces in the fillet geoms here.
        // A prerequisite to that is recalculating the vertex geom.
        self.calc_corner_vert_geom_unchecked(env)?;

        // Adjust the uv positions in the TsectPnts in the Fillet Vertexuses for the seams.
        // SM_ASSERT(pNewFG->GetFilletSolver() == pFS);
        let fs = env.ops.solver(si)?;
        let off = |i: usize| -> Result<OffsetSurface> {
            fs.surface(i)?
                .map(|slot| slot.surface.clone())
                .ok_or_else(|| invalid(OP, "solver without offset surface"))
        };
        let surf1 = off(0)?;
        let surf2 = off(1)?;
        let domain1 = surf1.surface.natural_uv_domain();
        let domain2 = surf2.surface.natural_uv_domain();

        // Note, surface closure is relatively expensive to calculate.
        let surf_closure1 =
            set_surface_closure(env.k, &Surface::from(surf1.surface.clone()), &domain1)?;
        let surf_closure2 =
            set_surface_closure(env.k, &Surface::from(surf2.surface.clone()), &domain2)?;

        // Process pOrigFG, then pNewFG: map any out-of-domain uv points back in, with a
        // periodic shift.
        let mut used_fvus = Vec::new();
        for g in [orig_fg, new_fg] {
            for ii in 0..2 {
                for jj in 0..2 {
                    // SM_ASSERT(pFVU != NULL);
                    let Some(fvu) = env.pseudo.rail_vertexuse(g, ii, jj)? else {
                        continue;
                    };
                    let fvertex = env
                        .pseudo
                        .brep()
                        .vertexuse(fvu)?
                        .vertex
                        .ok_or_else(|| invalid(OP, "rail vertex use without vertex"))?;
                    let fvx = env.pseudo.fillet_vertex(fvertex)?;
                    if fvx.status() == FilStatus::Unprocessed
                        || fvx.fillet_vertex_type() == FilletVertexType::Mate
                        || fvx.fillet_vertex_type() == FilletVertexType::RailXExtendedEdgeuse
                    {
                        continue;
                    }

                    let mut tsect = env.pseudo.fillet_vertexuse(fvu)?.tsect_pnt().clone();
                    let mut uv = tsect.uv_pos(SrfIndex::First);
                    if check_seam_out(
                        surf1.base_surface(),
                        &mut uv,
                        self.approx_tol_3d,
                        surf_closure1,
                    ) {
                        *tsect.uv_pos_mut(SrfIndex::First) = uv;
                    }
                    let mut uv = tsect.uv_pos(SrfIndex::Second);
                    if check_seam_out(
                        surf2.base_surface(),
                        &mut uv,
                        self.approx_tol_3d,
                        surf_closure2,
                    ) {
                        *tsect.uv_pos_mut(SrfIndex::Second) = uv;
                    }
                    env.pseudo.fillet_vertexuse_mut(fvu)?.set_tsect_pnt(&tsect);

                    // Also keep track of which FVU this came from.
                    used_fvus.push(fvertex);
                }
            }
        }

        // re-Calc Vert Geom with the modified UV values in the FVUs' TsectPnts.
        // First have to mark the changed ones UnProcessed.
        for fvertex in used_fvus {
            env.pseudo
                .fillet_vertex_mut(fvertex)?
                .set_status(FilStatus::Unprocessed);
        }

        self.calc_corner_vert_geom_unchecked(env)?;

        // We need a direction for ReCalcFilletGeom: the split is assumed very close to
        // the beginning of the smoothly-closed rail, so use the start parameter.
        let rail_ivl = env.pseudo.brep().edge(this_rail)?.interval;
        let march_dir = rail_tangent_unchecked(env, this_rail, rail_ivl.min())?;

        // Tell FilletSolver to use surfaces in the FilletGeoms.
        recalc_split_geoms(env, si, orig_fg, new_fg, march_dir)?;

        Ok(())
    }
}

impl FilletCorner {
    /// `SmFilletCorner::MakeAnalyticCornerSurface`: when the corner's three or four
    /// fillet edge curves can be filled by an analytic surface, makes it and adds it to
    /// the corner's surfaces (`true`). Corners touching a blend-curve fillet, a cubic
    /// rail-rail interpolation or a variable-radius fillet get none. A chamfer corner
    /// gets a plane (three edges) or a bilinear patch (four), and its non-linear edges
    /// are recomputed as intersections with it; a corner with a rail-rail interpolating
    /// edge gets a surface of revolution about it, or else a variable-radius fillet
    /// ([`FilletCorner::make_variable_radius_fillet`]); any other gets a sphere through
    /// its arcs, or else a constant-radius fillet (`MakeConstantRadiusFillet`).
    ///
    /// Upstream returns `SmBoolean` but exits through `SER` / `NER`, whose non-zero
    /// status its caller reads as `TRUE`: a failing status here is `Ok(true)` (no
    /// surface is added).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (a primary edge
    /// use, fillet solver, surface generator, start vertex or intersection curve);
    /// [`SolidError::Unported`] (`MakeConstantRadiusFillet`, unit
    /// `blend.fillet_corner_p1`).
    pub fn make_analytic_corner_surface(&mut self, env: &mut CornerEnv<'_>) -> Result<bool> {
        env.with_patch_env(|env| self.make_analytic_corner_surface_with(env))
    }

    /// [`FilletCorner::make_analytic_corner_surface`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::make_analytic_corner_surface`].
    pub fn make_analytic_corner_surface_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
    ) -> Result<bool> {
        match self.analytic_corner_surface(env) {
            Ok(made) => Ok(made),
            Err(
                e @ (SolidError::Unported { .. }
                | SolidError::InvalidInput { .. }
                | SolidError::PortInvariant { .. }),
            ) => Err(e),
            // `return sErr` / `return SM_ERR_NULL_POINTER` as an SmBoolean
            Err(_) => Ok(true),
        }
    }

    /// [`FilletCorner::make_analytic_corner_surface`] with its `SER` / `NER` exits as
    /// errors.
    #[allow(clippy::too_many_lines)]
    fn analytic_corner_surface(&mut self, env: &mut CornerPatchEnv<'_, '_>) -> Result<bool> {
        const OP: &str = OP_ANALYTIC;
        // no work - Vertex does not have exactly 3 or 4 edges
        let total_edges = self.edges.len();
        if !(3..=4).contains(&total_edges) {
            return Ok(false);
        }

        // no work - quit if FilletVertex is connected to any BlendCurve FilletEdge
        for &fillet_edge in &self.edges {
            let prim_eu = primary_eu(env.pseudo, fillet_edge, OP)?;
            if let Some(fillet_geom) = env.pseudo.fillet_edgeuse(prim_eu)?.fillet_geom() {
                let fs = solver_ref(env, fillet_geom, OP)?;
                let fsg = fs
                    .fillet_surface_generator()
                    .ok_or_else(|| invalid(OP, "fillet solver without surface generator"))?;
                if fsg.fillet_surface_generator_type() == FilletSurfaceGeneratorType::BlendCurve {
                    // corner is not analytic when connected to a BlendCurve
                    return Ok(false);
                }
            }
        }

        let mut create_surf_of_revo = false;
        let mut curves: Vec<Curve> = Vec::new();
        let mut plane_pnts: Vec<Point3> = Vec::new();
        let mut major_arc_index = 0usize;

        // for every FilletEdge connected to this FilletVertex: check the FilletEdge
        // types, and gather the start points (chamfer), the major arc (rail-rail
        // interpolation) and the curves
        for (ii, &fillet_edge) in self.edges.iter().enumerate() {
            let fe = env.pseudo.fillet_edge(fillet_edge)?;
            if self.chamfer {
                // corner will be planar - depends on FilletEdges being given in CCW order
                // about Vertex - order sets the surface normal direction
                let start = edge::start_vertex(env.pseudo.brep(), fillet_edge)?
                    .ok_or_else(|| invalid(OP, "fillet edge without start vertex"))?;
                plane_pnts.push(env.pseudo.vertex_point(start)?);
            } else if fe.fillet_edge_type() == FilletEdgeType::RailRailInterpolation {
                // can't make analytics for CUBIC_RAIL_RAIL_INTERPOLATE case
                if fe.status() == FilStatus::CubicRailRailInterpolate {
                    return Ok(false); // Unable to create analytic
                }
                // otherwise - let the last FilletEdge seen by the Major Arc for a
                // SurfOfRevolution
                create_surf_of_revo = true;
                major_arc_index = ii;
            } else {
                // quit if any FilletEdge is a VariableRadius Fillet
                let prim_eu = primary_eu(env.pseudo, fillet_edge, OP)?;
                if let Some(fillet_geom) = edgeuse_or_mate_geom(env.pseudo, prim_eu, OP)? {
                    let fs = solver_ref(env, fillet_geom, OP)?;
                    if fs.solver_type()? == FilletSolverType::VariableRadius {
                        return Ok(false);
                    }
                }
            }

            // gather all connected FilletEdge->Curves - quit if any edge is not a
            // BSplineCurve
            let curve = env
                .pseudo
                .brep()
                .edge(fillet_edge)?
                .curve
                .as_ref()
                .filter(|c| is_b_spline_kind(c))
                .ok_or_else(|| SolidError::sm_err(OP))?; // Not Nurbs??
            curves.push(curve.clone());
        }

        let surface = if self.chamfer {
            // Create a 'planar' surface - SmPlane for 3pts, SmBSpline BilinearSurface
            // for 4 pts
            let Some(surface) = soft(create_planar_surface(
                env.k,
                &plane_pnts,
                self.approx_tol_3d,
            ))?
            else {
                return Ok(false);
            };

            // Re-compute geometry of each edge
            for &fillet_edge in &self.edges {
                let prim_eu = primary_eu(env.pseudo, fillet_edge, OP)?;
                let curve = env
                    .pseudo
                    .brep()
                    .edge(fillet_edge)?
                    .curve
                    .as_ref()
                    .filter(|c| is_b_spline_kind(c))
                    .ok_or_else(|| SolidError::sm_err(OP))?;

                // skip FilletEdges which are linear
                if ignored(curve.is_line(env.k, 10, self.approx_tol_3d))?
                    .flatten()
                    .is_some()
                {
                    continue; // No need to recompute the edge
                }

                // Delete FilletEdge->Curve and UVTrimCurve
                set_edge_curve(env.pseudo.brep_mut(), fillet_edge, None)?;

                // Intersect FilletCorner->plane with FilletEdge->Surface
                let fe = env.pseudo.fillet_edge(fillet_edge)?;
                let (fillet_surface, approx_tol, ang_tol) = if fe.fillet_edge_type()
                    == FilletEdgeType::RailRailInterpolation
                {
                    // Get original face surface
                    let orig_face = fe
                        .original_face()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let s = env.target.face(orig_face)?.surface.clone();
                    (
                        s,
                        face_p2::tolerance(env.target, orig_face)?,
                        30.0 * PI / 180.0,
                    )
                } else {
                    let fillet_geom = env
                        .pseudo
                        .fillet_edgeuse(prim_eu)?
                        .fillet_geom()
                        .ok_or_else(|| invalid(OP, "fillet edge use without fillet geometry"))?;
                    let fs = solver_ref(env, fillet_geom, OP)?;
                    (
                        env.pseudo.geom(fillet_geom)?.fillet_surface(None)?.cloned(),
                        fs.this_approx_tol_3d(),
                        fs.this_ang_tol_rad(),
                    )
                };
                let fillet_surface = fillet_surface.ok_or_else(|| SolidError::null_pointer(OP))?;

                let start_v = edge::start_vertex(env.pseudo.brep(), fillet_edge)?
                    .ok_or_else(|| invalid(OP, "fillet edge without start vertex"))?;
                let end_v = edge::other_vertex(env.pseudo.brep(), fillet_edge, Some(start_v))?
                    .ok_or_else(|| invalid(OP, "fillet edge without other vertex"))?;
                let start_pnt = env.pseudo.vertex_point(start_v)?;
                let end_pnt = env.pseudo.vertex_point(end_v)?;
                let mut dir = end_pnt - start_pnt;

                // intersect
                let r = srf_srf_intersection(
                    env.k,
                    Some(start_pnt),
                    Some(end_pnt),
                    Some(&mut dir),
                    None,
                    &fillet_surface,
                    &surface,
                    approx_tol,
                    ang_tol,
                    SrfSrfOptions {
                        skip_two_pnts_intersection: false,
                        ref_point: None,
                    },
                )?;

                // (pUVCurve2 is deleted)
                let new_3d_curve = r
                    .curve_3d
                    .ok_or_else(|| invalid(OP, "intersection without curve"))?;
                set_edge_curve(env.pseudo.brep_mut(), fillet_edge, Some(new_3d_curve))?;
                env.pseudo
                    .set_edgeuse_uv_curve(prim_eu, r.uv_curve1, true, true)?;
            }
            surface
        } else if create_surf_of_revo {
            // create SurfOfRevolution to fill Loop of BoundaryEdges - if possible
            match soft(create_srf_of_revolution(
                env.k,
                &curves,
                major_arc_index,
                self.approx_tol_3d,
            ))? {
                Some(s) => s,
                None => {
                    // when SurfOfRevolution could not be found to fill Boundary Curves
                    // try to convert it to a variable-radius fillet to achive better G1
                    // continuity.
                    match soft(self.make_variable_radius_fillet_with(env))? {
                        Some(s) => s,
                        // when both tries fail - quit
                        None => return Ok(false),
                    }
                }
            }
        } else {
            // Try to create a sphere that fills boundary sCurves to within tolerance
            match soft(BSplineSurface::create_sphere_from_arcs(
                env.k,
                &curves,
                self.approx_tol_3d,
            ))? {
                Some(r) => Surface::from(r.sphere),
                None => {
                    // When a Sphere could not be found to fill Boundary Curves try to
                    // convert it to a constant-radius fillet.
                    match soft(self.make_constant_radius_fillet_with(env))? {
                        Some(s) => s,
                        // when both tries fail - quit
                        None => return Ok(false),
                    }
                }
            }
        };

        // save newSurface
        self.surfaces.push(surface);

        // all done
        Ok(true)
    }
}

impl FilletCorner {
    /// `SmFilletCorner::MakeVariableRadiusFillet`: a four-sided corner with one
    /// rail-rail interpolating edge, between two cross sections of the same surface
    /// generator, becomes a variable-radius fillet between the interpolating edge's
    /// original surface and the opposite fillet's surface: its radius runs from the
    /// cross sections' radii through the opposite fillet's (an elliptical B-spline law)
    /// along the reversed opposite edge curve. The new fillet's rails replace the curves
    /// of the interpolating and (reversed) opposite edges and their UV trim curves; its
    /// fillet surface is returned.
    ///
    /// The temporary solver (`SmObjDelete` upstream) is dropped on return; its fillet
    /// geometry stays in the pseudo B-rep's geometry table (upstream deletes it, but not
    /// its fillet surface), and so do the two rail edges, whose curves were moved.
    ///
    /// # Errors
    ///
    /// `SM_ERR` (unable to convert) for a corner that is not four-sided, has no
    /// rail-rail interpolating edge, mixes surface generators or blend scales, has a
    /// cross section that is not a circular arc of the centre line, an opposite edge
    /// that is not a cross section with a fillet geometry, no rail at the
    /// interpolating edge's vertex, end points that do not project once onto the
    /// opposite fillet or onto an iso line, an opposite radius not larger than both
    /// cross-section radii, a linear opposite fillet, a failed recalculation, or (a
    /// concave `NxN` corner) a rail-rail curve the corner intersects;
    /// `SM_ERR_NULL_POINTER` for a missing fillet geometry, centre line, original face,
    /// surface, fillet surface, curve or rail curve; `SM_ERR_INVALID_INPUT` for a zero
    /// axis or normal; those of the projections and evaluations;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (a vertex,
    /// primary edge use, fillet solver, surface generator, vertex use or UV trim curve).
    pub fn make_variable_radius_fillet(&mut self, env: &mut CornerEnv<'_>) -> Result<Surface> {
        env.with_patch_env(|env| self.make_variable_radius_fillet_with(env))
    }

    /// [`FilletCorner::make_variable_radius_fillet`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::make_variable_radius_fillet`].
    #[allow(clippy::too_many_lines)]
    pub fn make_variable_radius_fillet_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
    ) -> Result<Surface> {
        const OP: &str = OP_VAR_RAD;
        let sm_err = || SolidError::sm_err(OP); // Unable to convert
        let null = || SolidError::null_pointer(OP);

        // no work - corner does not have exactly 4 sides
        let total_edges = self.edges.len();
        if total_edges != 4 {
            return Err(sm_err());
        }

        // Find surfaces of blend
        let mut found_index = None;
        for (ii, &e) in self.edges.iter().enumerate() {
            if env.pseudo.fillet_edge(e)?.fillet_edge_type()
                == FilletEdgeType::RailRailInterpolation
            {
                found_index = Some(ii);
                break;
            }
        }
        // no work - can't find a corner bounding edge of type
        // SM_FE_RAIL_RAIL_INTERPOLATION
        let found_index = found_index.ok_or_else(sm_err)?;

        let edge_at = |i: usize| -> Result<EdgeId> {
            self.edges
                .get(i % total_edges)
                .copied()
                .ok_or_else(|| invalid(OP, "edge index"))
        };
        let interpolate_edge = edge_at(found_index)?;
        let opposite_edge = edge_at(found_index + 2)?;
        let cross_edges = [
            edge_at(found_index + total_edges - 1)?,
            edge_at(found_index + 1)?,
        ];
        let brep = env.pseudo.brep();
        let vertex_of = |e: EdgeId| -> Result<VertexId> {
            edge::vertex(brep, e)?.ok_or_else(|| invalid(OP, "fillet edge without vertex"))
        };
        let other_of = |e: EdgeId, v: VertexId| -> Result<VertexId> {
            edge::other_vertex(brep, e, Some(v))?
                .ok_or_else(|| invalid(OP, "fillet edge without other vertex"))
        };
        // (the interpolating edge's vertices are dereferenced only later)
        let interpolate_edge_start_v = edge::vertex(brep, interpolate_edge)?;
        let interpolate_edge_end_v =
            edge::other_vertex(brep, interpolate_edge, interpolate_edge_start_v)?;
        let opposite_edge_start_v = vertex_of(opposite_edge)?;
        let opposite_edge_end_v = other_of(opposite_edge, opposite_edge_start_v)?;
        let opposite_start = env.pseudo.vertex_point(opposite_edge_start_v)?;
        let opposite_end = env.pseudo.vertex_point(opposite_edge_end_v)?;

        let tol = self.approx_tol_3d;
        let mut start_rad = 0.0;
        let mut end_rad = 0.0;
        let mut fsg_type = FilletSurfaceGeneratorType::Unknown;
        let mut blend_scale = 1.0; // For use in SmBlendCurveCrossSectionFSG cases only
        let mut max_continuity = 0u32;
        let mut start_axis = Vec3::UNDEF;
        let mut end_axis = Vec3::UNDEF;
        let mut start_arc_center = Point3::UNDEF;
        let mut end_arc_center = Point3::UNDEF;
        let approx_arcs = false;
        let approx_arc_tol = 1.0e-3;

        for (ii, &e) in cross_edges.iter().enumerate() {
            if env.pseudo.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::CrossSection {
                return Err(sm_err()); // Unknown cases.
            }

            let prim_eu = primary_eu(env.pseudo, e, OP)?;
            let fg = env
                .pseudo
                .fillet_edgeuse(prim_eu)?
                .fillet_geom()
                .ok_or_else(null)?;
            let fsg = solver_ref(env, fg, OP)?
                .fillet_surface_generator()
                .ok_or_else(|| invalid(OP, "fillet solver without surface generator"))?;

            if ii == 0 {
                fsg_type = fsg.fillet_surface_generator_type();
                if fsg_type == FilletSurfaceGeneratorType::BlendCurve {
                    blend_scale = fsg.blend_scale().unwrap_or(blend_scale);
                    let continuity = fsg.continuity().unwrap_or(0);
                    if continuity > max_continuity {
                        max_continuity = continuity;
                    }
                }
            } else if fsg_type != fsg.fillet_surface_generator_type() {
                return Err(sm_err()); // Unable to convert - two different types
            } else if fsg_type == FilletSurfaceGeneratorType::BlendCurve {
                let other_scale = fsg.blend_scale().unwrap_or(blend_scale);
                if (blend_scale - other_scale).abs() > EFF_ZERO {
                    return Err(sm_err()); // Unable to convert
                }
            }

            // Drop pE's end points to center line to compute radius, axis & origin of
            // cross section
            let center_line = env
                .pseudo
                .geom(fg)?
                .center_line_geometry()
                .cloned()
                .ok_or_else(null)?;
            let center_line_ivl = center_line.natural_interval();
            let v = vertex_of(e)?;
            let start_pnt = env.pseudo.vertex_point(v)?;
            let end_pnt = env.pseudo.vertex_point(other_of(e, v)?)?;
            let arc_center = |p: Point3| -> Result<Point3> {
                let sols = center_line.global_point_solve(
                    env.k,
                    &center_line_ivl,
                    SolverOperation::Minimize,
                    p,
                    tol,
                    SolutionRequested::Single,
                    &GlobalPointSolveOptions::<ObjectHandle>::default(),
                )?;
                let [sol] = sols.as_slice() else {
                    return Err(sm_err());
                };
                center_line.evaluate_point(env.k, sol.start.param(0)?)
            };
            let start_center = arc_center(start_pnt)?;
            let vec = start_pnt - start_center;
            let rad = vec.length();
            let end_center = arc_center(end_pnt)?;
            let vec1 = end_pnt - end_center;
            let rad1 = vec1.length();

            if (rad - rad1).abs() > rad * 1.0e-8 {
                return Err(sm_err());
            }

            let axis = unitize_ser(vec.cross(vec1), OP)?;

            if ii == 0 {
                start_rad = rad;
                start_arc_center = end_center;
                start_axis = axis;
            } else {
                end_rad = rad;
                end_arc_center = end_center;
                end_axis = axis;
            }
        }

        // Find base surfaces for filleting
        let face = env
            .pseudo
            .fillet_edge(interpolate_edge)?
            .original_face()
            .ok_or_else(null)?;
        let orig_surface = env.target.face(face)?.surface.clone().ok_or_else(null)?;

        if env.pseudo.fillet_edge(opposite_edge)?.fillet_edge_type() != FilletEdgeType::CrossSection
        {
            return Err(sm_err()); // Unable to convert
        }

        let opposite_prim_eu = primary_eu(env.pseudo, opposite_edge, OP)?;
        let opposite_fg = env
            .pseudo
            .fillet_edgeuse(opposite_prim_eu)?
            .fillet_geom()
            .ok_or_else(sm_err)?; // Unable to convert
        let opposite_fillet = env
            .pseudo
            .geom(opposite_fg)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(null)?;
        let opposite_fs = solver_of_geom(env, opposite_fg, OP)?;

        // Now determine the orientation of pSurface (original surface where
        // pInterpolateEdge is on)
        let mut offset_orientation = 1.0;
        let mut found_rail = false;
        let interpolate_edge_start_v = interpolate_edge_start_v
            .ok_or_else(|| invalid(OP, "rail-rail interpolating edge without vertex"))?;
        for e in vertex::edges(env.pseudo.brep(), interpolate_edge_start_v, None)? {
            if env.pseudo.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::Rail {
                continue;
            }
            let eu = primary_eu(env.pseudo, e, OP)?;
            let fg = env
                .pseudo
                .fillet_edgeuse(eu)?
                .fillet_geom()
                .ok_or_else(null)?;
            let fs = solver_ref(env, fg, OP)?;
            let geom = env.pseudo.geom(fg)?;
            offset_orientation = if geom.rail(0) == Some(e) {
                fs.orientation(0)?
            } else if geom.rail(1) == Some(e) {
                fs.orientation(1)?
            } else {
                return Err(sm_err());
            };
            found_rail = true;
            break;
        }

        if !found_rail {
            return Err(sm_err());
        }

        // Then determine the orientation of pFilletSurface. But first, drop two end
        // points of pOppositeEdge onto pOppositeFillet
        let mut offset_orientation2 = 1.0;
        let uv_domain = opposite_fillet.natural_uv_domain_with_nurb(env.k)?;
        let drop_uv = |p: Point3| -> Result<Point2> {
            let sols = opposite_fillet.global_point_solve::<ObjectHandle>(
                env.k,
                None,
                &uv_domain,
                SolverOperation::Intersect,
                p,
                tol,
                None,
                SolutionRequested::All,
            )?;
            let [sol] = sols.as_slice() else {
                return Err(sm_err());
            };
            Ok(Point2::new(sol.start.param(0)?, sol.start.param(1)?))
        };
        let uv = drop_uv(opposite_start)?;
        let other_uv = drop_uv(opposite_end)?;

        // Now, derive dOffsetOrientation2 (Unitize's status is ignored)
        let surf_normal = opposite_fillet
            .evaluate_normal(env.k, uv, Side::Left, Side::Left)?
            .unitize_or_z();
        let offset_vec = end_arc_center - opposite_start;
        if surf_normal.dot(offset_vec) < -EFF_ZERO {
            offset_orientation2 = -1.0;
        }

        // Make a variable radius fillet solver
        // Determine the fillet radius of pOppositeFillet
        let mut uv2 = other_uv;
        let size = uv_domain.size();

        // note: fillets failed here: but test maybe too demanding (1.0e-12), so
        // modified 9/16/04
        if ((uv.y - other_uv.y).abs() - size.y).abs() < 100.0 * EFF_ZERO {
            uv2.x = uv.x;
        } else if ((uv.x - other_uv.x).abs() - size.x).abs() < 100.0 * EFF_ZERO {
            uv2.y = uv.y;
        } else {
            return Err(sm_err());
        }

        let surf_pnt2 = opposite_fillet.evaluate_point(env.k, uv2)?;
        let surf_normal2 = unitize_ser(
            opposite_fillet.evaluate_normal(env.k, uv2, Side::Left, Side::Left)?,
            OP,
        )?;

        // Use sOppositeStart, sSurfNormal, sSurfPnt2 & sSurfNormal2 to derive radius
        let (opposite_rad, angle_span) = fillet_radius(
            opposite_start,
            surf_normal,
            surf_pnt2,
            surf_normal2,
            offset_orientation2,
        )?;

        // Create a temorary center curve
        let mut center_curve = env
            .pseudo
            .brep()
            .edge(opposite_edge)?
            .curve
            .clone()
            .ok_or_else(null)?;

        // Reverse the curve so it goes along what the fillet goes
        let ivl = center_curve.natural_interval();
        let ivl = center_curve.reverse_parameterization(env.k, &ivl)?;

        // Make sure sStartAxis & sEndAxis follow the curve orientaion
        start_axis = unitize_ser(start_axis, OP)?;
        end_axis = unitize_ser(end_axis, OP)?;
        let tangent = |t: f64| -> Result<Vec3> {
            center_curve
                .evaluate(env.k, t, 1, true, true)?
                .get(1)
                .copied()
                .ok_or_else(|| invalid(OP, "evaluation without derivative"))
        };
        if start_axis.dot(tangent(ivl.min())?) < -EFF_ZERO {
            start_axis = -start_axis;
        }
        if end_axis.dot(tangent(ivl.max())?) < -EFF_ZERO {
            end_axis = -end_axis;
        }

        let opposite_fsg_type = env
            .solvers
            .get(opposite_fs)
            .ok_or_else(|| invalid(OP, "fillet solver id not in the solver table"))?
            .fillet_surface_generator()
            .ok_or_else(|| invalid(OP, "fillet solver without surface generator"))?
            .fillet_surface_generator_type();
        let law = if opposite_fsg_type == FilletSurfaceGeneratorType::Linear {
            return Err(sm_err()); // Not yet handled
        } else {
            if opposite_rad <= end_rad || opposite_rad <= start_rad {
                return Err(sm_err());
            }
            FilletLaw::bspline_elliptical(
                start_rad,
                end_rad,
                opposite_rad,
                angle_span,
                0.0,
                0.0,
                None,
            )?
        };

        // the temporary solver's identity: past the solver table
        let new_fs_id = FilletSolverId(
            u32::try_from(env.solvers.len()).map_err(|_| invalid(OP, "solver table size"))?,
        );
        let mut new_fs = VariableRadiusFs::from_surfaces(
            env.k,
            env.pseudo,
            new_fs_id,
            tol,
            30.0 * PI / 180.0,
            2.0 * PI / 180.0,
            [
                (orig_surface, SurfaceId::Face(face)),
                (opposite_fillet, SurfaceId::Free(opposite_fg.0)),
            ],
            [offset_orientation, offset_orientation2],
            center_curve.clone(),
            law,
            false,
        )?;

        new_fs
            .curve_based
            .solver
            .set_fillet_executive(Some(FilletExecutiveRef));
        crate::blend::fillet_solver::curve_blend_sih::inherit_executive_handler(
            &mut new_fs.curve_based.solver,
            env.solvers,
        );
        let fsg = match fsg_type {
            FilletSurfaceGeneratorType::Circular => Some(FilletSurfaceGenerator::circular(
                approx_arcs,
                approx_arc_tol,
                false,
                false,
            )),
            FilletSurfaceGeneratorType::Linear => Some(FilletSurfaceGenerator::Linear),
            FilletSurfaceGeneratorType::BlendCurve => Some(FilletSurfaceGenerator::blend_curve(
                blend_scale,
                max_continuity,
            )),
            FilletSurfaceGeneratorType::Unknown => None,
        };
        if fsg.is_some() {
            new_fs.curve_based.solver.set_fillet_surface_generator(fsg);
        }

        let new_fg = new_fs.curve_based.solver.first_fillet_geom()?;

        // Create two rails, one is along the pInterpolateEdge, the other one is along
        // the pOppositeFillet but reversed
        let rail0 = env.pseudo.make_fillet_edge(
            Some(interpolate_edge_start_v),
            interpolate_edge_end_v,
            None,
        )?;
        env.pseudo.geom_mut(new_fg)?.set_rail(0, Some(rail0));
        let rail1 = env.pseudo.make_fillet_edge(
            Some(opposite_edge_end_v),
            Some(opposite_edge_start_v),
            None,
        )?;
        env.pseudo.geom_mut(new_fg)?.set_rail(1, Some(rail1));

        // Prepare for computing the fillet
        let interpolate_start_uv = env
            .pseudo
            .fillet_vertex(interpolate_edge_start_v)?
            .original_uv();
        let interpolate_end_uv =
            env.pseudo
                .fillet_vertex(interpolate_edge_end_v.ok_or_else(|| {
                    invalid(OP, "rail-rail interpolating edge without end vertex")
                })?)?
                .original_uv();
        let mut eu0 = primary_eu(env.pseudo, rail0, OP)?;
        let mut eu1 = primary_eu(env.pseudo, rail1, OP)?;
        let ends = [
            (
                interpolate_start_uv,
                other_uv,
                ivl.min(),
                start_arc_center,
                start_axis,
            ),
            (interpolate_end_uv, uv, ivl.max(), end_arc_center, end_axis),
        ];
        for (end, (uv0, uv1, t, center, axis)) in ends.into_iter().enumerate() {
            if end == 1 {
                eu0 = eu1::mate(env.pseudo.brep(), eu0)?
                    .ok_or_else(|| invalid(OP, "rail edge use without mate"))?;
                eu1 = eu1::mate(env.pseudo.brep(), eu1)?
                    .ok_or_else(|| invalid(OP, "rail edge use without mate"))?;
            }
            for eu in [eu0, eu1] {
                let vu = env
                    .pseudo
                    .brep()
                    .edgeuse(eu)?
                    .vertexuse
                    .ok_or_else(|| invalid(OP, "rail edge use without vertex use"))?;
                let tp = env.pseudo.fillet_vertexuse_mut(vu)?.tsect_pnt_mut();
                *tp.uv_pos_mut(SrfIndex::First) = uv0;
                *tp.uv_pos_mut(SrfIndex::Second) = uv1;
                if let Some(d) = tp.user_doubles.first_mut() {
                    *d = t;
                }
                tp.curve_parameter = t;
                *tp.crv_pos_mut() = center;
                *tp.crv_deriv_mut() = axis;
            }
        }

        env.pseudo
            .geom_mut(new_fg)?
            .set_fillet_geom_type(FilletGeomType::CornerFillet);

        // Compute marching direction for solver
        let march_dir = tangent(ivl.min())?;
        match new_fs.re_calc_fillet_geom(
            env.k,
            env.pseudo,
            false,
            Some(march_dir),
            Some(&center_curve),
        ) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(sm_err()), // Unable to convert
        }

        if self.corner_type() == FilletCornerType::NByNConcave {
            // Test if the fillet intersect the corner
            let new_rail_rail_int_crv = env
                .pseudo
                .brep()
                .edge(rail0)?
                .curve
                .clone()
                .ok_or_else(|| invalid(OP, "rail without curve"))?;
            if self.test_rail_rail_interpolation_curve(env.k, env.target, &new_rail_rail_int_crv)? {
                // This curve intersect eith the corner and we may not use the new fillet
                // as the corner surface
                env.pseudo.delete_fillet_edge(rail0)?;
                env.pseudo.delete_fillet_edge(rail1)?;
                return Err(sm_err()); // Unable to convert
            }
        }

        // Reset all geometry of corresponding edges: retrieve 3d & uv curves from rails,
        // delete them and replace them by new ones
        let interpolate_mate_eu = eu1::mate(
            env.pseudo.brep(),
            primary_eu(env.pseudo, interpolate_edge, OP)?,
        )?
        .ok_or_else(|| invalid(OP, "edge use without mate"))?;

        // save all the UVTrimCurve pointers
        let eu0_uv_trim_curve = env.pseudo.brep().edgeuse(eu0)?.uv_trim_curve.clone();
        let eu1_uv_trim_curve = env.pseudo.brep().edgeuse(eu1)?.uv_trim_curve.clone();

        // clear references to the UVTrimCurve pointers
        for eu in [interpolate_mate_eu, opposite_prim_eu, eu0, eu1] {
            env.pseudo.set_edgeuse_uv_curve(eu, None, false, false)?;
        }

        // Move Rail0->Curve to pInterpolateEdge->Curve (delete pInterpolateEdge->Curve)
        let rail0_curve = env
            .pseudo
            .brep()
            .edge(rail0)?
            .curve
            .clone()
            .ok_or_else(null)?;
        let crv_ivl = rail0_curve.natural_interval();
        set_edge_curve(env.pseudo.brep_mut(), rail0, None)?;
        set_edge_curve(env.pseudo.brep_mut(), interpolate_edge, Some(rail0_curve))?;
        env.pseudo.brep_mut().edge_mut(interpolate_edge)?.interval = crv_ivl;

        // Move Rail1->Curve to pOppositeEdge->Curve - remember to reverse Curve for
        // Opposite Edge
        let mut rail1_curve = env
            .pseudo
            .brep()
            .edge(rail1)?
            .curve
            .clone()
            .ok_or_else(null)?;
        set_edge_curve(env.pseudo.brep_mut(), rail1, None)?;
        // Similarily, reverse 3d curve
        let crv_ivl = rail1_curve.natural_interval();
        let crv_ivl = rail1_curve.reverse_parameterization(env.k, &crv_ivl)?;
        set_edge_curve(env.pseudo.brep_mut(), opposite_edge, Some(rail1_curve))?;
        env.pseudo.brep_mut().edge_mut(opposite_edge)?.interval = crv_ivl;

        // move pEU0->UVTrimCurve to pInterpolatedMateEU
        env.pseudo.set_edgeuse_uv_curve(eu0, None, false, false)?;
        env.pseudo
            .set_edgeuse_uv_curve(interpolate_mate_eu, eu0_uv_trim_curve, true, true)?;

        // move pEU1->UVTrimCurve to pOppositePrimEU - remember to reverse UVTrimCurve for
        // Opposite EU
        let mut eu1_uv_trim_curve =
            eu1_uv_trim_curve.ok_or_else(|| invalid(OP, "rail edge use without UV trim curve"))?;
        let uv_crv_ivl = eu1_uv_trim_curve.natural_interval();
        eu1_uv_trim_curve.reverse_parameterization(env.k, &uv_crv_ivl)?;
        env.pseudo.set_edgeuse_uv_curve(eu1, None, false, false)?;
        env.pseudo
            .set_edgeuse_uv_curve(opposite_prim_eu, Some(eu1_uv_trim_curve), true, true)?;

        // set output
        env.pseudo
            .geom(new_fg)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(|| invalid(OP, "new fillet geometry without fillet surface"))
    }
}

impl FilletCorner {
    /// `SmFilletCorner::RecordFilletError(eErrorType, pFilletSurface1, pFilletSurface2,
    /// pStartFV, pEndFV, cComment)` into a standalone [`ErrorInfo`] (`NoteFilletError`
    /// -> `GetFilletErrorInfo()->NoteError`).
    pub fn record_fillet_error_in<O: Copy + PartialEq>(
        &self,
        error: &mut ErrorInfo<O>,
        error_type: FilletErrorType,
        filleted_vertex: Option<O>,
        surfaces: [Option<O>; 2],
        fillet_vertices: [Option<O>; 2],
        comment: Option<&str>,
    ) {
        error.note_corner_error(
            error_type,
            self.corner_type(),
            self.status,
            Some(self.kind.name()),
            filleted_vertex,
            surfaces,
            fillet_vertices,
            comment,
        );
    }

    /// `SmFilletCorner::RecordFilletError(eErrorType, pFilletSurface1, pFilletSurface2,
    /// pStartFV, pEndFV, cComment)`: tells the fillet executive about a corner error
    /// (`NoteFilletError` with this corner's type, status and name and its filleted
    /// vertex). The objects are the executive's identities: `filleted_vertex` is the
    /// caller's identity of `m_cpVertex` ([`FilletCorner::vertex`]), `surfaces` and
    /// `fillet_vertices` those of the two fillet surfaces and fillet vertices.
    pub fn record_fillet_error<O: ClassObject + SolutionObject + Ord>(
        &self,
        executive: &mut FilletExecutive<O>,
        error_type: FilletErrorType,
        filleted_vertex: Option<O>,
        surfaces: [Option<O>; 2],
        fillet_vertices: [Option<O>; 2],
        comment: Option<&str>,
    ) {
        // Pass the call along to the FilletExectutive
        self.record_fillet_error_in(
            &mut executive.error,
            error_type,
            filleted_vertex,
            surfaces,
            fillet_vertices,
            comment,
        );
    }
}

/// `SER(v.Unitize())`: the unit vector, or the `SM_ERR_INVALID_INPUT` status of a zero
/// vector.
fn unitize_ser(v: Vec3, operation: &'static str) -> Result<Vec3> {
    v.unitize().map_err(|_| SolidError::Upstream {
        operation,
        code: SM_ERR_INVALID_INPUT,
        entity: None,
    })
}

/// A call whose failing status upstream turns into a soft `FALSE` / `SM_ERR` return
/// (`if (SM_SUCCESS != f(..)) return FALSE;`): `None` on failure;
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (undefined upstream) still
/// propagate.
fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }),
        ) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `SM_CAST_PTR(SmBSplineCurve, pCurve)` succeeds: a B-spline or one of the analytic
/// curves derived from it.
pub(super) const fn is_b_spline_kind(c: &Curve) -> bool {
    matches!(
        c.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

/// `pFilletGeom->GetFilletSolver()` in [`CornerPatchEnv::solvers`] (dereferenced).
fn solver_ref<'e>(
    env: &'e CornerPatchEnv<'_, '_>,
    g: FilletGeomId,
    op: &'static str,
) -> Result<&'e FilletSolver> {
    let i = solver_of_geom(env, g, op)?;
    env.solvers
        .get(i)
        .ok_or_else(|| invalid(op, "fillet solver id not in the solver table"))
}

fn solver_of_geom(
    env: &CornerPatchEnv<'_, '_>,
    g: FilletGeomId,
    op: &'static str,
) -> Result<usize> {
    let si = env
        .pseudo
        .geom(g)?
        .fillet_solver()
        .ok_or_else(|| SolidError::invalid_input(op, "fillet geometry without solver"))?;
    usize::try_from(si.0).map_err(|_| SolidError::invalid_input(op, "fillet solver id"))
}

fn invalid(op: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(op, reason)
}

/// `m_vVertices[i]`.
fn vertex_at(vertices: &[VertexId], i: usize, op: &'static str) -> Result<VertexId> {
    vertices
        .get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "vertex index"))
}

/// `SM_CAST_PTR(SmFace, pV->GetPointClassObject())`: the face a fillet vertex is
/// classified on, `None` for another kind or none.
fn point_class_face(pseudo: &FilletBrep, v: VertexId) -> Option<FaceId> {
    match pseudo.fillet_vertex(v).ok()?.point_class_object() {
        Some(EntityRef::Face(f)) => Some(f),
        _ => None,
    }
}

/// The surface of a face of the B-rep being filleted (`GetSurface()`, dereferenced).
fn face_surface<'b>(b: &'b Brep, f: FaceId, op: &'static str) -> Result<&'b Surface> {
    b.face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| SolidError::invalid_input(op, "face without surface"))
}

/// `pEU->GetFilletGeom()`, or its mate's when it has none.
fn edgeuse_or_mate_geom(
    pseudo: &FilletBrep,
    eu: EdgeuseId,
    op: &'static str,
) -> Result<Option<FilletGeomId>> {
    let fg = pseudo.fillet_edgeuse(eu)?.fillet_geom();
    if fg.is_some() {
        return Ok(fg);
    }
    let mate = eu1::mate(pseudo.brep(), eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without mate"))?;
    Ok(pseudo.fillet_edgeuse(mate)?.fillet_geom())
}

/// `pFilletGeom->GetFilletSolver()` of a fillet geometry (dereferenced).
fn geom_solver(pseudo: &FilletBrep, g: FilletGeomId, op: &'static str) -> Result<FilletSolverId> {
    pseudo
        .geom(g)?
        .fillet_solver()
        .ok_or_else(|| SolidError::invalid_input(op, "fillet geometry without solver"))
}

/// The surface of a rail index (`UVPos(lRailIndex)`).
const fn srf_index(rail_index: usize) -> SrfIndex {
    if rail_index == 0 {
        SrfIndex::First
    } else {
        SrfIndex::Second
    }
}

/// `pCurve->Evaluate(t, 1, TRUE, sPV)` (`SER`): the first derivative.
fn tangent_at(k: &Kernel, curve: &Curve, t: f64, op: &'static str) -> Result<Vec3> {
    curve
        .evaluate(k, t, 1, true, true)?
        .get(1)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "evaluation without derivative"))
}

/// `rTsectPnt.UVPos(0) = rFrom.UVPos(0); rTsectPnt.UVPos(1) = rFrom.UVPos(1)` on the
/// intersection point of vertex use `vu`.
fn copy_uv_pos(pseudo: &mut FilletBrep, vu: VertexuseId, from: &TsectPnt) -> Result<()> {
    let t = pseudo.fillet_vertexuse_mut(vu)?.tsect_pnt_mut();
    *t.uv_pos_mut(SrfIndex::First) = from.uv_pos(SrfIndex::First);
    *t.uv_pos_mut(SrfIndex::Second) = from.uv_pos(SrfIndex::Second);
    Ok(())
}

fn null_deref(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP_CONCAVE_TOPO, reason)
}

/// `new (m_pPseudoBrep) SmFilletVertex(this)` of type `SM_FV_RAIL_X_EDGEUSE` with its
/// point, original curve parameter and `SM_FIL_PROCESSED` status.
fn new_rail_x_edgeuse_vertex_in(
    k: &Kernel,
    pseudo: &mut FilletBrep,
    corner: Option<FilletCornerId>,
    point: Point3,
    t: f64,
) -> Result<VertexId> {
    let v = pseudo.brep_mut().add_vertex(point)?;
    let mut fv = FilletVertex::new(k, corner);
    fv.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
    fv.set_original_t_param(t);
    fv.set_status(FilStatus::Processed);
    pseudo.insert_fillet_vertex(v, fv);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_corner::p3::FilletCornerKind;
    use crate::topology::Edgeuse;

    /// A pseudo B-rep with `n` fillet vertices on a circle and `n` curveless fillet edges
    /// joining them in a loop, and a corner of `kind` over them.
    fn loop_corner(kind: FilletCornerKind, n: usize) -> (FilletBrep, FilletCorner) {
        let k = Kernel::default();
        let mut pseudo = FilletBrep::new();
        let mut verts = Vec::new();
        for i in 0..n {
            let a = std::f64::consts::TAU * (i as f64) / (n as f64);
            let v = pseudo
                .brep_mut()
                .add_vertex(Point3::new(a.cos(), a.sin(), 0.0))
                .unwrap();
            let mut fv = FilletVertex::new(&k, None);
            fv.set_fillet_vertex_type(FilletVertexType::RailXRail);
            pseudo.insert_fillet_vertex(v, fv);
            verts.push(v);
        }
        let mut c = FilletCorner::new(kind, None, None, None, 1e-3, 0.1);
        for i in 0..n {
            let e = pseudo
                .make_fillet_edge(Some(verts[i]), Some(verts[(i + 1) % n]), None)
                .unwrap();
            c.edges.push(e);
        }
        c.vertices = verts;
        (pseudo, c)
    }

    fn with_env<T>(pseudo: &mut FilletBrep, run: impl FnOnce(&mut CornerEnv<'_>) -> T) -> T {
        let mut k = Kernel::default();
        let mut target = Brep::new();
        let mut exec = FilletExecutiveData::default();
        let mut env = CornerEnv {
            k: &mut k,
            target: &mut target,
            pseudo,
            exec: &mut exec,
            corner: None,
            solvers: &mut [],
            fillet: None,
        };
        run(&mut env)
    }

    #[test]
    fn analytic_corner_surface_needs_three_or_four_edges() {
        for n in [2, 5] {
            let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, n);
            let r = with_env(&mut pseudo, |env| c.make_analytic_corner_surface(env));
            assert!(!r.unwrap());
            assert!(c.surfaces.is_empty());
        }
    }

    /// An edge without curve exits through `SER(SM_ERR)`, which the caller reads as
    /// `TRUE`: no surface is made.
    #[test]
    fn analytic_corner_surface_reads_a_failing_status_as_true() {
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 3);
        let r = with_env(&mut pseudo, |env| c.make_analytic_corner_surface(env));
        assert!(r.unwrap());
        assert!(c.surfaces.is_empty());
    }

    #[test]
    fn analytic_corner_surface_skips_cubic_rail_rail_interpolation() {
        // (the first edge: an edge without curve before it would exit through SER)
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 4);
        let e = c.edges[0];
        let fe = pseudo.fillet_edge_mut(e).unwrap();
        fe.set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
        fe.set_status(FilStatus::CubicRailRailInterpolate);
        let r = with_env(&mut pseudo, |env| c.make_analytic_corner_surface(env));
        assert!(!r.unwrap());
    }

    #[test]
    fn variable_radius_fillet_needs_four_edges_and_an_interpolation() {
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 3);
        let r = with_env(&mut pseudo, |env| c.make_variable_radius_fillet(env));
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
        // four edges, none a rail-rail interpolation
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 4);
        let r = with_env(&mut pseudo, |env| c.make_variable_radius_fillet(env));
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
    }

    #[test]
    fn convex_nxn_topology_leaves_larger_corners_and_rejects_other_vertices() {
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 4);
        with_env(&mut pseudo, |env| c.adjust_topology_convex_nxn(env)).unwrap();
        assert_eq!((c.vertices.len(), c.edges.len()), (4, 4));
        assert!(!c.topology_adjusted);

        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConvexNByN, 3);
        pseudo
            .fillet_vertex_mut(c.vertices[1])
            .unwrap()
            .set_fillet_vertex_type(FilletVertexType::Mate);
        let r = with_env(&mut pseudo, |env| c.adjust_topology_convex_nxn(env));
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
    }

    /// Three `SM_FV_RAIL_X_RAIL` vertices on faces with no cross sections: no candidate
    /// (the `SM_UNDEF_DOUBLE` iso-edge estimates have equal lengths and no distance).
    #[test]
    fn convex_nxn_topology_without_cross_sections_is_unchanged() {
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::ConcaveNByN, 3);
        for &v in &c.vertices {
            pseudo.fillet_vertex_mut(v).unwrap().set_point_class(
                PointClassType::Face,
                Some(EntityRef::Face(FaceId::from_index(0))),
            );
        }
        with_env(&mut pseudo, |env| c.adjust_topology_convex_nxn(env)).unwrap();
        assert_eq!((c.vertices.len(), c.edges.len()), (3, 3));
        assert!(!c.topology_adjusted);
    }

    #[test]
    fn nx1_closed_tangent_corner_needs_one_filleted_edge_use() {
        let (mut pseudo, mut c) = loop_corner(FilletCornerKind::NByOneClosed, 2);
        with_env(&mut pseudo, |env| c.adjust_tangent_corner(env)).unwrap();
        assert_eq!(c.edges.len(), 2);
    }

    #[test]
    fn empty_edge_use_list_is_invalid_input() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let v = VertexId::from_index(0);
        let r = find_concave_edgeuses(&k, &mut b, v, &[]);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }

    /// The difftest refusal arm: a pair edge use without edge (so without radial) is the
    /// `InvalidInput` of a null dereference upstream, which the runner reports as `refused`.
    #[test]
    fn edge_use_without_edge_is_invalid_input() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let eu = b.add_edgeuse_record(Edgeuse::default()).unwrap();
        let v = VertexId::from_index(0);
        let r = find_concave_edgeuses(&k, &mut b, v, &[eu]);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
    }
}

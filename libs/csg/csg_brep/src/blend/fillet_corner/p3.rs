//! Fillet corners, part 3: corner classification ([`FilletCorner::create`]), the corner
//! constructors, the corner patch builders (default, 3x2 mixed-convexity, Nx1 and Nx2)
//! and the Nx2 corner's topology and tangent-corner adjustments.
//!
//! Upstream: `source/SMLib/src/SmFilletCorner.cpp`, `source/SMLib/inc/SmFilletCorner.h`.
//!
//! | Rust                                               | Upstream                                              |
//! |----------------------------------------------------|-------------------------------------------------------|
//! | [`FilletCorner`], [`FilletCornerKind`], [`FilletCorner::new`] | `SmFilletCorner` members and constructor, the derived classes and their constructors (CO-PORTED from `blend.fillet_corner_p1` / `_p2`, see below) |
//! | [`FilletCorner::new`] with [`FilletCornerKind::TwoByTwo`] | `SmFillet2x2Corner::SmFillet2x2Corner`          |
//! | [`FilletCorner::new`] with [`FilletCornerKind::ConcaveNByN`] | `SmFilletConcaveNxNCorner::SmFilletConcaveNxNCorner` |
//! | [`FilletCorner::new`] with [`FilletCornerKind::ConvexNByN`] | `SmFilletConvexNxNCorner::SmFilletConvexNxNCorner` |
//! | [`FilletCorner::new`] with [`FilletCornerKind::FourByThree`] | `SmFillet4x3Corner::SmFillet4x3Corner`        |
//! | [`FilletCorner::new`] with [`FilletCornerKind::Open`] | `SmFilletOpenCorner::SmFilletOpenCorner`           |
//! | [`FilletCorner::new`] with [`FilletCornerKind::Degenerate`] | `SmFilletDegenerateCorner::SmFilletDegenerateCorner` |
//! | [`FilletCorner::corner_type`], [`FilletCornerKind::name`] | `GetCornerType`, `GetName` (per derived class)   |
//! | [`FilletCorner::create`] | `SmFilletCorner::Create` |
//! | [`FilletCorner::create_with_solver_lookup`] | `Create` with borrowed executive solvers |
//! | [`FilletCorner::calc_corner_geom`]                 | `SmFilletCorner::CalcCornerGeom`                      |
//! | [`FilletCorner::create_corner_patch`]              | `CreateCornerPatch` (virtual: `SmFilletCorner`, `SmFillet3x2MixedConvexityCorner`, `SmFilletNx1Corner`, `SmFilletNx2Corner`) |
//! | [`FilletCorner::create_corner_patch_default_with`], [`CornerPatchEnv`], [`CornerSolverStore`] | `SmFilletCorner::CreateCornerPatch` on a caller's objects (the executive's members it reads) |
//! | [`FilletCorner::adjust_topology`]                  | `AdjustTopology` (virtual; `SmFilletNx2Corner::AdjustTopology` here) |
//! | [`FilletCorner::adjust_tangent_corner`]            | `AdjustTangentCorner` (virtual; `SmFilletNx2Corner::AdjustTangentCorner` here) |
//! | [`FilletCorner::adjust_topology_nx2_with`], [`Nx2AdjustContext`] | `SmFilletNx2Corner::AdjustTopology` (against a live or legacy context) |
//! | [`FilletCorner::is_nx2_tangent_adjustment_corner`] | opening checks of `SmFilletNx2Corner::AdjustTangentCorner` |
//! | [`TangentSplitEnv`], [`TangentSplitOps`] | the executive members the tangent-corner splits reach (legacy or live) |
//! | [`FilletExecutiveData`]                            | the `SmFilletExecutive` members the corner code reads and writes (`m_vEdgeuseSolverMap`, `GetFilletSolverOfEdgeuse`; `m_vExtendedSurfacesMap` for unit `blend.fillet_corner_p1`; CO-PORTED from `blend.fillet_executive_p1`) |
//!
//! Deviations:
//! - **Object model.** The class hierarchy is one [`FilletCorner`] record with a
//!   [`FilletCornerKind`]; the virtual methods dispatch on the kind. The pseudo-B-rep
//!   members (`m_vVertices`, `m_vEdges`) are ids of the executive's [`FilletBrep`];
//!   `m_cpVertex`, `m_pSolverEUs` and `m_pAllEUs` are ids of the B-rep being filleted.
//!   The executive and the fillet solvers are passed explicitly: the executive's
//!   edge-use-to-solver map as [`FilletExecutiveData`] with a solver slice, or a
//!   lookup borrowing its actual solver bases. A corner's identity (`this`, stored by
//!   new fillet vertices and edges) is [`CornerEnv::corner`]. The context is [`Kernel`].
//! - **Constructors.** `SmFilletCorner(...)` (unit `blend.fillet_corner_p1`) and the
//!   constructors of the derived classes owned by `blend.fillet_corner_p2`
//!   (`SmFillet1x1Corner`, `SmFilletNx1ClosedCorner`, `SmFilletNx1Corner`,
//!   `SmFillet3x2MixedConvexityCorner`, `SmFilletNx2Corner`, `SmFilletNxNCorner`) only
//!   initialise members: they are CO-PORTED here as [`FilletCorner::new`] so
//!   [`FilletCorner::create`] can build every kind. `m_eStatus` (never initialised
//!   upstream) starts as [`FilStatus::Unprocessed`]; `SmFilletNx2Corner`'s `m_bG1Case`
//!   (set by its `MakeCornerTopology`) starts as `false`.
//! - **Callees.** `MakeFaceBrep`, `sm_TestG1Continuity`,
//!   `SmFilletNx1Corner::CheckNx1Degeneracy`, `CreateEndPatch` and `CalcCornerVertGeom`
//!   are [`crate::blend::fillet_corner::p1`]'s. `TestAndConvertBigRadFillet`
//!   ([`FilletExecutiveData::test_and_convert_big_rad_fillet`]) and `NoteFilletError`
//!   ([`FilletExecutiveData::error`]) are CO-PORTED from `blend.fillet_executive_p1`.
//!   The corner methods take the kernel through [`CornerEnv::k`]
//!   (mutable: `MakeFaceBrep`'s `SmBrep::MakeFaceWithCurves` needs it). `MakeAnalyticCornerSurface` and
//!   the `SmFilletConvexNxNCorner` / `SmFilletNx1ClosedCorner` overrides are
//!   [`crate::blend::fillet_corner::p2`]'s, `SmFilletEdge::MakeFilletEdge` is
//!   `FilletBrep::make_fillet_edge` and `ReCalcFilletGeom` is
//!   `FilletBrep::re_calc_fillet_geom` on the corner's solvers.
//! - **Nx2 adjustments.** The replaced corner edge of the Nx2 topology adjustment is
//!   deleted from the pseudo B-rep (`FilletBrep::delete_fillet_edge`, upstream's
//!   `delete`). The tangent adjustment ignores the statuses of `CalcCornerVertGeom`,
//!   `ReCalcFilletGeom`, `SetMinMax` and `Trim` as upstream does (only
//!   [`SolidError::Unported`] propagates); a failed `GetRailIndexOfVertex` (which
//!   leaves the index unset upstream) is an error.
//! - **Intersection-point user pointers.** `SmTsectPnt::m_apUserPointer[0..2]` (the two
//!   surfaces of a fillet vertex use's intersection point, read by
//!   `AdjustTangentCorner`) are [`TsectPnt::user_pointer`]. `SmTemporaryChangeValue`
//!   enabling out-of-bounds evaluation of the user surfaces (every `SmBSplineSurface`,
//!   which includes `SmPlane` and `SmSurfOfRevolution`) is applied to copies.
//! - **Topology moves.** `SmVertex::Remove` / `PostInsert` of a vertex use (the
//!   `SmOwningTopology` list operations) are applied to the pseudo B-rep's records
//!   directly; `SmVertex::SetPoint`'s notification resets the vertex uses' cached gaps.
//! - **Undefined inputs.** Where upstream dereferences `NULL` (a missing edge use
//!   neighbour, solver, surface generator, curve or vertex use; `m_pAllEUs` unset) or
//!   reads past an array (`(*pSolverEUs)[0]` of an empty list, the three vertices of a
//!   three-sided patch), the port returns [`SolidError::InvalidInput`] or
//!   `SM_ERR_NULL_POINTER`; a classification object of the wrong kind where upstream
//!   casts blindly is [`SolidError::InvalidInput`]. `SmFilletNx2Corner::CreateCornerPatch`
//!   walks `m_vVertices` down from `GetSize() - 1` with an unsigned index: an empty list
//!   wraps around upstream and is [`SolidError::InvalidInput`] here.
//! - **Iteration.** The walk around the corner vertex in [`FilletCorner::create`]
//!   (next edge use = radial of the clockwise neighbour) is a permutation of the edge
//!   uses, so it returns to its start; it is bounded by the B-rep's edge use count (an
//!   error past it, which only corrupt topology reaches). There is no recursion.

use std::collections::BTreeMap;

use crate::blend::fillet_corner::p1;
use crate::blend::fillet_corner::p2::find_concave_edgeuses;
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilStatus, FilletBrep, FilletCornerId, FilletCornerType, FilletEdgeType,
    FilletErrorType, FilletGeomId, FilletGeomType, FilletSolverId, FilletVertex, FilletVertexType,
};
use crate::blend::fillet_geom::p3::SplitCorners;
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps, FilletSurfaceGeneratorType};
use crate::blend::fillet_solver::surfaces::with_geometry_surfaces;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::global_solver::SolverOperation;
use crate::intersect::point_class::PointClassType;
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, Vec3, BIG_DOUBLE};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::{BlendNeighbour, OrientType};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::{p1 as eu1, p2 as eu2, p3 as eu3};
use crate::topology::{edge, vertex, Brep, EdgeId, EdgeuseId, FaceId, VertexId, VertexuseId};

const OP_CREATE: &str = "blend::fillet_corner::p3::create";
const OP_PATCH: &str = "blend::fillet_corner::p3::create_corner_patch";
const OP_3X2: &str = "blend::fillet_corner::p3::create_corner_patch_3x2";
const OP_NX2_PATCH: &str = "blend::fillet_corner::p3::create_corner_patch_nx2";
const OP_TOPO: &str = "blend::fillet_corner::p3::adjust_topology";
const OP_TANGENT: &str = "blend::fillet_corner::p3::adjust_tangent_corner";
const OP_BIG_RAD: &str = "blend::fillet_executive::p1::test_and_convert_big_rad_fillet";

/// `SM_BIG_DOUBLE`.
const SM_BIG_DOUBLE: f64 = BIG_DOUBLE;

// ---- co-ported records --------------------------------------------------------------

/// The derived class of a fillet corner (`SmFilletCorner` is abstract upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilletCornerKind {
    /// `SmFillet1x1Corner`.
    OneByOne,
    /// `SmFillet2x2Corner` (derives from `SmFillet1x1Corner`).
    TwoByTwo,
    /// `SmFilletNx1ClosedCorner`.
    NByOneClosed,
    /// `SmFilletNx1Corner`.
    NByOne,
    /// `SmFilletDegenerateCorner`.
    Degenerate,
    /// `SmFilletOpenCorner` (derives from `SmFilletNx1Corner`).
    Open,
    /// `SmFillet3x2MixedConvexityCorner`.
    ThreeByTwoMixed,
    /// `SmFilletNx2Corner`.
    NByTwo,
    /// `SmFillet4x3Corner`.
    FourByThree,
    /// `SmFilletConvexNxNCorner` (derives from `SmFilletNxNCorner`).
    ConvexNByN,
    /// `SmFilletConcaveNxNCorner` (derives from `SmFilletConvexNxNCorner`).
    ConcaveNByN,
}

impl FilletCornerKind {
    /// `GetCornerType()` of the derived class.
    #[must_use]
    pub const fn corner_type(self) -> FilletCornerType {
        match self {
            Self::OneByOne => FilletCornerType::OneByOne,
            Self::TwoByTwo => FilletCornerType::TwoByTwo,
            Self::NByOneClosed => FilletCornerType::NByOneClosed,
            Self::NByOne => FilletCornerType::NByOne,
            Self::Degenerate => FilletCornerType::Degenerate,
            Self::Open => FilletCornerType::Open,
            Self::ThreeByTwoMixed => FilletCornerType::ThreeByTwoMixed,
            Self::NByTwo => FilletCornerType::NByTwo,
            Self::FourByThree => FilletCornerType::FourByThree,
            Self::ConvexNByN => FilletCornerType::NByNConvex,
            Self::ConcaveNByN => FilletCornerType::NByNConcave,
        }
    }

    /// `GetName()` of the derived class.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OneByOne => "SmFillet1x1Corner",
            Self::TwoByTwo => "SmFillet2x2Corner",
            Self::NByOneClosed => "SmFilletNx1ClosedCorner",
            Self::NByOne => "SmFilletNx1Corner",
            Self::Degenerate => "SmFilletDegenerateCorner",
            Self::Open => "SmFilletOpenCorner",
            Self::ThreeByTwoMixed => "SmFillet3x2MixedConvexityCorner",
            Self::NByTwo => "SmFilletNx2Corner",
            Self::FourByThree => "SmFillet4x3Corner",
            Self::ConvexNByN => "SmFilletConvexNxNCorner",
            Self::ConcaveNByN => "SmFilletConcaveNxNCorner",
        }
    }
}

/// An object identity recorded in [`FilletExecutiveData::error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutiveErrorObject {
    /// A vertex of the B-rep being filleted (`m_cpVertex`).
    TargetVertex(VertexId),
    /// A fillet vertex of the pseudo B-rep (`SmFilletVertex*`).
    FilletVertex(VertexId),
    /// The fillet surface of a [`FilletGeomId`] (`pFilletGeom->GetFilletSurface()`).
    FilletGeomSurface(FilletGeomId),
    /// A target face's surface (`extended = false`) or registered extended surface
    /// (`extended = true`, `m_vExtendedSurfacesMap`).
    TargetFaceSurface {
        /// Target face id.
        face: FaceId,
        /// Whether the surface came from `extended_surfaces_map`.
        extended: bool,
    },
    /// The temporary planar end-face surface constructed by `CalcFilletIntSideFace`
    /// when `!corner.is_extended_surface_patch()`.
    TempEndPlane(EdgeId),
}

/// The members of `SmFilletExecutive` read and written by the corner code (CO-PORTED from
/// `blend.fillet_executive_p1`).
#[derive(Debug, Clone, Default)]
pub struct FilletExecutiveData {
    /// `m_vEdgeuseSolverMap`: the fillet solver of each filleted edge use.
    pub edgeuse_solver_map: BTreeMap<EdgeuseId, FilletSolverId>,
    /// `m_vExtendedSurfacesMap`: the extended surface registered for a face of the
    /// original B-rep (`SmFilletCorner::AddExtendedOriginalSurface` /
    /// `GetExtendedSurface`, unit `blend.fillet_corner_p1`).
    pub extended_surfaces_map: BTreeMap<FaceId, Surface>,
    /// `m_bDoGlobalMerge` (read by `SmFilletCorner::CalcCornerEdgeGeom`).
    pub do_global_merge: bool,
    /// `m_vTopoEdges`: the fillet edges recorded for the topology insertion, in order
    /// (`None` keeps a null entry).
    pub topo_edges: Vec<Option<EdgeId>>,
    /// `m_sErrorInfo`: fillet error state recorded by `NoteFilletError` /
    /// `RecordFilletError`.
    pub error: ErrorInfo<ExecutiveErrorObject>,
    /// Corner arena indexed by [`FilletCornerId`] (`SmFilletCorner*` storage for
    /// [`FilletExecutiveData::test_and_convert_big_rad_fillet`]).
    pub corners: BTreeMap<FilletCornerId, FilletCorner>,
    /// `m_vFilletCorners`: ordered active corner ids in the executive.
    pub fillet_corners: Vec<FilletCornerId>,
    /// `m_vVertexCornerMap`: target B-rep vertex -> active corner id.
    pub vertex_corner_map: BTreeMap<VertexId, FilletCornerId>,
    /// `m_vFilletSolvers`: ordered active solver ids in the executive.
    pub fillet_solvers: Vec<FilletSolverId>,
    /// Fillet geometries deleted by `~SmFilletSolver` during
    /// [`FilletExecutiveData::test_and_convert_big_rad_fillet`].
    pub deleted_geoms: Vec<FilletGeomId>,
    /// `m_pAttributeE->GetUsers()`: the fillet-B-rep edges carrying the executive's
    /// edge attribute (`SM_AI_ALL_FILLET_EDGES`), in the order they were added. The
    /// executive's constructor creates the attribute with no users; `CreateFilletBrep`
    /// adds it to every edge of the fillet B-rep.
    pub attribute_e_users: Vec<EdgeId>,
}

impl FilletExecutiveData {
    /// `SmFilletExecutive::GetFilletSolverOfEdgeuse`: the solver registered for `eu`,
    /// `None` when there is none.
    #[must_use]
    pub fn fillet_solver_of_edgeuse(&self, eu: EdgeuseId) -> Option<FilletSolverId> {
        self.edgeuse_solver_map.get(&eu).copied()
    }

    /// `SmFilletExecutive::RemoveFromTopoEdges` (CO-PORTED from
    /// `blend.fillet_executive_p1`): drops the first entry equal to `e`, if any.
    pub fn remove_from_topo_edges(&mut self, e: EdgeId) {
        if let Some(i) = self.topo_edges.iter().position(|&x| x == Some(e)) {
            self.topo_edges.remove(i);
        }
    }

    /// `SmFilletExecutive::TestAndConvertBigRadFillet` (CO-PORTED from
    /// `blend.fillet_executive_p1`): tests whether `fg` between two `Nx2` blending
    /// corners twists back on itself (`dLen > dTol` on both rails and opposite rail
    /// directions) and, if so, squeezes a mate rail, recomputes the merged rail-rail
    /// vertex, converts the two `Nx2` corners and remaining rail into a single
    /// `SmFilletConvexNxNCorner`, updates `fillet_corners` and `vertex_corner_map`, and
    /// deletes the two original corners and `fg`'s solver.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without corner or filleted vertex; `SM_ERR` when neither
    /// rail has two `SM_FV_MATE` vertices, an original corner does not have one edge, or
    /// the corners/solver are missing from the executive's arrays;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` or aliases freed
    /// objects.
    pub fn test_and_convert_big_rad_fillet(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        solvers: &mut [FilletSolver],
        fg: FilletGeomId,
    ) -> Result<bool> {
        self.test_and_convert_big_rad_fillet_with_running(k, target, pseudo, solvers, fg, None)
    }

    /// [`FilletExecutiveData::test_and_convert_big_rad_fillet`] when a corner (`running`)
    /// is currently borrowed outside `self.corners` (as during
    /// [`FilletCorner::adjust_topology_nx2`]).
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub(crate) fn test_and_convert_big_rad_fillet_with_running(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        solvers: &mut [FilletSolver],
        fg: FilletGeomId,
        mut running: Option<(FilletCornerId, &mut FilletCorner)>,
    ) -> Result<bool> {
        fn corner_of<'a>(
            corners: &'a BTreeMap<FilletCornerId, FilletCorner>,
            running: Option<&'a (FilletCornerId, &mut FilletCorner)>,
            id: FilletCornerId,
        ) -> Result<&'a FilletCorner> {
            if let Some((rid, rc)) = running {
                if *rid == id {
                    return Ok(&**rc);
                }
            }
            corners
                .get(&id)
                .ok_or_else(|| invalid(OP_BIG_RAD, p1::NULL_DEREF))
        }
        let null = || invalid(OP_BIG_RAD, p1::NULL_DEREF);

        let mut start_v = [VertexId::from_index(0); 2];
        let mut end_v = [VertexId::from_index(0); 2];
        let mut rails = [EdgeId::from_index(0); 2];
        let mut start_corner_id = FilletCornerId(0);
        let mut end_corner_id = FilletCornerId(0);
        let mut rail_dir = [Vec3::ZERO; 2];

        for ii in 0..2usize {
            let rail = pseudo
                .geom(fg)
                .map_err(|_| null())?
                .rails
                .get(ii)
                .copied()
                .flatten()
                .ok_or_else(null)?;
            if let Some(slot) = rails.get_mut(ii) {
                *slot = rail;
            }
            let prim_eu = eu1::primary_edgeuse(pseudo.brep(), rail)?.ok_or_else(null)?;
            let mate_eu = eu1::mate(pseudo.brep(), prim_eu)?.ok_or_else(null)?;
            let prim_vu = pseudo.brep().edgeuse(prim_eu)?.vertexuse.ok_or_else(null)?;
            let mate_vu = pseudo.brep().edgeuse(mate_eu)?.vertexuse.ok_or_else(null)?;
            let sv = pseudo.brep().vertexuse(prim_vu)?.vertex.ok_or_else(null)?;
            let ev = pseudo.brep().vertexuse(mate_vu)?.vertex.ok_or_else(null)?;
            pseudo.fillet_vertex(sv).map_err(|_| null())?;
            pseudo.fillet_vertex(ev).map_err(|_| null())?;
            if let Some(slot) = start_v.get_mut(ii) {
                *slot = sv;
            }
            if let Some(slot) = end_v.get_mut(ii) {
                *slot = ev;
            }

            if ii == 0 {
                start_corner_id = pseudo
                    .fillet_vertex(sv)?
                    .fillet_corner()
                    .ok_or_else(|| SolidError::null_pointer(OP_BIG_RAD))?;
                end_corner_id = pseudo
                    .fillet_vertex(ev)?
                    .fillet_corner()
                    .ok_or_else(|| SolidError::null_pointer(OP_BIG_RAD))?;
                let sc = corner_of(&self.corners, running.as_ref(), start_corner_id)?;
                let ec = corner_of(&self.corners, running.as_ref(), end_corner_id)?;
                if sc.topology_adjusted || ec.topology_adjusted {
                    return Ok(false);
                }
            }
            let sc_type =
                corner_of(&self.corners, running.as_ref(), start_corner_id)?.corner_type();
            let ec_type = corner_of(&self.corners, running.as_ref(), end_corner_id)?.corner_type();
            if sc_type != FilletCornerType::NByTwo
                || ec_type != FilletCornerType::NByTwo
                || pseudo.fillet_vertex(sv)?.status() != FilStatus::Processed
                || pseudo.fillet_vertex(ev)?.status() != FilStatus::Processed
            {
                return Ok(false);
            }
            let pnt1 = pseudo.vertex_point(sv)?;
            let pnt2 = pseudo.vertex_point(ev)?;
            if let Some(dir) = rail_dir.get_mut(ii) {
                *dir = pnt2 - pnt1;
            }
        }

        let len1 = rail_dir[0].length();
        let len2 = rail_dir[1].length();
        let fs_id = pseudo.geom(fg)?.fillet_solver().ok_or_else(null)?;
        let fs_idx = usize::try_from(fs_id.0)
            .ok()
            .filter(|&i| i < solvers.len())
            .ok_or_else(null)?;
        let tol = solvers.get(fs_idx).ok_or_else(null)?.approx_tol_3d;
        let tan_tol = solvers.get(fs_idx).ok_or_else(null)?.tangency_tolerance;
        if len1 <= tol || len2 <= tol || rail_dir[0].dot(rail_dir[1]) > 0.0 {
            return Ok(false);
        }

        let mut squeeze = 0usize;
        if pseudo.fillet_vertex(start_v[0])?.fillet_vertex_type() != FilletVertexType::Mate
            || pseudo.fillet_vertex(end_v[0])?.fillet_vertex_type() != FilletVertexType::Mate
        {
            squeeze = 1;
            if pseudo.fillet_vertex(start_v[1])?.fillet_vertex_type() != FilletVertexType::Mate
                || pseudo.fillet_vertex(end_v[1])?.fillet_vertex_type() != FilletVertexType::Mate
            {
                return Err(SolidError::sm_err(OP_BIG_RAD));
            }
        }

        let other = 1 - squeeze;
        let sv_sq = *start_v.get(squeeze).ok_or_else(null)?;
        let ev_sq = *end_v.get(squeeze).ok_or_else(null)?;
        let sv_ot = *start_v.get(other).ok_or_else(null)?;
        let ev_ot = *end_v.get(other).ok_or_else(null)?;
        // Self-aliasing on synthetic topology would UAF or double-free in C++.
        if rails[0] == rails[1]
            || sv_sq == ev_sq
            || ev_sq == sv_ot
            || ev_sq == ev_ot
            || start_corner_id == end_corner_id
        {
            return Err(null());
        }

        let squeezed_rail = *rails.get(squeeze).ok_or_else(null)?;
        pseudo.delete_fillet_edge(squeezed_rail)?;
        pseudo.geom_mut(fg)?.set_rail(squeeze, None);

        let vus = pseudo.brep().vertex(ev_sq)?.vertexuses.clone();
        for vu in vus {
            move_vertexuse(pseudo, vu, ev_sq, sv_sq)?;
        }
        pseudo.delete_fillet_vertex(ev_sq)?;

        pseudo
            .fillet_vertex_mut(sv_sq)?
            .set_fillet_vertex_type(FilletVertexType::RailXRail);
        for ii in 0..2usize {
            let mate = pseudo.fillet_vertex(sv_sq)?.mate(ii);
            if let Some(m_id) = mate {
                if let Ok(mv) = pseudo.fillet_vertex_mut(m_id) {
                    if mv.mate(0) == Some(sv_sq) {
                        mv.set_mate(0, None);
                    } else if mv.mate(1) == Some(sv_sq) {
                        mv.set_mate(1, None);
                    }
                }
            }
        }
        pseudo.fillet_vertex_mut(sv_sq)?.set_mate(0, None);
        pseudo.fillet_vertex_mut(sv_sq)?.set_mate(1, None);
        pseudo
            .fillet_vertex_mut(sv_sq)?
            .set_status(FilStatus::Unprocessed);

        // Compute vertex geometry (upstream ignores the returned SmStatus)
        {
            let lookup = match running.as_ref() {
                Some((rid, rc)) => CornerLookup::of_corner(Some(*rid), rc, self),
                None => CornerLookup::of_executive(self),
            };
            match pseudo.calc_rail_int_rail(k, sv_sq, lookup, solvers, target) {
                Ok(()) | Err(SolidError::Upstream { .. }) => {}
                Err(e) => return Err(e),
            }
        }

        // Make remaining rail an edge for the new corner
        let kept_rail = *rails.get(other).ok_or_else(null)?;
        pseudo
            .fillet_edge_mut(kept_rail)?
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        let prim_eu1 = eu1::primary_edgeuse(pseudo.brep(), kept_rail)?.ok_or_else(null)?;
        let mate_eu1 = eu1::mate(pseudo.brep(), prim_eu1)?.ok_or_else(null)?;
        pseudo.set_edgeuse_fillet_geom(prim_eu1, None)?;
        pseudo.set_edgeuse_fillet_geom(mate_eu1, None)?;
        pseudo.geom_mut(fg)?.set_rail(other, None);

        let next_raw = self
            .corners
            .keys()
            .copied()
            .chain(self.fillet_corners.iter().copied())
            .chain(running.as_ref().map(|&(rid, _)| rid))
            .map(|id| id.0.saturating_add(1))
            .max()
            .unwrap_or(0);
        let new_corner_id = FilletCornerId(next_raw);
        let mut new_corner =
            FilletCorner::new(FilletCornerKind::ConvexNByN, None, None, None, tol, tan_tol);

        pseudo
            .fillet_vertex_mut(sv_sq)?
            .set_fillet_corner(Some(new_corner_id));
        pseudo
            .fillet_vertex_mut(sv_ot)?
            .set_fillet_corner(Some(new_corner_id));
        pseudo
            .fillet_vertex_mut(ev_ot)?
            .set_fillet_corner(Some(new_corner_id));
        new_corner.vertices.push(sv_ot);
        new_corner.vertices.push(ev_ot);
        new_corner.vertices.push(sv_sq);

        pseudo
            .fillet_edge_mut(kept_rail)?
            .set_fillet_corner(Some(new_corner_id));
        new_corner.edges.push(kept_rail);

        let end_edges = corner_of(&self.corners, running.as_ref(), end_corner_id)?
            .edges
            .clone();
        if end_edges.len() != 1 {
            return Err(SolidError::sm_err(OP_BIG_RAD));
        }
        let end_cs = *end_edges
            .first()
            .ok_or_else(|| SolidError::sm_err(OP_BIG_RAD))?;
        let end_cs_v = edge::vertex(pseudo.brep(), end_cs).map_err(|_| null())?;
        if end_cs_v != Some(ev_ot) {
            reverse_fillet_edge(pseudo, end_cs, OP_BIG_RAD)?;
        }
        modify_corneredge_fillet_relation(pseudo, end_cs, fg, OP_BIG_RAD)?;
        clear_corner_vertices(&mut self.corners, &mut running, end_corner_id);
        pseudo
            .fillet_edge_mut(end_cs)?
            .set_fillet_corner(Some(new_corner_id));
        new_corner.edges.push(end_cs);

        let start_edges = corner_of(&self.corners, running.as_ref(), start_corner_id)?
            .edges
            .clone();
        if start_edges.len() != 1 {
            return Err(SolidError::sm_err(OP_BIG_RAD));
        }
        let start_cs = *start_edges
            .first()
            .ok_or_else(|| SolidError::sm_err(OP_BIG_RAD))?;
        let start_cs_v = edge::vertex(pseudo.brep(), start_cs).map_err(|_| null())?;
        if start_cs_v != Some(sv_sq) {
            reverse_fillet_edge(pseudo, start_cs, OP_BIG_RAD)?;
        }
        modify_corneredge_fillet_relation(pseudo, start_cs, fg, OP_BIG_RAD)?;
        clear_corner_vertices(&mut self.corners, &mut running, start_corner_id);
        pseudo
            .fillet_edge_mut(start_cs)?
            .set_fillet_corner(Some(new_corner_id));
        new_corner.edges.push(start_cs);

        let Some(mut idx1) = self
            .fillet_corners
            .iter()
            .position(|&c| c == start_corner_id)
        else {
            return Err(SolidError::sm_err(OP_BIG_RAD));
        };
        let Some(mut idx2) = self.fillet_corners.iter().position(|&c| c == end_corner_id) else {
            return Err(SolidError::sm_err(OP_BIG_RAD));
        };
        if idx1 > idx2 {
            std::mem::swap(&mut idx1, &mut idx2);
        }
        let orig_vert_s =
            corner_of(&self.corners, running.as_ref(), start_corner_id)?.filleted_vertex();
        let orig_vert_e =
            corner_of(&self.corners, running.as_ref(), end_corner_id)?.filleted_vertex();
        if let Some(slot) = self.fillet_corners.get_mut(idx1) {
            *slot = new_corner_id;
        }
        if idx2 < self.fillet_corners.len() {
            self.fillet_corners.remove(idx2);
        }
        let orig_vert_s = orig_vert_s.ok_or_else(|| SolidError::null_pointer(OP_BIG_RAD))?;
        let orig_vert_e = orig_vert_e.ok_or_else(|| SolidError::null_pointer(OP_BIG_RAD))?;
        self.vertex_corner_map.insert(orig_vert_s, new_corner_id);
        self.vertex_corner_map.insert(orig_vert_e, new_corner_id);

        self.corners.remove(&start_corner_id);
        self.corners.remove(&end_corner_id);

        let Some(s_idx) = self.fillet_solvers.iter().position(|&s| s == fs_id) else {
            return Err(SolidError::sm_err(OP_BIG_RAD));
        };
        self.fillet_solvers.remove(s_idx);
        if let Some(fs) = solvers.get_mut(fs_idx) {
            let geoms = std::mem::take(&mut fs.fillet_geoms);
            for gid in geoms {
                if let Ok(g) = pseudo.geom_mut(gid) {
                    g.center_line_curve = None;
                    let g_verts = std::mem::take(&mut g.vertices);
                    g.off_surfs = [None, None];
                    for v in g_verts {
                        if pseudo.fillet_vertex(v).is_ok() {
                            pseudo.delete_fillet_vertex(v)?;
                        }
                    }
                }
                self.deleted_geoms.push(gid);
            }
            fs.surfaces = [None, None];
        }

        self.corners.insert(new_corner_id, new_corner);
        Ok(true)
    }
}

/// A fillet corner (`SmFilletCorner` and its derived classes).
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // upstream's independent SmBoolean members
pub struct FilletCorner {
    /// The derived class.
    pub kind: FilletCornerKind,
    /// `m_cpVertex`: the vertex being filleted (B-rep being filleted).
    pub vertex: Option<VertexId>,
    /// `m_pSolverEUs`: the outward edge uses around the corner that are filleted.
    pub solver_eus: Option<Vec<EdgeuseId>>,
    /// `m_pAllEUs`: all outward edge uses around the corner.
    pub all_eus: Option<Vec<EdgeuseId>>,
    /// `m_vVertices`: the fillet vertices of the corner (pseudo B-rep).
    pub vertices: Vec<VertexId>,
    /// `m_vEdges`: the fillet edges around the corner (pseudo B-rep).
    pub edges: Vec<EdgeId>,
    /// `m_vSurfaces`: the surfaces built for the corner (`SM_FILLETSURF_TYPE` is
    /// `SmSurface`: an end patch copied with `CopyAndAddAnalytics` can be analytic).
    pub surfaces: Vec<Surface>,
    /// The fillet-B-rep faces `MakeFaceBrep` built on [`Self::surfaces`], in order
    /// (upstream the faces own the `m_vSurfaces` pointers, so the registered corner
    /// patch surfaces are those faces' surfaces).
    pub surface_faces: Vec<FaceId>,
    /// `m_bBevel`.
    pub bevel: bool,
    /// `m_bBlending`.
    pub blending: bool,
    /// `m_bChamfer`.
    pub chamfer: bool,
    /// `m_bDegenerate`.
    pub degenerate: bool,
    /// `m_bCalcAsConstRad`.
    pub calc_as_const_rad: bool,
    /// `m_bTopologyAdjusted`.
    pub topology_adjusted: bool,
    /// `m_bExtendedSurfacePatch`.
    pub extended_surface_patch: bool,
    /// `m_dSetBackDist`.
    pub set_back_dist: f64,
    /// `m_dThisApproxTol3d`.
    pub approx_tol_3d: f64,
    /// `m_dTangencyTolRadians`.
    pub tangency_tol_radians: f64,
    /// `m_eStatus` (not initialised upstream).
    pub status: FilStatus,
    /// `m_bG1Case` of `SmFilletNx1ClosedCorner` / `SmFilletNx2Corner`.
    pub g1_case: bool,
}

/// What the corner methods operate on besides the corner itself.
#[derive(Debug)]
pub struct CornerEnv<'a> {
    /// `m_crContext`.
    pub k: &'a mut Kernel,
    /// The B-rep being filleted (`m_cpVertex`, the edge uses, original faces).
    pub target: &'a mut Brep,
    /// `m_pPseudoBrep`.
    pub pseudo: &'a mut FilletBrep,
    /// `m_pExecutive`'s members the corner methods read and write.
    pub exec: &'a mut FilletExecutiveData,
    /// The corner's identity (`this`, as new fillet vertices and edges store it).
    pub corner: Option<FilletCornerId>,
    /// The fillet solvers the geometries' `GetFilletSolver()` ids index (`id.0`).
    pub solvers: &'a mut [FilletSolver],
    /// `m_pExecutive->m_pFilletBrep`, the B-rep `MakeFaceBrep` builds the corner's faces
    /// in (`None`: `NULL`).
    pub fillet: Option<CornerFilletBrep<'a>>,
}

/// The solver store the corner-patch builders resolve fillet geometries' solvers
/// through (`pFilletGeom->GetFilletSolver()`, indexed by `FilletSolverId.0`).
pub trait CornerSolverStore {
    /// The solver at `index`, or `None` when it is not in the store.
    fn get(&self, index: usize) -> Option<&FilletSolver>;
    /// The number of solver slots (the identity a temporary solver takes).
    fn len(&self) -> usize;
    /// Whether the store has no slots.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A plain solver table as a [`CornerSolverStore`].
#[derive(Debug, Clone, Copy)]
pub struct IndexedSolvers<'s>(pub &'s [FilletSolver]);

impl CornerSolverStore for IndexedSolvers<'_> {
    fn get(&self, index: usize) -> Option<&FilletSolver> {
        self.0.get(index)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// What the corner-patch builders (`CreateCornerPatch`, `MakeAnalyticCornerSurface`,
/// `MakeFaceBrep` and their helpers) operate on besides the corner itself: the
/// legacy [`CornerEnv`] ([`CornerEnv::with_patch_env`]) or a live executive session.
pub struct CornerPatchEnv<'e, 'a> {
    /// `m_crContext`.
    pub k: &'e mut Kernel,
    /// The B-rep being filleted.
    pub target: &'e Brep,
    /// `m_pPseudoBrep`.
    pub pseudo: &'e mut FilletBrep,
    /// The executive's solvers.
    pub solvers: &'e dyn CornerSolverStore,
    /// `m_pExecutive->m_pFilletBrep` (`None`: `NULL`).
    pub fillet: Option<&'e mut CornerFilletBrep<'a>>,
    /// `FILEXEC_CORNER_ERR(this, "Corner Face Creation Error")`: notes the failed
    /// corner face on the executive.
    pub note_face_error: &'e mut dyn FnMut(&FilletCorner),
}

impl std::fmt::Debug for CornerPatchEnv<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CornerPatchEnv")
            .field("target", &self.target)
            .field("pseudo", &self.pseudo)
            .field("fillet", &self.fillet)
            .finish_non_exhaustive()
    }
}

impl CornerEnv<'_> {
    /// Run `f` on this environment's corner-patch view (its executive's error notes
    /// the failed corner face).
    ///
    /// # Errors
    /// Those of `f`.
    pub fn with_patch_env<T>(
        &mut self,
        f: impl FnOnce(&mut CornerPatchEnv<'_, '_>) -> Result<T>,
    ) -> Result<T> {
        let CornerEnv {
            k,
            target,
            pseudo,
            exec,
            solvers,
            fillet,
            ..
        } = self;
        let mut note = |corner: &FilletCorner| {
            exec.error.note_corner_error(
                FilletErrorType::VertexProblem,
                corner.corner_type(),
                FilStatus::Failure,
                Some(corner.kind.name()),
                corner
                    .filleted_vertex()
                    .map(ExecutiveErrorObject::TargetVertex),
                [None, None], // For now, we don't generally have just two surfaces.
                [None, None], // For now, we don't generally have just two edgeuses.
                Some("Fillet Corner Error: Corner Face Creation Error"),
            );
        };
        let store = IndexedSolvers(solvers);
        let mut env = CornerPatchEnv {
            k,
            target,
            pseudo,
            solvers: &store,
            fillet: fillet.as_mut(),
            note_face_error: &mut note,
        };
        f(&mut env)
    }
}

/// The fillet B-rep of [`CornerEnv::fillet`] and the notification environment of its
/// entities (the face makers notify the new faces, edges and edge uses).
#[derive(Debug)]
pub struct CornerFilletBrep<'a> {
    /// The B-rep (`m_pFilletBrep`).
    pub brep: &'a mut Brep,
    /// The notification environment of `brep`'s entities.
    pub notify: edge::NotifyEnv<'a>,
}

/// What `SmFilletNx2Corner::AdjustTopology` reaches besides the corner itself.
pub trait Nx2AdjustContext {
    /// The kernel, the target B-rep, the pseudo-B-rep and the corner's identity.
    fn parts(&mut self) -> (&Kernel, &mut Brep, &mut FilletBrep, Option<FilletCornerId>);
    /// `SmFilletNx2Corner::AdjustConcaveCornerTopology` on `corner`.
    ///
    /// # Errors
    /// Those of the concave adjustment.
    fn adjust_concave_corner_topology(&mut self, corner: &mut FilletCorner) -> Result<()>;
    /// `m_pExecutive->TestAndConvertBigRadFillet(fg, this)`.
    ///
    /// # Errors
    /// Those of the conversion.
    fn test_and_convert_big_rad_fillet(
        &mut self,
        corner: &mut FilletCorner,
        fg: FilletGeomId,
    ) -> Result<bool>;
}

impl Nx2AdjustContext for CornerEnv<'_> {
    fn parts(&mut self) -> (&Kernel, &mut Brep, &mut FilletBrep, Option<FilletCornerId>) {
        (&*self.k, &mut *self.target, &mut *self.pseudo, self.corner)
    }
    fn adjust_concave_corner_topology(&mut self, corner: &mut FilletCorner) -> Result<()> {
        corner.adjust_concave_corner_topology(self)
    }
    fn test_and_convert_big_rad_fillet(
        &mut self,
        corner: &mut FilletCorner,
        fg: FilletGeomId,
    ) -> Result<bool> {
        let this_id = self.corner;
        self.exec.test_and_convert_big_rad_fillet_with_running(
            self.k,
            self.target,
            self.pseudo,
            self.solvers,
            fg,
            this_id.map(|id| (id, corner)),
        )
    }
}

fn invalid(op: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(op, reason)
}

fn clear_corner_vertices(
    corners: &mut BTreeMap<FilletCornerId, FilletCorner>,
    running: &mut Option<(FilletCornerId, &mut FilletCorner)>,
    id: FilletCornerId,
) {
    if let Some((rid, ref mut rc)) = *running {
        if rid == id {
            rc.vertices.clear();
            return;
        }
    }
    if let Some(c) = corners.get_mut(&id) {
        c.vertices.clear();
    }
}

/// `SM_REVERSE_FILLET_EDGE(e)`.
pub(crate) fn reverse_fillet_edge(
    pseudo: &mut FilletBrep,
    e: EdgeId,
    op: &'static str,
) -> Result<()> {
    let prim_eu =
        eu1::primary_edgeuse(pseudo.brep(), e)?.ok_or_else(|| invalid(op, p1::NULL_DEREF))?;
    let brep = pseudo.brep_mut();
    brep.edge_mut(e)?.edgeuses.retain(|&x| x != prim_eu);
    brep.edgeuse_mut(prim_eu)?.edge = Some(e);
    brep.edge_mut(e)?.edgeuses.push(prim_eu);
    Ok(())
}

/// `SM_MODIFY_CORNEREDGE_FILLET_RELATION(e, fg)`.
pub(crate) fn modify_corneredge_fillet_relation(
    pseudo: &mut FilletBrep,
    e: EdgeId,
    fg: FilletGeomId,
    op: &'static str,
) -> Result<()> {
    let prim_eu =
        eu1::primary_edgeuse(pseudo.brep(), e)?.ok_or_else(|| invalid(op, p1::NULL_DEREF))?;
    let mate_eu = eu1::mate(pseudo.brep(), prim_eu)?.ok_or_else(|| invalid(op, p1::NULL_DEREF))?;
    if pseudo.fillet_edgeuse(prim_eu)?.fillet_geom() == Some(fg) {
        let mate_fg = pseudo.fillet_edgeuse(mate_eu)?.fillet_geom();
        pseudo.set_edgeuse_fillet_geom(prim_eu, mate_fg)?;
    }
    pseudo.set_edgeuse_fillet_geom(mate_eu, None)?;
    Ok(())
}

/// `SmVertex::SetPoint` on a pseudo-B-rep vertex: the notification resets the cached
/// gaps of every vertex use, then the point is stored.
pub(super) fn set_vertex_point(pseudo: &mut FilletBrep, v: VertexId, point: Point3) -> Result<()> {
    let brep = pseudo.brep_mut();
    for vu in brep.vertex(v)?.vertexuses.clone() {
        let rec = brep.vertexuse_mut(vu)?;
        rec.vertex_edge_gap.reset();
        rec.vertex_face_gap.reset();
    }
    brep.vertex_mut(v)?.point = point;
    Ok(())
}

/// `pFrom->Remove(pVU); pTo->PostInsert(pVU)` for a vertex use of the pseudo B-rep.
pub(crate) fn move_vertexuse(
    pseudo: &mut FilletBrep,
    vu: VertexuseId,
    from: VertexId,
    to: VertexId,
) -> Result<()> {
    let brep = pseudo.brep_mut();
    brep.vertex_mut(from)?.vertexuses.retain(|&x| x != vu);
    brep.vertexuse_mut(vu)?.vertex = Some(to);
    brep.vertex_mut(to)?.vertexuses.push(vu);
    Ok(())
}

/// The `SmBSplineCurve` part of a curve (`SM_CAST_PTR(SmBSplineCurve, ...)`: the
/// analytic curves derive from it upstream).
fn bspline_part(curve: &Curve) -> Option<BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Some(c.clone()),
        CurveKind::Line(c) => Some(c.base.clone()),
        CurveKind::Ellipse(c) => Some(c.base().clone()),
        CurveKind::Hyperbola(c) => Some(c.base().clone()),
        CurveKind::Parabola(c) => Some(c.base().clone()),
        _ => None,
    }
}

/// The primary edge use of a pseudo-B-rep edge (`GetPrimaryEdgeuse`), required.
pub(super) fn primary_eu(pseudo: &FilletBrep, e: EdgeId, op: &'static str) -> Result<EdgeuseId> {
    eu1::primary_edgeuse(pseudo.brep(), e)?.ok_or_else(|| SolidError::null_pointer(op))
}

impl FilletCorner {
    /// `SmFilletCorner(crContext, cpVertex, pSolverEUs, pAllEUs, pExec, dApproxTol3d,
    /// dTangencyTolRadians)` and the derived constructors (CO-PORTED, module docs):
    /// no fillet vertices, edges or surfaces, every flag off except
    /// `m_bExtendedSurfacePatch`, no setback; the degenerate corner sets
    /// `m_bDegenerate`, the closed Nx1 corner clears `m_bG1Case`.
    #[must_use]
    pub fn new(
        kind: FilletCornerKind,
        vertex: Option<VertexId>,
        solver_eus: Option<Vec<EdgeuseId>>,
        all_eus: Option<Vec<EdgeuseId>>,
        approx_tol_3d: f64,
        tangency_tol_radians: f64,
    ) -> Self {
        Self {
            kind,
            vertex,
            solver_eus,
            all_eus,
            vertices: Vec::new(),
            edges: Vec::new(),
            surfaces: Vec::new(),
            surface_faces: Vec::new(),
            bevel: false,
            blending: false,
            chamfer: false,
            // SmFilletDegenerateCorner: m_bDegenerate = TRUE
            degenerate: kind == FilletCornerKind::Degenerate,
            calc_as_const_rad: false,
            topology_adjusted: false,
            extended_surface_patch: true,
            set_back_dist: 0.0,
            approx_tol_3d,
            tangency_tol_radians,
            status: FilStatus::Unprocessed,
            // SmFilletNx1ClosedCorner: m_bG1Case = FALSE
            g1_case: false,
        }
    }

    /// `GetCornerType()`.
    #[must_use]
    pub const fn corner_type(&self) -> FilletCornerType {
        self.kind.corner_type()
    }

    // ---- SmFilletCorner::Create ------------------------------------------------------

    /// `SmFilletCorner::Create`: the corner for `vertex` whose kind follows from the
    /// number of edges around the vertex and how many of them are filleted
    /// (`solver_eus`, outward edge uses at the vertex). Its tolerances are the smallest
    /// approximation and tangency tolerances of the solvers of `solver_eus`; it is a
    /// chamfer corner when all of them generate linear cross sections. `None` for
    /// corner cases upstream does not implement.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty `solver_eus`, an edge use without a
    /// clockwise neighbour, radial partner or edge, a filleted edge use without a
    /// solver (or a solver id past `solvers`) or a solver without surface generator
    /// (upstream dereferences them), and a walk that does not close; those of the
    /// sector convexity test; [`SolidError::Unported`] (module docs).
    pub fn create(
        k: &Kernel,
        b: &mut Brep,
        vertex: VertexId,
        solver_eus: &[EdgeuseId],
        exec: &FilletExecutiveData,
        solvers: &[FilletSolver],
    ) -> Result<Option<Self>> {
        Self::create_with_solver_lookup(k, b, vertex, solver_eus, |edge_use| {
            exec.fillet_solver_of_edgeuse(edge_use)
                .and_then(|id| usize::try_from(id.0).ok())
                .and_then(|i| solvers.get(i))
                .ok_or_else(|| invalid(OP_CREATE, "filleted edge use without fillet solver"))
        })
    }

    /// `SmFilletCorner::Create` using the executive's live solver objects.
    ///
    /// `solver_of` resolves each filleted edge use to its actual borrowed solver
    /// base. Lookups occur in `solver_eus` order after the topology walk, where the
    /// native factory reads tolerances and the surface generator. No solver is copied.
    ///
    /// # Errors
    ///
    /// Those of [`Self::create`] and `solver_of`; the first lookup failure stops
    /// construction before any later solver is requested.
    pub fn create_with_solver_lookup<'solver>(
        k: &Kernel,
        b: &mut Brep,
        vertex: VertexId,
        solver_eus: &[EdgeuseId],
        solver_of: impl Fn(EdgeuseId) -> Result<&'solver FilletSolver>,
    ) -> Result<Option<Self>> {
        // get number of edgeuses connected to this vertex to be filleted
        let total_solver_eus = solver_eus.len();

        // locals
        let mut is_open_end_corner = false;
        let mut is_chamfer_corner = true;

        // get first to-be-filleted edgeuse connected to vertex
        let start_eu = *solver_eus
            .first()
            .ok_or_else(|| invalid(OP_CREATE, "no edge use to fillet"))?;

        // Create array of all outward edgeuses connected to unique edges that surround the corner
        let mut all_eus = vec![start_eu];
        let mut eu = start_eu;
        let edge_of = |b: &Brep, eu: EdgeuseId| -> Result<EdgeId> {
            eu1::edge(b, eu)?.ok_or_else(|| invalid(OP_CREATE, "edge use without edge"))
        };
        let mut steps = 0usize;
        let max_steps = b.edgeuses().len();
        loop {
            steps += 1;
            if steps > max_steps.saturating_add(1) {
                return Err(invalid(OP_CREATE, "walk around the vertex does not close"));
            }
            // get edgeuse neighbors circuling this vertex and march to next radial neighbor
            let eu0 = eu; // edgeuse starting at vertex
            let eu1_ = eu1::cw_edgeuse(b, eu0)?
                .ok_or_else(|| invalid(OP_CREATE, "edge use without clockwise neighbour"))?; // edgeuse in same loop ending at vertex
            let eu2_ = eu1::radial(b, eu1_)?
                .ok_or_else(|| invalid(OP_CREATE, "edge use without radial partner"))?; // next edgeuse starting at vertex in same region
            eu = eu2_;

            // quit - when we get back to StartEU
            if eu == start_eu {
                break;
            }

            // skip strut edgeuses
            if eu1::edge(b, eu0)? == eu1::edge(b, eu1_)? {
                continue;
            }

            // skip edgeuses that duplicate an edge - happens on closed curves
            let e = eu1::edge(b, eu)?;
            let mut found = false;
            for &eu3 in &all_eus {
                if e == eu1::edge(b, eu3)? {
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }

            // lamina edges have OpenEndCorners
            if eu1::edge_is_lamina(b, edge_of(b, eu)?)? {
                is_open_end_corner = true;
            }

            // If we have mixed convexity, pEU may not match the edgeuse in
            // pSolverEUs, use the one in pSolverEUs instead
            let mut solver_eu = None;
            for &s in solver_eus {
                if eu != s && e == eu1::edge(b, s)? {
                    // SM_DBG_WARN: CONFUSED CLASSIFICATION OF CORNER VERTEX
                    solver_eu = Some(s);
                    break; // mixed convexity
                }
            }
            all_eus.push(solver_eu.unwrap_or(eu));
        } // end while(TRUE) adding outward edgeuses to pAllEUs list

        // get total number of outward edgeuses connected to unique edges that are connected to vertex
        let total_eus = all_eus.len();

        // 3D tolerance for corner patch creation
        let mut approx_tol_3d = SM_BIG_DOUBLE;
        // Angle tol for tangent field approximation
        let mut tangency_tol_radians = SM_BIG_DOUBLE;

        // for every to-be-filleted edgeuse connected to vertex:
        // Collect max tolerances and check for Chamfer corner.
        for &edge_use in solver_eus {
            let solver = solver_of(edge_use)?;
            let tol = solver.approx_tol_3d;
            let tan_tol = solver.tangency_tolerance;

            // save tightest tolerance value
            if tol < approx_tol_3d {
                approx_tol_3d = tol;
            }
            if tan_tol < tangency_tol_radians {
                tangency_tol_radians = tan_tol;
            }

            // when any to-be-filleted edgeuse is not linear - this it not a ChamferCorner
            if is_chamfer_corner {
                let fsg = solver
                    .fsg
                    .as_ref()
                    .ok_or_else(|| invalid(OP_CREATE, "fillet solver without surface generator"))?;
                if fsg.fillet_surface_generator_type() != FilletSurfaceGeneratorType::Linear {
                    is_chamfer_corner = false;
                }
            }
        }

        let tan_tol_deg = rad_to_deg(tangency_tol_radians);
        let make = |kind| {
            Self::new(
                kind,
                Some(vertex),
                Some(solver_eus.to_vec()),
                Some(all_eus.clone()),
                approx_tol_3d,
                tangency_tol_radians,
            )
        };

        // switch on number of to-be-filleted edgeuses connected to vertex
        let corner = match total_solver_eus {
            0 => None,
            1 => {
                let eu0 = start_eu;
                let e = edge_of(b, eu0)?;
                if edge::is_closed(b, e)? {
                    // Corner with one 'closed' edge filleted
                    match total_eus {
                        // Need to be G1-closed
                        1 => {
                            if test_g1_continuity(k, b, vertex, e, e, tan_tol_deg)? {
                                Some(make(FilletCornerKind::OneByOne))
                            } else {
                                None // MSG: Corner Case Not Implemented
                            }
                        }
                        2 | 3 => Some(make(FilletCornerKind::NByOneClosed)),
                        _ => None, // MSG: Corner Case Not Implemented
                    }
                } else if check_nx1_degeneracy(k, b, vertex, eu0)? {
                    // Degeneracy-check needs to be checked first
                    Some(make(FilletCornerKind::Degenerate))
                } else if is_open_end_corner {
                    Some(make(FilletCornerKind::Open))
                } else {
                    Some(make(FilletCornerKind::NByOne))
                }
            }
            2 => {
                let eu0 = start_eu;
                let eu1_ = *solver_eus
                    .get(1)
                    .ok_or_else(|| invalid(OP_CREATE, "second filleted edge use"))?;
                match total_eus {
                    // 2x2: we can handle this only if it's a tangent fillet
                    2 => {
                        let e0 = edge_of(b, eu0)?;
                        let e1 = edge_of(b, eu1_)?;
                        if test_g1_continuity(k, b, vertex, e0, e1, tan_tol_deg)? {
                            Some(make(FilletCornerKind::TwoByTwo))
                        } else {
                            None // MSG: Corner Case Not Implemented
                        }
                    }
                    3 | 4 => {
                        if total_eus == 3
                            && eu2::is_convex_radial_sector(k, b, eu0, 4, false, 0.0)?
                                != eu2::is_convex_radial_sector(k, b, eu1_, 4, false, 0.0)?
                        {
                            Some(make(FilletCornerKind::ThreeByTwoMixed))
                        } else {
                            // else continue with case 4
                            Some(make(FilletCornerKind::NByTwo))
                        }
                    }
                    _ => None, // MSG: Corner Case Not Implemented
                }
            }
            _ => {
                if total_solver_eus == 3 && total_eus == 4 {
                    Some(make(FilletCornerKind::FourByThree))
                } else if total_eus != total_solver_eus {
                    // Basically, we are handling NxN cases here
                    None // MSG: Corner Case Not Implemented
                } else {
                    let mut concave_eus: Vec<EdgeuseId> = Vec::new();
                    if !is_chamfer_corner {
                        concave_eus = find_concave_edgeuses(k, b, vertex, &all_eus)?;
                    }
                    if concave_eus.is_empty() {
                        let mut c = make(FilletCornerKind::ConvexNByN);
                        c.chamfer = is_chamfer_corner; // SetChamfer
                        Some(c)
                    } else {
                        Some(make(FilletCornerKind::ConcaveNByN))
                    }
                }
            }
        };
        Ok(corner)
    }

    // ---- SmFilletCorner::CalcCornerGeom ----------------------------------------------

    /// `SmFilletCorner::CalcCornerGeom`: builds the corner patch (`CreateCornerPatch`)
    /// unless the corner is a bevel between several fillets or has fewer than three
    /// fillet edges.
    ///
    /// # Errors
    ///
    /// Those of [`FilletCorner::create_corner_patch`].
    pub fn calc_corner_geom(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        self.calc_corner_geom_with(env, |corner, env| corner.create_corner_patch(env))
    }

    /// Apply the native corner-patch predicate before lending the actual patch context.
    ///
    /// # Errors
    /// Those of the actual concrete patch operation when a patch is needed.
    pub fn calc_corner_geom_with<C>(
        &mut self,
        context: &mut C,
        create_patch: impl FnOnce(&mut Self, &mut C) -> Result<()>,
    ) -> Result<()> {
        let total_edges = self.edges.len();
        let fillet_edges = self.solver_eus.as_ref().map_or(0, Vec::len);

        // no work - Bevel corner or degenerate corner
        if (self.bevel && fillet_edges > 1) || total_edges < 3 {
            return Ok(());
        }

        // Otherwise, pass the call along to build a Corner->FilletSurface and store it in m_vSurfaces
        create_patch(self, context)
    }

    // ---- CreateCornerPatch -----------------------------------------------------------

    /// `CreateCornerPatch` (virtual): the 3x2 mixed-convexity, Nx1 (and open) and Nx2
    /// overrides, the default patch for every other kind.
    ///
    /// # Errors
    ///
    /// Those of the selected implementation.
    pub fn create_corner_patch(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        match self.kind {
            FilletCornerKind::ThreeByTwoMixed => self.create_corner_patch_3x2(env),
            FilletCornerKind::NByOne | FilletCornerKind::Open => self.create_corner_patch_nx1(env),
            FilletCornerKind::NByTwo => self.create_corner_patch_nx2(env),
            _ => self.create_corner_patch_default(env),
        }
    }

    /// `SmFilletCorner::CreateCornerPatch`: an analytic corner surface when one fits,
    /// else a three- or four-sided Coons patch over the corner's fillet edges (a
    /// three-sided one gets a degenerate point curve at the vertex selected for it),
    /// then the corner face (`MakeFaceBrep`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` after noting a vertex-problem fillet error when `MakeFaceBrep` fails;
    /// those of [`FilletCorner::make_analytic_corner_surface`]; `SM_ERR_NULL_POINTER`
    /// for a fillet edge without fillet geometry where one is needed;
    /// [`SolidError::InvalidInput`] where upstream dereferences null or indexes past the
    /// corner's arrays; those of the point curves and `CreateCornerBlend`.
    pub fn create_corner_patch_default(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        env.with_patch_env(|env| self.create_corner_patch_default_with(env))
    }

    /// [`FilletCorner::create_corner_patch_default`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::create_corner_patch_default`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn create_corner_patch_default_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
    ) -> Result<()> {
        // if an AnalyticSurface can approx FilletCorner->Curves to tol,
        //   Make and store that AnalyticSurface in m_vSurfaces, and return TRUE
        // else return FALSE.
        let success = self.make_analytic_corner_surface_with(env)?;

        if !success {
            // When unable to create analytic corner - create COONS patch
            let mut init_index = 0usize;
            let mut total_edges = self.edges.len();
            let mut fv_of_degenerate_curve: Option<VertexId> = None; // used to indicate degenerate Curve of 3 sided patches

            // for corners with 3 edges - select a degenerate vertex (lInitIndex)
            if total_edges == 3 {
                // Select degenerate corner - Pay special attention to Nx2 corners
                let corner_type = self.corner_type();
                if corner_type == FilletCornerType::NByTwo
                    || corner_type == FilletCornerType::ThreeByTwoMixed
                    || corner_type == FilletCornerType::NByOne
                {
                    // for every corner vertex
                    for &fv in &self.vertices {
                        match env.pseudo.fillet_vertex(fv)?.fillet_vertex_type() {
                            FilletVertexType::RailXEdgeuse => {
                                if fv_of_degenerate_curve.is_none() {
                                    fv_of_degenerate_curve = Some(fv);
                                }
                            }
                            FilletVertexType::FilletXFillet => fv_of_degenerate_curve = Some(fv),
                            FilletVertexType::OnVertex
                                if corner_type == FilletCornerType::NByOne =>
                            {
                                fv_of_degenerate_curve = Some(fv);
                            }
                            _ => {}
                        }
                    }
                }

                // when we have yet to select the degenerate corner
                let fv_degenerate = if let Some(fv) = fv_of_degenerate_curve {
                    fv
                } else {
                    // get distance between every filletCorner->FilletVertex pair
                    let vert = |i: usize| -> Result<VertexId> {
                        self.vertices
                            .get(i)
                            .copied()
                            .ok_or_else(|| invalid(OP_PATCH, "fewer than three corner vertices"))
                    };
                    let mut pnt_pnt_dist = [0.0f64; 4];
                    for (ii, d) in pnt_pnt_dist.iter_mut().enumerate().take(3) {
                        let next_vert = (ii + 1) % 3;
                        let pnt = env.pseudo.vertex_point(vert(ii)?)?;
                        *d = pnt.distance(env.pseudo.vertex_point(vert(next_vert)?)?);
                    }
                    pnt_pnt_dist[3] = pnt_pnt_dist[0];

                    // select degenerate vertex = vertex connected to edges most equal in length
                    let mut vert_of_deg_curve = 0usize;
                    let mut min_diff = (pnt_pnt_dist[2] - pnt_pnt_dist[0]).abs();
                    for (ii, w) in pnt_pnt_dist.windows(2).enumerate().take(2) {
                        let ii = ii + 1;
                        let &[prev, cur] = w else { continue };
                        let diff = (prev - cur).abs();
                        if diff < min_diff {
                            min_diff = diff;
                            vert_of_deg_curve = ii;
                        }
                    }

                    // set the Degenerate vertex pointer
                    vert(vert_of_deg_curve)?
                };
                fv_of_degenerate_curve = Some(fv_degenerate);

                // get index of degenerate vertex
                for ii in 0..3 {
                    let e = *self
                        .edges
                        .get(ii)
                        .ok_or_else(|| invalid(OP_PATCH, "corner edge index"))?;
                    if edge::vertex(env.pseudo.brep(), e)? == Some(fv_degenerate) {
                        init_index = ii; // Left curve index
                        break;
                    }
                }
            }
            // else for corners with 4 edges branch - set lInitIndex == EdgeOfType SM_FE_RAIL_RAIL_INTERPOLATION
            else if total_edges == 4 {
                for (ii, &fe) in self.edges.iter().enumerate() {
                    if env.pseudo.fillet_edge(fe)?.fillet_edge_type()
                        == FilletEdgeType::RailRailInterpolation
                    {
                        init_index = (ii + 1) % total_edges; // Left curve index
                        break;
                    }
                }
            }

            // Next - build ordered patch boundary arrays.
            let mut ordered_uv: Vec<Option<BSplineCurve>> = Vec::new();
            let mut ordered_3d: Vec<Option<Curve>> = Vec::new();
            let mut orients: Vec<OrientType> = Vec::new();
            let mut ordered_surfs: Vec<Option<Surface>> = Vec::new();

            // for every edge - get edge shape as (neighborFace->Surface,UVTrimCurve) pair
            for ii in 0..total_edges {
                let fe = *self
                    .edges
                    .get((init_index + ii) % total_edges)
                    .ok_or_else(|| invalid(OP_PATCH, "corner edge index"))?;
                let prim_eu = primary_eu(env.pseudo, fe, OP_PATCH)?;
                let mate_eu = eu1::mate(env.pseudo.brep(), prim_eu)?;
                let mut ps_curve: Option<BSplineCurve> = None;
                let mut base_surface: Option<Surface> = None;

                // when build g1 patches - get UVTrimCurve and Surface
                if self.extended_surface_patch {
                    // when the edge has an OriginalFace
                    if let Some(orig_face) = env.pseudo.fillet_edge(fe)?.original_face() {
                        let mate_eu = mate_eu.ok_or_else(|| SolidError::null_pointer(OP_PATCH))?;
                        ps_curve = eu1::uv_trim_curve(env.pseudo.brep(), mate_eu)?
                            .0
                            .and_then(bspline_part);
                        base_surface.clone_from(&env.target.face(orig_face)?.surface);
                    } else {
                        // no Original Face branch - curve is only connected to filletSurfaces
                        // use primaryEdgeuse to get (neighborFace,UVTrimCurve) pair
                        ps_curve = eu1::uv_trim_curve(env.pseudo.brep(), prim_eu)?
                            .0
                            .and_then(bspline_part);
                        let g = env
                            .pseudo
                            .fillet_edgeuse(prim_eu)?
                            .fillet_geom()
                            .ok_or_else(|| SolidError::null_pointer(OP_PATCH))?;
                        base_surface = env.pseudo.geom(g)?.fillet_surface(None)?.cloned();
                    }
                }

                // add edge 3DCurve, NeighborSurface, NeighborSurface->UVTrimCurve, orients to ordered arrays
                ordered_uv.push(ps_curve);
                ordered_3d.push(env.pseudo.brep().edge(fe)?.curve.clone());
                orients.push(OrientType::Same);
                ordered_surfs.push(base_surface);
            }

            // when building a 3 sided patch - add a degenerate curve to the Ordered arrays
            if let Some(fv) = fv_of_degenerate_curve {
                // make a temporary degenerate 3DCurve
                let pnt = env.pseudo.vertex_point(fv)?;
                let point_curve = BSplineCurve::create_point_curve(pnt)?;

                // when degenerate vertex classifies to a Face
                // and building g1 patches - give it a (surface,UVTrimCurve) pair
                let mut base_surface: Option<Surface> = None;
                let mut uv_point_curve: Option<BSplineCurve> = None;
                let rec = env.pseudo.fillet_vertex(fv)?;
                if let (Some(EntityRef::Face(face)), true) =
                    (rec.point_class_object(), self.extended_surface_patch)
                {
                    base_surface.clone_from(&env.target.face(face)?.surface);
                    let uv = rec.original_uv();
                    let pt = Point3::new(uv.x, uv.y, 0.0);
                    let mut c = BSplineCurve::create_point_curve(pt)?;
                    c.convert_to_2d()?;
                    uv_point_curve = Some(c);
                }

                // add degenerate curve to ordered boundary arrays
                ordered_uv.push(uv_point_curve);
                ordered_3d.push(Some(Curve::from(point_curve)));
                orients.push(OrientType::Same);
                ordered_surfs.push(base_surface);
                total_edges += 1;
            }
            let _ = total_edges;

            // build the corner patch from the ordered array boundary information
            // CreateCornerBlend checks `cr3DCurves.GetSize() < 4 -> SM_ERR` before it
            // touches any curve (a null one is SM_ERR_NULL_POINTER afterwards).
            if ordered_3d.len() < 4 {
                return Err(SolidError::sm_err(
                    "nurbs::b_spline_surface_n_lib::p2::BSplineSurface::create_corner_blend",
                ));
            }
            let mut curves_3d = Vec::with_capacity(ordered_3d.len());
            for c in &ordered_3d {
                let c = c
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP_PATCH))?;
                curves_3d.push(
                    bspline_part(c)
                        .ok_or_else(|| invalid(OP_PATCH, "boundary curve is not a B-spline"))?,
                );
            }
            let surfs: Vec<Option<BlendNeighbour<'_>>> = ordered_surfs
                .iter()
                .map(|s| s.as_ref().map(|surface| BlendNeighbour { surface }))
                .collect();
            let blends = BSplineSurface::create_corner_blend(
                env.k,
                &curves_3d,
                &orients,
                &ordered_uv,
                &surfs,
                self.approx_tol_3d,
                self.tangency_tol_radians,
                !self.extended_surface_patch,
            )?;

            // store New FilletSurface in m_vSurfaces
            self.surfaces.extend(blends.into_iter().map(Surface::from));
        }

        // Create Face (and Edges and Verts) in m_pFilletBrep from m_vSurfaces with call m_pFilletBrep->MakeFaceWithCurves()
        match self.make_face_brep_with(env) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(
                e @ SolidError::InvalidInput {
                    reason: p1::NULL_DEREF,
                    ..
                },
            ) => return Err(e),
            Err(_) => {
                // when MakeFaceBrep failed - note the error
                (env.note_face_error)(self);
                // FILEXEC_CORNER_ERR(this, "Corner Face Creation Error")
                return Err(SolidError::sm_err(OP_PATCH));
            }
        }

        // all done
        Ok(())
    }

    /// `SmFillet3x2MixedConvexityCorner::CreateCornerPatch`: joins the on-edge fillet
    /// edge's curve onto the on-extended-edge fillet edge's curve, merges the two edges
    /// (the on-edge edge and its corner vertex are deleted, the vertex's uses moved to
    /// the edge's other vertex), drops the joined curve onto the original face for the
    /// mate edge use's UV curve and builds the three-sided default patch.
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless the corner has four fillet edges, for a corner vertex that is
    /// not `SM_FV_ON_VERTEX` or starts the on-extended-edge edge, or one not in the
    /// corner; `SM_ERR_NULL_POINTER` for a missing on-edge / on-extended-edge edge, a
    /// non-B-spline curve or a missing original face; those of `JoinWith`;
    /// [`SolidError::Unported`] (the drop onto a surface kind whose `DropCurve` needs
    /// unported code, see [`Surface::drop_curve`]).
    pub fn create_corner_patch_3x2(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        env.with_patch_env(|env| self.create_corner_patch_3x2_with(env))
    }

    /// [`FilletCorner::create_corner_patch_3x2`] against any [`CornerPatchEnv`].
    ///
    /// # Errors
    /// As [`FilletCorner::create_corner_patch_3x2`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn create_corner_patch_3x2_with(&mut self, env: &mut CornerPatchEnv<'_, '_>) -> Result<()> {
        // check state - 4 edges
        if self.edges.len() != 4 {
            // SM_DBG_WARN: Failed state check: 3x2 FilletCorner with other than 4 edges
            return Err(SolidError::sm_err(OP_3X2));
        }

        // search for the onEdge and onExtended edges
        let mut fe_on_edge = None;
        let mut fe_on_extended_edge = None;
        let mut found_index = 0usize;
        for (ii, &fe) in self.edges.iter().enumerate() {
            match env.pseudo.fillet_edge(fe)?.fillet_edge_type() {
                FilletEdgeType::OnExtendEdge => fe_on_extended_edge = Some(fe),
                FilletEdgeType::OnEdge => {
                    // To be deleted later
                    found_index = ii;
                    fe_on_edge = Some(fe);
                }
                _ => {}
            }
        }

        // check state - must have onEdge and onExtendedEdge
        let fe_on_edge = fe_on_edge.ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
        let fe_on_extended_edge =
            fe_on_extended_edge.ok_or_else(|| SolidError::null_pointer(OP_3X2))?;

        // get onEdge->Vertices
        let corner_v = edge::vertex(env.pseudo.brep(), fe_on_edge)?
            .ok_or_else(|| invalid(OP_3X2, "on-edge fillet edge without vertex"))?;
        let surviving_fv = edge::other_vertex(env.pseudo.brep(), fe_on_edge, Some(corner_v))?;

        // check state
        if env.pseudo.fillet_vertex(corner_v)?.fillet_vertex_type() != FilletVertexType::OnVertex
            || edge::vertex(env.pseudo.brep(), fe_on_extended_edge)? == Some(corner_v)
        {
            // Internal error, check corner topology
            return Err(SolidError::sm_err(OP_3X2));
        }

        // get onEdge and onExtended curves (SM_CAST_PTR(SmBSplineCurve, ..): the analytic
        // curves derived from SmBSplineCurve pass)
        let bspline_curve = |pseudo: &FilletBrep, e: EdgeId| -> Result<Curve> {
            pseudo
                .brep()
                .edge(e)?
                .curve
                .as_ref()
                .filter(|c| {
                    matches!(
                        c.kind(),
                        CurveKind::BSpline(_)
                            | CurveKind::Line(_)
                            | CurveKind::Ellipse(_)
                            | CurveKind::Hyperbola(_)
                            | CurveKind::Parabola(_)
                    )
                })
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP_3X2))
        };
        let mut curve1 = bspline_curve(env.pseudo, fe_on_extended_edge)?;
        let mut curve2 = bspline_curve(env.pseudo, fe_on_edge)?;

        // join the 2 curves (JoinWith is virtual: SmLine and SmEllipse override it)
        join_with(&*env.k, &mut curve1, true, &mut curve2, false)?;
        env.pseudo.brep_mut().edge_mut(fe_on_extended_edge)?.curve = Some(curve1.clone());
        env.pseudo.brep_mut().edge_mut(fe_on_edge)?.curve = Some(curve2);

        // Combine two edges
        self.edges.remove(found_index);
        let Some(idx) = self.vertices.iter().position(|&v| v == corner_v) else {
            return Err(SolidError::sm_err(OP_3X2));
        };
        self.vertices.remove(idx);

        env.pseudo.delete_fillet_edge(fe_on_edge)?;

        let surviving_fv = surviving_fv.ok_or_else(|| invalid(OP_3X2, "no surviving vertex"))?;
        for vu in env.pseudo.brep().vertex(corner_v)?.vertexuses.clone() {
            move_vertexuse(env.pseudo, vu, corner_v, surviving_fv)?;
        }

        env.pseudo.delete_fillet_vertex(corner_v)?;

        // Project pFEOnExtendedEdge onto original face
        let orig_face: FaceId = env
            .pseudo
            .fillet_edge(fe_on_extended_edge)?
            .original_face()
            .ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
        let surface = env
            .target
            .face(orig_face)?
            .surface
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
        // `pSurface->DropCurve(m_crContext, pSurface->GetNaturalUVDomain(), *pCurve1,
        // pCurve1->GetNaturalInterval(), m_dThisApproxTol3d, ..)` (virtual with the
        // defaults `bKeepAllDropCurves = TRUE`, no fail record): the `SmPlane`
        // override for a plane (with the surface's owner), the base body otherwise.
        let domain = surface.natural_uv_domain_with_nurb(env.k)?;
        let interval = curve1.natural_interval();
        // (the drop gives `SmBSplineCurve`s; for a plane a UV `SmLine` stays a line, as
        // upstream keeps the `SmLine` object behind the pointer, `SetUVCurve` stores it)
        let uv_curves: Vec<Curve> = surface
            .drop_curve::<NoObject>(
                &*env.k,
                &domain,
                &curve1,
                &interval,
                self.approx_tol_3d,
                true,
                None,
                true,
            )?
            .uv_curves;
        if uv_curves.len() != 1 {
            return Err(SolidError::sm_err(OP_3X2)); // Unknown problem
        }
        let new_ps_curve = uv_curves
            .into_iter()
            .next()
            .ok_or_else(|| SolidError::sm_err(OP_3X2))?;
        let prim = primary_eu(env.pseudo, fe_on_extended_edge, OP_3X2)?;
        let mate_eu =
            eu1::mate(env.pseudo.brep(), prim)?.ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
        env.pseudo
            .set_edgeuse_uv_curve(mate_eu, Some(new_ps_curve), true, true)?;

        // Create a 3-sided corner patch
        self.create_corner_patch_default_with(env)
    }

    /// `SmFilletNx1Corner::CreateCornerPatch`: with three fillet edges an end patch
    /// from the extended side face (`CreateEndPatch`) or the default patch; otherwise
    /// nothing.
    ///
    /// # Errors
    ///
    /// Those of the patch builders; [`SolidError::Unported`] (`CreateEndPatch`).
    pub fn create_corner_patch_nx1(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        if self.edges.len() == 3 {
            if self.extended_surface_patch {
                self.create_end_patch(env)?;
            } else {
                self.create_corner_patch_default(env)?;
            }
        }
        // else SM_DBG_WARN: Unexpected SmFilletNx1Corner endFace construction case
        Ok(())
    }

    /// [`FilletCorner::create_corner_patch_nx1`] against any [`CornerPatchEnv`];
    /// `extended_surface` resolves the executive's extended surfaces and
    /// `note_end_face_error` notes a failed end face (`CreateEndPatch`).
    ///
    /// # Errors
    /// As [`FilletCorner::create_corner_patch_nx1`].
    pub fn create_corner_patch_nx1_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
        extended_surface: impl Fn(FaceId) -> Option<Surface>,
        note_end_face_error: &mut dyn FnMut(&FilletCorner),
    ) -> Result<()> {
        if self.edges.len() == 3 {
            if self.extended_surface_patch {
                let mut end_env = CornerPatchEnv {
                    k: &mut *env.k,
                    target: env.target,
                    pseudo: &mut *env.pseudo,
                    solvers: env.solvers,
                    fillet: env.fillet.as_deref_mut(),
                    note_face_error: note_end_face_error,
                };
                self.create_end_patch_with(&mut end_env, extended_surface)?;
            } else {
                self.create_corner_patch_default_with(env)?;
            }
        }
        // else SM_DBG_WARN: Unexpected SmFilletNx1Corner endFace construction case
        Ok(())
    }

    /// `SmFilletNx2Corner::CreateCornerPatch`: with six fillet edges a small end patch
    /// over the last three first; then the three-sided default patch over the first
    /// three edges and vertices; the edges, vertices and end patch are put back.
    ///
    /// # Errors
    ///
    /// Those of the patch builders; [`SolidError::InvalidInput`] for more than three
    /// edges and no vertex (module docs).
    pub fn create_corner_patch_nx2(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        let total_edges = self.edges.len();

        let mut first_three_edges: Vec<EdgeId> = Vec::new();
        let mut last_three_edges: Vec<EdgeId> = Vec::new();
        let mut last_two_verts: Vec<VertexId> = Vec::new();
        let mut end_patch: Option<Surface> = None;
        if total_edges > 3 {
            first_three_edges.extend(self.edges.iter().take(3).copied());
            last_three_edges.extend(self.edges.iter().skip(3).copied());

            if total_edges == 6 {
                // Use the last 3 edges to create a small patch
                self.edges.clone_from(&last_three_edges);
                // Create a small corner face
                self.create_end_patch(env)?;

                // Temporarily remove the small patch
                if self.surfaces.is_empty() {
                    return Err(invalid(OP_NX2_PATCH, "end patch built no surface"));
                }
                end_patch = Some(self.surfaces.remove(0));
                self.surfaces.clear();
            }

            self.edges.clone_from(&first_three_edges);

            // Temporarily remove the last two vertices
            if self.vertices.is_empty() {
                return Err(invalid(OP_NX2_PATCH, "unsigned vertex index wraps around"));
            }
            while self.vertices.len() > 3 {
                if let Some(v) = self.vertices.pop() {
                    last_two_verts.push(v);
                }
            }
        }

        // Create a 3-sided corner patch
        self.create_corner_patch_default(env)?;

        // Put the end patch back
        if let Some(p) = end_patch {
            self.surfaces.push(p);
        }

        if !last_three_edges.is_empty() {
            self.edges.extend(last_three_edges);
            self.vertices.extend(last_two_verts);
        }

        Ok(())
    }

    /// [`FilletCorner::create_corner_patch_nx2`] against any [`CornerPatchEnv`];
    /// `extended_surface` resolves the executive's extended surfaces and
    /// `note_end_face_error` notes a failed end face (`CreateEndPatch`).
    ///
    /// # Errors
    /// As [`FilletCorner::create_corner_patch_nx2`].
    pub fn create_corner_patch_nx2_with(
        &mut self,
        env: &mut CornerPatchEnv<'_, '_>,
        extended_surface: impl Fn(FaceId) -> Option<Surface>,
        note_end_face_error: &mut dyn FnMut(&FilletCorner),
    ) -> Result<()> {
        let total_edges = self.edges.len();

        let mut first_three_edges: Vec<EdgeId> = Vec::new();
        let mut last_three_edges: Vec<EdgeId> = Vec::new();
        let mut last_two_verts: Vec<VertexId> = Vec::new();
        let mut end_patch: Option<Surface> = None;
        if total_edges > 3 {
            first_three_edges.extend(self.edges.iter().take(3).copied());
            last_three_edges.extend(self.edges.iter().skip(3).copied());

            if total_edges == 6 {
                // Use the last 3 edges to create a small patch
                self.edges.clone_from(&last_three_edges);
                // Create a small corner face
                let mut end_env = CornerPatchEnv {
                    k: &mut *env.k,
                    target: env.target,
                    pseudo: &mut *env.pseudo,
                    solvers: env.solvers,
                    fillet: env.fillet.as_deref_mut(),
                    note_face_error: note_end_face_error,
                };
                self.create_end_patch_with(&mut end_env, extended_surface)?;

                // Temporarily remove the small patch
                if self.surfaces.is_empty() {
                    return Err(invalid(OP_NX2_PATCH, "end patch built no surface"));
                }
                end_patch = Some(self.surfaces.remove(0));
                self.surfaces.clear();
            }

            self.edges.clone_from(&first_three_edges);

            // Temporarily remove the last two vertices
            if self.vertices.is_empty() {
                return Err(invalid(OP_NX2_PATCH, "unsigned vertex index wraps around"));
            }
            while self.vertices.len() > 3 {
                if let Some(v) = self.vertices.pop() {
                    last_two_verts.push(v);
                }
            }
        }

        // Create a 3-sided corner patch
        self.create_corner_patch_default_with(env)?;

        // Put the end patch back
        if let Some(p) = end_patch {
            self.surfaces.push(p);
        }

        if !last_three_edges.is_empty() {
            self.edges.extend(last_three_edges);
            self.vertices.extend(last_two_verts);
        }

        Ok(())
    }

    // ---- AdjustTopology --------------------------------------------------------------

    /// `AdjustTopology` (virtual): the Nx2 override and the convex (also the concave)
    /// `NxN` one; every other kind does nothing.
    ///
    /// # Errors
    ///
    /// Those of [`FilletCorner::adjust_topology_nx2`] and
    /// [`FilletCorner::adjust_topology_convex_nxn`].
    pub fn adjust_topology(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        self.adjust_topology_with(
            env,
            Self::adjust_topology_nx2,
            Self::adjust_topology_convex_nxn,
        )
    }

    /// Dispatch topology adjustment using the caller's live derived-corner context.
    /// The inherited native base implementation needs no context and returns success.
    ///
    /// # Errors
    /// The selected concrete adjustment callback's errors.
    pub fn adjust_topology_with<C>(
        &mut self,
        context: &mut C,
        nx2: impl FnOnce(&mut Self, &mut C) -> Result<()>,
        nxn: impl FnOnce(&mut Self, &mut C) -> Result<()>,
    ) -> Result<()> {
        match self.kind {
            FilletCornerKind::NByTwo => nx2(self, context),
            FilletCornerKind::ConvexNByN | FilletCornerKind::ConcaveNByN => nxn(self, context),
            _ => Ok(()),
        }
    }

    /// `SmFilletNx2Corner::AdjustTopology`: where the rails of the two fillets meet the
    /// side edge at different points of a bevel corner, splits the
    /// `SM_FV_RAIL_X_EDGEUSE` vertex in two (the farther point stays on the side edge,
    /// a new `SM_FV_FILLET_X_FILLET` vertex goes on the side face) joined by new fillet
    /// edges. Blending corners are checked for the big-radius case instead.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than four edges around the corner;
    /// [`SolidError::InvalidInput`] for an unset `m_pAllEUs`, a classification object
    /// that is not an edge use, a missing rail vertex use or side edge curve (upstream
    /// dereferences them); [`SolidError::Unported`] (module docs).
    pub fn adjust_topology_nx2(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        self.adjust_topology_nx2_with(env)
    }

    /// [`FilletCorner::adjust_topology_nx2`] against any [`Nx2AdjustContext`] (the
    /// legacy [`CornerEnv`] or a live executive session).
    ///
    /// # Errors
    ///
    /// As [`FilletCorner::adjust_topology_nx2`], plus those of the context's
    /// concave-corner and big-radius operations.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn adjust_topology_nx2_with(&mut self, env: &mut impl Nx2AdjustContext) -> Result<()> {
        // Can't do it if more than four total edges
        let all_eus = self
            .all_eus
            .as_ref()
            .ok_or_else(|| invalid(OP_TOPO, "m_pAllEUs is NULL"))?;
        if all_eus.len() > 4 {
            return Err(SolidError::sm_err(OP_TOPO));
        }

        // Check for mixed-convexity case: special routine for that.
        let num_vertex_edges = self.edges.len();
        if num_vertex_edges == 3 {
            // Mixed-convexity corners
            return env.adjust_concave_corner_topology(self);
        }

        let mut ii = 0usize;
        while ii < self.vertices.len() {
            let adjust_vert = *self
                .vertices
                .get(ii)
                .ok_or_else(|| invalid(OP_TOPO, "vertex index"))?;

            let (k, target, pseudo, _) = env.parts();
            // Check whether this is the vertex that corresponds to the
            // rail hitting the side edge
            if pseudo.fillet_vertex(adjust_vert)?.fillet_vertex_type()
                != FilletVertexType::RailXEdgeuse
            {
                ii += 1;
                continue;
            }

            let geoms = pseudo.vertex_fillet_geoms(adjust_vert)?;
            let [fillet_geom1, fillet_geom2] = geoms[..] else {
                ii += 1;
                continue; // maybe already adjusted...
            };
            let Some(EntityRef::Edgeuse(side_eu1)) =
                pseudo.fillet_vertex(adjust_vert)?.point_class_object()
            else {
                return Err(invalid(OP_TOPO, "classification object is not an edge use"));
            };
            let vu1 = pseudo
                .vu_at_rail_end(adjust_vert, Some(fillet_geom1), None, None)?
                .ok_or_else(|| invalid(OP_TOPO, "no rail vertex use"))?;
            let vu2 = pseudo
                .vu_at_rail_end(adjust_vert, Some(fillet_geom2), Some(vu1), None)?
                .ok_or_else(|| invalid(OP_TOPO, "no rail vertex use"))?;
            let edge_param1 = pseudo.fillet_vertexuse(vu1)?.tsect_pnt().curve_parameter;
            let edge_param2 = pseudo.fillet_vertexuse(vu2)?.tsect_pnt().curve_parameter;
            let side_edge = eu1::edge(target, side_eu1)?
                .ok_or_else(|| invalid(OP_TOPO, "side edge use without edge"))?;
            let side_edge_curve = target
                .edge(side_edge)?
                .curve
                .as_ref()
                .ok_or_else(|| invalid(OP_TOPO, "side edge without curve"))?;

            let pnt1 = unchecked_point(side_edge_curve.evaluate_point(k, edge_param1))?;
            let pnt2 = unchecked_point(side_edge_curve.evaluate_point(k, edge_param2))?;

            let dist = pnt1.distance(pnt2);

            // Check for the big-radius case
            if self.blending {
                let mut converted = env.test_and_convert_big_rad_fillet(self, fillet_geom1)?;
                if !converted {
                    converted = env.test_and_convert_big_rad_fillet(self, fillet_geom2)?;
                }
                if converted {
                    return Ok(());
                }
                ii += 1;
                continue;
            }

            // SM_ASSERT(m_bBevel == TRUE); Bevel cases
            let (k, target, pseudo, this_id) = env.parts();

            // If rails hit side edge at same point, no adjustment is needed.
            if dist < self.approx_tol_3d {
                ii += 1;
                continue;
            }

            self.topology_adjusted = true;

            let vert_pnt = target
                .vertex(
                    self.vertex
                        .ok_or_else(|| invalid(OP_TOPO, "m_cpVertex is NULL"))?,
                )?
                .point;
            let (side_face, vu_on_side_face, small_rad_geom, large_rad_geom, pnt_on_side_face);

            // We'll split this vertex and keep the farther point (along the edge)
            if pnt1.distance(vert_pnt) < pnt2.distance(vert_pnt) {
                set_vertex_point(pseudo, adjust_vert, pnt2)?;
                pseudo
                    .fillet_vertex_mut(adjust_vert)?
                    .set_original_t_param(edge_param2);

                side_face = eu1::face(target, side_eu1)?;
                vu_on_side_face = vu1;
                small_rad_geom = fillet_geom1;
                large_rad_geom = fillet_geom2;
                pnt_on_side_face = pnt1;
            } else {
                set_vertex_point(pseudo, adjust_vert, pnt1)?;
                pseudo
                    .fillet_vertex_mut(adjust_vert)?
                    .set_original_t_param(edge_param1);

                let radial = eu1::radial(target, side_eu1)?
                    .ok_or_else(|| invalid(OP_TOPO, "side edge use without radial partner"))?;
                side_face = eu1::face(target, radial)?;
                vu_on_side_face = vu2;
                small_rad_geom = fillet_geom2;
                large_rad_geom = fillet_geom1;
                pnt_on_side_face = pnt2;
            }

            // The vert that is farther from the filleted vertex is now done.
            pseudo
                .fillet_vertex_mut(adjust_vert)?
                .set_status(FilStatus::Processed);

            let vert_on_side_edge = adjust_vert;

            // Create new fillet vertex on the 'side' face
            let vert_on_side_face = pseudo.brep_mut().add_vertex(pnt_on_side_face)?;
            let mut fv = FilletVertex::new(k, this_id);
            fv.set_point_class(PointClassType::Face, side_face.map(EntityRef::Face));
            fv.set_fillet_vertex_type(FilletVertexType::FilletXFillet);
            pseudo.insert_fillet_vertex(vert_on_side_face, fv);

            let mut insert_after = true;
            let mut index = ii + 1;
            let other_vert = if index < self.vertices.len() {
                *self
                    .vertices
                    .get(index)
                    .ok_or_else(|| invalid(OP_TOPO, "vertex index"))?
            } else {
                // Insert new vertex before i
                index -= 1;
                insert_after = false;
                *index
                    .checked_sub(1)
                    .and_then(|i| self.vertices.get(i))
                    .ok_or_else(|| invalid(OP_TOPO, "vertex index"))?
            };

            self.vertices.insert(index, vert_on_side_face);

            // Find the vertexuse of rail which is on the
            // 'side' face and reconnect it to the newly created vertex.
            move_vertexuse(
                pseudo,
                vu_on_side_face,
                vert_on_side_edge,
                vert_on_side_face,
            )?;

            // Recreate fillet edges
            let fillet_edge1 = if insert_after {
                pseudo.make_fillet_edge(
                    Some(vert_on_side_edge),
                    Some(vert_on_side_face),
                    this_id,
                )?
            } else {
                pseudo.make_fillet_edge(
                    Some(vert_on_side_face),
                    Some(vert_on_side_edge),
                    this_id,
                )?
            };
            pseudo
                .fillet_edge_mut(fillet_edge1)?
                .set_fillet_edge_type(FilletEdgeType::FilletXSideFace);

            let prim_eu = primary_eu(pseudo, fillet_edge1, OP_TOPO)?;
            pseudo.set_edgeuse_fillet_geom(prim_eu, Some(large_rad_geom))?;
            pseudo
                .fillet_edge_mut(fillet_edge1)?
                .set_original_face(side_face);

            let fillet_edge2 = if insert_after {
                pseudo.make_fillet_edge(Some(vert_on_side_face), Some(other_vert), this_id)?
            } else {
                pseudo.make_fillet_edge(Some(other_vert), Some(vert_on_side_face), this_id)?
            };
            pseudo
                .fillet_edge_mut(fillet_edge2)?
                .set_fillet_edge_type(FilletEdgeType::FilletXFillet);

            let prim_eu = primary_eu(pseudo, fillet_edge2, OP_TOPO)?;
            let mate_eu = eu1::mate(pseudo.brep(), prim_eu)?
                .ok_or_else(|| invalid(OP_TOPO, "new fillet edge use without mate"))?;

            pseudo.set_edgeuse_fillet_geom(prim_eu, Some(large_rad_geom))?;
            pseudo.set_edgeuse_fillet_geom(mate_eu, Some(small_rad_geom))?;

            // (lIndex >= 1: it is ii + 1, or ii >= 1 when it was decremented)
            let index = index
                .checked_sub(1)
                .ok_or_else(|| invalid(OP_TOPO, "edge index"))?;
            if index >= self.edges.len() {
                // m_vEdges[lIndex] past the array
                return Err(invalid(OP_TOPO, "edge index"));
            }
            let removed_edge = self.edges.remove(index);
            pseudo.delete_fillet_edge(removed_edge)?;

            if insert_after {
                self.edges.push(fillet_edge1);
                self.edges.push(fillet_edge2);
            } else {
                self.edges.push(fillet_edge2);
                self.edges.push(fillet_edge1);
            }
            ii += 1;
            // (the loop's own ii++)
            ii += 1;
        }

        // all done
        Ok(())
    }

    // ---- AdjustTangentCorner ---------------------------------------------------------

    /// `AdjustTangentCorner` (virtual): the Nx2 and the closed Nx1 overrides; every other
    /// kind does nothing.
    ///
    /// # Errors
    ///
    /// Those of [`FilletCorner::adjust_tangent_corner_nx2`] and
    /// [`FilletCorner::adjust_tangent_corner_nx1_closed`].
    pub fn adjust_tangent_corner(&mut self, env: &mut CornerEnv<'_>) -> Result<()> {
        let CornerEnv {
            k,
            target,
            pseudo,
            exec,
            corner,
            solvers,
            ..
        } = env;
        let mut ops = CornerEnvTangentOps {
            exec,
            solvers,
            corner: *corner,
        };
        let mut env = TangentSplitEnv {
            k,
            target,
            pseudo,
            corner: *corner,
            ops: &mut ops,
        };
        self.adjust_tangent_corner_with(
            &mut env,
            |corner, env| corner.adjust_tangent_corner_nx2(env),
            |corner, env| corner.adjust_tangent_corner_nx1_closed(env),
        )
    }

    /// Dispatch tangent adjustment against the caller's actual derived context.
    /// The inherited native base operation requires no context.
    ///
    /// # Errors
    /// The selected concrete callback's failures.
    pub fn adjust_tangent_corner_with<C>(
        &mut self,
        context: &mut C,
        nx2: impl FnOnce(&mut Self, &mut C) -> Result<()>,
        nx1_closed: impl FnOnce(&mut Self, &mut C) -> Result<()>,
    ) -> Result<()> {
        match self.kind {
            FilletCornerKind::NByTwo => nx2(self, context),
            FilletCornerKind::NByOneClosed => nx1_closed(self, context),
            _ => Ok(()),
        }
    }

    /// The opening checks of `SmFilletNx2Corner::AdjustTangentCorner`: whether this
    /// corner has exactly two filleted edge uses, four edge uses around it and its two
    /// filleted edges tangent (within 2 degrees) at the vertex; otherwise the
    /// adjustment does nothing.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the edge-use evaluations (their other failures
    /// leave the derivative default-constructed, as upstream ignores the status).
    pub fn is_nx2_tangent_adjustment_corner(&self, k: &Kernel, target: &mut Brep) -> Result<bool> {
        // First check that this is the appropriate type.
        // Must be Nx2 tangent corner, and currently N must be 4.
        let Some(&[eu_a, eu_b]) = self.solver_eus.as_deref() else {
            return Ok(false);
        };
        if self.all_eus.as_ref().is_none_or(|v| v.len() != 4) {
            return Ok(false);
        }

        // Check that the two filleted edges are tangent at this vertex.
        // (the status is ignored upstream: a failed evaluation leaves the derivative
        // default-constructed)
        let mut deriv = |eu: EdgeuseId| -> Result<Vec3> {
            match eu3::normalized_evaluate(k, target, eu, 0.0, false, true, true) {
                Ok(ev) => Ok(ev.first_deriv.unwrap_or(Vec3::UNDEF)),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(Vec3::UNDEF),
            }
        };
        let first_deriv1 = deriv(eu_a)?;
        let first_deriv2 = deriv(eu_b)?;

        let ang_tol_deg = 2.0; // cbi: some places use 5 degrees; sort that all out.
        Ok(first_deriv1.is_parallel_to(first_deriv2, ang_tol_deg))
    }

    /// `SmFilletNx2Corner::AdjustTangentCorner`: for a 4x2 corner whose two filleted
    /// edges are tangent at the vertex (within 2 degrees), checks that the two fillets'
    /// surfaces meet where their rails hit the side edges; when they do not, splits the
    /// fillet geometries at the side edges so another fillet surface fills the gap; the
    /// split geometries are recalculated and the other geometry is trimmed back to the
    /// split vertex.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] / `SM_ERR_NULL_POINTER` where upstream dereferences
    /// null (a missing rail, rail curve, rail vertex use, solver or intersection
    /// surface); [`SolidError::Unported`] (module docs); those of the evaluations and of
    /// `GetRailIndexOfVertex`.
    pub fn adjust_tangent_corner_nx2(&mut self, env: &mut TangentSplitEnv<'_>) -> Result<()> {
        if !self.is_nx2_tangent_adjustment_corner(env.k, env.target)? {
            return Ok(());
        }

        // Ok, we are an Nx2-tangent corner.
        let mut close_enough = true;

        for &adjust_vert in &self.vertices {
            if env.pseudo.fillet_vertex(adjust_vert)?.fillet_vertex_type()
                != FilletVertexType::RailXEdgeuse
            {
                continue;
            }

            let geoms = env.pseudo.vertex_fillet_geoms(adjust_vert)?;
            let [fillet_geom1, fillet_geom2] = geoms[..] else {
                continue; // maybe already adjusted...
            };

            // We need the rail curves to work from.
            // If not there yet, just wait for a later call. (Upstream's `||` chain
            // short-circuits: the first missing curve returns before a later rail is
            // looked at.)
            for g in [fillet_geom1, fillet_geom2] {
                for r in 0..2 {
                    let rail = env
                        .pseudo
                        .geom(g)?
                        .rail(r)
                        .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                    if env.pseudo.brep().edge(rail)?.curve.is_none() {
                        return Ok(());
                    }
                }
            }

            // Get the vertexuses corresponding to the two surfaces, and their TSectPnts:
            let vu1 = env
                .pseudo
                .vu_at_rail_end(adjust_vert, Some(fillet_geom1), None, None)?
                .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
            let vu2 = env
                .pseudo
                .vu_at_rail_end(adjust_vert, Some(fillet_geom2), Some(vu1), None)?
                .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
            let int_pt1 = env.pseudo.fillet_vertexuse(vu1)?.tsect_pnt().clone();
            let int_pt2 = env.pseudo.fillet_vertexuse(vu2)?.tsect_pnt().clone();

            // (SmSurface*)rIntPt.m_apUserPointer[i]
            let user_surface = |int_pt: &TsectPnt, i: usize| -> Result<Surface> {
                let mut s = int_pt
                    .user_pointer
                    .get(i)
                    .and_then(Option::as_ref)
                    .map(|u| (*u.0).clone())
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                // SmTemporaryChangeValue: GetOutOfBoundsEnabled() = TRUE on every
                // SmBSplineSurface (SM_CAST_PTR also succeeds for the analytic kinds
                // derived from it: SmPlane, SmSurfOfRevolution)
                match s.kind_mut() {
                    SurfaceKind::BSpline(b) => *b.out_of_bounds_enabled_mut() = true,
                    SurfaceKind::Plane(p) => *p.out_of_bounds_enabled_mut() = true,
                    SurfaceKind::SurfOfRevolution(r) => {
                        *r.base_mut().out_of_bounds_enabled_mut() = true;
                    }
                    SurfaceKind::SurfOfExtrusion(r) => {
                        *r.base_mut().out_of_bounds_enabled_mut() = true;
                    }
                    SurfaceKind::Cone(c) => {
                        *c.base_mut().out_of_bounds_enabled_mut() = true;
                    }
                    SurfaceKind::Sphere(c) => {
                        *c.base.base_mut().out_of_bounds_enabled_mut() = true;
                    }
                    SurfaceKind::Torus(c) => {
                        *c.base.base_mut().out_of_bounds_enabled_mut() = true;
                    }
                    SurfaceKind::CurveBounded(_)
                    | SurfaceKind::Step(_)
                    | SurfaceKind::OffsetSurface(_)
                    | SurfaceKind::SrfInVolume(_) => {}
                }
                Ok(s)
            };
            let srf1a = user_surface(&int_pt1, 0)?;
            let srf1b = user_surface(&int_pt1, 1)?;
            let srf2a = user_surface(&int_pt2, 0)?;
            let srf2b = user_surface(&int_pt2, 1)?;

            let srf1_pt =
                unchecked_point(srf1a.evaluate_point(env.k, int_pt1.uv_pos(SrfIndex::First)))?;
            let srf2_pt =
                unchecked_point(srf2a.evaluate_point(env.k, int_pt2.uv_pos(SrfIndex::First)))?;

            if srf1_pt.distance(srf2_pt) > self.approx_tol_3d {
                close_enough = false;
                break;
            }

            // The mates must agree as well for 4x2
            let srf1_pt =
                unchecked_point(srf1b.evaluate_point(env.k, int_pt1.uv_pos(SrfIndex::Second)))?;
            let srf2_pt =
                unchecked_point(srf2b.evaluate_point(env.k, int_pt2.uv_pos(SrfIndex::Second)))?;

            if srf1_pt.distance(srf2_pt) > self.approx_tol_3d {
                close_enough = false;
                break;
            }
        }

        if close_enough {
            return Ok(());
        }

        // Ok, we have to make a new fillet face in between.
        // When we process a vertex, we don't want to process its mate.
        let mut fvs_to_process = self.vertices.clone();

        let mut which_fv = 0usize;
        while which_fv < fvs_to_process.len() {
            let this_fv = *fvs_to_process
                .get(which_fv)
                .ok_or_else(|| invalid(OP_TANGENT, "vertex index"))?;

            // Looking for a vertex where a rail hits a side edge:
            if env.pseudo.fillet_vertex(this_fv)?.fillet_vertex_type()
                != FilletVertexType::RailXEdgeuse
            {
                which_fv += 1;
                continue;
            }

            let geoms = env.pseudo.vertex_fillet_geoms(this_fv)?;
            let [geom0, geom1] = geoms[..] else {
                which_fv += 1;
                continue;
            };

            // This Fillet Vertex has two Fillet Vertexuses, one on each FilletGeom.
            let vu0 = env
                .pseudo
                .vu_at_rail_end(this_fv, Some(geom0), None, None)?;
            let vu1 = env.pseudo.vu_at_rail_end(this_fv, Some(geom1), vu0, None)?;
            let vus = [vu0, vu1];

            let vertex_pos = env.pseudo.vertex_point(this_fv)?;

            // This is something we'll need after the next loop:
            let mut opposite_fv: Option<VertexId> = None;

            // Look at both FilletGeoms
            for which_fillet_geom in 0..2usize {
                let orig_geom = geoms
                    .get(which_fillet_geom)
                    .copied()
                    .ok_or_else(|| invalid(OP_TANGENT, "geometry index"))?;
                let this_fv_use = vus.get(which_fillet_geom).copied().flatten();

                // Grap the correct rail
                let rail_idx = env.pseudo.rail_index_of_vertex(orig_geom, Some(this_fv))?;
                let this_rail = env
                    .pseudo
                    .geom(orig_geom)?
                    .rail(rail_idx)
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;

                let rail_ivl = env.pseudo.brep().edge(this_rail)?.interval;
                let rail_curve = env
                    .pseudo
                    .brep()
                    .edge(this_rail)?
                    .curve
                    .clone()
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                let drop = rail_curve.drop_point(
                    env.k,
                    &rail_ivl,
                    vertex_pos,
                    None,
                    SM_BIG_DOUBLE,
                    None,
                    SolverOperation::Minimize,
                );
                // the status is ignored upstream (bSuccess stays unset on a failure):
                // a failed drop counts as no drop point
                let drop = match drop {
                    Ok(d) => d,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };
                if !drop.success {
                    continue;
                }
                let split_param = drop.parameter;

                if rail_ivl
                    .is_value_on_boundary(split_param, self.approx_tol_3d)
                    .0
                {
                    continue;
                }

                // Which end of the FilletGeom are we at?  0 for start, 1 for end.
                let which_end =
                    usize::from(env.pseudo.rail_vertexuse(orig_geom, rail_idx, 1)? == this_fv_use);

                if which_end == 0 {
                    env.pseudo
                        .geom_mut(orig_geom)?
                        .set_fillet_geom_type(FilletGeomType::TangentRollover);
                }

                // pOrigGeom->GetFilletSolver()->SplitFilletGeomAtSideEdge(..): upstream
                // dereferences a NULL solver
                let si = env
                    .pseudo
                    .geom(orig_geom)?
                    .fillet_solver()
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                // a NULL fillet vertex use fails GetRailIndexOfVertexuse's NER
                let split = match this_fv_use {
                    Some(vu) => env.ops.split_fillet_geom_at_side_edge(
                        env.k, env.target, env.pseudo, self, si, orig_geom, vu, which_end,
                    ),
                    None => Err(SolidError::null_pointer(OP_TANGENT)),
                };
                // If failure, the whole fillet could still succeed, so don't SER.
                let new_geom = match split {
                    Ok(g) => g,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };

                env.pseudo
                    .fillet_vertex_mut(this_fv)?
                    .set_fillet_vertex_type(FilletVertexType::Mate);
                env.pseudo
                    .fillet_vertex_mut(this_fv)?
                    .set_status(FilStatus::Unprocessed);

                // Set it as mate of its opposite fillet vertex.
                let other_rail = 1usize
                    .checked_sub(rail_idx)
                    .ok_or_else(|| invalid(OP_TANGENT, "rail index"))?;
                let opposite_fvu = if which_end == 0 {
                    env.pseudo.rail_vertexuse(orig_geom, other_rail, 0)?
                } else {
                    env.pseudo.rail_vertexuse(new_geom, other_rail, 1)?
                };
                let opposite_fvu =
                    opposite_fvu.ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                opposite_fv = env.pseudo.brep().vertexuse(opposite_fvu)?.vertex;

                if let Some(opp) = opposite_fv {
                    env.pseudo
                        .fillet_vertex_mut(this_fv)?
                        .set_mate(0, Some(opp));
                    env.pseudo
                        .fillet_vertex_mut(opp)?
                        .set_mate(0, Some(this_fv));
                    env.pseudo
                        .fillet_vertex_mut(opp)?
                        .set_status(FilStatus::Unprocessed);
                }

                // Also, pThisFV is now in a face.
                let new_rail = env
                    .pseudo
                    .geom(new_geom)?
                    .rail(rail_idx)
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
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

                // Also: there are now two new FilletVertex's, which belong
                // to this FilletCorner.  Add them to our list, and set
                // their back-pointer to us.
                // 'Same' is on the same side as pThisFV, 'Opp' as pOppositeFV.
                // Also recall that the new FilletVertex's are at the high end
                // of the original FG -- hence the 2nd arguments:
                let new_fv_same = env.pseudo.rail_vertex(orig_geom, rail_idx, 1)?;
                let new_fv_opp = env.pseudo.rail_vertex(orig_geom, other_rail, 1)?;

                // pNewFVSame should be all set, it's what pThisFV used to be.
                // But pNewFVOpp is now in a face.
                let opp_rail = env
                    .pseudo
                    .geom(orig_geom)?
                    .rail(other_rail)
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                let opp_base_face = env.pseudo.fillet_edge(opp_rail)?.original_face();
                env.pseudo
                    .fillet_vertex_mut(new_fv_opp)?
                    .set_point_class(PointClassType::Face, opp_base_face.map(EntityRef::Face));

                // We also have to take responsibility for the new edge.
                let new_edges =
                    vertex::common_edges(env.pseudo.brep(), new_fv_same, Some(new_fv_opp), None)?;
                if let Some(&this_fil_edge) = new_edges.first() {
                    self.edges.push(this_fil_edge);
                    env.pseudo
                        .fillet_edge_mut(this_fil_edge)?
                        .set_fillet_corner(env.corner);
                }

                // We have to update the fillet surfaces in the fillet geoms here.
                // A prerequisite to that is recalculating the vertex geom (its status is
                // ignored upstream).
                self.calc_corner_vert_geom_unchecked(env)?;

                // We need a direction for ReCalcFilletGeom.
                // We already dropped this vertex to the rail being split,
                // so just evaluate the rail's tangent direction there.
                let march_dir = rail_tangent_unchecked(env, this_rail, split_param)?;

                // Tell FilletSolver to use surfaces in the FilletGeoms.
                // First back up what's there, if anything, to restore when done.
                recalc_split_geoms(env, si, orig_geom, new_geom, march_dir)?;

                // Now, the other FilletGeom, the one that didn't get split,
                // needs to be trimmed back to the far end of the split FG.
                let other_geom = geoms
                    .get(1 - which_fillet_geom)
                    .copied()
                    .ok_or_else(|| invalid(OP_TANGENT, "geometry index"))?;

                // Grap the correct rail (GetRailIndexOfVertex's status is ignored; it
                // leaves the index unset when it fails)
                let other_rail_idx = env.pseudo.rail_index_of_vertex(other_geom, Some(this_fv))?;
                let other_rail = env
                    .pseudo
                    .geom(other_geom)?
                    .rail(other_rail_idx)
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;

                // The fillet vertex at which the other FG gets trimmed to
                // is still pointed to by pThisFV.  (See earlier comments.)
                let trim_pt = env.pseudo.vertex_point(this_fv)?;

                let mut other_rail_ivl = env.pseudo.brep().edge(other_rail)?.interval;

                let other_curve = env
                    .pseudo
                    .brep()
                    .edge(other_rail)?
                    .curve
                    .clone()
                    .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
                let drop = other_curve.drop_point(
                    env.k,
                    &other_rail_ivl,
                    trim_pt,
                    None,
                    SM_BIG_DOUBLE,
                    None,
                    SolverOperation::Minimize,
                );
                // (the status is ignored upstream: a failed drop counts as no drop point)
                let drop = match drop {
                    Ok(d) => d,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };
                if !drop.success {
                    continue;
                }
                let other_split_param = drop.parameter;

                // Low or high end of other FG? (upstream compares the rail vertex use of
                // the split geometry's rail index)
                let other_fv_use = vus.get(1 - which_fillet_geom).copied().flatten();
                let other_which_end = usize::from(
                    env.pseudo.rail_vertexuse(other_geom, rail_idx, 1)? == other_fv_use,
                );

                // Trim sOtherRailIvl accordingly (SetMinMax's status is ignored: a
                // negative interval leaves it unchanged)
                if other_which_end == 0 {
                    ignore_status(
                        other_rail_ivl.set_min_max(other_split_param, other_rail_ivl.max()),
                    )?;
                } else {
                    ignore_status(
                        other_rail_ivl.set_min_max(other_rail_ivl.min(), other_split_param),
                    )?;
                }

                // Finally, do the trim (status ignored).
                ignore_status(env.pseudo.trim_geom(env.k, other_geom, &mut other_rail_ivl))?;
            }

            // Remove the opposite-rail FilletVertex from the list to be processed.
            if let Some(opp) = opposite_fv {
                if let Some(idx) = fvs_to_process.iter().position(|&v| v == opp) {
                    fvs_to_process.remove(idx);
                    if idx <= which_fv {
                        // if we removed one before this one, slide us back one.
                        // (upstream's unsigned decrement wraps from 0 and the loop
                        // increment brings it back)
                        if which_fv == 0 {
                            continue;
                        }
                        which_fv -= 1;
                    }
                }
            }
            which_fv += 1;
        }

        Ok(())
    }
}

/// The executive's objects the tangent-corner splits (`AdjustTangentCorner` of
/// `SmFilletNx2Corner` / `SmFilletNx1ClosedCorner`) reach: the fillet solvers (the
/// base solver's side-edge split and the concrete solver's virtual operations for
/// `ReCalcFilletGeom`) and the corner's own `CalcCornerVertGeom`. Implemented by the
/// legacy [`CornerEnv`] and by a live executive session.
pub trait TangentSplitOps {
    /// The base of solver `id` (`pFG->GetFilletSolver()`).
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] for a solver missing from the store.
    fn solver(&self, id: FilletSolverId) -> Result<&FilletSolver>;
    /// Solver `id`'s virtual operations (the solver `ReCalcFilletGeom` runs against).
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] for a solver missing from the store.
    fn solver_ops(&mut self, id: FilletSolverId) -> Result<&mut dyn FilletSolverOps>;
    /// `SmFilletSolver::SplitFilletGeomAtSideEdge` on solver `id`, from the running
    /// corner `corner` (its `TopologySplit` gives the new vertices to the corner of the
    /// solver's end vertex, which may be `corner`).
    ///
    /// # Errors
    /// Those of [`FilletSolver::split_fillet_geom_at_side_edge`];
    /// [`SolidError::InvalidInput`] for a solver missing from the store.
    #[allow(clippy::too_many_arguments)] // upstream's arguments plus the borrowed objects
    fn split_fillet_geom_at_side_edge(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        corner: &mut FilletCorner,
        id: FilletSolverId,
        fillet_geom: FilletGeomId,
        split_vertexuse: VertexuseId,
        which_end: usize,
    ) -> Result<FilletGeomId>;
    /// `corner->CalcCornerVertGeom()` (the virtual corner-vertex calculation of
    /// `corner`, identified by `corner_id`).
    ///
    /// # Errors
    /// Those of the corner-vertex calculation.
    fn calc_corner_vert_geom(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        corner: &mut FilletCorner,
        corner_id: Option<FilletCornerId>,
    ) -> Result<()>;
}

/// What the tangent-corner splits operate on besides the corner itself.
pub struct TangentSplitEnv<'e> {
    /// `m_crContext`.
    pub k: &'e Kernel,
    /// The B-rep being filleted.
    pub target: &'e mut Brep,
    /// `m_pPseudoBrep`.
    pub pseudo: &'e mut FilletBrep,
    /// The corner's identity (`this`, as new fillet edges store it).
    pub corner: Option<FilletCornerId>,
    /// The executive's solvers and corner-vertex calculation.
    pub ops: &'e mut dyn TangentSplitOps,
}

impl std::fmt::Debug for TangentSplitEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TangentSplitEnv")
            .field("corner", &self.corner)
            .finish_non_exhaustive()
    }
}

/// The legacy [`CornerEnv`]'s solver table and executive members as
/// [`TangentSplitOps`].
struct CornerEnvTangentOps<'a> {
    exec: &'a mut FilletExecutiveData,
    solvers: &'a mut [FilletSolver],
    corner: Option<FilletCornerId>,
}

/// The legacy executive data's corners (`m_vFilletCorners`, `m_vVertexCornerMap`) and
/// the running corner (out of the table) as [`SplitCorners`].
pub struct ExecutiveSplitCorners<'a> {
    /// The executive's members.
    pub exec: &'a mut FilletExecutiveData,
    /// The running corner and its id.
    pub this: Option<(Option<FilletCornerId>, &'a mut FilletCorner)>,
}

impl std::fmt::Debug for ExecutiveSplitCorners<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutiveSplitCorners")
            .finish_non_exhaustive()
    }
}

impl SplitCorners for ExecutiveSplitCorners<'_> {
    fn corner_of_vertex(&self, vertex: VertexId) -> Option<FilletCornerId> {
        self.exec.vertex_corner_map.get(&vertex).copied()
    }
    fn lookup(&self) -> CornerLookup<'_> {
        CornerLookup {
            table: None,
            this: self.this.as_ref().map(|(id, c)| (*id, &**c)),
            exec: Some(&*self.exec),
        }
    }
    fn corner_mut(&mut self, id: FilletCornerId) -> Result<&mut FilletCorner> {
        if let Some((this_id, this)) = self.this.as_mut() {
            if *this_id == Some(id) {
                return Ok(this);
            }
        }
        self.exec
            .corners
            .get_mut(&id)
            .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))
    }
}

impl CornerEnvTangentOps<'_> {
    fn slot(&mut self, id: FilletSolverId) -> Result<&mut FilletSolver> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.solvers.get_mut(i))
            .ok_or_else(|| invalid(OP_TANGENT, "fillet solver id not in the solver table"))
    }
}

impl TangentSplitOps for CornerEnvTangentOps<'_> {
    fn solver(&self, id: FilletSolverId) -> Result<&FilletSolver> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.solvers.get(i))
            .ok_or_else(|| invalid(OP_TANGENT, "fillet solver id not in the solver table"))
    }
    fn solver_ops(&mut self, id: FilletSolverId) -> Result<&mut dyn FilletSolverOps> {
        Ok(self.slot(id)?)
    }
    fn split_fillet_geom_at_side_edge(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        corner: &mut FilletCorner,
        id: FilletSolverId,
        fillet_geom: FilletGeomId,
        split_vertexuse: VertexuseId,
        which_end: usize,
    ) -> Result<FilletGeomId> {
        let fs = usize::try_from(id.0)
            .ok()
            .and_then(|i| self.solvers.get_mut(i))
            .ok_or_else(|| invalid(OP_TANGENT, "fillet solver id not in the solver table"))?;
        fs.split_fillet_geom_at_side_edge(
            k,
            target,
            pseudo,
            fillet_geom,
            split_vertexuse,
            which_end,
            &mut ExecutiveSplitCorners {
                exec: self.exec,
                this: Some((self.corner, corner)),
            },
        )
    }
    fn calc_corner_vert_geom(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &mut FilletBrep,
        corner: &mut FilletCorner,
        corner_id: Option<FilletCornerId>,
    ) -> Result<()> {
        let exec = &*self.exec;
        let mut context = (target, pseudo, &mut *self.solvers);
        let this: &FilletCorner = corner;
        this.calc_corner_vert_geom_with(
            &mut context,
            |(_, pseudo, _), fv| Ok(pseudo.fillet_vertex(fv)?.is_processed()),
            |(target, pseudo, solvers), fv| {
                pseudo.calc_corner_vert_geom(
                    k,
                    fv,
                    CornerLookup::of_corner(corner_id, this, exec),
                    target,
                    solvers,
                )
            },
            |(target, pseudo, solvers), corner, processed| {
                corner.adjust_vertex_law_intervals(pseudo, target, *solvers, processed)
            },
        )
    }
}

impl FilletCorner {
    /// `this->CalcCornerVertGeom()` with its status ignored (the tangent-corner
    /// adjustments): only [`SolidError::Unported`] propagates.
    pub(super) fn calc_corner_vert_geom_unchecked(
        &mut self,
        env: &mut TangentSplitEnv<'_>,
    ) -> Result<()> {
        let corner_id = env.corner;
        ignore_status(
            env.ops
                .calc_corner_vert_geom(env.k, env.target, env.pseudo, self, corner_id),
        )
    }
}

/// `pRail->GetCurve()->Evaluate(t, 1, FALSE, sPtDer)` with its status ignored: the first
/// derivative, or the default-constructed vector ([`Vec3::UNDEF`]) when it fails.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a rail without curve (upstream dereferences it);
/// [`SolidError::Unported`].
pub(super) fn rail_tangent_unchecked(
    env: &TangentSplitEnv<'_>,
    rail: EdgeId,
    t: f64,
) -> Result<Vec3> {
    let curve = env
        .pseudo
        .brep()
        .edge(rail)?
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP_TANGENT))?;
    Ok(ignored(curve.evaluate(env.k, t, 1, false, true))?
        .and_then(|pd| pd.get(1).copied())
        .unwrap_or(Vec3::UNDEF))
}

/// The two `ReCalcFilletGeom(FALSE, &dir)` calls after a split (statuses ignored), each
/// with the solver's fillet geometry for the surfaces set to the recalculated geometry
/// and restored after. The geometries recalculate against the solver's virtual
/// operations ([`TangentSplitOps::solver_ops`]).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a solver missing from the store;
/// [`SolidError::Unported`].
pub(super) fn recalc_split_geoms(
    env: &mut TangentSplitEnv<'_>,
    solver: FilletSolverId,
    orig_geom: FilletGeomId,
    new_geom: FilletGeomId,
    march_dir: Vec3,
) -> Result<()> {
    let fs = env.ops.solver_ops(solver)?;
    let k = env.k;
    let target = &*env.target;
    // pFS->SetFilletGeomForSurfaces(pOrigGeom); pOrigGeom->ReCalcFilletGeom(..), then
    // the same for the other FG, the backup restored after each
    for g in [orig_geom, new_geom] {
        ignore_status(with_geometry_surfaces(fs, env.pseudo, g, |fs, pseudo| {
            pseudo.re_calc_fillet_geom(k, g, fs, target, false, Some(march_dir), None)
        }))?;
    }
    Ok(())
}

/// An evaluation whose status upstream ignores (`EvaluatePoint` without `SER`): a
/// failure leaves the default-constructed point ([`Point3::UNDEF`]); an unported
/// evaluator still propagates.
pub(super) fn unchecked_point(r: Result<Point3>) -> Result<Point3> {
    match r {
        Ok(p) => Ok(p),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(Point3::UNDEF),
    }
}

/// `SmBSplineCurve::JoinWith` (virtual) on an edge's curve: the `SmLine` and `SmEllipse`
/// overrides for those kinds, the B-spline join on the `SmBSplineCurve` part otherwise.
fn join_with(
    k: &Kernel,
    this: &mut Curve,
    this_at_end: bool,
    other: &mut Curve,
    other_at_end: bool,
) -> Result<()> {
    let owner = this.owner();
    match this.kind_mut() {
        CurveKind::Line(l) => {
            l.join_with(u64::from(this_at_end), other, u64::from(other_at_end), None)
        }
        CurveKind::Ellipse(e) => e.join_with(
            k,
            u64::from(this_at_end),
            other,
            u64::from(other_at_end),
            None,
            owner,
        ),
        kind => {
            let this_base =
                bspline_part_mut(kind).ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
            let other_base = bspline_part_mut(other.kind_mut())
                .ok_or_else(|| SolidError::null_pointer(OP_3X2))?;
            this_base.join_with(k, this_at_end, other_base, other_at_end, None)
        }
    }
}

/// The `SmBSplineCurve` part of a curve, mutable (mirrors [`bspline_part`]).
fn bspline_part_mut(kind: &mut CurveKind) -> Option<&mut BSplineCurve> {
    match kind {
        CurveKind::BSpline(c) => Some(c),
        CurveKind::Line(c) => Some(&mut c.base),
        CurveKind::Ellipse(c) => Some(c.base_mut()),
        CurveKind::Hyperbola(c) => Some(c.base_mut()),
        CurveKind::Parabola(c) => Some(c.base_mut()),
        _ => None,
    }
}

/// `sm_TestG1Continuity` (unit `blend.fillet_corner_p1`).
fn test_g1_continuity(
    k: &Kernel,
    b: &Brep,
    vertex: VertexId,
    edge1: EdgeId,
    edge2: EdgeId,
    ang_tol_deg: f64,
) -> Result<bool> {
    Ok(p1::test_g1_continuity(k, b, Some(vertex), Some(edge1), Some(edge2), ang_tol_deg)?.0)
}

/// `SmFilletNx1Corner::CheckNx1Degeneracy` as `Create` tests it
/// (`if (CheckNx1Degeneracy(...))`): the routine returns an `SmBoolean`, and on an
/// evaluation failure its `SER` returns the status through it, which is nonzero, so any
/// failure selects the degenerate corner, whichever Rust error variant carries the
/// native status (an `Upstream` code, or the `InvalidInput` an evaluator reports for a
/// degenerate input, as `SmOffsetCurve::Evaluate`'s zero binormal). Only an unported
/// callee stays an error (the case is deferred).
fn check_nx1_degeneracy(k: &Kernel, b: &mut Brep, vertex: VertexId, eu: EdgeuseId) -> Result<bool> {
    // the status a failed side binormal evaluation returns through the SmBoolean is
    // nonzero (true); the guards (a null dereference, a dangling id, an unported callee)
    // stay errors
    Ok(p1::check_nx1_degeneracy_status(k, b, vertex, eu)?.unwrap_or(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_upstream_types_and_names() {
        assert_eq!(
            FilletCornerKind::NByTwo.corner_type(),
            FilletCornerType::NByTwo
        );
        assert_eq!(
            FilletCornerKind::ConcaveNByN.corner_type(),
            FilletCornerType::NByNConcave
        );
        assert_eq!(FilletCornerKind::Open.name(), "SmFilletOpenCorner");
    }

    #[test]
    fn constructor_defaults() {
        let c = FilletCorner::new(FilletCornerKind::Degenerate, None, None, None, 1e-3, 0.1);
        assert!(c.degenerate);
        assert!(c.extended_surface_patch);
        assert!(!c.bevel && !c.blending && !c.chamfer);
        assert_eq!(c.set_back_dist, 0.0);
        let c = FilletCorner::new(FilletCornerKind::NByOne, None, None, None, 1e-3, 0.1);
        assert!(!c.degenerate);
    }

    /// A vertex with a filleted edge use whose side edge uses exist (clockwise
    /// neighbour, radial's counter-clockwise neighbour) but bound no face: the Nx1
    /// degeneracy test's binormal evaluation returns a status (`SmEdgeuse::EvaluateBinormal`
    /// on a non-loop edge use), which `Create`'s `if (CheckNx1Degeneracy(...))` reads as
    /// true. The standalone helper keeps the status.
    #[test]
    fn nx1_degeneracy_status_selects_the_degenerate_corner() {
        use crate::geom::extent1d::Extent1d;
        use crate::topology::{Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};

        let k = Kernel::default();
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let w = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let e_fil = b.add_edge(v, w).unwrap();
        let e_side = b.add_edge(v, w).unwrap();
        for e in [e_fil, e_side] {
            b.edge_mut(e).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        }
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
            eu
        };
        let fil = add_eu(&mut b, e_fil, OrientType::Same, v);
        let fil_radial = add_eu(&mut b, e_fil, OrientType::Opposite, w);
        let side1 = add_eu(&mut b, e_side, OrientType::Same, v);
        let _side1_mate = add_eu(&mut b, e_side, OrientType::Opposite, w);
        // the clockwise neighbour of the filleted edge use and the counter-clockwise
        // neighbour of its radial coincide: the 2x1 case, the second side is its mate
        b.edgeuse_mut(fil).unwrap().cw = Some(side1);
        b.edgeuse_mut(fil_radial).unwrap().ccw = Some(side1);
        // Create's walk closes through the mate's clockwise neighbour
        b.edgeuse_mut(_side1_mate).unwrap().cw = Some(fil_radial);

        let raw = p1::check_nx1_degeneracy(&k, &mut b, v, fil);
        assert!(matches!(raw, Err(SolidError::Upstream { .. })), "{raw:?}");
        assert!(check_nx1_degeneracy(&k, &mut b, v, fil).unwrap());

        // through Create: the status selects the degenerate corner ...
        let (exec, solvers) = one_solver(fil);
        let c = FilletCorner::create(&k, &mut b, v, &[fil], &exec, &solvers).unwrap();
        assert_eq!(c.map(|c| c.kind), Some(FilletCornerKind::Degenerate));

        // ... while a null dereference inside the test (the side edge use without
        // vertex use) is a guard, not a status, and fails the construction
        b.edgeuse_mut(side1).unwrap().vertexuse = None;
        let raw = p1::check_nx1_degeneracy(&k, &mut b, v, fil);
        assert!(
            matches!(raw, Err(SolidError::InvalidInput { .. })),
            "{raw:?}"
        );
        let r = FilletCorner::create(&k, &mut b, v, &[fil], &exec, &solvers);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
    }

    /// An executive with one base linear solver for `eu` (the tolerances `Create` reads).
    fn one_solver(eu: EdgeuseId) -> (FilletExecutiveData, Vec<FilletSolver>) {
        let mut exec = FilletExecutiveData::default();
        exec.edgeuse_solver_map.insert(eu, FilletSolverId(0));
        let mut fs = FilletSolver::new(1e-3, 1e-3, 0.01);
        fs.fsg = Some(crate::blend::fillet_solver::p1::FilletSurfaceGenerator::Linear);
        (exec, vec![fs])
    }

    /// The same vertex with side loop edge uses whose edge carries an `SmOffsetCurve`
    /// with a zero binormal (base line along `z`, offset-plane normal `z`): upstream's
    /// `SmOffsetCurve::Evaluate` returns `SM_ERR_INVALID_INPUT` from `SER(sUBin.Unitize())`
    /// before the binormal evaluation reads the face, `CheckNx1Degeneracy` forwards it and
    /// `Create` reads the nonzero value as true. The Rust evaluator reports the same
    /// failure as `InvalidInput`, which the wrapper must read as true too.
    #[test]
    fn nx1_degeneracy_evaluator_status_selects_the_degenerate_corner() {
        use crate::curve::line::Line;
        use crate::curve::offset_curve::OffsetCurve;
        use crate::geom::extent1d::Extent1d;
        use crate::topology::{Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};

        let k = Kernel::default();
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let w = b.add_vertex(Point3::new(0.0, 0.0, 1.0)).unwrap();
        let e_fil = b.add_edge(v, w).unwrap();
        let e_side = b.add_edge(v, w).unwrap();
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        for e in [e_fil, e_side] {
            b.edge_mut(e).unwrap().interval = ivl;
        }
        let line = Line::from_points(Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, 1.0), 3);
        let offset = OffsetCurve::new(
            3,
            Curve::from(line),
            ivl,
            Vec3::new(0.0, 0.0, 1.0),
            1.0,
            false,
        )
        .unwrap();
        b.edge_mut(e_side).unwrap().curve = Some(Curve::from(offset));
        let add_eu = |b: &mut Brep,
                      e: EdgeId,
                      orientation: OrientType,
                      at: VertexId,
                      owner: EdgeuseOwner| {
            let eu = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    owner,
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
            eu
        };
        let fil = add_eu(&mut b, e_fil, OrientType::Same, v, EdgeuseOwner::Unknown);
        let fil_radial = add_eu(
            &mut b,
            e_fil,
            OrientType::Opposite,
            w,
            EdgeuseOwner::Unknown,
        );
        // the side edge uses bound a loop: the evaluation reaches the curve
        let side1 = add_eu(
            &mut b,
            e_side,
            OrientType::Same,
            v,
            EdgeuseOwner::Loopuse(None),
        );
        let _side1_mate = add_eu(
            &mut b,
            e_side,
            OrientType::Opposite,
            w,
            EdgeuseOwner::Loopuse(None),
        );
        b.edgeuse_mut(fil).unwrap().cw = Some(side1);
        b.edgeuse_mut(fil_radial).unwrap().ccw = Some(side1);
        b.edgeuse_mut(_side1_mate).unwrap().cw = Some(fil_radial);

        let raw = p1::check_nx1_degeneracy(&k, &mut b, v, fil);
        assert!(
            matches!(raw, Err(SolidError::InvalidInput { .. })),
            "{raw:?}"
        );
        assert!(check_nx1_degeneracy(&k, &mut b, v, fil).unwrap());

        // through Create: the evaluator's status selects the degenerate corner
        let (exec, solvers) = one_solver(fil);
        let c = FilletCorner::create(&k, &mut b, v, &[fil], &exec, &solvers).unwrap();
        assert_eq!(c.map(|c| c.kind), Some(FilletCornerKind::Degenerate));
    }

    fn box_corner() -> Result<(Kernel, Brep, VertexId, Vec<EdgeuseId>)> {
        let mut k = Kernel::default();
        let b = crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 2.0, 3.0, 4.0)?;
        let vertex = b
            .vertices()
            .ids()
            .next()
            .ok_or_else(|| invalid(OP_CREATE, "box vertex"))?;
        let mut edges = std::collections::BTreeSet::new();
        let mut outgoing = Vec::new();
        for &vu in &b.vertex(vertex)?.vertexuses {
            if let Some(eu) = crate::topology::vertexuse::edgeuse(&b, vu)? {
                if let Some(edge) = eu1::edge(&b, eu)? {
                    if edges.insert(edge) {
                        outgoing.push(eu);
                    }
                }
            }
        }
        Ok((k, b, vertex, outgoing))
    }

    #[test]
    fn factory_borrows_live_solver_state_in_edgeuse_order() {
        use std::cell::RefCell;

        let (k, mut b, vertex, outgoing) = box_corner().unwrap();
        assert_eq!(outgoing.len(), 3);
        let mut solvers = [
            FilletSolver::new(0.03, 0.1, 0.3),
            FilletSolver::new(0.02, 0.1, 0.2),
            FilletSolver::new(0.01, 0.1, 0.1),
        ];
        for solver in &mut solvers {
            solver.fsg = Some(crate::blend::fillet_solver::p1::FilletSurfaceGenerator::Linear);
        }
        let seen = RefCell::new(Vec::new());
        let corner = FilletCorner::create_with_solver_lookup(&k, &mut b, vertex, &outgoing, |eu| {
            seen.borrow_mut().push(eu);
            let index = outgoing
                .iter()
                .position(|&candidate| candidate == eu)
                .unwrap();
            Ok(solvers.get(index).unwrap())
        })
        .unwrap()
        .unwrap();
        assert_eq!(*seen.borrow(), outgoing);
        assert_eq!(corner.kind, FilletCornerKind::ConvexNByN);
        assert_eq!(corner.approx_tol_3d, 0.01);
        assert_eq!(corner.tangency_tol_radians, 0.1);

        // The next factory call sees the same owned solver's updated state.
        solvers.get_mut(1).unwrap().approx_tol_3d = 0.001;
        let corner = FilletCorner::create_with_solver_lookup(&k, &mut b, vertex, &outgoing, |eu| {
            let index = outgoing
                .iter()
                .position(|&candidate| candidate == eu)
                .unwrap();
            Ok(solvers.get(index).unwrap())
        })
        .unwrap()
        .unwrap();
        assert_eq!(corner.approx_tol_3d, 0.001);
    }

    #[test]
    fn factory_lookup_failure_preserves_native_lazy_prefix() {
        use std::cell::RefCell;

        let (k, mut b, vertex, outgoing) = box_corner().unwrap();
        assert_eq!(outgoing.len(), 3);
        let mut solver = FilletSolver::new(0.01, 0.1, 0.1);
        solver.fsg = Some(crate::blend::fillet_solver::p1::FilletSurfaceGenerator::Linear);
        let seen = RefCell::new(Vec::new());
        let failure = invalid("factory_lookup_test", "second solver unavailable");
        let result = FilletCorner::create_with_solver_lookup(&k, &mut b, vertex, &outgoing, |eu| {
            seen.borrow_mut().push(eu);
            if seen.borrow().len() == 2 {
                Err(failure.clone())
            } else {
                Ok(&solver)
            }
        });
        assert_eq!(result.unwrap_err(), failure);
        assert_eq!(*seen.borrow(), outgoing.get(..2).unwrap());

        // Invalid topology fails before the first native solver read.
        seen.borrow_mut().clear();
        b.edgeuse_mut(*outgoing.first().unwrap()).unwrap().cw = None;
        let result = FilletCorner::create_with_solver_lookup(&k, &mut b, vertex, &outgoing, |eu| {
            seen.borrow_mut().push(eu);
            Ok(&solver)
        });
        assert!(result.is_err());
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn calc_corner_geom_early_outs() {
        let mut k = Kernel::default();
        let mut target = Brep::new();
        let mut pseudo = FilletBrep::new();
        let mut exec = FilletExecutiveData::default();
        let mut env = CornerEnv {
            k: &mut k,
            target: &mut target,
            pseudo: &mut pseudo,
            exec: &mut exec,
            corner: None,
            solvers: &mut [],
            fillet: None,
        };
        let mut c = FilletCorner::new(FilletCornerKind::FourByThree, None, None, None, 1e-3, 0.1);
        assert!(c.calc_corner_geom(&mut env).is_ok());
    }

    #[test]
    fn create_rejects_empty() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let r = FilletCorner::create(&k, &mut b, v, &[], &FilletExecutiveData::default(), &[]);
        assert!(r.is_err());
    }
}

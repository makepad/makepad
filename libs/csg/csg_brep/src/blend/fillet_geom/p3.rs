//! Fillet geometry, part 3: the corner dispatchers of the fillet vertex and fillet edge
//! (`CalcCornerVertGeom`, `CalcCornerEdgeGeom`), the mate vertex and the
//! fillet/fillet intersection edge, recomputing a fillet geometry's surface through all
//! of its rail ends (`ReCalcFilletGeom`) and splitting a fillet geometry in two
//! topologically (`TopologySplit`).
//!
//! Upstream: `source/SMLib/src/SmFilletGeom.cpp`, `source/SMLib/inc/SmFilletGeom.h`.
//!
//! | Rust                                     | Upstream                              |
//! |------------------------------------------|---------------------------------------|
//! | [`FilletBrep::calc_corner_vert_geom`]    | `SmFilletVertex::CalcCornerVertGeom`  |
//! | [`FilletBrep::calc_mate_geom`], [`FilletBrep::calc_mate_geom_with`] | `SmFilletVertex::CalcMateGeom` |
//! | [`FilletBrep::calc_corner_edge_geom`]    | `SmFilletEdge::CalcCornerEdgeGeom`    |
//! | [`FilletBrep::calc_edge_fillet_int_fillet`] | `SmFilletEdge::CalcFilletIntFillet` |
//! | [`FilletBrep::re_calc_fillet_geom`]      | `SmFilletGeom::ReCalcFilletGeom`      |
//! | [`FilletBrep::topology_split`]           | `SmFilletGeom::TopologySplit`         |
//! | [`new_solver_fillet_geom`]               | `SmFilletGeom(pFilletSolver, eType)` with a base `SmFilletSolver` (CO-PORTED from `blend.fillet_geom_p5`) |
//!
//! Deviations:
//! - **Object model.** As in `blend.fillet_geom_p1`, `this` is a pseudo-B-rep id of the
//!   [`FilletBrep`] (`self`); the target B-rep the vertex classifications and the
//!   solvers' edge uses refer to is `target`. `GetFilletSolver()` of a geometry is an
//!   opaque [`FilletSolverId`]: the dispatchers resolve it in the caller's `solvers`
//!   table (index `id.0`); `ReCalcFilletGeom` and `TopologySplit` take their geometry's
//!   solver directly (`&mut dyn FilletSolverOps` for the virtual `SetupOffsetValues`);
//!   the solver's geometries (`GetFirstFilletGeom`, `GetLastFilletGeom`) are this
//!   pseudo B-rep's [`FilletGeomId`]s.
//!   `GetFilletExecutive()->GetPseudoBrep()` (unit `blend.fillet_executive_p1`) is
//!   `self`; a solver without an executive is [`SolidError::InvalidInput`] (upstream
//!   dereferences `NULL`). The `SmContext` of the corner or solver is the [`Kernel`].
//! - **Recursion.** `CalcMateGeom` computes its main vertex first through
//!   `CalcCornerVertGeom`, which recurses into `CalcMateGeom` when the main vertex is a
//!   mate too. Upstream does not bound this (a cycle of mates overflows the stack);
//!   here the nesting is bounded by [`MAX_MATE_DEPTH`] (upstream's corners nest it at
//!   most once) and deeper nesting is [`SolidError::InvalidInput`], which propagates
//!   through the ignored status of the inner call.
//! - **Statuses.** `SER`/`NER` failures are errors (`SM_ERR`, `SM_ERR_NULL_POINTER`);
//!   ignored upstream statuses (the main vertex's `CalcCornerVertGeom`,
//!   `SmExtent1d::SetMinMax` of the trim interval) are ignored here too, except that
//!   [`SolidError::Unported`] always propagates. A `NULL` dereference upstream (a
//!   missing primary edge use, mate, vertex, vertex use or solver) is
//!   `SM_ERR_NULL_POINTER`; a classification object of the wrong kind where upstream
//!   casts blindly is [`SolidError::InvalidInput`].
//! - **Offset surfaces.** `ReCalcFilletGeom` evaluates the solver's own first offset
//!   surface after `SetupOffsetValues` (so an offset the solver changes is seen) and
//!   tests the closure, seams and extension of copies of the solver's offsets wrapped
//!   as a [`Surface`] (their evaluations dispatch to the offset, as upstream's virtual
//!   calls do).
//! - **Fillet intersector.** `ReCalcFilletGeom` builds the
//!   [`fillet_intersector::FilletIntersector`] over copies of the solver's offset
//!   surfaces (as the offset tests above). Its `DoPointIntersection` runs
//!   the base driver with the fillet overrides ([`fillet_intersector::FilletIntersector::do_point_intersection`]),
//!   stepping through the solver as `IntersectorSolver` (`blend.fillet_solver_p4`).
//! - **Analytic fillets.** `CalcConeFilletGeom` / `CalcTorusFilletGeom`
//!   (`blend.fillet_solver_p2`) read copies of the sorted vertex uses' points
//!   (`SmTArray<SmTsectPnt*>` upstream; the builders only read them).
//! - **Unported callees.** `SmFilletEdge::CalcCrossSection`
//!   (`blend.fillet_geom_p5`) after the intersection (it needs a solver table `ReCalcFilletGeom` does
//!   not have); after a failed intersection in the edge's `CalcFilletIntFillet`,
//!   `SmFilletCorner::RecordFilletError` (`blend.fillet_corner_p2`) records into the
//!   caller's [`ErrorInfo`] (a scratch record when the caller has none; the
//!   dispatchers' vertex and edge calculators of parts 2, 4, 5 and 6 are called); in
//!   `TopologySplit` the executive's `GetFilletCornerOfVertex` and corner table are the
//!   caller's [`SplitCorners`] ([`NoCorners`] for a caller without an executive: a
//!   `NULL` corner is `SM_ERR_NULL_POINTER`, where upstream dereferences it).
//! - **Topology edits.** `TopologySplit`'s `pThisRail->Remove(pEndEU)`
//!   (`SmOwningTopology::Remove`) unlinks the edge use from the rail's list and clears
//!   its edge, as `blend.fillet_geom_p1`'s destructors unlink uses; `PostInsert`
//!   appends it to the new rail's list. The new fillet vertex (`new (pPseudoBrep)
//!   SmFilletVertex()`, CO-PORTED in `blend.fillet_geom_p1`) is a pseudo-B-rep vertex
//!   at the origin with an unknown type and no corner.
//! - **Tests.** `TopologySplit` is covered by this file's unit tests only: the C++
//!   needs a real `SmFilletExecutive` (it calls `GetFilletCornerOfVertex`), so difftest
//!   does not run it.

use crate::blend::fillet_corner::p3::{ExecutiveErrorObject, FilletCorner};
use crate::blend::fillet_custom_solver::OffsetSurface;
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_intersector;
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::intersect::surface_intersector::p1::{IntersectionPointType, SrfIndex, TsectPnt};
use crate::intersect::surface_intersector::p4::PointIntersectionOptions;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::edge;
use crate::topology::edgeuse::p1::{self as eu1, primary_edgeuse};
use crate::topology::{Brep, EdgeId, EdgeuseId, VertexId, VertexuseId};

use super::p1::{
    check_seam_on, check_seam_out, evaluate_mate, set_surface_closure, CornerLookup, FilStatus,
    FilletBrep, FilletCornerId, FilletCornerType, FilletEdgeType, FilletErrorType, FilletGeom,
    FilletGeomId, FilletGeomType, FilletSolverId, FilletVertex, FilletVertexType,
};
use super::p2::{srf_srf_intersection, SrfSrfCurves, SrfSrfOptions};
use super::p4::set_edge_curve;
use super::p6::move_tsp;

const OP: &str = "blend::fillet_geom::p3";
const OP_MATE: &str = "blend::fillet_geom::p3::FilletBrep::calc_mate_geom";
const OP_CORNER_EDGE: &str = "blend::fillet_geom::p3::FilletBrep::calc_corner_edge_geom";
const OP_FIF: &str = "blend::fillet_geom::p3::FilletBrep::calc_edge_fillet_int_fillet";
const OP_RECALC: &str = "blend::fillet_geom::p3::FilletBrep::re_calc_fillet_geom";
const OP_SPLIT: &str = "blend::fillet_geom::p3::FilletBrep::topology_split";

/// The deepest nesting of `CalcMateGeom` -> `CalcCornerVertGeom` (a mate whose main
/// vertex is itself a mate). Upstream's corners nest it at most once.
pub const MAX_MATE_DEPTH: usize = 64;

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(operation, reason)
}

const fn srf(i: usize) -> SrfIndex {
    if i == 0 {
        SrfIndex::First
    } else {
        SrfIndex::Second
    }
}

/// `smos_Min(a, b)`: `a < b ? a : b`.
fn smos_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// The solver `id` of the caller's table.
fn solver_of<'s>(
    solvers: &'s [FilletSolver],
    id: FilletSolverId,
    op: &'static str,
) -> Result<&'s FilletSolver> {
    usize::try_from(id.0)
        .ok()
        .and_then(|i| solvers.get(i))
        .ok_or_else(|| invalid(op, "fillet solver id not in the solver table"))
}

/// An offset surface as a [`Surface`] (a copy; its evaluations dispatch to the
/// offset, as upstream's virtual calls on the `SmOffsetSurface*` do).
fn offset_as_surface(s: &OffsetSurface) -> Surface {
    Surface::from(s.surface.clone())
}

/// `SmFilletGeom(pFilletSolver, eType)` for a base [`FilletSolver`] (CO-PORTED from
/// `blend.fillet_geom_p5`, as [`FilletGeom::new`] for a constant-radius-assisted
/// solver): no rails, curves or surface, unprocessed, with copies of the solver's
/// offset surfaces (a failed copy leaves `None`, as upstream ignores its status).
///
/// # Errors
///
/// [`SolidError::Unported`] from [`FilletSolver::surface`] (a geometry set for the
/// surfaces) or from a copy.
pub fn new_solver_fillet_geom(
    k: &Kernel,
    id: FilletSolverId,
    solver: &FilletSolver,
    geom_type: FilletGeomType,
) -> Result<FilletGeom> {
    let mut geom = FilletGeom::new(k, None, geom_type)?;
    geom.solver = Some(id);
    for (slot, i) in geom.off_surfs.iter_mut().zip(0..2usize) {
        *slot = match solver.surface(i)?.map(|s| &s.surface) {
            // SmOffsetSurface::Copy(); a failed copy leaves NULL
            Some(off) => ignored(off.surface.copy(k))?.map(|surface| OffsetSurface {
                surface,
                base_id: off.base_id,
            }),
            None => None,
        };
    }
    Ok(geom)
}

/// The fillet intersector `ReCalcFilletGeom` traces with (no solution objects; the
/// current fillet geometry is a pseudo-B-rep geometry id).
type FilletIntersector<'s> = fillet_intersector::FilletIntersector<'s, 's, NoObject, FilletGeomId>;

/// `sFI.DoPointIntersection(crContext, sTsectPnts, bExtendBefore, bExtendAfter,
/// pOptMarchDir, NULL, &dTolerance, &dAngTol, ...)`: writes each point's curve
/// parameter (negative when it is not on the traced curve).
///
/// The points are copied into the intersector's point arena, the inherited driver
/// runs with the `SmFilletIntersector` overrides
/// ([`fillet_intersector::FilletIntersector::do_point_intersection`], stepping through
/// `solver` as `IntersectorSolver`; `FlushCurve` stores into `pseudo`), and every
/// point is copied back whatever the result (the error is returned after the copy).
fn do_point_intersection(
    fi: &mut FilletIntersector<'_>,
    solver: &mut dyn FilletSolverOps,
    pseudo: &mut FilletBrep,
    k: &Kernel,
    tsect_pnts: &mut [TsectPnt],
    opts: &PointIntersectionOptions,
) -> Result<()> {
    use crate::blend::fillet_solver::p4::IntersectorSolver;

    let ids: Vec<_> = tsect_pnts
        .iter()
        .map(|p| fi.base.base.alloc_point(p.clone()))
        .collect();
    let trace = [
        fi.base.base.surface(SrfIndex::First),
        fi.base.base.surface(SrfIndex::Second),
    ];
    let r = match IntersectorSolver::new(solver, trace) {
        Ok(mut is) => fi
            .do_point_intersection(&mut is, pseudo, k, &ids, opts)
            .map(|_| ()),
        Err(e) => Err(e),
    };
    for (p, &id) in tsect_pnts.iter_mut().zip(&ids) {
        p.clone_from(fi.base.base.point(id)?);
    }
    r
}

/// Whether `c` is of a kind derived from `SmBSplineCurve` (`SM_CAST_PTR` succeeds):
/// a B-spline, `SmLine`, `SmEllipse`/`SmCircle`, `SmHyperbola` or `SmParabola`.
const fn is_b_spline_kind(c: &Curve) -> bool {
    matches!(
        c.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

impl FilletBrep {
    /// The intersection point of vertex use `vu` (what `sm_MoveTSP` reads of it).
    fn tsect_of(&self, vu: VertexuseId) -> Result<TsectPnt> {
        Ok(self.fillet_vertexuse(vu)?.tsect_pnt().clone())
    }

    // ---- SmFilletVertex -------------------------------------------------------------

    /// `SmFilletVertex::CalcCornerVertGeom`: computes vertex `v` by its type (nothing
    /// for a processed vertex, an `SM_FV_FILLET_X_EDGEUSE` or an unhandled type).
    /// `target` is the B-rep being filleted (the vertices' classification objects and
    /// the solvers' edge uses); `solvers` resolves the geometries' solvers.
    ///
    /// # Errors
    ///
    /// Those of the type's calculator (`corners` resolves the vertex's corner for the
    /// calculators of `blend.fillet_geom_p2`, `p4` and `p5`).
    pub fn calc_corner_vert_geom(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        target: &mut Brep,
        solvers: &mut [FilletSolver],
    ) -> Result<()> {
        self.corner_vert_geom(k, v, corners, target, solvers, 0)
    }

    fn corner_vert_geom(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        target: &mut Brep,
        solvers: &mut [FilletSolver],
        depth: usize,
    ) -> Result<()> {
        let fv = self.fillet_vertex(v)?;
        if fv.is_processed() {
            return Ok(());
        }

        // switch on FilletVertex->m_eType
        match fv.fillet_vertex_type() {
            FilletVertexType::RailXRail => self.calc_rail_int_rail(k, v, corners, solvers, target),
            FilletVertexType::RailXEdgeuse => {
                self.calc_rail_int_edgeuse(k, v, None, corners, solvers, target)
            }
            FilletVertexType::OnVertex => self.calc_vert_on_vert(v, target),
            FilletVertexType::FilletXFillet => {
                self.calc_fillet_int_fillet(k, v, corners, solvers, target)
            }
            FilletVertexType::RailXExtendedEdgeuse => {
                self.calc_rail_int_extended_edgeuse(k, v, corners, solvers, target)
            }
            FilletVertexType::FilletX2Fillets => {
                self.calc_fillet_int_2_fillets(k, v, corners, solvers, target)
            }
            FilletVertexType::OnCrossSection => {
                self.calc_vert_on_cross_section(k, v, None, corners, solvers, target)
            }
            FilletVertexType::Setback => self.calc_set_back_vert(k, v, corners, solvers, target),
            FilletVertexType::Mate => self.mate_geom(k, v, corners, target, solvers, depth),
            // SM_FV_FILLET_X_EDGEUSE and default
            _ => Ok(()),
        }
    }

    /// `SmFilletVertex::CalcMateGeom`: places mate vertex `v` on the rail opposite its
    /// main vertex (`GetMate(0)`, computed first) at the main vertex's solver point,
    /// evaluated on the target face `v` is classified to (`sm_EvaluateMate`), and
    /// marks it processed. When the main vertex cannot be computed, or has no
    /// classification object, or `v` has no rail-end vertex use on the geometry, `v`
    /// stays unprocessed (success).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a main vertex, solver or face; `SM_ERR` when `v`
    /// is on no rail of a fillet geometry; [`SolidError::InvalidInput`] for a
    /// classification object of the wrong kind, a main vertex without a rail-end
    /// vertex use (upstream dereferences `NULL`), a solver missing from `solvers` or
    /// mates nested deeper than [`MAX_MATE_DEPTH`]; those of
    /// `FindIndexOfRailXSideEdgeuse` and [`evaluate_mate`]; [`SolidError::Unported`]
    /// from the main vertex's calculation.
    pub fn calc_mate_geom(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        target: &mut Brep,
        solvers: &mut [FilletSolver],
    ) -> Result<()> {
        self.mate_geom(k, v, corners, target, solvers, 0)
    }

    fn mate_geom(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        target: &mut Brep,
        solvers: &mut [FilletSolver],
        depth: usize,
    ) -> Result<()> {
        self.calc_mate_geom_with(
            k,
            v,
            target,
            solvers,
            depth,
            |fb, target, solvers, main| {
                fb.corner_vert_geom(k, main, corners, target, solvers, depth + 1)
            },
            |solvers, target, solver_id, side| {
                solver_of(solvers, solver_id, OP_MATE)?
                    .find_index_of_rail_x_side_edgeuse(target, side)
            },
        )
    }

    /// [`FilletBrep::calc_mate_geom`] at mate nesting `depth` against the caller's
    /// solver store `solvers`: `main` computes the main vertex (`CalcCornerVertGeom`
    /// at `depth + 1`) and `rail_x_side` is the geometry solver's
    /// `FindIndexOfRailXSideEdgeuse`.
    ///
    /// # Errors
    ///
    /// As [`FilletBrep::calc_mate_geom`], with `main`'s and `rail_x_side`'s errors.
    #[allow(clippy::too_many_arguments)]
    pub fn calc_mate_geom_with<S: ?Sized>(
        &mut self,
        k: &Kernel,
        v: VertexId,
        target: &mut Brep,
        solvers: &mut S,
        depth: usize,
        main: impl FnOnce(&mut Self, &mut Brep, &mut S, VertexId) -> Result<()>,
        rail_x_side: impl FnOnce(&mut S, &Brep, FilletSolverId, EdgeuseId) -> Result<usize>,
    ) -> Result<()> {
        if depth >= MAX_MATE_DEPTH {
            return Err(invalid(
                OP_MATE,
                "mate vertices nested deeper than MAX_MATE_DEPTH",
            ));
        }

        // our mate (which is the 'real' fillet vertex) must be done first.
        let main_fv = self
            .fillet_vertex(v)?
            .mate(0)
            .ok_or_else(|| SolidError::null_pointer(OP_MATE))?;
        if !self.fillet_vertex(main_fv)?.is_processed() {
            match main(self, target, solvers, main_fv) {
                Err(e) if e.is_unported() || is_depth_error(&e) => return Err(e),
                _ => {} // status ignored
            }
        }
        if !self.fillet_vertex(main_fv)?.is_processed() {
            return Ok(());
        }

        // Calculating Main's geom can cause us to be calculated also.
        if self.fillet_vertex(v)?.is_processed() {
            return Ok(());
        }

        // Just work from the first FilletGeom.
        let geoms = self.vertex_fillet_geoms(v)?;

        // check state - at least 1 railEdge must be connected to this vertex
        let fillet_geom = *geoms.first().ok_or_else(|| SolidError::sm_err(OP_MATE))?;

        // We have to figure out which rail of the FilletGeom we're on.
        let other_side_eu = match self.fillet_vertex(main_fv)?.point_class_object() {
            None => return Ok(()),
            Some(EntityRef::Edgeuse(eu)) => eu,
            Some(_) => {
                return Err(invalid(
                    OP_MATE,
                    "main vertex classification object is not an edge use",
                ))
            }
        };

        let solver_id = self
            .geom(fillet_geom)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(OP_MATE))?;
        let rail_index = rail_x_side(solvers, target, solver_id, other_side_eu)?;
        let rail_index = 1 - rail_index; // We're on the opposite rail.

        let main_vtx_use = self
            .vu_at_rail_end(main_fv, Some(fillet_geom), None, None)?
            .ok_or_else(|| invalid(OP_MATE, "main vertex without a rail-end vertex use"))?;
        let uv = self
            .fillet_vertexuse(main_vtx_use)?
            .tsect_pnt()
            .uv_pos(srf(rail_index));

        if self
            .vu_at_rail_end(v, Some(fillet_geom), None, None)?
            .is_none()
        {
            return Ok(());
        }

        let face = match self.fillet_vertex(v)?.point_class_object() {
            Some(EntityRef::Face(f)) => f,
            None => return Err(SolidError::null_pointer(OP_MATE)),
            Some(_) => return Err(invalid(OP_MATE, "classification object is not a face")),
        };

        // compute sPnt = pFace->Surface(sUV), use ExtendedSurface as needed
        let pnt = evaluate_mate(k, target, face, uv)?;

        // set mate outputs
        let fv = self.fillet_vertex_mut(v)?;
        fv.set_original_uv(uv);
        self.set_vertex_point(v, pnt)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    // ---- SmFilletEdge ---------------------------------------------------------------

    /// `SmFilletEdge::CalcCornerEdgeGeom`: computes edge `e` (made by a fillet corner)
    /// by its type (nothing for rails, precomputed, blending, cliff, split-face and
    /// unknown edges). `target` is the B-rep being filleted; `corners` resolves the
    /// edge's corner for the calculators that read it (a corner computing its own edges
    /// passes [`CornerLookup::of_corner`]: upstream asserts the edge is the corner's
    /// own, in debug builds only).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for an edge without a fillet corner; those of
    /// [`CornerLookup::corner`] and of the type's calculator.
    pub fn calc_corner_edge_geom(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corners: CornerLookup<'_>,
        solvers: &mut [FilletSolver],
        target: &mut Brep,
    ) -> Result<()> {
        self.calc_corner_edge_geom_with_error_info(k, e, corners, solvers, target, None)
    }

    /// [`Self::calc_corner_edge_geom`] with an optional [`ErrorInfo`] sink for
    /// `SmFilletCorner::RecordFilletError`.
    ///
    /// # Errors
    /// As [`Self::calc_corner_edge_geom`].
    pub fn calc_corner_edge_geom_with_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corners: CornerLookup<'_>,
        solvers: &mut [FilletSolver],
        target: &mut Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        // check state - filletEdge must have been generated by a filletCorner
        let fe = self.fillet_edge(e)?;
        let corner = fe
            .fillet_corner()
            .ok_or_else(|| SolidError::null_pointer(OP_CORNER_EDGE))?;

        // switch on FilletEdge->m_eType to pass the call along
        match fe.fillet_edge_type() {
            FilletEdgeType::CrossSection => {
                self.calc_cross_section(k, e, Some(corner), corners, solvers, target)
            }
            FilletEdgeType::FilletXFillet => self.calc_edge_fillet_int_fillet_with_error_info(
                k,
                e,
                Some(corner),
                corners,
                solvers,
                target,
                error_info,
            ),
            FilletEdgeType::FilletXSideFace => self.calc_fillet_int_side_face_with_error_info(
                k,
                e,
                Some(corner),
                corners,
                solvers,
                target,
                error_info,
            ),
            FilletEdgeType::RailRailInterpolation => {
                self.calc_rail_rail_interpolation(k, e, Some(corner), corners, solvers, target)
            }
            FilletEdgeType::OnEdge => self.calc_edge_on_edge(k, e, target),
            FilletEdgeType::OnExtendEdge => {
                self.calc_edge_on_extended_edge(k, e, Some(corner), corners, target)
            }
            FilletEdgeType::FilletEnd => {
                let c = corners.corner(corner)?;
                self.calc_fillet_end(k, e, target, Some(c), solvers)
            }
            FilletEdgeType::SetbackRail | FilletEdgeType::RailExtension => {
                let c = corners.corner(corner)?;
                self.calc_set_back_rail(k, e, target, Some(c), solvers)
            }
            // SM_FE_RAIL, SM_FE_PRECOMPUTED, SM_FE_BLENDING_RAIL, SM_FE_CLIFF_RAIL,
            // SM_FE_SPLIT_FACE, SM_FE_UNKNOWN
            _ => Ok(()),
        }
    }

    /// Dispatch an actual corner edge using retained solver/surface context.
    /// Concrete operations not yet connected to this context remain explicit.
    ///
    /// # Errors
    /// Actual corner/context resolution or selected calculator failures.
    pub fn calc_corner_edge_geom_with_context(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corners: CornerLookup<'_>,
        context: &mut impl super::p5::FilletEdgeContext,
        target: &mut Brep,
    ) -> Result<()> {
        self.calc_corner_edge_geom_with_context_and_error_info(k, e, corners, context, target, None)
    }

    /// [`Self::calc_corner_edge_geom_with_context`] with an optional [`ErrorInfo`] sink
    /// for `SmFilletCorner::RecordFilletError` (the live executive's error record).
    ///
    /// # Errors
    /// As [`Self::calc_corner_edge_geom_with_context`].
    pub fn calc_corner_edge_geom_with_context_and_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corners: CornerLookup<'_>,
        context: &mut impl super::p5::FilletEdgeContext,
        target: &mut Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        let edge = self.fillet_edge(e)?;
        let corner = edge
            .fillet_corner()
            .ok_or_else(|| SolidError::null_pointer(OP_CORNER_EDGE))?;
        match edge.fillet_edge_type() {
            FilletEdgeType::CrossSection => self.calc_cross_section_with_lookup(
                k,
                e,
                Some(corner),
                corners,
                &|id| context.solver(id),
                target,
            ),
            FilletEdgeType::FilletXSideFace => self
                .calc_fillet_int_side_face_with_context_and_error_info(
                    k,
                    e,
                    Some(corner),
                    corners,
                    context,
                    target,
                    error_info,
                ),
            FilletEdgeType::FilletXFillet => self
                .calc_edge_fillet_int_fillet_with_context_and_error_info(
                    k,
                    e,
                    Some(corner),
                    corners,
                    context,
                    target,
                    error_info,
                ),
            FilletEdgeType::OnEdge => self.calc_edge_on_edge(k, e, target),
            FilletEdgeType::RailRailInterpolation => self
                .calc_rail_rail_interpolation_with_context(
                    k,
                    e,
                    Some(corner),
                    corners,
                    &mut *context,
                    target,
                ),
            FilletEdgeType::OnExtendEdge => self.calc_edge_on_extended_edge_with_context(
                k,
                e,
                Some(corner),
                corners,
                context,
                target,
            ),
            FilletEdgeType::FilletEnd => {
                let c = corners.corner(corner)?;
                self.calc_fillet_end_with_lookup(k, e, target, Some(c), &|id| context.solver(id))
            }
            FilletEdgeType::SetbackRail | FilletEdgeType::RailExtension => {
                let c = corners.corner(corner)?;
                self.calc_set_back_rail_with_lookup(k, e, target, Some(c), &|id| context.solver(id))
            }
            _ => Ok(()),
        }
    }

    /// `SmFilletEdge::CalcFilletIntFillet(crContext, pCorner)`: the edge where two
    /// fillet surfaces meet. Adds the edge's uses to their geometries' side edge uses;
    /// with both end vertices processed, trims the edge's 3D curve and both UV trim
    /// curves to the parameters of the end points dropped onto the 3D curve; when the
    /// curve is not computed yet, the surface/surface intersection of the two fillet
    /// surfaces through the end points makes it and its UV curves instead (`SetCurve`,
    /// `SetUVCurve`). `corner` is the fillet corner that made the edge (upstream asserts
    /// it is the edge's own, in debug builds only).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a use without fillet geometry, a geometry without
    /// fillet surface, a missing primary edge use, mate, vertex or solver, or (without a
    /// 3D curve) an edge without fillet corner (upstream reads its context); `SM_ERR`
    /// when neither end vertex is processed, or (status
    /// `SM_FE_END_PNT_DROP_FAILURE`) when an end point does not drop onto the curve;
    /// [`SolidError::InvalidInput`] for a solver missing from `solvers`; those of
    /// `DropPoint` and `Trim` (a missing UV trim curve is `SM_ERR_NULL_POINTER`) and of
    /// the intersection; [`SolidError::Unported`] for an unprocessed end vertex or from
    /// the intersection.
    pub fn calc_edge_fillet_int_fillet(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.calc_edge_fillet_int_fillet_with_error_info(
            k, e, corner, corners, solvers, target, None,
        )
    }

    /// [`Self::calc_edge_fillet_int_fillet`] with an optional [`ErrorInfo`] sink for
    /// `SmFilletCorner::RecordFilletError`.
    ///
    /// # Errors
    /// As [`Self::calc_edge_fillet_int_fillet`].
    #[allow(clippy::too_many_arguments)]
    pub fn calc_edge_fillet_int_fillet_with_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        self.calc_edge_fillet_int_fillet_with_context_and_error_info(
            k,
            e,
            corner,
            corners,
            &super::p5::IndexedEdgeContext {
                solvers,
                corners,
                op: OP_FIF,
            },
            target,
            error_info,
        )
    }

    /// [`Self::calc_edge_fillet_int_fillet_with_error_info`] resolving the
    /// geometries' solvers through `context` (a live executive's retained solvers).
    ///
    /// # Errors
    /// As [`Self::calc_edge_fillet_int_fillet`], with `context`'s resolution failures.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream routine
    pub fn calc_edge_fillet_int_fillet_with_context_and_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        context: &impl super::p5::FilletEdgeContext,
        target: &Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_FIF);

        // locals: get filletSurface FilletGeoms
        let prim_eu = primary_edgeuse(self.brep(), e)?.ok_or_else(nul)?;
        let mate_eu = eu1::mate(self.brep(), prim_eu)?.ok_or_else(nul)?;
        let fillet_geom1 = self.fillet_edgeuse(prim_eu)?.fillet_geom();
        let fillet_geom2 = self.fillet_edgeuse(mate_eu)?.fillet_geom();
        let fillet_geom1 = fillet_geom1.ok_or_else(nul)?;
        let fillet_geom2 = fillet_geom2.ok_or_else(nul)?;

        // add this FilletEdge->Edgeuses to FilletGeoms Side Edgeuse lists
        self.geom_mut(fillet_geom1)?
            .add_side_fillet_edgeuse(prim_eu);
        self.geom_mut(fillet_geom2)?
            .add_side_fillet_edgeuse(mate_eu);

        // get FilletEdge->Vertices
        let start_fv = edge::vertex(self.brep(), e)?.ok_or_else(nul)?;
        let end_fv = edge::other_vertex(self.brep(), e, Some(start_fv))?.ok_or_else(nul)?;
        let start_processed = self.fillet_vertex(start_fv)?.is_processed();
        let end_processed = self.fillet_vertex(end_fv)?.is_processed();

        // check state - at least one FilletEdge->Vertex is processed
        if !start_processed && !end_processed {
            return Err(SolidError::sm_err(OP_FIF));
        }

        // when either vertex is not processed pass the call to
        // SmFilletVertex::CalcFilletIntFillet
        if !start_processed {
            return self.calc_fillet_int_fillet_with_context(k, start_fv, corners, context, target);
        }
        if !end_processed {
            return self.calc_fillet_int_fillet_with_context(k, end_fv, corners, context, target);
        }

        // get intersecting filletSurfaces and FilletSolvers
        let g1 = self.geom(fillet_geom1)?;
        let g2 = self.geom(fillet_geom2)?;
        let fil_solver1 = context.solver(g1.fillet_solver().ok_or_else(nul)?)?;
        let fil_solver2 = context.solver(g2.fillet_solver().ok_or_else(nul)?)?;
        let fillet_surface1 = g1.fillet_surface(None)?.ok_or_else(nul)?;
        let fillet_surface2 = g2.fillet_surface(None)?.ok_or_else(nul)?;

        // pick appoxTol and angTol as tightest filletSolver tolerances
        let approx_tol = smos_min(fil_solver1.approx_tol_3d, fil_solver2.approx_tol_3d);
        let ang_tol = smos_min(fil_solver1.ang_tol_rad, fil_solver2.ang_tol_rad);

        // SM_CAST_PTR(SmBSplineCurve, GetCurve()): NULL for a kind not derived from it
        let has_3d_curve = self
            .brep()
            .edge(e)?
            .curve
            .as_ref()
            .is_some_and(is_b_spline_kind);

        // when 3D curve has not yet been calculated
        if !has_3d_curve {
            // Use 2-pts intersector - which can try the global surf/surf intersector as a
            // last resort
            let start_pnt = self.vertex_point(start_fv)?;
            let end_pnt = self.vertex_point(end_fv)?;
            let mut dir = end_pnt - start_pnt;
            // m_cpCorner->GetContext(): the edge's own corner is dereferenced
            self.fillet_edge(e)?.fillet_corner().ok_or_else(nul)?;
            // &sDir is both the start and the end direction: the two-point intersection
            // unitises it in place before it reads the end direction
            let end_dir = dir.unitize_or_z();
            let xsect = srf_srf_intersection(
                k,
                Some(start_pnt),
                Some(end_pnt),
                Some(&mut dir),
                Some(end_dir),
                fillet_surface1,
                fillet_surface2,
                approx_tol,
                ang_tol,
                SrfSrfOptions {
                    skip_two_pnts_intersection: false,
                    ref_point: None,
                },
            );
            let (curve_3d, uv_curve1, uv_curve2) = match xsect {
                Err(err) if err.is_unported() => return Err(err),
                Ok(SrfSrfCurves {
                    curve_3d: Some(c),
                    uv_curve1,
                    uv_curve2,
                }) => (c, uv_curve1, uv_curve2),
                _ => {
                    // error: sm_SrfSrfIntersection() failed or failed to produce a p3DCurve
                    self.fillet_edge_mut(e)?
                        .set_status(FilStatus::SurfIntFailure);
                    let c = corners.corner(corner.ok_or_else(nul)?)?;
                    record_fillet_error(
                        c,
                        error_info,
                        fillet_geom1,
                        fillet_geom2,
                        start_fv,
                        end_fv,
                    );
                    return Err(SolidError::sm_err(OP_FIF));
                }
            };

            // set SmFilletEdge Curve and UVTrimCurves (SetCurve deletes the current UV
            // trim curves; SetOwner has no Rust counterpart)
            set_edge_curve(self.brep_mut(), e, Some(curve_3d))?;
            self.set_edgeuse_uv_curve(prim_eu, uv_curve1, true, true)?;
            self.set_edgeuse_uv_curve(mate_eu, uv_curve2, true, true)?;
            return Ok(());
        }

        // Trim the intersection curve by Start and End FilletPoint
        let start_pnt = self.vertex_point(start_fv)?;
        let end_pnt = self.vertex_point(end_fv)?;
        let curve_3d = self.brep().edge(e)?.curve.as_ref().ok_or_else(nul)?;
        let mut trim_ivl = curve_3d.natural_interval();

        // drop StartFV->Point to 3DCurve
        let drop1 = curve_3d.drop_point(
            k,
            &trim_ivl,
            start_pnt,
            None,
            approx_tol * 10.0,
            None,
            SolverOperation::Minimize,
        );
        let param1 = match drop1 {
            Ok(d) if d.success => d.parameter,
            Err(e) if e.is_unported() => return Err(e),
            _ => {
                // error: DropPoint to Curve from StartFV failed
                self.fillet_edge_mut(e)?
                    .set_status(FilStatus::EndPntDropFailure);
                return Err(SolidError::sm_err(OP_FIF));
            }
        };

        // drop EndFV->Point to 3DCurve
        let drop2 = self
            .brep()
            .edge(e)?
            .curve
            .as_ref()
            .ok_or_else(nul)?
            .drop_point(
                k,
                &trim_ivl,
                end_pnt,
                None,
                approx_tol * 10.0,
                None,
                SolverOperation::Minimize,
            );
        let param2 = match drop2 {
            Ok(d) if d.success => d.parameter,
            Err(e) if e.is_unported() => return Err(e),
            _ => {
                // error: DropPoint to Curve from EndFV failed
                self.fillet_edge_mut(e)?
                    .set_status(FilStatus::EndPntDropFailure);
                return Err(SolidError::sm_err(OP_FIF));
            }
        };

        // trim 3DCurve and UVTrimCurves to dropPoint parameter values
        // (SetMinMax's status is ignored: an inverted pair leaves the interval)
        let _ = trim_ivl.set_min_max(param1, param2);
        // each Trim reads SmTol::GetZoneTol3d of the curve's holding edge (the 3D curve's
        // owner is this edge, set by every producer with SetOwner; a UV trim curve's is
        // its edge use, whose owner is the edge)
        let edge_tol = super::p5::held_curve_zone_tol_3d(k, self.brep(), Some(e))?;
        let prim_edge = self.brep().edgeuse(prim_eu)?.edge;
        let prim_tol = super::p5::held_curve_zone_tol_3d(k, self.brep(), prim_edge)?;
        let mate_edge = self.brep().edgeuse(mate_eu)?.edge;
        let mate_tol = super::p5::held_curve_zone_tol_3d(k, self.brep(), mate_edge)?;
        let brep = self.brep_mut();
        brep.edge_mut(e)?
            .curve
            .as_mut()
            .ok_or_else(nul)?
            .trim_with_zone_tol(k, &mut trim_ivl, TrimMode::Notify, edge_tol)?;
        brep.edgeuse_mut(prim_eu)?
            .uv_trim_curve
            .as_mut()
            .ok_or_else(nul)?
            .trim_with_zone_tol(k, &mut trim_ivl, TrimMode::Notify, prim_tol)?;
        brep.edgeuse_mut(mate_eu)?
            .uv_trim_curve
            .as_mut()
            .ok_or_else(nul)?
            .trim_with_zone_tol(k, &mut trim_ivl, TrimMode::Notify, mate_tol)?;
        Ok(())
    }

    // ---- SmFilletGeom ---------------------------------------------------------------

    /// `SmFilletGeom::ReCalcFilletGeom(bIsAnalyticFillet, pOptMarchDir, pOptRefCurve)`:
    /// recreates geometry `g`'s fillet surface through all of its rail ends. The
    /// processed rail-end points (not mates or extended-edge-use ends) are projected
    /// onto the reference curve (`opt_ref_curve` for a corner fillet; else the target
    /// edge of the solver's first edge use: its 3D curve for a tangent roll-over, its
    /// edge uses' UV trim curves otherwise), sorted by that parameter without
    /// duplicates, and the fillet is traced through them (an analytic cone or torus
    /// when `is_analytic_fillet`); then the cross-section edges are recomputed and the
    /// geometry is processed. `solver` is `g`'s solver.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a reference curve, edge curve, rail or UV trim
    /// curve; `SM_ERR` for a rail without two edge uses or a point that does not
    /// project to exactly one curve point, or when the traced curve misses a point;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (no solver, edge
    /// use, offset surface, vertex use); those of `SetupOffsetValues`, the offset
    /// evaluation and the curve solve; those of the analytic fillets' `Unported`
    /// callees (`blend.fillet_solver_p2`).
    pub fn re_calc_fillet_geom(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solver: &mut dyn FilletSolverOps,
        target: &Brep,
        is_analytic_fillet: bool,
        opt_march_dir: Option<Vec3>,
        opt_ref_curve: Option<&Curve>,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_RECALC);
        let geom = self.geom(g)?;
        if geom.fillet_solver().is_none() {
            return Err(invalid(OP_RECALC, "fillet geometry without solver"));
        }
        let geom_type = geom.fillet_geom_type();

        let mut b3d_test = true;
        let mut curve: &Curve;
        if geom_type == FilletGeomType::CornerFillet {
            curve = opt_ref_curve.ok_or_else(nul)?;
        } else {
            if geom_type != FilletGeomType::TangentRollover {
                b3d_test = false;
            }
            let eu0 = solver
                .solver()
                .edgeuse(0)?
                .ok_or_else(|| invalid(OP_RECALC, "solver without first edge use"))?;
            let e = eu1::edge(target, eu0)?
                .ok_or_else(|| invalid(OP_RECALC, "solver edge use without edge"))?;
            curve = target.edge(e)?.curve.as_ref().ok_or_else(nul)?;
        }
        let ivl = curve.natural_interval();

        let is_closed_solver_edge =
            geom_type != FilletGeomType::CornerFillet && curve.is_closed(k, &ivl, 0.0)?;

        let off = |s: &dyn FilletSolverOps, i: usize| -> Result<OffsetSurface> {
            s.solver()
                .surface(i)?
                .map(|slot| slot.surface.clone())
                .ok_or_else(|| invalid(OP_RECALC, "solver without offset surface"))
        };
        let surf1 = off(&*solver, 0)?;
        let surf2 = off(&*solver, 1)?;
        let mut domain1 = surf1.surface.natural_uv_domain_with_nurb(k)?;
        let mut domain2 = surf2.surface.natural_uv_domain_with_nurb(k)?;
        let tolerance = solver.solver().approx_tol_3d;
        let ang_tol = solver.solver().ang_tol_rad;
        let scale = ivl.length();

        let surf_closure1 = set_surface_closure(k, &offset_as_surface(&surf1), &domain1)?;
        let surf_closure2 = set_surface_closure(k, &offset_as_surface(&surf2), &domain2)?;

        let mut tsect_pnts: Vec<VertexuseId> = Vec::new();
        let mut proj_params: Vec<f64> = Vec::new();
        for jj in 0..2usize {
            if !b3d_test {
                let eu_on_surf = solver.solver().edgeuse(jj)?.ok_or_else(nul)?;
                curve = eu1::uv_trim_curve(target, eu_on_surf)?.0.ok_or_else(nul)?;
            }

            let geom = self.geom(g)?;
            let mut all_rails = vec![geom.rail(jj)];
            let others = if jj == 0 {
                &geom.other_rails1
            } else {
                &geom.other_rails2
            };
            all_rails.extend(others.iter().copied().map(Some));

            for rail in all_rails {
                let rail = rail.ok_or_else(nul)?;
                let fil_eus = self.brep().edge(rail)?.edgeuses.clone();
                if fil_eus.len() != 2 {
                    return Err(SolidError::sm_err(OP_RECALC));
                }
                for eu in fil_eus {
                    let vu = self
                        .brep()
                        .edgeuse(eu)?
                        .vertexuse
                        .ok_or_else(|| invalid(OP_RECALC, "edge use without vertex use"))?;
                    let fv = self
                        .brep()
                        .vertexuse(vu)?
                        .vertex
                        .ok_or_else(|| invalid(OP_RECALC, "vertex use without vertex"))?;
                    let fv = self.fillet_vertex(fv)?;
                    if fv.status() == FilStatus::Unprocessed
                        || fv.fillet_vertex_type() == FilletVertexType::Mate
                        || fv.fillet_vertex_type() == FilletVertexType::RailXExtendedEdgeuse
                    {
                        continue;
                    }

                    let tol = EFF_ZERO;
                    let mut tsect = self.fillet_vertexuse(vu)?.tsect_pnt().clone();
                    let mut uv = tsect.uv_pos(SrfIndex::First);
                    if check_seam_out(surf1.base_surface(), &mut uv, tol, surf_closure1) {
                        *tsect.uv_pos_mut(SrfIndex::First) = uv;
                    }
                    let mut uv = tsect.uv_pos(SrfIndex::Second);
                    if check_seam_out(surf2.base_surface(), &mut uv, tol, surf_closure2) {
                        *tsect.uv_pos_mut(SrfIndex::Second) = uv;
                    }
                    self.fillet_vertexuse_mut(vu)?.set_tsect_pnt(&tsect);

                    let pnt = if b3d_test {
                        // Do a 3D test
                        if tsect.point_type == IntersectionPointType::Unknown
                            || tsect.crv_pos().is_undef()
                        {
                            let uv_pt = tsect.uv_pos(SrfIndex::First);
                            if uv_pt.x == -BIG_DOUBLE || uv_pt.y == -BIG_DOUBLE {
                                continue;
                            }
                            solver.setup_offset_values(&tsect, None)?;
                            let p = solver
                                .solver_mut()
                                .surface_mut(0)?
                                .ok_or_else(|| invalid(OP_RECALC, "solver without offset surface"))?
                                .surface
                                .surface
                                .evaluate_point_extending(k, uv_pt)?;
                            *tsect.crv_pos_mut() = p;
                            self.fillet_vertexuse_mut(vu)?.set_tsect_pnt(&tsect);
                        }
                        tsect.crv_pos()
                    } else {
                        // Do a 2D test
                        let uv = tsect.uv_pos(srf(jj));
                        let p = Point3::new(uv.x, uv.y, 0.0);
                        if p.x == -BIG_DOUBLE || p.y == -BIG_DOUBLE {
                            continue;
                        }
                        p
                    };

                    let solutions = curve.global_point_solve::<NoObject>(
                        k,
                        &ivl,
                        SolverOperation::Minimize,
                        pnt,
                        tol,
                        SolutionRequested::Single,
                        &GlobalPointSolveOptions::default(),
                    )?;
                    if solutions.len() != 1 {
                        return Err(SolidError::sm_err(OP_RECALC));
                    }
                    let mut t = solutions
                        .get(0)
                        .ok_or_else(|| SolidError::sm_err(OP_RECALC))?
                        .start
                        .param(0)?;

                    if ivl.is_value_on_boundary(t, EFF_ZERO).0 {
                        let pv = curve.evaluate(k, t, 1, true, true)?;
                        let (p0, p1) = match pv.as_slice() {
                            [p0, p1, ..] => (*p0, *p1),
                            _ => return Err(SolidError::sm_err(OP_RECALC)),
                        };
                        let adjustment = line_closest_point(p0, p1, pnt);
                        t += adjustment;
                    }

                    if b3d_test
                        && is_closed_solver_edge
                        && ivl.is_value_on_boundary(t, EFF_ZERO).0
                        && eu1::orientation(self.brep(), eu)? == OrientType::Opposite
                    {
                        t = ivl.max();
                    }

                    let mut index_of_insert = proj_params.len();
                    let mut do_insertion = true;
                    for (kk, &p) in proj_params.iter().enumerate() {
                        if (t - p).abs() < EFF_ZERO_SQRT * scale {
                            do_insertion = false;
                            break;
                        } else if t < p {
                            index_of_insert = kk;
                            break;
                        }
                    }
                    if do_insertion {
                        proj_params.insert(index_of_insert, t);
                        self.fillet_vertexuse_mut(vu)?
                            .tsect_pnt_mut()
                            .curve_parameter = -BIG_DOUBLE;
                        tsect_pnts.insert(index_of_insert, vu);
                    }
                } // end loop on both edgeuses of this rail
            } // end loop on all rails on this side
        } // end loop on both main rails, sorting SmTsectPnts

        let mut fillet_created = false;
        if is_analytic_fillet && geom_type != FilletGeomType::TangentRollover {
            // Create analytics (the solver reads copies of the vertex uses' points)
            let tsects = tsect_pnts
                .iter()
                .map(|&vu| Ok(self.fillet_vertexuse(vu)?.tsect_pnt().clone()))
                .collect::<Result<Vec<TsectPnt>>>()?;
            let cone =
                FilletSolver::calc_cone_fillet_geom(solver, k, target, self, &tsects, Some(g));
            fillet_created = match ignored(cone)? {
                Some(()) => true,
                None => ignored(FilletSolver::calc_torus_fillet_geom(
                    solver,
                    k,
                    target,
                    self,
                    &tsects,
                    Some(g),
                ))?
                .is_some(),
            };
        }

        if !fillet_created {
            let fi_surf1 = offset_as_surface(&off(&*solver, 0)?);
            let fi_surf2 = offset_as_surface(&off(&*solver, 1)?);
            solver.setup_offset_extension(k, &fi_surf1, &mut domain1)?;
            solver.setup_offset_extension(k, &fi_surf2, &mut domain2)?;
            // Setup FilletIntersector
            let mut fi =
                FilletIntersector::new(k, &fi_surf1, &domain1, &fi_surf2, &domain2, Some(g))?;

            let mut extend_before = false;
            let mut extend_after = false;
            let fs = solver.solver();
            if fs.first_fillet_geom()? == g {
                extend_before = fs.extend_before;
            }
            if fs.last_fillet_geom()? == g {
                extend_after = fs.extend_after;
            }

            let march_dir = if let Some(d) = opt_march_dir {
                d
            } else {
                let rail0 = self
                    .geom(g)?
                    .rail(0)
                    .ok_or_else(|| invalid(OP_RECALC, "fillet geometry without rail"))?;
                if self.brep().edge(rail0)?.curve.is_some() {
                    edge::end_tangent(k, self.brep(), rail0, true)?
                } else {
                    let first = *tsect_pnts
                        .first()
                        .ok_or_else(|| invalid(OP_RECALC, "no rail-end point"))?;
                    self.fillet_vertexuse(first)?.tsect_pnt().crv_deriv()
                }
            };

            let mut success = true;
            let mut done_checking_seams = false;
            let mut i_try = 0;

            while !done_checking_seams {
                let mut pnts = tsect_pnts
                    .iter()
                    .map(|&vu| self.fillet_vertexuse(vu).map(|r| r.tsect_pnt().clone()))
                    .collect::<Result<Vec<_>>>()?;
                let traced = do_point_intersection(
                    &mut fi,
                    &mut *solver,
                    self,
                    k,
                    &mut pnts,
                    &PointIntersectionOptions {
                        extend_before_start: extend_before,
                        extend_after_end: extend_after,
                        start_direction: Some(march_dir),
                        end_direction: None,
                        approx_tol_3d: Some(tolerance),
                        ang_tol_rad: Some(ang_tol),
                    },
                );
                // the points are the vertex uses' SmTsectPnts upstream (written in
                // place even when the call fails): copy them back before SER(eStat)
                for (&vu, p) in tsect_pnts.iter().zip(&pnts) {
                    self.fillet_vertexuse_mut(vu)?.set_tsect_pnt(p);
                }
                traced?;

                success = true;
                for &vu in tsect_pnts.iter().skip(1) {
                    if self.fillet_vertexuse(vu)?.tsect_pnt().curve_parameter < 0.0 {
                        success = false;
                        break;
                    }
                }
                if success {
                    break; // Break out of while not bDoneCheckingSeams
                }

                i_try += 1;
                let last_tsp_idx = tsect_pnts.len().saturating_sub(1);
                // sm_CheckSeamOn(pSurf, sTsectPnts[i]->UVPos(which), ..) takes the UV by
                // reference: a point on a seam moves to the other side in the vertex
                // use's own SmTsectPnt, which the next DoPointIntersection try traces
                let on_seam =
                    |fb: &mut Self, s: &OffsetSurface, i: usize, which: SrfIndex, closure| {
                        let vu = *tsect_pnts
                            .get(i)
                            .ok_or_else(|| invalid(OP_RECALC, "no rail-end point"))?;
                        let uv = fb
                            .fillet_vertexuse_mut(vu)?
                            .tsect_pnt_mut()
                            .uv_pos_mut(which);
                        Ok::<_, SolidError>(check_seam_on(
                            &offset_as_surface(s),
                            uv,
                            tolerance,
                            closure,
                        ))
                    };
                // case 1 .. 4 fall through to the next when not on a seam
                if i_try == 1 {
                    if on_seam(self, &off(&*solver, 0)?, 0, SrfIndex::First, surf_closure1)? {
                        ignore_status(move_tsp(&mut tsect_pnts, 0, last_tsp_idx, |vu| {
                            self.tsect_of(*vu)
                        }))?;
                        continue;
                    }
                    i_try += 1;
                }
                if i_try == 2 {
                    if on_seam(
                        self,
                        &off(&*solver, 0)?,
                        last_tsp_idx,
                        SrfIndex::First,
                        surf_closure1,
                    )? {
                        ignore_status(move_tsp(&mut tsect_pnts, last_tsp_idx, 0, |vu| {
                            self.tsect_of(*vu)
                        }))?;
                        continue;
                    }
                    i_try += 1;
                }
                if i_try == 3 {
                    if on_seam(self, &off(&*solver, 1)?, 0, SrfIndex::Second, surf_closure2)? {
                        ignore_status(move_tsp(&mut tsect_pnts, 0, last_tsp_idx, |vu| {
                            self.tsect_of(*vu)
                        }))?;
                        continue;
                    }
                    i_try += 1;
                }
                if i_try == 4
                    && on_seam(
                        self,
                        &off(&*solver, 1)?,
                        last_tsp_idx,
                        SrfIndex::Second,
                        surf_closure2,
                    )?
                {
                    ignore_status(move_tsp(&mut tsect_pnts, last_tsp_idx, 0, |vu| {
                        self.tsect_of(*vu)
                    }))?;
                    continue;
                }
                // case 4 without a seam, and default: that's all.
                done_checking_seams = true;
            } // end while not bDoneCheckingSeams

            if !success {
                return Err(SolidError::sm_err(OP_RECALC));
            }
        } // end if Not bFilletCreated

        let mut edges = Vec::new();
        self.geom(g)?.fillet_edges(&mut edges);
        for e in edges {
            if self.fillet_edge(e)?.fillet_edge_type() == FilletEdgeType::CrossSection {
                // CalcCrossSection(crContext): no corner. It reads the solvers of the
                // edge's geometries: this geometry's is `solver`.
                let own = self.geom(g)?.fillet_solver().ok_or_else(nul)?;
                let own_solver: &FilletSolver = solver.solver();
                let resolve = |id: FilletSolverId| -> Result<&FilletSolver> {
                    if id == own {
                        return Ok(own_solver);
                    }
                    // UNPORTED(blend.fillet_geom_p5): SmFilletEdge::CalcCrossSection (a neighbouring geometry's solver: ReCalcFilletGeom holds only its own)
                    // Unreachable by construction upstream: no geometry's m_vEdges holds a cross-section
                    // edge with a use on another solver's geometry; see
                    // plan/findings/blend.fillet_geom_p3--calc-cross-section-neighbour-unreachable.md
                    Err(SolidError::unported(
                        "SmFilletEdge::CalcCrossSection (neighbouring geometry's solver)",
                        "blend.fillet_geom_p5",
                    ))
                };
                self.calc_cross_section_with_lookup(
                    k,
                    e,
                    None,
                    CornerLookup::default(),
                    &resolve,
                    target,
                )?;
            }
        }
        self.geom_mut(g)?.status = FilStatus::Processed;
        Ok(())
    }

    /// `SmFilletGeom::TopologySplit(rpNewFilletGeom, eNewGeomType)`: splits geometry
    /// `g` in two. Both rails (their last pieces after a roll-over) get a new vertex
    /// before their end: the end edge use moves to a new rail of a new geometry of
    /// type `new_geom_type`, the two new vertices (rail end and mate) are joined by a
    /// cross-section edge between the geometries and given to the fillet corner at the
    /// split's end, and the side edge uses of that corner move to the new geometry.
    /// `solver` is `g`'s solver (`solver_id` its id); `target` holds its edge. Returns
    /// the new geometry (`rpNewFilletGeom`), added to this pseudo B-rep.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a rail has no rail-end vertex use at its end or the
    /// solver's edge has no vertex; [`SolidError::InvalidInput`] where upstream
    /// dereferences `NULL` (no solver executive, rail or vertex); those of the
    /// topology queries, of [`FilletBrep::make_rail_edgeuse_on`] (its corner,
    /// [`SolidError::Unported`] at `blend.fillet_corner_p1`) and
    /// [`FilletBrep::make_fillet_edge`]; [`SolidError::Unported`] for the corner calls
    /// after them.
    pub fn topology_split(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        new_geom_type: FilletGeomType,
        solver_id: FilletSolverId,
        solver: &FilletSolver,
        target: &Brep,
        corners: &mut dyn SplitCorners,
    ) -> Result<FilletGeomId> {
        // 1. initialize output value - a new SmFilletGeom (the 2nd child geom)
        // GetFilletExecutive()->GetPseudoBrep() is this pseudo B-rep (checked first, so
        // the error path leaves no orphan geometry; upstream dereferences NULL here)
        if solver.executive.is_none() {
            return Err(invalid(OP_SPLIT, "solver without fillet executive"));
        }
        let new_fillet_geom =
            self.add_geom(new_solver_fillet_geom(k, solver_id, solver, new_geom_type)?);

        let geom_type = self.geom(g)?.fillet_geom_type();
        let mut new_vert = [None; 2];

        // 2. for both rail Edges - split the railEdge topology
        for this_rail_idx in 0..2usize {
            let geom = self.geom(g)?;
            let mut this_rail = geom.rail(this_rail_idx);
            // when a railcurve has been split into pieces due to rollover get the last piece
            if this_rail_idx == 0 && !geom.other_rails1.is_empty() {
                this_rail = geom.other_rails1.last().copied();
            } else if this_rail_idx == 1 && !geom.other_rails2.is_empty() {
                this_rail = geom.other_rails2.last().copied();
            }
            let this_rail =
                this_rail.ok_or_else(|| invalid(OP_SPLIT, "fillet geometry without rail"))?;

            // get pThisRail's vertices
            let start_vert = edge::vertex(self.brep(), this_rail)?;
            let end_vert = edge::other_vertex(self.brep(), this_rail, start_vert)?
                .ok_or_else(|| invalid(OP_SPLIT, "rail without end vertex"))?;

            // the SmFilletVertexuse and FilletEdgeuse attached to the end of this rail
            let mut end_vu = self
                .vu_at_rail_end(end_vert, Some(g), None, Some(this_rail))?
                .ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?;
            let mut end_eu = eu1::vertexuse_edgeuse(self.brep(), end_vu)?
                .ok_or_else(|| invalid(OP_SPLIT, "vertex use without edge use"))?;
            // Its orientation will normally be OPPOSITE.
            if eu1::orientation(self.brep(), end_eu)? != OrientType::Opposite {
                // Typically, this might happen for closed rails.
                end_vu = self
                    .vu_at_rail_end(end_vert, Some(g), Some(end_vu), None)?
                    .ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?;
                end_eu = eu1::vertexuse_edgeuse(self.brep(), end_vu)?
                    .ok_or_else(|| invalid(OP_SPLIT, "vertex use without edge use"))?;
            }

            // 2b. create a new SmFilletVertex
            let v = self.brep_mut().add_vertex(Point3::new(0.0, 0.0, 0.0))?;
            self.insert_fillet_vertex(v, FilletVertex::new(k, None));
            if let Some(slot) = new_vert.get_mut(this_rail_idx) {
                *slot = Some(v);
            }

            // 2c. remove the end EU from the rail edge (SmOwningTopology::Remove)
            self.brep_mut()
                .edge_mut(this_rail)?
                .edgeuses
                .retain(|&x| x != end_eu);
            self.brep_mut().edgeuse_mut(end_eu)?.edge = None;

            // hold the old Edgeuse's UVTrimCurve
            let uv_curve = self.brep_mut().edgeuse_mut(end_eu)?.uv_trim_curve.take();

            // connect pNewVert to pThisRail with new topology
            let new_edgeuse = self.make_rail_edgeuse_on(
                g,
                this_rail_idx,
                Some(v),
                OrientType::Opposite,
                Some(this_rail),
                corners.lookup(),
                solver,
                target,
            )?;

            // move the old edgeuse's UVTrimcurve onto the new UVTrimcurve
            self.set_edgeuse_uv_curve(new_edgeuse, uv_curve, true, true)?;

            // Create new edgeuse and new edge in rpNewFilletGeom.
            let new_edgeuse = self.make_rail_edgeuse_on(
                new_fillet_geom,
                this_rail_idx,
                Some(v),
                OrientType::Same,
                None,
                corners.lookup(),
                solver,
                target,
            )?;

            // Grab the newly created other Rail
            let new_rail = eu1::edge(self.brep(), new_edgeuse)?
                .ok_or_else(|| invalid(OP_SPLIT, "new rail edge use without edge"))?;

            // Insert pThisRail's removed edgeuse to new rail (SmOwningTopology::PostInsert)
            self.brep_mut().edge_mut(new_rail)?.edgeuses.push(end_eu);
            self.brep_mut().edgeuse_mut(end_eu)?.edge = Some(new_rail);
            self.set_edgeuse_fillet_geom(end_eu, Some(new_fillet_geom))?;
            self.geom_mut(g)?.ref_eus.retain(|&x| x != end_eu);

            let orig_face = self.fillet_edge(this_rail)?.original_face();
            let fe = self.fillet_edge_mut(new_rail)?;
            fe.set_fillet_edge_type(FilletEdgeType::Rail);
            fe.set_original_face(orig_face);
            self.geom_mut(new_fillet_geom)?
                .set_rail(this_rail_idx, Some(new_rail));
        } // end iter both rail edges

        let [Some(new_vert0), Some(new_vert1)] = new_vert else {
            return Err(invalid(OP, "rail split without new vertex"));
        };

        // 3. connect the rail splitting vertices with a new FilletEdge and as Mates
        self.fillet_vertex_mut(new_vert0)?
            .set_fillet_vertex_type(FilletVertexType::RailEnd);
        self.fillet_vertex_mut(new_vert1)?
            .set_fillet_vertex_type(FilletVertexType::Mate);

        // build a FilletEdge between pNewVert[0] and pNewVert[1]
        // cpCorner defaults to NULL
        let divide_edge = self.make_fillet_edge(Some(new_vert0), Some(new_vert1), None)?;
        self.fillet_edge_mut(divide_edge)?
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        let prim_eu = primary_edgeuse(self.brep(), divide_edge)?
            .ok_or_else(|| invalid(OP_SPLIT, "edge without primary edge use"))?;
        let mate_eu = eu1::mate(self.brep(), prim_eu)?
            .ok_or_else(|| invalid(OP_SPLIT, "edge use without mate"))?;

        let mut owning_fg = g;
        let mut fg2 = new_fillet_geom;
        if geom_type == FilletGeomType::Blends || geom_type == FilletGeomType::TangentRollover {
            owning_fg = new_fillet_geom;
            fg2 = g;
        }
        self.set_edgeuse_fillet_geom(prim_eu, Some(owning_fg))?;
        // Primary edgeuse will be attached to the previous geom
        self.geom_mut(owning_fg)?.edges.push(divide_edge);
        // Mate edgeuse will always be attached to the new geom
        self.geom_mut(fg2)?.add_side_fillet_edgeuse(mate_eu);

        // Attach new FV's to the FilletCorner at the start or end
        let mut which_end = 1;
        if geom_type == FilletGeomType::Blends || geom_type == FilletGeomType::TangentRollover {
            which_end = 0;
        }
        let end_vtx = solver
            .vertex(target, which_end)?
            .ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?;
        let mut fillet_corner = corners.corner_of_vertex(end_vtx);

        // Stick them into that corner (upstream dereferences a NULL corner).
        let corner = fillet_corner.ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?;
        corners.corner_mut(corner)?.add_fillet_vertex(new_vert0);
        corners.corner_mut(corner)?.add_fillet_vertex(new_vert1);
        self.fillet_vertex_mut(new_vert0)?
            .set_fillet_corner(fillet_corner);
        self.fillet_vertex_mut(new_vert1)?
            .set_fillet_corner(fillet_corner);

        // the corner at the end vertex of the solver's edge
        if which_end != 1 {
            let end_vtx = solver
                .vertex(target, 1)?
                .ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?;
            fillet_corner = corners.corner_of_vertex(end_vtx);
        }
        // pFilletCorner->GetCornerType() (upstream dereferences a NULL corner)
        let corner_type = corners
            .corner_type(fillet_corner.ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?)?;

        // move the side edgeuses associated to the original geom to the new geom
        let ref_eus = self.geom(g)?.ref_eus.clone();
        for (j, &eu) in ref_eus.iter().enumerate().rev() {
            if self.fillet_edgeuse(eu)?.fillet_geom().is_none() {
                return Err(SolidError::null_pointer(OP_SPLIT));
            }
            // no work - no Edge or Edge not connected to filletCorner
            let Some(e) = eu1::edge(self.brep(), eu)? else {
                continue;
            };
            if self.fillet_edge(e)?.fillet_corner() != fillet_corner {
                continue;
            }
            if (corner_type == FilletCornerType::NByOneClosed
                || corner_type == FilletCornerType::OneByOne)
                && Some(eu) == primary_edgeuse(self.brep(), e)?
            {
                continue;
            }

            // set this Edgeuse's FilletGeom pointer
            self.set_edgeuse_fillet_geom(eu, Some(new_fillet_geom))?;
            // delete the Edgeuse's UVTrimCurve
            self.set_edgeuse_uv_curve(eu, None, true, true)?;
            // delete the Edgeuse->Mate's UVTrimCurve
            let mate = eu1::mate(self.brep(), eu)?
                .ok_or_else(|| invalid(OP_SPLIT, "edge use without mate"))?;
            self.brep_mut().edgeuse_mut(mate)?.uv_trim_curve = None;
            // remove the Edgeuse from the FilletGeom Edgeuse lists
            let geom = self.geom_mut(g)?;
            if j < geom.ref_eus.len() {
                geom.ref_eus.remove(j);
            }
            if let Some(i) = geom.side_eus.iter().position(|&x| x == eu) {
                geom.side_eus.remove(i);
            }
        }
        Ok(new_fillet_geom)
    }
}

/// The executive's corner members `SmFilletGeom::TopologySplit` reaches (through its
/// fillet solver's executive): `GetFilletCornerOfVertex`, the corner table
/// (`m_vFilletCorners`) and the corners it changes (`AddFilletVertex`).
pub trait SplitCorners {
    /// `GetFilletCornerOfVertex(vertex)`: the corner of a target vertex (`None`:
    /// `NULL`).
    fn corner_of_vertex(&self, vertex: VertexId) -> Option<FilletCornerId>;
    /// The corner table as the pseudo-B-rep calculators read it.
    fn lookup(&self) -> CornerLookup<'_>;
    /// Corner `id`, to change.
    ///
    /// # Errors
    /// [`CornerLookup::corner`]'s for an id not in the table.
    fn corner_mut(&mut self, id: FilletCornerId) -> Result<&mut FilletCorner>;
    /// `pCorner->GetCornerType()`.
    ///
    /// # Errors
    /// [`CornerLookup::corner`]'s for an id not in the table.
    fn corner_type(&self, id: FilletCornerId) -> Result<FilletCornerType> {
        Ok(self.lookup().corner(id)?.corner_type())
    }
}

/// A caller without an executive (a standalone entry point): no vertex has a corner
/// and the corner table is empty.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCorners;

impl SplitCorners for NoCorners {
    fn corner_of_vertex(&self, _vertex: VertexId) -> Option<FilletCornerId> {
        None
    }
    fn lookup(&self) -> CornerLookup<'_> {
        CornerLookup::default()
    }
    fn corner_mut(&mut self, _id: FilletCornerId) -> Result<&mut FilletCorner> {
        Err(SolidError::null_pointer(OP_SPLIT))
    }
}

/// `pCorner->RecordFilletError(SM_FILERR_BAD_INT, pFilletSurface1, pFilletSurface2,
/// pStartFV, pEndFV, ..)` (unit `blend.fillet_corner_p2`) into the executive's
/// [`ErrorInfo`].
fn record_fillet_error(
    corner: &FilletCorner,
    error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    fillet_geom1: FilletGeomId,
    fillet_geom2: FilletGeomId,
    start_fv: VertexId,
    end_fv: VertexId,
) {
    // m_pExecutive->NoteFilletError: a caller without an executive record (a
    // standalone entry point) notes into a scratch record it does not keep.
    let mut scratch = ErrorInfo::default();
    let error_info = match error_info {
        Some(error_info) => error_info,
        None => &mut scratch,
    };
    corner.record_fillet_error_in(
        error_info,
        FilletErrorType::BadInt,
        corner
            .filleted_vertex()
            .map(ExecutiveErrorObject::TargetVertex),
        [
            Some(ExecutiveErrorObject::FilletGeomSurface(fillet_geom1)),
            Some(ExecutiveErrorObject::FilletGeomSurface(fillet_geom2)),
        ],
        [
            Some(ExecutiveErrorObject::FilletVertex(start_fv)),
            Some(ExecutiveErrorObject::FilletVertex(end_fv)),
        ],
        Some(
            "SmFilletEdge::CalcFilletIntFillet(): failure to calculate intersection of two fillet surfaces",
        ),
    );
}

/// Whether `e` is the mate-nesting bound of [`FilletBrep::calc_mate_geom`].
fn is_depth_error(e: &SolidError) -> bool {
    matches!(
        e,
        SolidError::InvalidInput { operation, reason }
            if *operation == OP_MATE && reason.contains("MAX_MATE_DEPTH")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_solver::p1::FilletExecutiveRef;
    use crate::error::{SM_ERR, SM_ERR_NULL_POINTER};
    use crate::topology::{Edgeuse, EdgeuseId, Vertexuse, VertexuseOwner};

    use super::super::p1::{FilletEdgeuse, FilletVertexuse};

    fn code(e: &SolidError) -> Option<i32> {
        match e {
            SolidError::Upstream { code, .. } => Some(*code),
            _ => None,
        }
    }

    fn unit_of(e: &SolidError) -> Option<&'static str> {
        match e {
            SolidError::Unported { unit, .. } => Some(*unit),
            _ => None,
        }
    }

    fn vertex(fb: &mut FilletBrep, k: &Kernel, p: Point3) -> VertexId {
        let v = fb.brep_mut().add_vertex(p).unwrap();
        fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
        v
    }

    /// A pseudo-B-rep edge from `vs` to `ve` with two edge uses (`SAME` at `vs`,
    /// `OPPOSITE` at `ve`) and their vertex uses, as the runner's `link_uses`.
    fn edge_with_uses(
        fb: &mut FilletBrep,
        vs: VertexId,
        ve: VertexId,
        edge_type: FilletEdgeType,
    ) -> (EdgeId, [EdgeuseId; 2]) {
        let e = fb.new_fillet_edge(vs, ve, None).unwrap();
        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_edge_type(edge_type);
        let mut eus = Vec::new();
        for (v, o) in [(vs, OrientType::Same), (ve, OrientType::Opposite)] {
            let b = fb.brep_mut();
            let eu = b.add_edgeuse_record(Edgeuse::default()).unwrap();
            let vu = b.add_vertexuse_record(Vertexuse::default()).unwrap();
            let r = b.edgeuse_mut(eu).unwrap();
            r.edge = Some(e);
            r.orientation = o;
            r.vertexuse = Some(vu);
            let u = b.vertexuse_mut(vu).unwrap();
            u.vertex = Some(v);
            u.owner = VertexuseOwner::Edgeuse(Some(eu));
            b.vertex_mut(v).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
            fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
            eus.push(eu);
        }
        (e, [eus[0], eus[1]])
    }

    /// A blend geometry with two rails whose edge uses all belong to it; returns the
    /// geometry and each rail's end edge use.
    fn two_rail_geom(fb: &mut FilletBrep, k: &Kernel) -> (FilletGeomId, [EdgeuseId; 2]) {
        let g = fb.add_geom(FilletGeom::new(k, None, FilletGeomType::Blends).unwrap());
        let mut ends = [EdgeuseId::from_index(0); 2];
        for (i, y) in [0.0, 1.0].into_iter().enumerate() {
            let vs = vertex(fb, k, Point3::new(0.0, y, 0.0));
            let ve = vertex(fb, k, Point3::new(2.0, y, 0.0));
            let (rail, eus) = edge_with_uses(fb, vs, ve, FilletEdgeType::Rail);
            for eu in eus {
                fb.set_edgeuse_fillet_geom(eu, Some(g)).unwrap();
            }
            fb.geom_mut(g).unwrap().set_rail(i, Some(rail));
            ends[i] = eus[1];
        }
        (g, ends)
    }

    fn solver(executive: bool) -> FilletSolver {
        let mut s = FilletSolver::new(1e-6, 1e-3, 1e-6);
        s.executive = executive.then_some(FilletExecutiveRef);
        s
    }

    #[test]
    fn solver_fillet_geom_starts_unprocessed_with_the_solver() {
        let k = Kernel::default();
        let g = new_solver_fillet_geom(
            &k,
            FilletSolverId(4),
            &solver(false),
            FilletGeomType::Blends,
        )
        .unwrap();
        assert_eq!(g.fillet_solver(), Some(FilletSolverId(4)));
        assert_eq!(g.fillet_geom_type(), FilletGeomType::Blends);
        assert_eq!(g.status, FilStatus::Unprocessed);
        assert!(g.rail(0).is_none() && g.rail(1).is_none());
        assert!(g.off_surfs.iter().all(Option::is_none));
    }

    #[test]
    fn topology_split_needs_an_executive() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, _) = two_rail_geom(&mut fb, &k);
        let e = fb
            .topology_split(
                &k,
                g,
                FilletGeomType::Blends,
                FilletSolverId(0),
                &solver(false),
                &Brep::new(),
                &mut NoCorners,
            )
            .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }), "{e:?}");
        // no orphan geometry is left behind
        assert_eq!(fb.geoms().len(), 1);
    }

    #[test]
    fn topology_split_stops_at_the_new_rail_edgeuse() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, ends) = two_rail_geom(&mut fb, &k);
        let rail0 = fb.geom(g).unwrap().rail(0).unwrap();
        let e = fb
            .topology_split(
                &k,
                g,
                FilletGeomType::Blends,
                FilletSolverId(0),
                &solver(true),
                &Brep::new(),
                &mut NoCorners,
            )
            .unwrap_err();
        // MakeRailEdgeuse (blend.fillet_geom_p5) needs the pseudo B-rep's region and
        // shell for its new edge use, which this bare FilletBrep lacks
        // (with topology.brep_p3 ported, this is a null pointer, not a deferral)
        assert!(
            matches!(
                e,
                SolidError::Upstream {
                    operation: "blend::fillet_geom::p5::FilletBrep::make_rail_edgeuse",
                    code: SM_ERR_NULL_POINTER,
                    ..
                }
            ),
            "{e:?}"
        );
        // the end edge use was taken off the first rail before the stub
        assert!(!fb.brep().edge(rail0).unwrap().edgeuses.contains(&ends[0]));
        assert_eq!(fb.brep().edgeuse(ends[0]).unwrap().edge, None);
    }

    #[test]
    fn topology_split_without_rail_end_use_is_a_null_pointer() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, ends) = two_rail_geom(&mut fb, &k);
        // the first rail's end use belongs to another geometry
        let other = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap());
        fb.set_edgeuse_fillet_geom(ends[0], Some(other)).unwrap();
        let e = fb
            .topology_split(
                &k,
                g,
                FilletGeomType::Blends,
                FilletSolverId(0),
                &solver(true),
                &Brep::new(),
                &mut NoCorners,
            )
            .unwrap_err();
        assert_eq!(code(&e), Some(SM_ERR_NULL_POINTER), "{e:?}");
    }

    #[test]
    fn mate_cycle_is_bounded() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let b = vertex(&mut fb, &k, Point3::new(1.0, 0.0, 0.0));
        for (v, m) in [(a, b), (b, a)] {
            let fv = fb.fillet_vertex_mut(v).unwrap();
            fv.set_fillet_vertex_type(FilletVertexType::Mate);
            fv.set_mate(0, Some(m));
        }
        let e = fb
            .calc_mate_geom(&k, a, CornerLookup::default(), &mut Brep::new(), &mut [])
            .unwrap_err();
        assert!(is_depth_error(&e), "{e:?}");
        let e = fb
            .calc_corner_vert_geom(&k, b, CornerLookup::default(), &mut Brep::new(), &mut [])
            .unwrap_err();
        assert!(is_depth_error(&e), "{e:?}");
        assert!(!fb.fillet_vertex(a).unwrap().is_processed());
    }

    #[test]
    fn mate_without_main_vertex_is_a_null_pointer() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let e = fb
            .calc_mate_geom(&k, a, CornerLookup::default(), &mut Brep::new(), &mut [])
            .unwrap_err();
        assert_eq!(code(&e), Some(SM_ERR_NULL_POINTER), "{e:?}");
    }

    #[test]
    fn corner_vertex_dispatch() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let v = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let mut target = Brep::new();
        // the calculators of parts 2 and 4 run (here they stop at the missing corner or
        // geometry), those of part 5 are stubs, the rest do nothing
        let ported = ["blend.fillet_geom_p2", "blend.fillet_geom_p4"];
        for t in [
            FilletVertexType::RailXRail,
            FilletVertexType::RailXEdgeuse,
            FilletVertexType::OnCrossSection,
            FilletVertexType::Setback,
            FilletVertexType::RailXExtendedEdgeuse,
            FilletVertexType::FilletX2Fillets,
        ] {
            fb.fillet_vertex_mut(v).unwrap().set_fillet_vertex_type(t);
            let r = fb.calc_corner_vert_geom(&k, v, CornerLookup::default(), &mut target, &mut []);
            let unit = r.as_ref().err().and_then(unit_of);
            assert!(unit.is_none_or(|u| !ported.contains(&u)), "{t:?}: {r:?}");
            assert!(r.is_err(), "{t:?}: {r:?}");
        }
        for (t, unit) in [
            (FilletVertexType::FilletXEdgeuse, None),
            (FilletVertexType::RailEnd, None),
            (FilletVertexType::Unknown, None),
        ] {
            fb.fillet_vertex_mut(v).unwrap().set_fillet_vertex_type(t);
            let r = fb.calc_corner_vert_geom(&k, v, CornerLookup::default(), &mut target, &mut []);
            assert_eq!(r.as_ref().err().and_then(unit_of), unit, "{t:?}: {r:?}");
        }
        // a processed vertex is not recomputed
        fb.fillet_vertex_mut(v)
            .unwrap()
            .set_fillet_vertex_type(FilletVertexType::RailXRail);
        fb.fillet_vertex_mut(v)
            .unwrap()
            .set_status(FilStatus::Processed);
        assert!(fb
            .calc_corner_vert_geom(&k, v, CornerLookup::default(), &mut target, &mut [])
            .is_ok());
    }

    #[test]
    fn corner_edge_dispatch() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let b = vertex(&mut fb, &k, Point3::new(1.0, 0.0, 0.0));
        let (e, _) = edge_with_uses(&mut fb, a, b, FilletEdgeType::CrossSection);

        // not made by a fillet corner
        let mut target = Brep::new();
        let err = fb
            .calc_corner_edge_geom(&k, e, CornerLookup::table(&[]), &mut [], &mut target)
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER), "{err:?}");

        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_corner(Some(FilletCornerId(0)));
        // the part 5 calculators run
        for t in [
            FilletEdgeType::CrossSection,
            FilletEdgeType::FilletXSideFace,
            FilletEdgeType::OnExtendEdge,
        ] {
            fb.fillet_edge_mut(e).unwrap().set_fillet_edge_type(t);
            let r = fb.calc_corner_edge_geom(&k, e, CornerLookup::table(&[]), &mut [], &mut target);
            assert_ne!(
                r.as_ref().err().and_then(unit_of),
                Some("blend.fillet_geom_p5"),
                "{t:?}: {r:?}"
            );
        }
        for (t, unit) in [
            (FilletEdgeType::Rail, None),
            (FilletEdgeType::Unknown, None),
        ] {
            fb.fillet_edge_mut(e).unwrap().set_fillet_edge_type(t);
            let r = fb.calc_corner_edge_geom(&k, e, CornerLookup::table(&[]), &mut [], &mut target);
            assert_eq!(r.as_ref().err().and_then(unit_of), unit, "{t:?}: {r:?}");
        }
        // CalcRailRailInterpolation (part 2) runs
        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::RailRailInterpolation);
        let r = fb.calc_corner_edge_geom(&k, e, CornerLookup::table(&[]), &mut [], &mut target);
        assert_ne!(
            r.as_ref().err().and_then(unit_of),
            Some("blend.fillet_geom_p2"),
            "{r:?}"
        );

        // the part 6 calculators run: without an original edge, or a corner in the
        // table (both SM_ERR_NULL_POINTER)
        for (t, want) in [
            (FilletEdgeType::OnEdge, Some(SM_ERR_NULL_POINTER)),
            (FilletEdgeType::FilletEnd, Some(SM_ERR_NULL_POINTER)),
            (FilletEdgeType::SetbackRail, Some(SM_ERR_NULL_POINTER)),
            (FilletEdgeType::RailExtension, Some(SM_ERR_NULL_POINTER)),
        ] {
            fb.fillet_edge_mut(e).unwrap().set_fillet_edge_type(t);
            let err = fb
                .calc_corner_edge_geom(&k, e, CornerLookup::table(&[]), &mut [], &mut target)
                .unwrap_err();
            assert!(!err.is_unported(), "{t:?}: {err:?}");
            assert_eq!(code(&err), want, "{t:?}: {err:?}");
        }
    }

    #[test]
    fn fillet_int_fillet_needs_a_processed_end() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let b = vertex(&mut fb, &k, Point3::new(1.0, 0.0, 0.0));
        let (e, [eu0, eu1]) = edge_with_uses(&mut fb, a, b, FilletEdgeType::FilletXFillet);

        // uses without fillet geometry
        let err = fb
            .calc_edge_fillet_int_fillet(&k, e, None, CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER), "{err:?}");

        let g1 = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap());
        let g2 = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Blends).unwrap());
        fb.set_edgeuse_fillet_geom(eu0, Some(g1)).unwrap();
        fb.set_edgeuse_fillet_geom(eu1, Some(g2)).unwrap();

        // neither end processed
        let err = fb
            .calc_edge_fillet_int_fillet(&k, e, None, CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR), "{err:?}");
        // the uses were still added to their geometries' side edge uses
        assert!(fb.geom(g1).unwrap().side_eus.contains(&eu0));
        assert!(fb.geom(g2).unwrap().side_eus.contains(&eu1));

        // one end processed: the vertex calculator (part 5) takes over
        fb.fillet_vertex_mut(a)
            .unwrap()
            .set_status(FilStatus::Processed);
        let r =
            fb.calc_edge_fillet_int_fillet(&k, e, None, CornerLookup::default(), &[], &Brep::new());
        assert_ne!(
            r.as_ref().err().and_then(unit_of),
            Some("blend.fillet_geom_p5"),
            "{r:?}"
        );

        // both processed, but the geometries have no solver
        fb.fillet_vertex_mut(b)
            .unwrap()
            .set_status(FilStatus::Processed);
        let err = fb
            .calc_edge_fillet_int_fillet(&k, e, None, CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER), "{err:?}");
    }

    #[test]
    fn recalc_needs_a_solver() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, _) = two_rail_geom(&mut fb, &k);
        let mut s = solver(true);
        let err = fb
            .re_calc_fillet_geom(&k, g, &mut s, &Brep::new(), false, None, None)
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn record_fillet_error_records_bad_int_with_or_without_error_info() {
        use crate::blend::fillet_corner::p3::FilletCornerKind;

        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let b = vertex(&mut fb, &k, Point3::new(1.0, 0.0, 0.0));
        let corner = FilletCorner::new(FilletCornerKind::TwoByTwo, None, None, None, 1e-6, 0.01);

        // without the executive's record the note goes to a scratch record
        record_fillet_error(&corner, None, FilletGeomId(0), FilletGeomId(1), a, b);

        let mut error_info = ErrorInfo::default();
        record_fillet_error(
            &corner,
            Some(&mut error_info),
            FilletGeomId(0),
            FilletGeomId(1),
            a,
            b,
        );
        assert_eq!(error_info.code, FilletErrorType::BadInt as i32);
        assert_eq!(
            error_info.surfaces,
            vec![
                Some(ExecutiveErrorObject::FilletGeomSurface(FilletGeomId(0))),
                Some(ExecutiveErrorObject::FilletGeomSurface(FilletGeomId(1))),
            ]
        );
        assert_eq!(
            error_info.fillet_vertices,
            [
                Some(ExecutiveErrorObject::FilletVertex(a)),
                Some(ExecutiveErrorObject::FilletVertex(b)),
            ]
        );
        assert_eq!(error_info.entity_name, "SmFillet2x2Corner");
    }

    /// A non-rational curve of dimension `dim` (`NOW` weights; `NOZ` z for 2D).
    fn nl_curve(dim: u64, cpts: &[[f64; 3]], p: usize, u: &[f64]) -> Curve {
        use crate::nurbs::math::{CPoint, NOW};
        let pw = cpts
            .iter()
            .map(|&[x, y, z]| CPoint::new(x, y, z, NOW))
            .collect();
        let nurb = crate::nurbs::crv_basic::Curve::new(pw, p, u.to_vec()).unwrap();
        Curve::from(crate::nurbs::b_spline_curve::BSplineCurve::from_nurb(
            nurb, dim, false,
        ))
    }

    fn nl_surface(net: &[&[[f64; 3]]], p: usize, q: usize, u: &[f64], v: &[f64]) -> Surface {
        use crate::nurbs::math::{CPoint, NOW};
        let net = net
            .iter()
            .map(|r| {
                r.iter()
                    .map(|&[x, y, z]| CPoint::new(x, y, z, NOW))
                    .collect()
            })
            .collect();
        let nl = crate::nurbs::srf_basic::Surface::new(net, p, q, u.to_vec(), v.to_vec()).unwrap();
        Surface::from(crate::nurbs::b_spline_surface::BSplineSurface::from_nurb(
            nl, false,
        ))
    }

    /// Regression for #1260: `SmFilletEdge::CalcFilletIntFillet` trims the edge's 3D
    /// curve with `SmTol::GetZoneTol3d` of its holding edge (tolerance 0 here), not the
    /// system zone tolerance (1e-5). `blend.fillet_geom_p3` case gen0252 (seed
    /// 9207379614715588057): the trimmed curve's last span is about 5e-6 long, which a
    /// 1e-5 trim removes as degenerate; the C++ reference keeps it.
    #[test]
    fn fillet_int_fillet_trims_with_the_holding_edges_zone_tolerance() {
        use crate::nurbs::math::NOZ;
        // the C++ reference's trimmed 3D curve: degree 1, three control points
        const CPP_KNOTS: [f64; 5] = [
            -1.104_513_452_122_645_8,
            -1.104_513_452_122_645_8,
            1.201_280_111_256_772_8,
            1.222_877_848_654_892_6,
            1.222_877_848_654_892_6,
        ];
        const CPP_LAST_CPT: [f64; 3] = [
            0.000_338_501_802_028_413_25,
            0.000_078_518_153_924_537_57,
            -0.000_248_006_678_376_056_8,
        ];
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let mut processed = |p: Point3| {
            let v = vertex(&mut fb, &k, p);
            let fv = fb.fillet_vertex_mut(v).unwrap();
            fv.set_fillet_vertex_type(FilletVertexType::FilletXFillet);
            fv.set_status(FilStatus::Processed);
            v
        };
        let vs = processed(Point3::new(
            0.000_105_817_878_476_310_83,
            -0.000_066_443_487_047_124_61,
            0.000_112_863_628_475_403_11,
        ));
        let ve = processed(Point3::new(
            0.000_338_501_802_028_413_25,
            0.000_078_518_153_924_537_57,
            -0.000_248_006_678_376_056_8,
        ));
        let (e, [eu0, eu1]) = edge_with_uses(&mut fb, vs, ve, FilletEdgeType::FilletXFillet);
        let (a, b) = (-2.177_188_262_530_754_6, 3.695_862_267_709_534_5);
        {
            let edge = fb.brep_mut().edge_mut(e).unwrap();
            edge.curve = Some(nl_curve(
                3,
                &[
                    [
                        0.0,
                        -0.000_133_769_082_604_263_4,
                        0.000_280_931_205_387_305_47,
                    ],
                    [
                        0.000_333_333_333_333_333_3,
                        0.000_078_339_309_224_387_8,
                        -0.000_248_352_554_485_777_14,
                    ],
                    [
                        0.000_666_666_666_666_666_6,
                        0.000_089_873_654_030_745_55,
                        -0.000_226_045_746_923_379_07,
                    ],
                    [
                        0.001,
                        -0.000_255_656_071_096_027,
                        -0.000_239_737_035_766_988_37,
                    ],
                ],
                1,
                &[a, a, 1.201_280_111_256_772_8, 2.594_196_703_017_6, b, b],
            ));
            edge.interval = crate::geom::extent1d::Extent1d::from_min_max(a, b);
        }
        fb.brep_mut().edgeuse_mut(eu0).unwrap().uv_trim_curve = Some(nl_curve(
            2,
            &[
                [0.277_635_386_028_107_06, 0.881_132_710_821_883_4, NOZ],
                [0.893_729_069_862_596_6, 0.388_682_638_272_226_16, NOZ],
                [2.234_800_311_318_374, 0.193_686_268_600_660_4, NOZ],
                [2.702_601_334_513_802_5, -0.644_854_980_880_039_2, NOZ],
            ],
            2,
            &[0.0, 0.0, 0.0, 0.145_627_121_098_850_12, 1.0, 1.0, 1.0],
        ));
        fb.brep_mut().edgeuse_mut(eu1).unwrap().uv_trim_curve = Some(nl_curve(
            2,
            &[
                [0.0, 0.293_485_605_878_511_3, NOZ],
                [0.25, 0.162_720_483_899_117_97, NOZ],
                [0.5, -0.056_321_910_574_329_476, NOZ],
                [0.75, -0.255_386_910_539_379_5, NOZ],
                [1.0, -0.261_044_370_175_247_56, NOZ],
            ],
            2,
            &[
                a,
                a,
                a,
                2.650_093_897_450_678_2,
                3.015_371_142_299_211_5,
                b,
                b,
                b,
            ],
        ));

        let solvers = [
            FilletSolver::new(1.000_000_000_000_000_1e-7, 0.003_870_572_434_734_073, 0.01),
            FilletSolver::new(1.000_000_000_000_000_1e-7, 0.096_393_681_031_218_52, 0.01),
        ];
        let s1 = nl_surface(
            &[
                &[
                    [0.0, 0.0, -0.000_173_903_058_274_925_46],
                    [0.0, 0.0005, -0.000_172_803_636_844_708_77],
                    [0.0, 0.001, 0.000_038_076_718_940_271_59],
                ],
                &[
                    [
                        0.000_333_333_333_333_333_3,
                        0.0,
                        -0.000_085_774_398_936_551_81,
                    ],
                    [
                        0.000_333_333_333_333_333_3,
                        0.0005,
                        0.000_133_617_778_651_449_14,
                    ],
                    [
                        0.000_333_333_333_333_333_3,
                        0.001,
                        -0.000_076_690_057_952_548_91,
                    ],
                ],
                &[
                    [
                        0.000_666_666_666_666_666_6,
                        0.0,
                        0.000_026_802_323_645_016_252,
                    ],
                    [
                        0.000_666_666_666_666_666_6,
                        0.0005,
                        0.000_107_681_378_539_725_81,
                    ],
                    [
                        0.000_666_666_666_666_666_6,
                        0.001,
                        -0.000_179_497_678_246_563_82,
                    ],
                ],
                &[
                    [0.001, 0.0, -0.000_128_131_732_519_285_1],
                    [0.001, 0.0005, 0.000_096_610_858_228_524_5],
                    [0.001, 0.001, 0.000_057_059_943_579_616_247],
                ],
            ],
            2,
            1,
            &[0.0, 0.0, 0.0, 0.499_833_800_950_345_1, 1.0, 1.0, 1.0],
            &[0.0, 0.0, 0.164_652_975_445_663_43, 1.0, 1.0],
        );
        let s2 = nl_surface(
            &[
                &[
                    [0.0, 0.0, 0.000_411_449_121_375_960_54],
                    [0.0, 0.000_333_333_333_333_333_3, 0.000_349_270_388_934_398],
                    [
                        0.0,
                        0.000_666_666_666_666_666_6,
                        0.000_584_502_492_069_972_4,
                    ],
                    [0.0, 0.001, 0.000_645_357_628_003_180_3],
                ],
                &[
                    [0.0005, 0.0, 0.000_374_530_506_766_174_14],
                    [
                        0.0005,
                        0.000_333_333_333_333_333_3,
                        0.000_451_442_735_062_868_35,
                    ],
                    [
                        0.0005,
                        0.000_666_666_666_666_666_6,
                        0.000_649_504_122_310_550_2,
                    ],
                    [0.0005, 0.001, 0.000_674_899_707_060_231_2],
                ],
                &[
                    [0.001, 0.0, 0.000_608_871_472_768_614_4],
                    [
                        0.001,
                        0.000_333_333_333_333_333_3,
                        0.000_361_322_113_274_977_2,
                    ],
                    [
                        0.001,
                        0.000_666_666_666_666_666_6,
                        0.000_693_412_711_301_771,
                    ],
                    [0.001, 0.001, 0.000_463_392_478_135_597_3],
                ],
            ],
            2,
            2,
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &[0.0, 0.0, 0.0, 0.242_484_947_411_334_47, 1.0, 1.0, 1.0],
        );
        for (i, (eu, srf)) in [(eu0, s1), (eu1, s2)].into_iter().enumerate() {
            let id = FilletSolverId(i as u32);
            let mut g =
                new_solver_fillet_geom(&k, id, &solvers[i], FilletGeomType::Blends).unwrap();
            g.set_fillet_surface(Some(srf));
            let g = fb.add_geom(g);
            fb.set_edgeuse_fillet_geom(eu, Some(g)).unwrap();
        }

        fb.calc_edge_fillet_int_fillet(
            &k,
            e,
            None,
            CornerLookup::default(),
            &solvers,
            &Brep::new(),
        )
        .unwrap();

        let curve = fb.brep().edge(e).unwrap().curve.as_ref().unwrap();
        let nurb = match curve.kind() {
            CurveKind::BSpline(bs) => bs.gw_nurb().unwrap(),
            other => panic!("not a B-spline: {other:?}"),
        };
        assert_eq!(nurb.knots(), &CPP_KNOTS[..]);
        assert_eq!(nurb.cpts().len(), 3);
        let last = nurb.cpts().last().unwrap();
        assert_eq!([last.x, last.y, last.z], CPP_LAST_CPT);
    }
}

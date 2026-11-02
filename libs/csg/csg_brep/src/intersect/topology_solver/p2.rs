//! Local topology traversal, face solves and static B-rep/shape queries.
//!
//! Upstream: `source/SMLib/src/SmTopologySolver.cpp`, `source/SMLib/inc/SmTopologySolver.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`LocalSolver::new`] (shared in p1) | `SmTLSVertexEdge` / `SmTLSEdgeEdge` constructors |
//! | [`TopologySolver::local_solve_branch`] | `SmTopologySolver::LocalSolve` |
//! | `LocalSolver::solve_vertex_face` | `SmTLSVertexFace::SolveIt` |
//! | `LocalSolver::solve_curve_vertex` | `SmTLSCurveVertex::SolveIt` |
//! | `LocalSolver::solve_edge_vertex` | `SmTLSEdgeVertex::SolveIt` |
//! | `LocalSolver::solve_curve_face` | `SmTLSCurveFace::SolveIt` |
//! | `LocalSolver::solve_edge_face` | `SmTLSEdgeFace::SolveIt` |
//! | `LocalSolver::solve_face_face` | `SmTLSFaceFace::SolveIt` |
//! | [`TopologySolver::brep_point_solve`] | `SmTopologySolver::BrepPointSolve` |
//! | [`TopologySolver::shape_point_solve`] | `SmTopologySolver::ShapePointSolve` |
//! | [`TopologySolver::brep_curve_solve`] | `SmTopologySolver::BrepCurveSolve` |
//! | [`TopologySolver::shape_curve_solve`] | `SmTopologySolver::ShapeCurveSolve` |
//! | [`TopologySolver::solve_two_tree_sets`] | `sm_SolveTwoTrees` |
//! | [`TopologySolver::brep_brep_solve`] | `SmTopologySolver::BrepBrepSolve` |
//!
//! Deviations:
//! - Topology pointers are adapter handles. [`TopologyTrees`] borrows cache trees;
//!   the default B-rep cache lookup uses the adapter's `BrepCache`. Shape topology keys
//!   resolve to vertices, edge curves and face surfaces before tree traversal.
//! - The local solver is taken from its parent and restored even on error.
//!   The existing global solver and object-list walks supply traversal bounds;
//!   this unit adds no input-driven recursion.
//! - Static output arrays become returned [`SolutionArray`] values. On error the
//!   caller receives the error, not the partial output C++ may have accumulated.
//! - `SmTLSEdgeVertex::SolveIt` selects the vertex operand as a curve (upstream
//!   line 1032). Its invalid cast is a controlled `InvalidInput` here; the operand
//!   is not silently corrected. Native tests probe its class without that cast.
//! - Range endpoint refinement needs `SmSurface::GlobalPointSolve` in part 4.
//!   It must disable point testing on that SAME cache for BOTH endpoint solves.
//! - Signed B-rep/curve and shape/curve queries test only the start parameter and
//!   rewrite only start parameters, preserving upstream's `i < 1` behavior.

use super::p1::TopologyObjects;
use super::p1::{self, best_answer_so_far, CurveRef};
use super::p1::{LocalSolver, LocalSolverKind, TopologySolver};
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::Branch;
use crate::geom::global_solver::{GlobalSolve, SolutionRequested, SolverOperation};
use crate::geom::object::{Shape, ShapeMember};
use crate::geom::solution_array::SolutionObject;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionType, SortKey};
use crate::geom::tol::{are_same, zone_tol3d};
use crate::geom::tree::{AuxDataType, Tree};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p4::ClassificationInit;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec2};
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::query::face::p2 as qf2;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::surface::trim_srf_cache::POINT_CLASS_UNKNOWN;
use crate::topology::face::p3::{IntersectObject, IntersectObjects};

const OP_LOCAL: &str = "intersect::topology_solver::p2::TopologySolver::local_solve_branch";
const OP_VF: &str = "intersect::topology_solver::p2::LocalSolver::solve_vertex_face";
const OP_CV: &str = "intersect::topology_solver::p2::LocalSolver::solve_curve_vertex";

impl<'t, O: SolutionObject + 't> TopologySolver<'t, '_, O> {
    /// Solves the object pairs of the two branch nodes in their native list order
    /// (`LocalSolve`), asking for further subdivision when either has child one.
    ///
    /// # Errors
    /// Invalid branches or object lists, missing local solver, and local-solve errors.
    pub fn local_solve_branch(&mut self, k: &Kernel, branch: &Branch) -> Result<bool> {
        let tree1 = self.base.tree(0)?;
        let tree2 = self.base.tree(1)?;
        let node1 = self.base.branch_node(branch, 0)?;
        let node2 = self.base.branch_node(branch, 1)?;
        let id1 = branch
            .first()
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_LOCAL))?;
        let id2 = branch
            .get(1)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_LOCAL))?;
        let first_objects = if node1.aux_type == AuxDataType::ObjectList {
            tree1.node_object_list(id1)?
        } else {
            Vec::new()
        };
        for list1 in first_objects {
            let object1 = tree1.list(list1)?;
            if !self.may_contain_answer(&object1.bbox, &node2.bbox)? {
                continue;
            }
            let second_objects = if node2.aux_type == AuxDataType::ObjectList {
                tree2.node_object_list(id2)?
            } else {
                Vec::new()
            };
            for list2 in second_objects {
                let object2 = tree2.list(list2)?;
                if !self.may_contain_answer(&object1.bbox, &object2.bbox)? {
                    continue;
                }
                let mut local = self
                    .local_solver
                    .take()
                    .ok_or(SolidError::null_pointer(OP_LOCAL))?;
                let result = local.solve_it(k, self, object1.object, object2.object);
                self.local_solver = Some(local);
                result?;
            }
        }
        Ok(node1.child1.is_some() || node2.child1.is_some())
    }
}

impl<'a, O: SolutionObject> LocalSolver<'a, O> {
    pub(super) fn solve_vertex_face(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let vertex = if self.swap_order { object2 } else { object1 }
            .ok_or(SolidError::null_pointer(OP_VF))?;
        self.point_vertex = Some(vertex);
        self.point = ts.objects.vertex_point(vertex)?;
        self.solve_point_face(k, ts, object1, object2)
    }

    pub(super) fn solve_curve_vertex(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        _object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let mut point_curve = Self::new(LocalSolverKind::PointCurve, !self.swap_order, self.shape);
        if let Some(edge) = self.curve_edge {
            point_curve.curve_owner = Some(edge);
            point_curve.interval = ts.objects.edge_interval(edge)?;
        } else {
            point_curve.interval = self.interval;
        }
        let vertex = object2.ok_or(SolidError::null_pointer(OP_CV))?;
        point_curve.point_vertex = Some(vertex);
        point_curve.point = ts.objects.vertex_point(vertex)?;
        let curve = self.curve.ok_or(SolidError::null_pointer(OP_CV))?;
        point_curve.point_curve(k, ts, curve.curve, Some(curve.handle))
    }
}

const OP_EV: &str = "intersect::topology_solver::p2::LocalSolver::solve_edge_vertex";
const OP_CF: &str = "intersect::topology_solver::p2::LocalSolver::solve_curve_face";
const OP_EF: &str = "intersect::topology_solver::p2::LocalSolver::solve_edge_face";
const OP_FF: &str = "intersect::topology_solver::p2::LocalSolver::solve_face_face";

fn swap_curve_surface_parameters<O>(solution: &mut Solution<O>) -> Result<()> {
    solution
        .start
        .parameters
        .get_mut(..3)
        .ok_or(SolidError::sm_err(OP_CF))?
        .rotate_left(1);
    if solution.solution_type == SolutionType::RangeOfValues {
        solution
            .end
            .parameters
            .get_mut(..3)
            .ok_or(SolidError::sm_err(OP_CF))?
            .rotate_left(1);
    }
    Ok(())
}

fn name_curve_face<O: Copy + PartialEq>(
    solution: &mut Solution<O>,
    objects: [Option<O>; 2],
    surface: O,
    face: O,
) {
    solution.objects[0] = if objects[0] == Some(surface) {
        Some(face)
    } else {
        objects[0]
    };
    solution.objects[1] = if objects[1] == Some(surface) {
        Some(face)
    } else {
        objects[1]
    };
    solution.num_objects = 2;
    solution.num_variables = 3;
}

/// `SmCurveClassification sCurveClass(m_pCurve, interval, NULL, tol, 10, sCIData)` followed
/// by `pFace->CurveOnClassify(TRUE, sCurveClass)` (the remaining arguments default): the
/// intervals and their interior point classes.
///
/// The face is the B-rep face [`TopologyObjects::face_entity`] names; the classification
/// runs on that B-rep through [`qf2::curve_on_classify`], with the solver's curve as the
/// classified curve ([`IntersectObject::Curve`] over a one-curve slice) and the face's
/// topology as handles of the B-rep.
fn classified_curve_intervals<O: Copy>(
    kernel: &Kernel,
    objects: &dyn TopologyObjects<O>,
    curve: &Curve,
    interval: &Extent1d,
    face: O,
    tol: f64,
) -> Result<Vec<(Extent1d, i32)>> {
    let init = ClassificationInit {
        curve: Some(IntersectObject::Curve(0)),
        interval: *interval,
        uv_curve: None,
        // the double converts to the SmZoneTol3d parameter
        src_zone_tol_3d: zone_tol3d(kernel, tol),
        opt_data_size: 10,
        is_curve_owned: false,
        is_curve_current: true,
        surface1: None,
        surface2: None,
        uv_curve2: None,
        ti: None,
    };
    let mut classification = CurveClassification::with_curve(kernel, &init)?;
    let Some((b, f)) = objects.face_entity(face) else {
        // DISPATCH GAP: SmFace::CurveOnClassify is ported (`query::face::p2`), but this
        // adapter cannot resolve the face handle to its `Brep` and `FaceId`
        // (`TopologyObjects::face_entity`).
        return Err(SolidError::unported(
            "SmFace::CurveOnClassify",
            "query.face_p2",
        ));
    };
    let classify_objects = IntersectObjects {
        b,
        other: b,
        side: false,
        curves_3d: core::slice::from_ref(curve),
        uv1: &[],
        uv2: &[],
    };
    qf2::curve_on_classify(
        kernel,
        b,
        f,
        &classify_objects,
        true,
        &mut classification,
        false,
        false,
        true,
    )?;
    Ok(classification
        .intervals()
        .iter()
        .map(|i| (i.interval, i.mid.point_class().code()))
        .collect())
}

/// The start ends of `pSurface->GlobalPointSolve(domain, SM_SO_INTERSECT, point, tol,
/// NULL, SM_SR_ALL, ..)` with the surface cache's point test switched off.
fn point_test_off_starts(
    kernel: &Kernel,
    surface: &Surface,
    face_domain: &Extent2d,
    point: Point3,
    tol: f64,
) -> Result<Vec<crate::geom::solution_array::SolutionEnd>> {
    let solutions = surface.global_point_solve_point_test_off::<NoObject>(
        kernel,
        Some(NoObject),
        face_domain,
        SolverOperation::Intersect,
        point,
        tol,
        None,
        SolutionRequested::All,
    )?;
    Ok(solutions.as_slice().iter().map(|s| s.start).collect())
}

fn untrimmed_range_endpoints<O>(
    kernel: &Kernel,
    surface: &Surface,
    _surface_handle: O,
    face_domain: &Extent2d,
    start: Point3,
    end: Point3,
    tol: f64,
) -> Result<Option<(Vec2, Vec2)>> {
    // Upstream switches the surface cache's point test off around both solves
    // (`SmTemporaryChangeValue` on `m_bPointTestEnabled`), so every point solution is
    // kept without classifying it against the face's trim boundaries.
    let first = point_test_off_starts(kernel, surface, face_domain, start, tol)?;
    let Some(s) = first.first() else {
        return Ok(None); // error - something is wrong here - quit
    };
    let uv_start = Vec2::new(s.param(0)?, s.param(1)?);
    let second = point_test_off_starts(kernel, surface, face_domain, end, tol)?;
    let Some(e) = second.first() else {
        return Ok(None);
    };
    Ok(Some((uv_start, Vec2::new(e.param(0)?, e.param(1)?))))
}

impl<'a, O: SolutionObject> LocalSolver<'a, O> {
    pub(super) fn solve_edge_vertex(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let vertex = if self.swap_order { object1 } else { object2 }
            .ok_or(SolidError::null_pointer(OP_EV))?;
        // Upstream selects this SAME object as pCurve (SmTopologySolver.cpp:1032):
        // for a real vertex that cast is invalid. Validate the selected object class
        // instead of reinterpreting memory or silently correcting the operand.
        let curve_object = if self.swap_order { object1 } else { object2 }
            .ok_or(SolidError::null_pointer(OP_EV))?;
        if curve_object.kind() != crate::geom::solution_array::ObjectKind::Curve {
            return Err(SolidError::invalid_input(
                OP_EV,
                "upstream selects a vertex as a curve",
            ));
        }
        let _ = ts.objects.curve(curve_object)?;
        let edge = ts.objects.curve_edge(curve_object)?;
        if self.in_shape(ts.objects, edge) {
            let edge = edge.ok_or(SolidError::invalid_input(
                OP_EV,
                "upstream selects a vertex as a curve",
            ))?;
            self.curve_edge = Some(edge);
            self.interval = ts.objects.edge_interval(edge)?;
            self.curve = match ts.objects.edge_curve(edge)? {
                Some(handle) => Some(CurveRef {
                    curve: ts.objects.curve(handle)?,
                    handle,
                }),
                None => None,
            };
            self.solve_curve_vertex(k, ts, Some(vertex), Some(vertex))?;
        }
        Ok(())
    }

    pub(super) fn solve_edge_face(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let curve = if self.swap_order { object2 } else { object1 }
            .ok_or(SolidError::null_pointer(OP_EF))?;
        let edge = ts.objects.curve_edge(curve)?;
        if self.in_shape(ts.objects, edge) {
            let edge = edge.ok_or(SolidError::invalid_input(OP_EF, "curve without an edge"))?;
            self.curve_edge = Some(edge);
            self.interval = ts.objects.edge_interval(edge)?;
            self.curve = match ts.objects.edge_curve(edge)? {
                Some(handle) => Some(CurveRef {
                    curve: ts.objects.curve(handle)?,
                    handle,
                }),
                None => None,
            };
            self.solve_curve_face(k, ts, object1, object2)?;
        }
        Ok(())
    }

    pub(super) fn solve_curve_face(
        &self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let surface_handle = if self.swap_order { object1 } else { object2 }
            .ok_or(SolidError::null_pointer(OP_CF))?;
        let owner = self
            .curve_edge
            .or(self.curve_face)
            .or(self.curve.map(|c| c.handle));
        let named = if self.swap_order {
            [Some(surface_handle), owner]
        } else {
            [owner, Some(surface_handle)]
        };
        let face = ts.objects.surface_face(surface_handle)?;
        if !self.in_shape(ts.objects, face) {
            return Ok(());
        }
        let face = face.ok_or(SolidError::invalid_input(OP_CF, "surface without a face"))?;
        let curve = self.curve.ok_or(SolidError::null_pointer(OP_CF))?;
        if ts.base.projected_operation {
            let (curves, uv_curves) = p1::face_silhouettes(
                ts.kernel,
                ts.objects,
                face,
                &ts.base.opt_vectors,
                ts.base.tolerance_3d,
            )?;
            for (i, other) in curves.iter().enumerate() {
                let mut local =
                    LocalSolver::new(LocalSolverKind::CurveCurve, self.swap_order, self.shape);
                local.curve = Some(curve);
                local.curve_edge = self.curve_edge;
                local.curve_face = self.curve_face;
                local.interval = self.interval;
                local.surface_curve = self.surface_curve;
                // The face owner replaces this temporary curve in every result;
                // its placeholder handle is never used as a curve lookup.
                local.curve2 = Some(CurveRef {
                    curve: other,
                    handle: face,
                });
                local.curve_edge2 = None;
                local.interval2 = other.natural_interval();
                local.curve_face2 = Some(face);
                local.surface_curve2 = Some(uv_curves.get(i).ok_or(SolidError::sm_err(OP_CF))?);
                // (the silhouette curve exists only for this solve)
                local.temporary_curves = true;
                local.solve_curve_curve(k, ts)?;
            }
            return Ok(());
        }
        let best = best_answer_so_far(&ts.base);
        let face_surface_handle = ts.objects.face_surface(face)?;
        let surface = ts.objects.surface(face_surface_handle)?;
        let domain = ts.objects.face_uv_domain(face)?;
        let tol = ts.base.tolerance_3d + ts.objects.face_tolerance(face)?;
        // the surface reaches its face's cache through the caller's context
        // (`smsurf_GetSurfaceCache`), when it provides one
        let mut context = ts.objects.surface_solve_context();
        let solutions = if ts.base.operation == SolverOperation::Intersect {
            // surface->GlobalCurveIntersect(domain, curve, interval, tol, solutions)
            // (SER): the solutions name the curve and the surface
            if let Some(context) = context.as_deref_mut() {
                surface.global_curve_intersect_with_context(
                    ts.kernel,
                    [Some(curve.handle), Some(surface_handle)],
                    None,
                    context,
                    &domain,
                    curve.curve,
                    &self.interval,
                    tol,
                )?
            } else {
                surface.global_curve_intersect(
                    ts.kernel,
                    [curve.handle, surface_handle],
                    &domain,
                    curve.curve,
                    &self.interval,
                    tol,
                )?
            }
        } else {
            let mut vectors = ts.base.opt_vectors.clone();
            if ts.base.operation == SolverOperation::Signed3dDirectedMinimize && self.swap_order {
                let vector = vectors
                    .first_mut()
                    .ok_or(SolidError::invalid_input(OP_CF, "missing direction vector"))?;
                *vector = -*vector;
                vectors.truncate(1);
            }
            if let Some(context) = context.as_deref_mut() {
                surface.global_curve_solve_with_context(
                    ts.kernel,
                    &domain,
                    curve.curve,
                    &self.interval,
                    ts.base.operation,
                    tol,
                    best,
                    &vectors,
                    SolutionRequested::All,
                    [Some(curve.handle), Some(face_surface_handle)],
                    context,
                )?
            } else {
                surface.global_curve_solve(
                    ts.kernel,
                    &domain,
                    curve.curve,
                    &self.interval,
                    ts.base.operation,
                    tol,
                    best,
                    &vectors,
                    SolutionRequested::All,
                    [Some(curve.handle), Some(face_surface_handle)],
                )?
            }
        };
        for found in solutions.as_slice() {
            let mut solution = *found;
            if ts.base.operation == SolverOperation::Intersect {
                if solution.solution_type == SolutionType::RangeOfValues
                    && !are_same(solution.start.param(0)?, solution.end.param(0)?)
                {
                    let interval =
                        Extent1d::from_min_max(solution.start.param(0)?, solution.end.param(0)?);
                    let intervals = classified_curve_intervals(
                        ts.kernel,
                        ts.objects,
                        curve.curve,
                        &interval,
                        face,
                        ts.base.tolerance_3d,
                    )?;
                    for (interval, class) in intervals {
                        // Keep only SM_PC_FACE / SM_PC_EDGE intervals.
                        if class != crate::surface::trim_srf_cache::POINT_CLASS_FACE
                            && class != crate::surface::trim_srf_cache::POINT_CLASS_EDGE
                        {
                            continue;
                        }
                        solution.start.parameters[0] = interval.min();
                        solution.end.parameters[0] = interval.max();
                        let start = curve.curve.evaluate_point(k, interval.min())?;
                        let end = curve.curve.evaluate_point(k, interval.max())?;
                        if let Some((uv_start, uv_end)) = untrimmed_range_endpoints(
                            ts.kernel,
                            ts.objects.surface(surface_handle)?,
                            surface_handle,
                            &domain,
                            start,
                            end,
                            2.0 * tol,
                        )? {
                            solution.start.parameters[1] = uv_start.x;
                            solution.start.parameters[2] = uv_start.y;
                            solution.end.parameters[1] = uv_end.x;
                            solution.end.parameters[2] = uv_end.y;
                        } else {
                            continue;
                        }
                        name_curve_face(&mut solution, named, surface_handle, face);
                        if self.swap_order {
                            swap_curve_surface_parameters(&mut solution)?;
                        }
                        ts.add_sorted_solution(&solution, SortKey::BySolutionValue, false)?;
                    }
                    continue;
                }
                let src_tol = if let Some(edge) = self.curve_edge {
                    ts.objects.edge_tolerance(edge)?
                } else if let Some(face) = self.curve_face {
                    ts.objects.face_tolerance(face)?
                } else {
                    p1::curve_zone_tol_3d(ts.kernel, ts.objects, self.curve.map(|c| c.handle))?
                };
                let uv = Vec2::new(solution.start.param(1)?, solution.start.param(2)?);
                if p1::face_point_classify(ts.kernel, ts.objects, face, uv, src_tol, false, false)?
                    == POINT_CLASS_UNKNOWN
                {
                    continue;
                }
            }
            name_curve_face(&mut solution, named, surface_handle, face);
            if self.swap_order {
                swap_curve_surface_parameters(&mut solution)?;
            }
            ts.add_sorted_solution(&solution, SortKey::BySolutionValue, true)?;
        }
        Ok(())
    }

    pub(super) fn solve_face_face(
        &self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let surface1 = object1.ok_or(SolidError::null_pointer(OP_FF))?;
        let surface2 = object2.ok_or(SolidError::null_pointer(OP_FF))?;
        let face1 = ts.objects.surface_face(surface1)?;
        let face2 = ts.objects.surface_face(surface2)?;
        if !self.in_shape(ts.objects, face1) || !self.in_shape(ts.objects, face2) {
            return Ok(());
        }
        let face1 = face1.ok_or(SolidError::invalid_input(
            OP_FF,
            "first surface without a face",
        ))?;
        let face2 = face2.ok_or(SolidError::invalid_input(
            OP_FF,
            "second surface without a face",
        ))?;
        let best = best_answer_so_far(&ts.base);
        if ts.base.projected_operation {
            let (curves, uv_curves) = p1::face_silhouettes(
                ts.kernel,
                ts.objects,
                face1,
                &ts.base.opt_vectors,
                ts.base.tolerance_3d,
            )?;
            for (i, curve) in curves.iter().enumerate() {
                let mut local =
                    LocalSolver::new(LocalSolverKind::CurveFace, self.swap_order, self.shape);
                local.curve = Some(CurveRef {
                    curve,
                    handle: face1,
                });
                local.curve_face = Some(face1);
                local.curve_edge = None;
                local.interval = curve.natural_interval();
                local.surface_curve = Some(uv_curves.get(i).ok_or(SolidError::sm_err(OP_FF))?);
                local.solve_curve_face(k, ts, Some(surface2), Some(surface2))?;
            }
        } else {
            let a = ts.objects.surface(surface1)?;
            let b = ts.objects.surface(surface2)?;
            // the surfaces reach their faces' caches through the caller's context
            // (`smsurf_GetSurfaceCache`), when it provides one
            let mut context = ts.objects.surface_solve_context();
            let solutions = if let Some(context) = context.as_deref_mut() {
                a.global_surface_solve_with_context(
                    ts.kernel,
                    &ts.objects.face_uv_domain(face1)?,
                    b,
                    &ts.objects.face_uv_domain(face2)?,
                    ts.base.operation,
                    ts.base.tolerance_3d,
                    best,
                    &ts.base.opt_vectors,
                    ts.base.solution_requested,
                    [Some(surface1), Some(surface2)],
                    context,
                )?
            } else {
                a.global_surface_solve(
                    ts.kernel,
                    &ts.objects.face_uv_domain(face1)?,
                    b,
                    &ts.objects.face_uv_domain(face2)?,
                    ts.base.operation,
                    ts.base.tolerance_3d,
                    best,
                    &ts.base.opt_vectors,
                    ts.base.solution_requested,
                    [Some(surface1), Some(surface2)],
                )?
            };
            for found in solutions.as_slice() {
                let mut solution = *found;
                solution.objects[0] = Some(face1);
                solution.objects[1] = Some(face2);
                solution.num_objects = 2;
                solution.num_variables = 4;
                ts.add_sorted_solution(&solution, SortKey::BySolutionValue, true)?;
            }
        }
        Ok(())
    }
}

/// The three B-rep cache trees, with object handles resolved by the adapter.
/// These are the `GetVertexTree`, `GetCurveTree`, and `GetSurfaceTree` views.
#[derive(Debug, Clone, Copy)]
pub struct TopologyTrees<'t, O> {
    /// Vertices.
    pub vertices: &'t Tree<O>,
    /// Edge curves.
    pub curves: &'t Tree<O>,
    /// Face surfaces.
    pub surfaces: &'t Tree<O>,
}

/// Options shared by the static topology-solver entry points.
#[derive(Debug, Clone, Copy)]
pub struct TopologySolveOptions<'v> {
    /// Operation to solve.
    pub operation: SolverOperation,
    /// Which solutions to keep.
    pub requested: SolutionRequested,
    /// Geometric tolerance.
    pub tolerance_3d: f64,
    /// Initial squared best answer (including the signed convention).
    pub best_answer_so_far_sq: f64,
    /// Optional operation vectors, empty for the upstream null pointer.
    pub opt_vectors: &'v [crate::math::Vec3],
}

struct ShapeTreeViews<O> {
    vertices: Tree<O>,
    curves: Tree<O>,
    surfaces: Tree<O>,
}

impl<O: SolutionObject> ShapeTreeViews<O> {
    fn new(objects: &dyn TopologyObjects<O>, shape: &Shape) -> Result<Self> {
        Ok(Self {
            vertices: shape
                .vertex_tree()
                .map_objects(|key| objects.shape_geometry_object(ShapeMember::Vertex(key)))?,
            curves: shape
                .curve_tree()
                .map_objects(|key| objects.shape_geometry_object(ShapeMember::Edge(key)))?,
            surfaces: shape
                .surface_tree()
                .map_objects(|key| objects.shape_geometry_object(ShapeMember::Face(key)))?,
        })
    }
    fn views(&self) -> TopologyTrees<'_, O> {
        TopologyTrees {
            vertices: &self.vertices,
            curves: &self.curves,
            surfaces: &self.surfaces,
        }
    }
}

fn valid_point_operation(op: SolverOperation) -> bool {
    matches!(
        op,
        SolverOperation::Minimize
            | SolverOperation::Maximize
            | SolverOperation::Rayfire
            | SolverOperation::ProjectedMinimize
            | SolverOperation::ProjectedMaximize
            | SolverOperation::SignedDirectedMinimize
            | SolverOperation::Signed3dDirectedMinimize
            | SolverOperation::DirectedMinimize
            | SolverOperation::DirectedMaximize
            | SolverOperation::Intersect
            | SolverOperation::Normalize
    )
}

fn valid_curve_operation(op: SolverOperation, wireframe: bool) -> bool {
    (valid_point_operation(op) && op != SolverOperation::Rayfire)
        || (wireframe && op == SolverOperation::IntersectWireframe)
}

fn point_tree<O: Copy + PartialEq>(point: Point3) -> Tree<O> {
    Tree::from_point(point, 0.0, true)
}

fn curve_tree<O: SolutionObject>(
    k: &Kernel,
    curve: CurveRef<'_, O>,
    interval: &Extent1d,
) -> Result<Tree<O>> {
    const OP: &str = "intersect::topology_solver::p2::curve_tree";
    let mut tree = point_tree(Point3::new(0.0, 0.0, 0.0));
    let bbox = curve
        .curve
        .calculate_bounding_box(
            k,
            interval,
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
            true,
        )?
        .normal
        .ok_or(SolidError::sm_err(OP))?;
    let root = tree.top_node().ok_or(SolidError::sm_err(OP))?;
    tree.node_mut(root)?.bbox = bbox;
    let list = tree.node(root)?.list_head().ok_or(SolidError::sm_err(OP))?;
    let item = tree.list_mut(list)?;
    item.bbox = bbox;
    item.object = Some(curve.handle);
    Ok(tree)
}

fn new_topology_solver<'t, 'a, O: SolutionObject + 't>(
    kernel: &'a Kernel,
    objects: &'a dyn TopologyObjects<O>,
    options: TopologySolveOptions<'_>,
) -> TopologySolver<'t, 'a, O> {
    TopologySolver::new(
        kernel,
        objects,
        options.operation,
        options.requested,
        options.tolerance_3d,
        options.best_answer_so_far_sq,
        options.opt_vectors,
    )
}

fn early_point_answer<O: SolutionObject>(
    ts: &TopologySolver<'_, '_, O>,
    options: TopologySolveOptions<'_>,
) -> bool {
    options.requested == SolutionRequested::Single
        && ts
            .base
            .solutions
            .as_slice()
            .first()
            .is_some_and(|s| s.start.solution_value <= options.tolerance_3d)
}

fn rewrite_endpoint_solutions<O: SolutionObject>(
    solutions: &mut SolutionArray<O>,
    curve: O,
    parameter: f64,
) {
    for i in 0..solutions.len() {
        if let Some(solution) = solutions.get_mut(i) {
            if solution.num_objects == 1 && solution.num_variables == 2 {
                solution.num_objects = 2;
                solution.objects[1] = solution.objects[0];
                solution.objects[0] = Some(curve);
                solution.start.parameters[2] = solution.start.parameters[1];
                solution.start.parameters[1] = solution.start.parameters[0];
                solution.start.parameters[0] = parameter;
                solution.num_variables = 3;
            }
        }
    }
}

impl<'t, O: SolutionObject + 't> TopologySolver<'t, '_, O> {
    /// Solves a query point against a B-rep's vertex, edge and face trees
    /// (`BrepPointSolve`), retaining upstream's early single-answer exits.
    ///
    /// # Errors
    /// Invalid operation or null B-rep, unavailable B-rep cache, or a local solve.
    pub fn brep_point_solve(
        kernel: &Kernel,
        objects: &dyn TopologyObjects<O>,
        brep: Option<O>,
        point: Point3,
        options: TopologySolveOptions<'_>,
    ) -> Result<SolutionArray<O>> {
        const OP: &str = "intersect::topology_solver::p2::TopologySolver::brep_point_solve";
        if !valid_point_operation(options.operation) {
            return Err(SolidError::invalid_input(OP, "unsupported operation"));
        }
        let brep = brep.ok_or(SolidError::null_pointer(OP))?;
        let mut ts = new_topology_solver(kernel, objects, options);
        let trees = objects.brep_trees(brep)?;
        let query = point_tree(point);
        if options.operation != SolverOperation::Normalize {
            let mut vertices = LocalSolver::new(LocalSolverKind::PointVertex, false, None);
            vertices.set_point(point);
            ts.solve_trees(&query, trees.vertices, &mut vertices)?;
            if early_point_answer(&ts, options) {
                return Ok(ts.base.solutions);
            }
            let mut edges = LocalSolver::new(LocalSolverKind::PointEdge, false, None);
            edges.set_point(point);
            ts.solve_trees(&query, trees.curves, &mut edges)?;
            if early_point_answer(&ts, options) {
                return Ok(ts.base.solutions);
            }
        }
        let mut faces = LocalSolver::new(LocalSolverKind::PointFace, false, None);
        faces.set_point(point);
        ts.solve_trees(&query, trees.surfaces, &mut faces)?;
        Ok(ts.base.solutions)
    }

    /// Solves a point against the members of `shape` (`ShapePointSolve`).
    ///
    /// # Errors
    /// Invalid operation, handle mapping failure or a local solve.
    pub fn shape_point_solve(
        kernel: &Kernel,
        objects: &dyn TopologyObjects<O>,
        shape: &Shape,
        point: Point3,
        options: TopologySolveOptions<'_>,
    ) -> Result<SolutionArray<O>> {
        const OP: &str = "intersect::topology_solver::p2::TopologySolver::shape_point_solve";
        if !valid_point_operation(options.operation) {
            return Err(SolidError::invalid_input(OP, "unsupported operation"));
        }
        let mut ts = new_topology_solver(kernel, objects, options);
        let mapped = ShapeTreeViews::new(objects, shape)?;
        let trees = mapped.views();
        let query = point_tree(point);
        if options.operation != SolverOperation::Normalize {
            let mut vertices = LocalSolver::new(LocalSolverKind::PointVertex, false, Some(shape));
            vertices.set_point(point);
            ts.solve_trees(&query, trees.vertices, &mut vertices)?;
            let mut edges = LocalSolver::new(LocalSolverKind::PointEdge, false, Some(shape));
            edges.set_point(point);
            ts.solve_trees(&query, trees.curves, &mut edges)?;
        }
        let mut faces = LocalSolver::new(LocalSolverKind::PointFace, false, Some(shape));
        faces.set_point(point);
        ts.solve_trees(&query, trees.surfaces, &mut faces)?;
        Ok(ts.base.solutions)
    }

    /// Solves a curve against a B-rep (`BrepCurveSolve`). Wireframe intersection
    /// skips faces; signed directed distance additionally tests the start point.
    ///
    /// # Errors
    /// Invalid operation or null B-rep, cache, bounding-box or local-solve errors.
    pub fn brep_curve_solve(
        kernel: &Kernel,
        objects: &dyn TopologyObjects<O>,
        brep: Option<O>,
        curve: CurveRef<'_, O>,
        interval: &Extent1d,
        mut options: TopologySolveOptions<'_>,
    ) -> Result<SolutionArray<O>> {
        const OP: &str = "intersect::topology_solver::p2::TopologySolver::brep_curve_solve";
        if !valid_curve_operation(options.operation, true) {
            return Err(SolidError::invalid_input(OP, "unsupported operation"));
        }
        let wires_only = options.operation == SolverOperation::IntersectWireframe;
        if wires_only {
            options.operation = SolverOperation::Intersect;
        }
        let mut ts = new_topology_solver(kernel, objects, options);
        let trees = objects.brep_trees(brep.ok_or(SolidError::null_pointer(OP))?)?;
        let query = curve_tree(kernel, curve, interval)?;
        if options.operation != SolverOperation::Normalize {
            let mut vertices = LocalSolver::new(LocalSolverKind::CurveVertex, false, None);
            vertices.set_curve(Some(curve), *interval);
            ts.solve_trees(&query, trees.vertices, &mut vertices)?;
            let mut edges = LocalSolver::new(LocalSolverKind::CurveEdge, false, None);
            edges.set_curve(Some(curve), *interval);
            ts.solve_trees(&query, trees.curves, &mut edges)?;
        }
        if !wires_only {
            let mut faces = LocalSolver::new(LocalSolverKind::CurveFace, false, None);
            faces.set_curve(Some(curve), *interval);
            ts.solve_trees(&query, trees.surfaces, &mut faces)?;
            if options.operation == SolverOperation::Signed3dDirectedMinimize {
                let parameter = interval.min(); // upstream currently tests only the start
                let point = curve.curve.evaluate_point(kernel, parameter)?;
                let query = point_tree(point);
                let mut faces = LocalSolver::new(LocalSolverKind::PointFace, false, None);
                faces.set_point(point);
                let mut scoped = TopologySolver {
                    base: ts.take_base(),
                    kernel,
                    objects,
                    local_solver: None,
                };
                let result = scoped.solve_trees(&query, trees.surfaces, &mut faces);
                ts.restore_base(scoped.base, trees.surfaces);
                result?;
                rewrite_endpoint_solutions(&mut ts.base.solutions, curve.handle, parameter);
            }
        }
        Ok(ts.base.solutions)
    }

    /// Solves a curve against `shape` (`ShapeCurveSolve`).
    ///
    /// # Errors
    /// Invalid operation, handle mapping, curve bounding box or local solve.
    pub fn shape_curve_solve(
        kernel: &Kernel,
        objects: &dyn TopologyObjects<O>,
        shape: &Shape,
        curve: CurveRef<'_, O>,
        interval: &Extent1d,
        options: TopologySolveOptions<'_>,
    ) -> Result<SolutionArray<O>> {
        const OP: &str = "intersect::topology_solver::p2::TopologySolver::shape_curve_solve";
        if !valid_curve_operation(options.operation, false) {
            return Err(SolidError::invalid_input(OP, "unsupported operation"));
        }
        let mut ts = new_topology_solver(kernel, objects, options);
        let mapped = ShapeTreeViews::new(objects, shape)?;
        let trees = mapped.views();
        let query = curve_tree(kernel, curve, interval)?;
        if options.operation != SolverOperation::Normalize {
            let mut vertices = LocalSolver::new(LocalSolverKind::CurveVertex, false, Some(shape));
            vertices.set_curve(Some(curve), *interval);
            ts.solve_trees(&query, trees.vertices, &mut vertices)?;
            let mut edges = LocalSolver::new(LocalSolverKind::CurveEdge, false, Some(shape));
            edges.set_curve(Some(curve), *interval);
            ts.solve_trees(&query, trees.curves, &mut edges)?;
        }
        let mut faces = LocalSolver::new(LocalSolverKind::CurveFace, false, Some(shape));
        faces.set_curve(Some(curve), *interval);
        ts.solve_trees(&query, trees.surfaces, &mut faces)?;
        if options.operation == SolverOperation::Signed3dDirectedMinimize {
            let parameter = interval.min();
            let point = curve.curve.evaluate_point(kernel, parameter)?;
            let query = point_tree(point);
            let mut faces = LocalSolver::new(LocalSolverKind::PointFace, false, Some(shape));
            faces.set_point(point);
            let mut scoped = TopologySolver {
                base: ts.take_base(),
                kernel,
                objects,
                local_solver: None,
            };
            let result = scoped.solve_trees(&query, trees.surfaces, &mut faces);
            ts.restore_base(scoped.base, trees.surfaces);
            result?;
            rewrite_endpoint_solutions(&mut ts.base.solutions, curve.handle, parameter);
        }
        Ok(ts.base.solutions)
    }

    /// Performs the ordered vertex/edge/face comparisons of `sm_SolveTwoTrees`.
    /// Normalize skips every comparison except face/face.
    ///
    /// # Errors
    /// Tree traversal or local-solver errors.
    pub fn solve_two_tree_sets(
        &mut self,
        first: TopologyTrees<'t, O>,
        second: TopologyTrees<'t, O>,
    ) -> Result<()> {
        if self.base.operation != SolverOperation::Normalize {
            for (kind, swap, tree1, tree2) in [
                (
                    LocalSolverKind::VertexVertex,
                    false,
                    first.vertices,
                    second.vertices,
                ),
                (
                    LocalSolverKind::VertexEdge,
                    false,
                    first.vertices,
                    second.curves,
                ),
                (
                    LocalSolverKind::VertexEdge,
                    true,
                    first.curves,
                    second.vertices,
                ),
                (
                    LocalSolverKind::EdgeEdge,
                    false,
                    first.curves,
                    second.curves,
                ),
                (
                    LocalSolverKind::VertexFace,
                    false,
                    first.vertices,
                    second.surfaces,
                ),
                (
                    LocalSolverKind::VertexFace,
                    true,
                    first.surfaces,
                    second.vertices,
                ),
                (
                    LocalSolverKind::EdgeFace,
                    false,
                    first.curves,
                    second.surfaces,
                ),
                (
                    LocalSolverKind::EdgeFace,
                    true,
                    first.surfaces,
                    second.curves,
                ),
            ] {
                let mut local = LocalSolver::new(kind, swap, None);
                self.solve_two_trees(tree1, tree2, &mut local)?;
            }
        }
        let mut faces = LocalSolver::new(LocalSolverKind::FaceFace, false, None);
        self.solve_two_trees(first.surfaces, second.surfaces, &mut faces)
    }

    /// Solves two B-reps using their cache trees (`BrepBrepSolve`).
    ///
    /// # Errors
    /// Invalid operation or null B-reps, unavailable caches or local-solve errors.
    pub fn brep_brep_solve(
        kernel: &Kernel,
        objects: &dyn TopologyObjects<O>,
        first: Option<O>,
        second: Option<O>,
        options: TopologySolveOptions<'_>,
    ) -> Result<SolutionArray<O>> {
        const OP: &str = "intersect::topology_solver::p2::TopologySolver::brep_brep_solve";
        if !valid_curve_operation(options.operation, false) {
            return Err(SolidError::invalid_input(OP, "unsupported operation"));
        }
        let mut ts = new_topology_solver(kernel, objects, options);
        let first = objects.brep_trees(first.ok_or(SolidError::null_pointer(OP))?)?;
        let second = objects.brep_trees(second.ok_or(SolidError::null_pointer(OP))?)?;
        ts.solve_two_tree_sets(first, second)?;
        Ok(ts.base.solutions)
    }
}

#[cfg(test)]
mod tests {
    use super::super::p1::tests::{scene, Obj};
    use super::*;
    use crate::math::BIG_DOUBLE;

    #[test]
    fn nodes_without_object_lists_solve_as_empty_in_both_orders() {
        let kernel = Kernel::default();
        let objects = scene();
        let empty = Tree::<Obj>::from_point(Point3::ZERO, 0.0, false);
        let mut populated = Tree::<Obj>::from_point(Point3::ZERO, 0.0, true);
        let item = populated.object_list().unwrap()[0];
        populated.list_mut(item).unwrap().object = Some(Obj::Vertex(0));
        for swap in [false, true] {
            let mut solver = TopologySolver::new(
                &kernel,
                &objects,
                SolverOperation::Intersect,
                SolutionRequested::All,
                1e-6,
                BIG_DOUBLE,
                &[],
            );
            let mut local = LocalSolver::new(LocalSolverKind::Base, false, None);
            let (first, second) = if swap {
                (&populated, &empty)
            } else {
                (&empty, &populated)
            };
            solver.solve_trees(first, second, &mut local).unwrap();
            assert!(solver.base.solutions.is_empty());
        }
    }
    #[test]
    fn edge_vertex_rejects_the_upstream_wrong_class_operand() {
        let k = &Kernel::default();
        let kernel = Kernel::default();
        let objects = scene();
        for swap in [false, true] {
            let mut solver = TopologySolver::new(
                &kernel,
                &objects,
                SolverOperation::Intersect,
                SolutionRequested::All,
                1e-6,
                BIG_DOUBLE,
                &[],
            );
            let mut local = LocalSolver::new(LocalSolverKind::EdgeVertex, swap, None);
            let curve = Obj::Curve(0, Extent1d::from_min_max(0.0, 10.0));
            let vertex = Obj::Vertex(0);
            let (first, second) = if swap {
                (vertex, curve)
            } else {
                (curve, vertex)
            };
            assert!(matches!(
                local.solve_it(k, &mut solver, Some(first), Some(second)),
                Err(SolidError::InvalidInput { .. })
            ));
            assert!(solver.base.solutions.is_empty());
        }
    }
}

//! Public closest-point, boundary, bounding-box and classification queries.
//!
//! Upstream: `source/SM_API/src/SmApiQueries.cpp`, `source/SM_API/inc/SmApiQueries.h`,
//! `source/SMLib/src/SmSolutionArray.cpp` (solution point extraction).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`curve_closest_point`], [`curve_closest_point_all`] | `SmApiCurveClosestPoint`, `SmApiCurveClosestPointAll` |
//! | [`surface_closest_point`] | `SmApiSurfaceClosestPoint` |
//! | [`get_closest_point`], [`brep_closest_point`], [`brep_closest_point_all`] | `SmApiGetClosestPoint`, `SmApiBrepClosestPoint`, `SmApiBrepClosestPointAll` |
//! | [`brep_relationship`], [`brep_distance`] | `SmApiBrepRelationship`, `SmApiBrepDistance` |
//! | [`brep_relationship_solve`], [`brep_relationship_classify`] | the solve and the classification of `SmApiBrepRelationship` |
//! | [`get_edges`], [`get_faces`], [`find_edge`], [`find_faces`] | `SmApiGetEdges`, `SmApiGetFaces`, `SmApiFindEdge`, `SmApiFindFaces` |
//! | [`face_get_boundary_loops`] | `SmApiFaceGetBoundaryLoops` |
//! | [`brep_is_manifold_solid`], [`brep_bounding_box`] | `SmApiBrepIsManifoldSolid`, `SmApiBrepBoundingBox` |
//! | [`brep_material_census`] | `SmApiBrepMaterialCensus` |
//! | [`face_bounding_box`], [`face_internal_point`], [`face_classify_uv`], [`face_closest_point`], [`face_closest_point_in_shape`] | `SmApiFaceBoundingBox`, `SmApiFaceInternalPoint`, `SmApiFaceClassifyUV`, `SmApiFaceClosestPoint` |
//! | [`edge_bounding_box`], [`edge_closest_point`], [`edge_parameter_range`], [`edge_classify_parameter`], [`edge_tangent`] | `SmApiEdgeBoundingBox`, `SmApiEdgeClosestPoint`, `SmApiEdgeParameterRange`, `SmApiEdgeClassifyParameter`, `SmApiEdgeTangent` |
//! | [`vertex_get_point`] | `SmApiVertexGetPoint` |
//! | `count_contained_samples`, `edge_piece_points`, `point_in_solid`, `target_curve_crossings` (private) | `sm_CountContainedSamples`, `sm_PointInSolid`, its `BrepCurveSolve` |
//! | `solution_point` (private) | `SmSolution::GetPoint` |
//!
//! Deviations: typed references/ids replace nullable pointers; results replace
//! output parameters (including append-only arrays), so errors expose no partial
//! arrays. Callers supply the Kernel and topology-solver object adapter/cache (an
//! adapter holding the B-reps mutably calls the split relationship and face
//! closest-point steps, so the solve can make face caches on them);
//! handles retain the caller's identity. Missing topology queries return Unported
//! at the call site. Solution point extraction retains upstream's zero parameter
//! advance for a preceding Surface and ignores ordinary evaluation errors (the
//! output stays undefined); Unported errors still propagate. The containment tests of
//! [`brep_relationship`] (`sm_PointInSolid`, `sm_CountContainedSamples`) classify on a
//! private copy of each target ([`BrepClassifier::of_brep`], keyed by the target's
//! recorded naming scheme and proven owner keys); `sm_CountContainedSamples`'
//! `SmApiFaceInternalPoint` of each sample face runs on a copy of the sample B-rep
//! (borrowed next to the target), and its `BrepCurveSolve` of each sample edge runs
//! through the caller's objects on the edge curves of the sample's curve tree (so in
//! that tree's order, which only the counts read), while the public
//! [`face_internal_point`] works on its B-rep in place. No new recursion or allocation from
//! serialized counts.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::Curve;
use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{BrepStore, FaceKey, ObjectRef, Shape, ShapeMember};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tools::{self, EdgePick};
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::intersect::topology_solver::p1::{CurveRef, TopologyObjects, TopologySolver};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, EFF_ZERO};
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::BrepClassifier;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::surface::surface::p1::zone_tol_3d;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::Surface;
use crate::topology::brep_cache::CacheObject;
use crate::topology::{
    edge, edgeuse::p1 as eu, face::p2 as face, Brep, BrepId, EdgeId, FaceId, VertexId,
};

const OP: &str = "query::api_queries";

/// A closest spatial point and its upstream (unsquared) solution distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoint {
    /// Point on the queried entity.
    pub point: Point3,
    /// Distance supplied by the solver.
    pub distance: f64,
}

/// A closest curve point with its parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveClosestPoint {
    /// Spatial result.
    pub closest: ClosestPoint,
    /// Curve parameter.
    pub parameter: f64,
}

/// One spatial point and all returned UV representations of that point.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceClosestPoint {
    /// Spatial result.
    pub closest: ClosestPoint,
    /// UVs in solver order, including seam representations.
    pub uvs: Vec<Point2>,
}

#[derive(Clone, Copy, PartialEq)]
struct CurveHandle(u8, Option<Extent1d>);
impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        self.1
    }
}

fn curve_solutions(
    k: &Kernel,
    curve: &Curve,
    point: Point3,
    requested: SolutionRequested,
) -> Result<SolutionArray<CurveHandle>> {
    curve.global_point_solve(
        k,
        &curve.natural_interval(),
        SolverOperation::Minimize,
        point,
        crate::geom::tol::ZONE_TOL_3D,
        requested,
        &GlobalPointSolveOptions {
            handle: Some(CurveHandle(0, Some(curve.natural_interval()))),
            analytic_handle: Some(CurveHandle(1, None)),
            ..GlobalPointSolveOptions::default()
        },
    )
}

fn curve_result<O: Copy>(
    k: &Kernel,
    curve: &Curve,
    solution: &Solution<O>,
) -> Result<CurveClosestPoint> {
    let parameter = solution.start.param(0)?;
    let point = curve.evaluate_point(k, parameter)?;
    Ok(CurveClosestPoint {
        closest: ClosestPoint {
            point,
            distance: solution.start.solution_value,
        },
        parameter,
    })
}

/// Closest point on the curve's natural interval (`SmApiCurveClosestPoint`).
/// Solver and evaluation errors propagate; an empty solution set returns `SM_ERR`.
pub fn curve_closest_point(k: &Kernel, curve: &Curve, point: Point3) -> Result<CurveClosestPoint> {
    let solutions = curve_solutions(k, curve, point, SolutionRequested::Single)?;
    curve_result(
        k,
        curve,
        solutions.as_slice().first().ok_or(SolidError::sm_err(OP))?,
    )
}

/// All closest curve solutions, retaining solver order (`SmApiCurveClosestPointAll`).
/// An empty solution set or an evaluation failure returns an error.
pub fn curve_closest_point_all(
    k: &Kernel,
    curve: &Curve,
    point: Point3,
) -> Result<Vec<CurveClosestPoint>> {
    let solutions = curve_solutions(k, curve, point, SolutionRequested::All)?;
    if solutions.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    solutions
        .as_slice()
        .iter()
        .map(|s| curve_result(k, curve, s))
        .collect()
}

/// Closest surface point and UV representations (`SmApiSurfaceClosestPoint`).
/// Strictly smaller distance replaces the selected solution; failed secondary
/// evaluations are ignored. UVs must evaluate within the surface zone tolerance.
pub fn surface_closest_point(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
) -> Result<SurfaceClosestPoint> {
    let drop = surface.drop_point::<NoObject>(
        k,
        point,
        &surface.natural_uv_domain_with_nurb(k)?,
        &DropPointOptions {
            want_solutions: true,
            ..DropPointOptions::default()
        },
    )?;
    if !drop.success || drop.solutions.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    let tolerance = zone_tol_3d(k, surface)?;
    let mut best = drop
        .solutions
        .as_slice()
        .first()
        .ok_or(SolidError::sm_err(OP))?;
    for solution in drop.solutions.as_slice().iter().skip(1) {
        if solution.start.solution_value < best.start.solution_value {
            best = solution;
        }
    }
    let selected = Point2::new(best.start.param(0)?, best.start.param(1)?);
    let point = surface.evaluate_point(k, selected)?;
    let mut uvs = Vec::new();
    for solution in drop.solutions.as_slice() {
        let uv = Point2::new(solution.start.param(0)?, solution.start.param(1)?);
        if let Some(candidate) = ignored(surface.evaluate_point(k, uv))? {
            if candidate.closer_than(tolerance, point) {
                uvs.push(uv);
            }
        }
    }
    if uvs.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    Ok(SurfaceClosestPoint {
        closest: ClosestPoint {
            point,
            distance: best.start.solution_value,
        },
        uvs,
    })
}

fn solve_options(_k: &Kernel, requested: SolutionRequested) -> TopologySolveOptions<'static> {
    TopologySolveOptions {
        operation: SolverOperation::Minimize,
        requested,
        tolerance_3d: crate::geom::tol::ZONE_TOL_3D,
        best_answer_so_far_sq: 1.0e20 * 1.0e20,
        opt_vectors: &[],
    }
}

// SmSolution::GetPoint using the same caller-owned geometry adapter as the solver.
fn solution_point<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    solution: &Solution<O>,
    index: usize,
) -> Result<Point3> {
    let mut parameter = 0;
    for (i, object) in solution
        .objects
        .iter()
        .take(solution.num_objects)
        .enumerate()
    {
        let object = object.ok_or(SolidError::null_pointer(OP))?;
        let kind = object.kind();
        if i == index {
            if kind == ObjectKind::Vertex {
                return objects.vertex_point(object);
            }
            if matches!(kind, ObjectKind::Curve | ObjectKind::Edge) {
                let curve = if kind == ObjectKind::Edge {
                    objects
                        .edge_curve(object)?
                        .ok_or(SolidError::null_pointer(OP))?
                } else {
                    object
                };
                return Ok(ignored(
                    objects
                        .curve(curve)?
                        .evaluate_point(k, solution.start.param(parameter)?),
                )?
                .unwrap_or(Point3::UNDEF));
            }
            if matches!(kind, ObjectKind::Surface | ObjectKind::Face) {
                let surface = if kind == ObjectKind::Face {
                    objects.face_surface(object)?
                } else {
                    object
                };
                return Ok(ignored(objects.surface(surface)?.evaluate_point(
                    k,
                    Point2::new(
                        solution.start.param(parameter)?,
                        solution.start.param(parameter + 1)?,
                    ),
                ))?
                .unwrap_or(Point3::UNDEF));
            }
            return Err(SolidError::sm_err(OP));
        }
        parameter += match kind {
            ObjectKind::Curve | ObjectKind::Edge => 1,
            ObjectKind::Face => 2,
            _ => 0,
        };
    }
    Err(SolidError::sm_err(OP))
}

/// Closest point on a B-rep (`SmApiGetClosestPoint`); the adapter supplies its cache.
/// Solver errors, point-extraction failures and empty solution sets propagate.
pub fn get_closest_point<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    brep: O,
    point: Point3,
) -> Result<ClosestPoint> {
    brep_closest_point(k, objects, brep, point)
}

/// Single closest B-rep point (`SmApiBrepClosestPoint`).
/// Errors are those of `BrepPointSolve` and solution point extraction.
pub fn brep_closest_point<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    brep: O,
    point: Point3,
) -> Result<ClosestPoint> {
    let solutions = TopologySolver::brep_point_solve(
        k,
        objects,
        Some(brep),
        point,
        solve_options(k, SolutionRequested::Single),
    )?;
    let solution = solutions.as_slice().first().ok_or(SolidError::sm_err(OP))?;
    Ok(ClosestPoint {
        point: solution_point(k, objects, solution, 0)?,
        distance: solution.start.solution_value,
    })
}

/// All closest B-rep points in solver order (`SmApiBrepClosestPointAll`).
/// An empty solution set is `SM_ERR`; extraction failures propagate.
pub fn brep_closest_point_all<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    brep: O,
    point: Point3,
) -> Result<Vec<ClosestPoint>> {
    let solutions = TopologySolver::brep_point_solve(
        k,
        objects,
        Some(brep),
        point,
        solve_options(k, SolutionRequested::All),
    )?;
    if solutions.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    solutions
        .as_slice()
        .iter()
        .map(|s| {
            Ok(ClosestPoint {
                point: solution_point(k, objects, s, 0)?,
                distance: s.start.solution_value,
            })
        })
        .collect()
}

/// B-rep arena and its handle in the caller's topology-solver adapter.
#[derive(Clone, Copy)]
pub struct BrepRef<'a, O> {
    /// Exact topology.
    pub brep: &'a Brep,
    /// Adapter handle.
    pub handle: O,
}

/// Relationship code and boundary distance (`SmApiBrepRelationship`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrepRelationship {
    /// 0 separate, 1 touching, 2 interpenetrating, 3 A contains B, 4 B contains A.
    pub relationship: i32,
    /// Boundary gap, or zero when within the sum of the B-rep tolerances.
    pub distance: f64,
}

/// A B-rep points are classified against (`pTarget`).
struct Target<'b>(BrepClassifier<'static, 'b>);

impl<'b> Target<'b> {
    fn new(brep: &'b Brep) -> Self {
        Self(BrepClassifier::of_brep(brep))
    }

    /// `pTarget->Point3DClassify(point, tolerance, flag, sPC)`: the point class and
    /// whether its object is a region that is not void (`pReg != NULL &&
    /// !pReg->IsVoid()`).
    fn classify(
        &mut self,
        k: &Kernel,
        point: Point3,
        tolerance: f64,
        flag: bool,
    ) -> Result<(PointClassType, bool)> {
        let pc = self.0.point_3d_classify(k, point, tolerance, flag)?;
        let material = match pc.object() {
            Some(EntityRef::Region(r)) => self.0.brep().region(r)?.is_void() == 0,
            _ => false,
        };
        Ok((pc.point_class(), material))
    }
}

/// `sm_PointInSolid`: whether `point` classifies to a material region of `target`
/// (a failing classification is `false`).
fn point_in_solid(
    k: &Kernel,
    target: &mut Target<'_>,
    point: Point3,
    tolerance: f64,
) -> Result<bool> {
    Ok(ignored(target.classify(k, point, tolerance, false))?
        .is_some_and(|(class, material)| class == PointClassType::Region && material))
}

fn brep_vertices(brep: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p1::vertices(brep, None)
}

/// `SmTopologySolver::BrepCurveSolve(pTarget, curve, interval, SM_SO_INTERSECT,
/// SM_SR_ALL, tolerance, SM_BIG_DOUBLE, NULL)` through the caller's objects: every
/// solution's start parameter, and its end parameter for a range of values, in solution
/// order.
fn target_curve_crossings<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    target: O,
    curve: CurveRef<'_, O>,
    interval: &Extent1d,
    tolerance: f64,
) -> Result<Vec<f64>> {
    let solutions = TopologySolver::brep_curve_solve(
        k,
        objects,
        Some(target),
        curve,
        interval,
        TopologySolveOptions {
            operation: SolverOperation::Intersect,
            requested: SolutionRequested::All,
            tolerance_3d: tolerance,
            best_answer_so_far_sq: crate::math::BIG_DOUBLE,
            opt_vectors: &[],
        },
    )?;
    let mut params = Vec::new();
    for solution in solutions.as_slice() {
        params.push(solution.start.param(0)?);
        if solution.solution_type == SolutionType::RangeOfValues {
            params.push(solution.end.param(0)?);
        }
    }
    Ok(params)
}

/// One point on each piece of each edge of `sample` between its crossings of
/// `target`'s boundary (`sm_CountContainedSamples`' edge loop over `GetEdges`). The edges
/// come from the sample's curve tree (one object per edge curve; edges without a curve
/// are skipped, as upstream), so their order is the tree's: only the inside/outside
/// counts of the points are used.
fn edge_piece_points<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    sample: O,
    target: O,
    tolerance: f64,
) -> Result<Vec<Point3>> {
    let mut curve_objects: Vec<O> = Vec::new();
    {
        let trees = objects.brep_trees(sample)?;
        for item in trees.curves.object_list()? {
            if let Some(o) = trees.curves.list(item)?.object {
                if !curve_objects.contains(&o) {
                    curve_objects.push(o);
                }
            }
        }
    }
    let mut points = Vec::new();
    for handle in curve_objects {
        let Some(edge) = objects.curve_edge(handle)? else {
            continue;
        };
        let interval = objects.edge_interval(edge)?;
        let curve = objects.curve(handle)?;
        let mut params = vec![interval.min()];
        if let Some(crossings) = ignored(target_curve_crossings(
            k,
            objects,
            target,
            CurveRef { curve, handle },
            &interval,
            tolerance,
        ))? {
            params.extend(crossings);
        }
        params.push(interval.max());
        params.sort_by(f64::total_cmp);
        for pair in params.windows(2) {
            let &[a, b] = pair else {
                continue;
            };
            if b - a <= EFF_ZERO * (1.0 + interval.length()) {
                continue;
            }
            if let Some(mid) = ignored(curve.evaluate_point(k, 0.5 * (a + b)))? {
                points.push(mid);
            }
        }
    }
    Ok(points)
}

fn count_contained_samples<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    sample: &Brep,
    handles: [O; 2],
    target: &mut Target<'_>,
    tolerance: f64,
) -> Result<(usize, usize)> {
    let [sample_handle, target_handle] = handles;
    let mut points = Vec::new();
    for vertex in brep_vertices(sample)? {
        points.push(sample.vertex(vertex)?.point);
    }
    for face in brep_faces(sample)? {
        // SmApiFaceInternalPoint on a sample borrowed next to the classified target: on a
        // copy of the sample (module Deviations)
        if let Some(point) = ignored(face::calculate_an_internal_point_snapshot(k, sample, face))? {
            points.push(point);
        }
    }

    // One point on each piece of each edge between its crossings of the target's
    // boundary: each piece is wholly inside or outside the target.
    points.extend(edge_piece_points(
        k,
        objects,
        sample_handle,
        target_handle,
        tolerance,
    )?);

    let (mut inside, mut outside) = (0, 0);
    for point in points {
        if let Some((class, material)) = ignored(target.classify(k, point, tolerance, true))? {
            match class {
                PointClassType::Region => {
                    if material {
                        inside += 1;
                    } else {
                        outside += 1;
                    }
                }
                PointClassType::Unknown => outside += 1,
                _ => {}
            }
        }
    }
    Ok((inside, outside))
}

/// Boundary distance followed by containment/touching classification.
/// Missing point-classification/list dependencies return `Unported` rather than
/// being swallowed by upstream's ignored ordinary classification statuses.
pub fn brep_relationship<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    a: BrepRef<'_, O>,
    b: BrepRef<'_, O>,
) -> Result<BrepRelationship> {
    let solution = brep_relationship_solve(k, objects, a.handle, b.handle)?;
    brep_relationship_classify(k, objects, &solution, [a.handle, b.handle], a.brep, b.brep)
}

/// The boundary distance solve of [`brep_relationship`] (`BrepBrepSolve`, single
/// solution) on the B-reps behind `a` and `b`. A caller whose `objects` hold the
/// B-reps mutably (face caches made on demand) runs it, then
/// [`brep_relationship_classify`] on the B-reps.
///
/// # Errors
///
/// Those of the solver; `SM_ERR` without a solution.
pub fn brep_relationship_solve<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    a: O,
    b: O,
) -> Result<Solution<O>> {
    let solutions = TopologySolver::brep_brep_solve(
        k,
        objects,
        Some(a),
        Some(b),
        solve_options(k, SolutionRequested::Single),
    )?;
    solutions
        .as_slice()
        .first()
        .copied()
        .ok_or(SolidError::sm_err(OP))
}

/// The containment and touching classification of [`brep_relationship`] after its
/// solve ([`brep_relationship_solve`]), `brep_a` and `brep_b` being the B-reps the
/// solution's objects name and `handles` their handles in `objects` (the edge-piece
/// samples solve each sample edge against the other B-rep through them).
///
/// # Errors
///
/// Those of the solution points and the point classifications.
pub fn brep_relationship_classify<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    solution: &Solution<O>,
    handles: [O; 2],
    brep_a: &Brep,
    brep_b: &Brep,
) -> Result<BrepRelationship> {
    let [ha, hb] = handles;
    let a = BrepRef {
        brep: brep_a,
        handle: (),
    };
    let b = BrepRef {
        brep: brep_b,
        handle: (),
    };
    let distance = solution.start.solution_value;
    let tolerance =
        crate::geom::tol::context_zone_tol_3d(k, None, crate::geom::tol::SmallTopoType::NormalSize);
    let (mut target_a, mut target_b) = (Target::new(a.brep), Target::new(b.brep));
    if distance > a.brep.tolerance() + b.brep.tolerance() {
        let pa = ignored(solution_point(k, objects, solution, 0))?;
        let pb = if pa.is_some() {
            ignored(solution_point(k, objects, solution, 1))?
        } else {
            None
        };
        let relationship = if let (Some(pa), Some(pb)) = (pa, pb) {
            let bin_a = point_in_solid(k, &mut target_a, pb, tolerance)?;
            let ain_b = point_in_solid(k, &mut target_b, pa, tolerance)?;
            if bin_a {
                3
            } else if ain_b {
                4
            } else {
                0
            }
        } else {
            let (bin, bout) =
                count_contained_samples(k, objects, b.brep, [hb, ha], &mut target_a, tolerance)?;
            let (ain, aout) =
                count_contained_samples(k, objects, a.brep, [ha, hb], &mut target_b, tolerance)?;
            if bin > 0 && bout == 0 {
                3
            } else if ain > 0 && aout == 0 {
                4
            } else {
                0
            }
        };
        Ok(BrepRelationship {
            relationship,
            distance,
        })
    } else {
        let (bin, _) =
            count_contained_samples(k, objects, b.brep, [hb, ha], &mut target_a, tolerance)?;
        let (ain, _) =
            count_contained_samples(k, objects, a.brep, [ha, hb], &mut target_b, tolerance)?;
        Ok(BrepRelationship {
            relationship: if bin > 0 || ain > 0 { 2 } else { 1 },
            distance: 0.0,
        })
    }
}

/// Boundary distance through the same relationship query (`SmApiBrepDistance`).
/// Containment/classification errors propagate, even though only distance is returned.
pub fn brep_distance<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    a: BrepRef<'_, O>,
    b: BrepRef<'_, O>,
) -> Result<f64> {
    Ok(brep_relationship(k, objects, a, b)?.distance)
}

fn brep_edges(brep: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(brep, None)
}
fn brep_faces(brep: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(brep, None)
}

/// Exact B-rep edge list (`SmApiGetEdges`). Empty lists return `SM_ERR`.
/// Missing list traversal is `Unported`.
pub fn get_edges(brep: &Brep) -> Result<Vec<EdgeId>> {
    let edges = brep_edges(brep)?;
    if edges.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    Ok(edges)
}

/// Exact B-rep face list (`SmApiGetFaces`). Empty lists return `SM_ERR`.
/// Missing list traversal is `Unported`.
pub fn get_faces(brep: &Brep) -> Result<Vec<FaceId>> {
    let faces = brep_faces(brep)?;
    if faces.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    Ok(faces)
}

/// Flattened edges and orientations in upward face-use loop order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryLoops {
    /// Number of edge uses in each loop (including zero for a vertex loop).
    pub edge_counts: Vec<usize>,
    /// Edge of each edge use, retaining repeated edges.
    pub edges: Vec<EdgeId>,
    /// Same or opposite orientation of each edge use.
    pub orientations: Vec<OrientType>,
}

/// Upward boundary loops (`SmApiFaceGetBoundaryLoops`).
/// Missing face use/loops and invalid edge/orientation return `SM_ERR`.
pub fn face_get_boundary_loops(brep: &Brep, id: FaceId) -> Result<BoundaryLoops> {
    let mut result = BoundaryLoops {
        edge_counts: Vec::new(),
        edges: Vec::new(),
        orientations: Vec::new(),
    };
    let loops = if let Some(fu) = face::upward_faceuse(brep, id)? {
        eu::faceuse_loopuses(brep, fu, false, None)?
    } else {
        Vec::new()
    };
    if loops.is_empty() {
        return Err(SolidError::sm_err(OP));
    }
    for lu in loops {
        let edges = eu::loopuse_edgeuses(brep, lu)?;
        result.edge_counts.push(edges.len());
        for edgeuse in edges {
            let edgeuse = brep.edgeuse(edgeuse)?;
            let edge = edgeuse.edge.ok_or(SolidError::sm_err(OP))?;
            if !matches!(edgeuse.orientation, OrientType::Same | OrientType::Opposite) {
                return Err(SolidError::sm_err(OP));
            }
            result.edges.push(edge);
            result.orientations.push(edgeuse.orientation);
        }
    }
    Ok(result)
}

/// Finds an edge index and parameter (`SmApiFindEdge`); missing match is `SM_ERR`.
pub fn find_edge(k: &Kernel, brep: &Brep, point: Point3) -> Result<EdgePick> {
    tools::find_edge(k, brep, point)?.ok_or(SolidError::sm_err(OP))
}

/// Finds a face (`SmApiFindFaces`); missing match is `SM_ERR`.
pub fn find_faces(
    k: &Kernel,
    brep: &mut Brep,
    id: BrepId,
    point: Point3,
    keys: &dyn Fn(CacheObject) -> ObjectRef,
    owners: &dyn Fn(ObjectRef) -> Option<ObjectRef>,
) -> Result<FaceId> {
    tools::find_face(k, brep, id, point, keys, owners)?.ok_or(SolidError::sm_err(OP))
}

/// Manifold solid predicate (`SmApiBrepIsManifoldSolid`: `SmBrep::IsManifoldSolid`).
pub fn brep_is_manifold_solid(brep: &Brep) -> Result<bool> {
    crate::topology::brep::p2::is_manifold_solid(brep)
}

/// Counts a B-rep's material regions and enclosed void cavities
/// (`SmApiBrepMaterialCensus`), returned as `(solids, voids)`.
///
/// One walk of `SmBrep::GetRegions`: a region whose `IsVoid()` is nonzero (`TRUE` or
/// `UNSURE`) counts as a void unless it is the infinite region; any other region counts
/// as a solid. A null B-rep (`SM_ERR_INVALID_INPUT`) has no typed equivalent.
///
/// # Errors
///
/// A dangling region on the region list.
pub fn brep_material_census(brep: &Brep) -> Result<(usize, usize)> {
    let mut solids = 0usize;
    let mut voids = 0usize;
    for id in crate::topology::brep::p1::regions(brep, None)? {
        let region = brep.region(id)?;
        if region.is_void() != 0 {
            if !region.is_infinite_region(brep, id) {
                voids += 1;
            }
        } else {
            solids += 1;
        }
    }
    Ok((solids, voids))
}

/// Loose or tight B-rep box (`SmApiBrepBoundingBox`); missing queries are `Unported`.
pub fn brep_bounding_box(k: &Kernel, brep: &Brep, tight: bool) -> Result<Extent3d> {
    if tight {
        crate::query::brep::calculate_tight_bounding_box(k, brep)
    } else {
        qbrep::calculate_bounding_box(k, brep, BoundingBoxOptions::default())
    }
}

/// Loose or tight face box (`SmApiFaceBoundingBox`); missing queries are `Unported`.
pub fn face_bounding_box(k: &Kernel, brep: &Brep, face: FaceId, tight: bool) -> Result<Extent3d> {
    if tight {
        brep.face(face)?;
        return crate::query::face::p3::calculate_tight_bounding_box(k, brep, face);
    }
    crate::query::face::p1::calculate_bounding_box(k, brep, face, false)
}

/// Interior face point (`SmApiFaceInternalPoint`). The face's line classifications run
/// on `brep` in place, so the lazy edits upstream's `const` query makes (UV trim
/// curves, gap caches, tolerance growth) stay on it. Errors from the face query
/// propagate, including explicit deferrals of its unfinished dependencies.
pub fn face_internal_point(k: &Kernel, brep: &mut Brep, face: FaceId) -> Result<Point3> {
    face::calculate_an_internal_point(k, brep, face)
}

/// Trim-aware UV classification (`SmApiFaceClassifyUV`), preserving the face
/// classifier's vertex tests, loop containment and default classification tolerance.
pub fn face_classify_uv(
    k: &Kernel,
    brep: &mut Brep,
    face: FaceId,
    uv: Point2,
) -> Result<PointClassType> {
    let source_tolerance = brep.face(face)?.tolerance;
    let tolerance = crate::geom::tol::zone_tol3d(k, crate::geom::tol::USE_DEFAULT);
    let mut classification = PointClassification::new(tolerance);
    crate::query::face::p2::point_classify(
        k,
        brep,
        face,
        uv,
        &mut classification,
        crate::query::face::p2::PointClassifyOptions {
            src_zone_tol_3d: source_tolerance,
            use_uv_classification: true,
            ..crate::query::face::p2::PointClassifyOptions::default()
        },
    )?;
    Ok(classification.point_class())
}

/// Trim-aware closest point on one face (`SmApiFaceClosestPoint`), the face's B-rep
/// held in `breps`. Shape construction and solver errors propagate.
pub fn face_closest_point<O: SolutionObject>(
    k: &mut Kernel,
    breps: &dyn BrepStore,
    objects: &dyn TopologyObjects<O>,
    face: FaceKey,
    point: Point3,
) -> Result<ClosestPoint> {
    let shape = Shape::new(k, breps, &[], &[face], &[], &[])?;
    face_closest_point_in_shape(k, &shape, objects, face, point)
}

/// [`face_closest_point`] on the face's shape already made (`SmShape` of the face):
/// the solve may then read the face's B-rep through `objects` mutably (face caches
/// and UV trim curves made on demand).
///
/// # Errors
///
/// Those of the solver and of the solution point.
pub fn face_closest_point_in_shape<O: SolutionObject>(
    k: &Kernel,
    shape: &Shape,
    objects: &dyn TopologyObjects<O>,
    face: FaceKey,
    point: Point3,
) -> Result<ClosestPoint> {
    let mut options = solve_options(k, SolutionRequested::Single);
    options.tolerance_3d =
        objects.face_tolerance(objects.shape_member_object(ShapeMember::Face(face))?)?;
    let solutions = TopologySolver::shape_point_solve(k, objects, shape, point, options)?;
    let solution = solutions.as_slice().first().ok_or(SolidError::sm_err(OP))?;
    Ok(ClosestPoint {
        point: solution_point(k, objects, solution, 0)?,
        distance: solution.start.solution_value,
    })
}

/// Axis-aligned edge box (`SmApiEdgeBoundingBox`), preserving the distinction
/// between the direct tight-box call and loose-box calculation.
pub fn edge_bounding_box(k: &Kernel, brep: &Brep, edge: EdgeId, tight: bool) -> Result<Extent3d> {
    if tight {
        return Ok(
            crate::query::edge::calculate_tight_bounding_box(k, brep, edge)?.unwrap_or_default(),
        );
    }
    crate::query::edge::calculate_bounding_box(
        k,
        brep,
        edge,
        BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        },
        false,
    )?
    .normal
    .ok_or(SolidError::sm_err(OP))
}

/// Closest point within an edge interval (`SmApiEdgeClosestPoint`).
/// A failed drop or absent curve returns `SM_ERR`; evaluation errors propagate.
pub fn edge_closest_point(
    k: &Kernel,
    brep: &Brep,
    id: EdgeId,
    point: Point3,
) -> Result<CurveClosestPoint> {
    let result =
        crate::query::edge::closest_point(k, brep, id, point)?.ok_or(SolidError::sm_err(OP))?;
    let curve = brep
        .edge(id)?
        .curve
        .as_ref()
        .ok_or(SolidError::sm_err(OP))?;
    Ok(CurveClosestPoint {
        closest: ClosestPoint {
            point: curve.evaluate_point(k, result.parameter)?,
            distance: result.distance,
        },
        parameter: result.parameter,
    })
}

/// Parameter interval (`SmApiEdgeParameterRange`); dangling ids are errors.
pub fn edge_parameter_range(brep: &Brep, id: EdgeId) -> Result<Extent1d> {
    Ok(brep.edge(id)?.interval)
}

/// Classifies a parameter (`SmApiEdgeClassifyParameter`). Vertices are tested
/// first with strict distance comparisons, including parameters outside the edge
/// interval. An ordinary evaluation failure falls through to interval containment.
pub fn edge_classify_parameter(
    k: &Kernel,
    brep: &Brep,
    id: EdgeId,
    parameter: f64,
) -> Result<PointClassType> {
    let e = brep.edge(id)?;
    let curve = e.curve.as_ref().ok_or(SolidError::sm_err(OP))?;
    if let Some(point) = ignored(curve.evaluate_point(k, parameter))? {
        for v in [edge::start_vertex(brep, id)?, edge::end_vertex(brep, id)?]
            .into_iter()
            .flatten()
        {
            let tolerance = crate::topology::vertex::tolerance(brep, v)?;
            if point.distance(brep.vertex(v)?.point) < tolerance {
                return Ok(PointClassType::Vertex);
            }
        }
    }
    Ok(if e.interval.contains_value(parameter, 0.0) {
        PointClassType::Edge
    } else {
        PointClassType::Unknown
    })
}

/// Unit tangent (`SmApiEdgeTangent`), evaluating from the left as upstream.
/// A missing curve, evaluation failure or zero derivative returns an error.
pub fn edge_tangent(k: &Kernel, brep: &Brep, id: EdgeId, parameter: f64) -> Result<Vec3> {
    let curve = brep
        .edge(id)?
        .curve
        .as_ref()
        .ok_or(SolidError::sm_err(OP))?;
    let values = curve.evaluate(k, parameter, 1, true, true)?;
    values
        .get(1)
        .copied()
        .ok_or(SolidError::sm_err(OP))?
        .unitize()
}

/// Vertex position (`SmApiVertexGetPoint`); dangling ids are errors.
pub fn vertex_get_point(brep: &Brep, id: VertexId) -> Result<Point3> {
    Ok(brep.vertex(id)?.point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent2d::Extent2d;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::CPoint;
    use crate::nurbs::srf_basic::Surface as NlSurface;

    #[test]
    fn material_census_of_a_box_and_an_empty_brep() {
        // the repro of issue 1436: one material region, the infinite region is no void
        let mut k = Kernel::default();
        let b = crate::primitive::api_primitives::create_box(
            &mut k,
            Vec3::new(1.0, -2.0, 3.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        assert_eq!(brep_material_census(&b).unwrap(), (1, 0));
        assert_eq!(brep_material_census(&Brep::new()).unwrap(), (0, 0));
    }

    #[derive(Clone, Copy, PartialEq)]
    struct Object(ObjectKind);
    impl SolutionObject for Object {
        fn kind(&self) -> ObjectKind {
            self.0
        }
    }

    struct Geometry {
        curve: Curve,
        surface: Surface,
    }
    impl Geometry {
        fn new() -> Self {
            let knots = vec![0., 0., 1., 1.];
            Self {
                curve: Curve::from(BSplineCurve::from_nurb(
                    NlCurve::new(
                        vec![CPoint::new(0., 0., 0., 1.), CPoint::new(10., 0., 0., 1.)],
                        1,
                        knots.clone(),
                    )
                    .unwrap(),
                    3,
                    false,
                )),
                surface: Surface::from(BSplineSurface::from_nurb(
                    NlSurface::new(
                        vec![
                            vec![CPoint::new(0., 0., 0., 1.), CPoint::new(0., 1., 0., 1.)],
                            vec![CPoint::new(1., 0., 0., 1.), CPoint::new(1., 1., 0., 1.)],
                        ],
                        1,
                        1,
                        knots.clone(),
                        knots,
                    )
                    .unwrap(),
                    false,
                )),
            }
        }
    }
    impl TopologyObjects<Object> for Geometry {
        fn curve(&self, _: Object) -> Result<&Curve> {
            Ok(&self.curve)
        }
        fn analytic_curve_handle(&self, _: Object) -> Option<Object> {
            None
        }
        fn vertex_point(&self, _: Object) -> Result<Point3> {
            Ok(Point3::ZERO)
        }
        fn vertex_tolerance(&self, _: Object) -> Result<f64> {
            Ok(1e-5)
        }
        fn curve_edge(&self, _: Object) -> Result<Option<Object>> {
            Ok(None)
        }
        fn edge_curve(&self, _: Object) -> Result<Option<Object>> {
            Ok(Some(Object(ObjectKind::Curve)))
        }
        fn edge_interval(&self, _: Object) -> Result<Extent1d> {
            Ok(self.curve.natural_interval())
        }
        fn edge_tolerance(&self, _: Object) -> Result<f64> {
            Ok(1e-5)
        }
        fn surface_face(&self, _: Object) -> Result<Option<Object>> {
            Ok(None)
        }
        fn face_surface(&self, _: Object) -> Result<Object> {
            Ok(Object(ObjectKind::Surface))
        }
        fn surface(&self, _: Object) -> Result<&Surface> {
            Ok(&self.surface)
        }
        fn face_uv_domain(&self, _: Object) -> Result<Extent2d> {
            Ok(self.surface.natural_uv_domain_if_built())
        }
        fn face_tolerance(&self, _: Object) -> Result<f64> {
            Ok(1e-5)
        }
        fn surface_owner(&self, _: Object) -> Result<Option<Object>> {
            Ok(None)
        }
        fn shape_member(&self, _: Object) -> ShapeMember {
            ShapeMember::Other
        }
    }

    #[test]
    fn solution_point_preserves_upstream_parameter_advances() {
        let k = &Kernel::default();
        let objects = Geometry::new();
        for (preceding, expected_x) in [
            (ObjectKind::Surface, 2.5),
            (ObjectKind::Curve, 5.),
            (ObjectKind::Edge, 5.),
            (ObjectKind::Face, 7.5),
        ] {
            let mut solution = Solution::new();
            solution.num_objects = 2;
            solution.objects[0] = Some(Object(preceding));
            solution.objects[1] = Some(Object(ObjectKind::Curve));
            for (i, t) in [0.25, 0.5, 0.75].into_iter().enumerate() {
                *solution.start.param_mut(i).unwrap() = t;
            }
            assert_eq!(
                solution_point(k, &objects, &solution, 1).unwrap(),
                Point3::new(expected_x, 0., 0.),
                "preceding {preceding:?}"
            );
        }
    }

    #[test]
    fn solution_point_ignores_ordinary_evaluation_failures() {
        let k = &Kernel::default();
        let objects = Geometry {
            curve: Curve::from(BSplineCurve::new(3)),
            surface: Surface::from(BSplineSurface::new()),
        };
        assert!(objects.curve.evaluate_point(k, 2.).is_err());
        assert!(objects
            .surface
            .evaluate_point(k, Point2::new(2., 2.))
            .is_err());
        for kind in [
            ObjectKind::Curve,
            ObjectKind::Edge,
            ObjectKind::Surface,
            ObjectKind::Face,
        ] {
            let mut solution = Solution::new();
            solution.num_objects = 1;
            solution.objects[0] = Some(Object(kind));
            *solution.start.param_mut(0).unwrap() = 2.;
            *solution.start.param_mut(1).unwrap() = 2.;
            assert_eq!(
                solution_point(k, &objects, &solution, 0).unwrap(),
                Point3::UNDEF
            );
        }
    }

    #[test]
    fn target_classifier_resolves_notify_named_primitive_box() {
        let mut k = Kernel::default();
        let box_brep =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 1.0, 1.0, 1.0)
                .unwrap();
        let mut target = Target::new(&box_brep);
        assert_eq!(
            target
                .classify(&k, Point3::new(0.5, 0.5, 0.5), 1e-5, false)
                .unwrap(),
            (PointClassType::Region, true)
        );
        assert_eq!(
            target
                .classify(&k, Point3::new(2.0, 2.0, 2.0), 1e-5, false)
                .unwrap(),
            (PointClassType::Region, false)
        );
    }
}

//! The topology solver's state, box culling and tree traversal, and its local
//! solvers for point/vertex, point/curve, point/edge, point/face, curve/curve and
//! edge/edge pairs.
//!
//! Upstream: `source/SMLib/src/SmTopologySolver.cpp`, `source/SMLib/inc/SmTopologySolver.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`TopologyObjects`]                               | the `SmObject*` casts and the `SmVertex`, `SmEdge`, `SmFace`, `SmSurface`, `SmCurve::GetEdge`, `SmShape::IsInShape` accessors the solvers call |
//! | [`CurveRef`]                                      | an `SmCurve*` member (`m_pCurve`, `m_pCurve2`)   |
//! | [`LocalSolverKind`]                               | the `SmTopoLocalSolver` class hierarchy          |
//! | [`LocalSolver`], [`LocalSolver::new`]             | `SmTopoLocalSolver` and every derived class, their constructors |
//! | `Drop` (implicit)                                 | the local solver destructors                     |
//! | [`LocalSolver::set_point`]                        | `SmTLSPoint::SetPoint`                           |
//! | [`LocalSolver::set_point_vertex`]                 | `SmTLSPoint::SetPointVertex`                     |
//! | [`LocalSolver::set_curve`]                        | `SmTLSCurve::SetCurve`                           |
//! | [`LocalSolver::set_curve_edge`]                   | `SmTLSCurve::SetCurveEdge`                       |
//! | [`LocalSolver::solve_it`]                         | the virtual `SolveIt` of every local solver: `SmTopoLocalSolver::SolveIt`, `SmTLSPoint::SolveIt`, `SmTLSCurve::SolveIt` (`SM_ERR`), `SmTLSPointVertex::SolveIt`, `SmTLSVertexVertex::SolveIt`, `SmTLSPointCurve::SolveIt`, `SmTLSPointEdge::SolveIt`, `SmTLSVertexEdge::SolveIt`, `SmTLSPointFace::SolveIt`, `SmTLSCurveEdge::SolveIt`, `SmTLSCurveCurve::SolveIt`, `SmTLSEdgeEdge::SolveIt` |
//! | `face_silhouettes` (private)                      | `sm_GetFaceSilhouettes`                          |
//! | [`TopologySolver`], [`TopologySolver::new`]       | `SmTopologySolver`, `SmTopologySolver::SmTopologySolver` |
//! | `Drop` (implicit)                                 | `~SmTopologySolver`                              |
//! | [`TopologySolver::may_contain_answer`]            | `SmTopologySolver::MayContainAnswer`             |
//! | [`TopologySolver::solve_trees`]                   | `SmTopologySolver::SolveTrees`                   |
//! | [`TopologySolver::solve_two_trees`]               | `SmTopologySolver::SolveTwoTrees`                |
//! | [`TopologySolver::best_answer_so_far`], [`best_answer_so_far`] | `SmTopologySolver::GetBestAnswerSoFar`  |
//! | `GlobalSolve::are_ready_for_local_solve` for [`TopologySolver`] | `SmTopologySolver::AreReadyForLocalSolve` |
//! | `GlobalSolve::identical_solutions` for [`TopologySolver`], `TopologyDomains` (private) | `SmGlobalSolver::IdenticalSolutions` (not overridden upstream: the solutions' `SmObject*` resolve through [`TopologyObjects`]) |
//!
//! Deviations:
//! - **Objects.** Tree objects and solution objects are a caller-chosen handle `O`
//!   (upstream `SmObject*`; a null pointer is `None`). Everything the solvers read
//!   through those pointers (the curve behind a tree object, a vertex's point and
//!   tolerance, an edge's curve and interval, a curve's edge, a surface's face, a
//!   face's surface, UV domain and tolerance, shape membership) goes through the
//!   [`TopologyObjects`] the solver is built with, so a B-rep, a shape or a set of
//!   free-standing objects can back a solve. The handle of the analytic replacement
//!   a curve cache makes of a curve is also asked of it
//!   ([`TopologyObjects::analytic_curve_handle`]).
//! - **Class hierarchy.** The local solver classes are one [`LocalSolver`] record
//!   (the union of their data members) tagged with its [`LocalSolverKind`];
//!   [`LocalSolver::solve_it`] dispatches on the kind and a derived class calling
//!   its base class's `SolveIt` calls the corresponding private method. Members
//!   upstream only reaches through `friend` access are public fields here.
//! - **Owner links.** The local solver's reference to its topology solver
//!   (`m_rTopologySolver`) is the `ts` argument of [`LocalSolver::solve_it`]; the
//!   topology solver holds its local solver (`m_pLocalSolver`) by value while
//!   [`TopologySolver::solve_trees`] runs and hands it back afterwards (with the
//!   state the solves left in it, as upstream's caller-owned object).
//! - **Null pointers.** Paths where upstream dereferences a null object, curve,
//!   edge or face (a curve without an edge in the edge solvers, a surface without a
//!   face, missing optional vectors) return [`SolidError::InvalidInput`]; the
//!   `NER` checks return `SM_ERR_NULL_POINTER`. `SolveTrees`' null-tree check has
//!   no counterpart (trees are references). The `SM_ASSERT`s, the
//!   `SM_DEBUG_CODE` drawing blocks, `Dump` and `IsKindOf` are not ported.
//! - **`MayContainAnswer`** reads the uninitialised results of
//!   `SmExtent3d::IntersectLine` when that call fails (a ray vector shorter than
//!   `SM_EFF_ZERO` or an undefined box); here the failure is returned. Operations it
//!   does not handle print an `ERR` message upstream and answer `TRUE`; here they
//!   silently answer `true`. The same `ERR` in `SmTLSPointVertex::SolveIt` only
//!   prints, so those operations still add their solution.
//! - **`SolveTwoTrees`** solves each object of the first tree through a one-node
//!   object tree local to the call; afterwards upstream's first tree pointer
//!   dangles, here it is cleared.
//! - **Unported callees.** Part 2 now supplies the local traversal and its six
//!   local classes. Curve/curve solves and surface solves still propagate their
//!   owning units' unported errors.
//!   Face silhouettes use the ported surface cache factory. The face-silhouette loop names temporary curves
//!   by their face owner through the adapter.
//! - **Face classification.** `SmFace::PointClassify` is
//!   [`crate::query::face::p2::point_classify_prepared`] on the B-rep and face
//!   [`TopologyObjects::face_entity`] names (unported when the adapter names none or
//!   the classification would first create UV trims; the solver only reads its
//!   objects). No trimmed surface cache is passed, so upstream's cache fast path
//!   (taken when the face's surface has an up-to-date one) is skipped.
//!   `SmFace::Point3DClassify` is [`crate::query::face::p1::point_3d_classify`] on a
//!   copy of that B-rep (its UV trim curve and trimmed surface cache additions are not
//!   kept).

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::Curve;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::p9::CurveSolveOptions;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::{Extent3d, LineHit};
use crate::geom::geom_utility::p1::{line_closest_point, point_project_to_plane};
use crate::geom::global_solver::{
    Branch, GlobalSolve, GlobalSolver, ObjectZoneTol, OperationCategory, PreferredSolution,
    SolutionPairType, SolutionRequested, SolverOperation, MAX_SOLVER_TREES,
};
use crate::geom::object::{Shape, ShapeMember};
use crate::geom::solution_array::{
    ParameterDomains, Solution, SolutionArray, SolutionObject, SolutionType, SortKey, MAX_TREES,
};
use crate::geom::tol::{zone_tol_3d, SmallTopoType};
use crate::geom::tree::Tree;
use crate::geom::vector2d::Vec2;
use crate::geom::vector3d::sqrt;
use crate::intersect::point_class::PointClassification;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::query::face::p2::{
    point_classify_prepared, point_classify_requested, PointClassifyOptions,
};
use crate::surface::surface::p2::{surface_cache, Surface, B_SPLINE_SURFACE_TYPE};
use crate::surface::surface_cache::p1::{AltCacheKey, AltSrfCacheType};
use crate::surface::trim_srf_cache::POINT_CLASS_UNKNOWN;
use crate::topology::face::p2::create_curves_by_trimming_snapshot;

const OP_MAY: &str = "intersect::topology_solver::p1::TopologySolver::may_contain_answer";
const OP_TWO_TREES: &str = "intersect::topology_solver::p1::TopologySolver::solve_two_trees";
const OP_PV: &str = "intersect::topology_solver::p1::LocalSolver::solve_point_vertex";
const OP_PC: &str = "intersect::topology_solver::p1::LocalSolver::solve_point_curve";
const OP_PE: &str = "intersect::topology_solver::p1::LocalSolver::solve_point_edge";
const OP_VERTEX: &str = "intersect::topology_solver::p1::LocalSolver::load_vertex";
const OP_PF: &str = "intersect::topology_solver::p1::LocalSolver::solve_point_face";
const OP_CE: &str = "intersect::topology_solver::p1::LocalSolver::solve_curve_edge";
const OP_CC: &str = "intersect::topology_solver::p1::LocalSolver::solve_curve_curve";
const OP_EE: &str = "intersect::topology_solver::p1::LocalSolver::solve_edge_edge";
const OP_BASE: &str = "intersect::topology_solver::p1::LocalSolver::solve_it";
const OP_FACE_SIL: &str = "intersect::topology_solver::p1::face_silhouettes";

/// Error for a pointer upstream would dereference while null.
const fn null_deref(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// The objects a topology solve works on, as seen through their handles: the
/// geometry behind a tree object and the topology accessors the local solvers
/// call (upstream reaches these through casts of `SmObject*` and the
/// `SmVertex`/`SmEdge`/`SmFace`/`SmSurface` members).
pub trait TopologyObjects<O> {
    /// Optional owning-face cache service for surface solves.
    fn surface_solve_context(
        &self,
    ) -> Option<Box<dyn crate::surface::surface::p1::SurfaceSolveContext<O> + '_>>
    where
        O: Copy + PartialEq,
    {
        None
    }

    /// An optional live topology classifier for callers that can prepare trims in place.
    fn classify_face_point(
        &self,
        _k: &Kernel,
        _face: O,
        _uv: crate::math::Point2,
        _tolerance: f64,
        _boundaries: bool,
        _use_uv: bool,
    ) -> Option<Result<i32>> {
        None
    }

    /// Optional live `Point3DClassify` test, including the native tolerance retry.
    fn classifies_on_face(
        &self,
        _k: &Kernel,
        _face: O,
        _point: Point3,
        _tolerance: f64,
    ) -> Option<Result<bool>> {
        None
    }

    /// Optional operation-local request for a UV trim needed by classification.
    fn uv_trim_preparation(&self) -> Option<&crate::query::brep::RayPreparation> {
        None
    }

    /// The cache associated with a B-rep handle. Adapters that expose B-reps
    /// provide this, or override `brep_trees` with their own tree storage.
    fn brep_cache(&self, _brep: O) -> Result<&crate::topology::brep_cache::BrepCache<O>> {
        Err(SolidError::invalid_input(
            "intersect::topology_solver::p1::TopologyObjects::brep_cache",
            "adapter has no B-rep cache mapping",
        ))
    }

    /// The B-rep's three cache trees, addressed by this adapter's handles.
    fn brep_trees(&self, brep: O) -> Result<super::p2::TopologyTrees<'_, O>> {
        const OP: &str = "intersect::topology_solver::p1::TopologyObjects::brep_trees";
        let cache = self.brep_cache(brep)?;
        Ok(super::p2::TopologyTrees {
            vertices: cache.vertex_tree().ok_or(SolidError::null_pointer(OP))?,
            curves: cache.curve_tree().ok_or(SolidError::null_pointer(OP))?,
            surfaces: cache.surface_tree().ok_or(SolidError::null_pointer(OP))?,
        })
    }

    /// The object represented by a typed shape member, inverse of `shape_member`.
    /// Only nonempty shape trees require this mapping; an unsupported adapter
    /// returns an input error rather than inventing an object handle.
    fn shape_member_object(&self, _member: ShapeMember) -> Result<O> {
        Err(SolidError::invalid_input(
            "intersect::topology_solver::p2::TopologyObjects::shape_member_object",
            "adapter has no shape-member mapping",
        ))
    }

    /// The geometry stored in a shape tree: vertices themselves, edge curves,
    /// and face surfaces. Shape trees use topology keys on the Rust side, whereas
    /// the upstream object lists store the corresponding geometry pointers.
    fn shape_geometry_object(&self, member: ShapeMember) -> Result<O> {
        let object = self.shape_member_object(member)?;
        match member {
            ShapeMember::Vertex(_) => Ok(object),
            ShapeMember::Edge(_) => self.edge_curve(object)?.ok_or(SolidError::null_pointer(
                "intersect::topology_solver::p2::shape_geometry_object",
            )),
            ShapeMember::Face(_) => self.face_surface(object),
            ShapeMember::Other => Err(SolidError::invalid_input(
                "intersect::topology_solver::p2::shape_geometry_object",
                "not a shape geometry member",
            )),
        }
    }

    /// The curve a curve handle refers to (the `SmCurve*` cast of a tree object).
    fn curve(&self, object: O) -> Result<&Curve>;

    /// The handle naming the analytic replacement (an `SmLine` of a straight
    /// curve) the curve cache of `curve` may make; `None` when there is none to
    /// name.
    fn analytic_curve_handle(&self, curve: O) -> Option<O>;

    /// `SmVertex::GetPoint`.
    fn vertex_point(&self, vertex: O) -> Result<Point3>;

    /// `SmVertex::GetTolerance`.
    fn vertex_tolerance(&self, vertex: O) -> Result<f64>;

    /// `SmCurve::GetEdge`: the edge a curve defines, if any.
    fn curve_edge(&self, curve: O) -> Result<Option<O>>;

    /// `SmEdge::GetCurve`: the handle of an edge's curve, if any.
    fn edge_curve(&self, edge: O) -> Result<Option<O>>;

    /// `SmEdge::GetInterval`.
    fn edge_interval(&self, edge: O) -> Result<Extent1d>;

    /// `SmEdge::GetTolerance`.
    fn edge_tolerance(&self, edge: O) -> Result<f64>;

    /// `SmSurface::GetFace`: the face a surface defines, if any.
    fn surface_face(&self, surface: O) -> Result<Option<O>>;

    /// `SmFace::GetSurface`.
    fn face_surface(&self, face: O) -> Result<O>;

    /// The surface a surface handle refers to (the `SmSurface*` of a face's
    /// `GetSurface`).
    fn surface(&self, surface: O) -> Result<&Surface>;

    /// `SmFace::GetUVDomain`.
    fn face_uv_domain(&self, face: O) -> Result<Extent2d>;

    /// `SmFace::GetTolerance`.
    fn face_tolerance(&self, face: O) -> Result<f64>;

    /// The B-rep and face record a face handle names, for the face members that work
    /// on the B-rep (`SmFace::PointClassify`). `None` (the default) is a handle the
    /// adapter cannot resolve: a gap in the caller's adapter, which the caller reports.
    fn face_entity(&self, face: O) -> Option<(&crate::topology::Brep, crate::topology::FaceId)> {
        let _ = face;
        None
    }

    /// `SmSurface::GetOwner`.
    fn surface_owner(&self, surface: O) -> Result<Option<O>>;

    /// The member an edge or face handle is for `SmShape::IsInShape`.
    fn shape_member(&self, object: O) -> ShapeMember;

    /// `SmTol::GetZoneTol3d(const SmObject*)` of a non-null handle (`SM_USE_OLDTOL`):
    /// a topology object's own `GetTolerance`, a curve's edge's or a surface's face's
    /// tolerance (the system zone tolerance when it has none), a B-rep's tolerance,
    /// and the system zone tolerance for other objects.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] by default: an adapter that cannot resolve its handles
    /// to their B-rep entities.
    fn object_zone_tol_3d(&self, k: &Kernel, object: O) -> Result<f64> {
        let _ = (k, object);
        // UNPORTED(geom.tol): SmTol::GetZoneTol3d(const SmObject*) on a handle this
        // adapter does not resolve to its B-rep entity
        // Proof (unreachable): only Rayfire solvers read this; the library ones
        // (offset_geometry_creation, brep p2) pass a resolver or override
        // object_zone_tol_3d; 0 hits in the green difftest sweep.
        Err(SolidError::unported(
            "SmTol::GetZoneTol3d(const SmObject*)",
            "geom.tol",
        ))
    }
}

/// A curve a local solver works on (`m_pCurve`, `m_pCurve2`): the curve and the
/// handle naming it in solutions.
#[derive(Debug, Clone, Copy)]
pub struct CurveRef<'a, O> {
    /// The curve.
    pub curve: &'a Curve,
    /// Its handle.
    pub handle: O,
}

/// The class of a local solver (the `SmTopoLocalSolver` hierarchy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LocalSolverKind {
    /// `SmTopoLocalSolver` (its `SolveIt` fails with `SM_ERR`).
    #[default]
    Base,
    /// `SmTLSPoint` (its `SolveIt` fails with `SM_ERR`).
    Point,
    /// `SmTLSPointVertex`.
    PointVertex,
    /// `SmTLSVertexVertex`.
    VertexVertex,
    /// `SmTLSPointCurve`.
    PointCurve,
    /// `SmTLSPointEdge`.
    PointEdge,
    /// `SmTLSVertexEdge`.
    VertexEdge,
    /// `SmTLSPointFace`.
    PointFace,
    /// `SmTLSVertexFace`.
    VertexFace,
    /// `SmTLSCurve` (its `SolveIt` fails with `SM_ERR`).
    Curve,
    /// `SmTLSCurveVertex`.
    CurveVertex,
    /// `SmTLSEdgeVertex`.
    EdgeVertex,
    /// `SmTLSCurveCurve`.
    CurveCurve,
    /// `SmTLSCurveEdge`.
    CurveEdge,
    /// `SmTLSEdgeEdge`.
    EdgeEdge,
    /// `SmTLSCurveFace`.
    CurveFace,
    /// `SmTLSEdgeFace`.
    EdgeFace,
    /// `SmTLSFaceFace`.
    FaceFace,
}

/// A local solver: the data members of the `SmTopoLocalSolver` classes. Which
/// members a kind uses follows its upstream class.
#[derive(Debug, Clone)]
pub struct LocalSolver<'a, O> {
    /// The class.
    pub kind: LocalSolverKind,
    /// `m_bSwapOrder`: the object order passed to [`LocalSolver::solve_it`] is
    /// reversed (normally the point or first curve comes first).
    pub swap_order: bool,
    /// `m_pShape`: the shape being queried; edges and faces outside it are
    /// skipped.
    pub shape: Option<&'a Shape>,
    /// `m_vPoint` (`SmTLSPoint`): the query point.
    pub point: Point3,
    /// `m_pPointVertex` (`SmTLSPoint`): the vertex the point represents, if any.
    pub point_vertex: Option<O>,
    /// `m_pCurveOwner` (`SmTLSPointCurve`): the object named by solutions in place
    /// of the curve.
    pub curve_owner: Option<O>,
    /// `m_vInterval` (`SmTLSPointCurve`, `SmTLSCurve`): the interval of the
    /// (first) curve.
    pub interval: Extent1d,
    /// `m_pSurfaceCurve` (`SmTLSPointCurve`, `SmTLSCurve`): the UV curve whose
    /// point replaces the curve parameter in solutions.
    pub surface_curve: Option<&'a Curve>,
    /// `m_pCurve` (`SmTLSCurve`): the first curve.
    pub curve: Option<CurveRef<'a, O>>,
    /// `m_pCurveEdge` (`SmTLSCurve`): the first curve's edge.
    pub curve_edge: Option<O>,
    /// `m_pCurveFace` (`SmTLSCurve`): the face the first curve lies on.
    pub curve_face: Option<O>,
    /// `m_pCurve2` (`SmTLSCurveCurve`): the second curve.
    pub curve2: Option<CurveRef<'a, O>>,
    /// `m_vInterval2` (`SmTLSCurveCurve`).
    pub interval2: Extent1d,
    /// `m_pCurveEdge2` (`SmTLSCurveCurve`).
    pub curve_edge2: Option<O>,
    /// `m_pCurveFace2` (`SmTLSCurveCurve`).
    pub curve_face2: Option<O>,
    /// `m_pSurfaceCurve2` (`SmTLSCurveCurve`).
    pub surface_curve2: Option<&'a Curve>,
    /// The curves are temporary (a face's silhouette curves, upstream's curve objects
    /// made for this solve): their solves name no handle of their own, so their
    /// solutions name the curves themselves, with their natural intervals, as
    /// upstream's do (the face stands in only for the results).
    pub(super) temporary_curves: bool,
}

/// `SmTopologySolver::GetBestAnswerSoFar` of the solver state `s`: the best answer
/// so far (`sqrt` of the squared one, negated for a negative signed one) when the
/// operation limits its solutions by it, `None` (upstream a null pointer and `0.0`)
/// otherwise.
#[must_use]
pub fn best_answer_so_far<O>(s: &GlobalSolver<'_, O>) -> Option<f64> {
    // init output
    let mut best = None;

    if s.best_answer_so_far_sq < BIG_DOUBLE && s.category == OperationCategory::Minimize {
        best = Some(sqrt(s.best_answer_so_far_sq));
    }

    if s.best_answer_so_far_sq > 0.0 && s.category == OperationCategory::Maximize {
        best = Some(sqrt(s.best_answer_so_far_sq));
    }

    if s.category == OperationCategory::Signed {
        best = Some(if s.best_answer_so_far_sq >= 0.0 {
            sqrt(s.best_answer_so_far_sq.abs())
        } else {
            -sqrt(s.best_answer_so_far_sq.abs())
        });
    }

    best
}

/// Optional vector `index` of the solver state (`m_cpOptVectors[index]`).
fn opt_vector<O: SolutionObject>(s: &GlobalSolver<'_, O>, index: usize) -> Result<Vec3> {
    s.opt_vector(index)
}

/// Appends `object` to the first `count` entries of `objects`
/// (`sSol.m_apObjects[lObjectCount++] = object`).
fn push_object<O: Copy>(
    objects: &mut [Option<O>; MAX_TREES],
    count: &mut usize,
    object: Option<O>,
) {
    if let Some(slot) = objects.get_mut(*count) {
        *slot = object;
    }
    *count += 1;
}

/// The topology solver (`SmTopologySolver`): global solver state, the objects it
/// solves on and the local solver its tree traversals call.
pub struct TopologySolver<'t, 'a, O> {
    /// The `SmGlobalSolver` base state.
    pub base: GlobalSolver<'t, O>,
    /// The kernel the local solves evaluate with.
    pub kernel: &'a Kernel,
    /// The objects behind the handles.
    pub objects: &'a dyn TopologyObjects<O>,
    /// `m_pLocalSolver`: the local solver of the running tree solve.
    pub local_solver: Option<LocalSolver<'a, O>>,
}

impl<'t, 'a, O: SolutionObject + 't> TopologySolver<'t, 'a, O> {
    /// `SmTopologySolver::SmTopologySolver`: a solver for `operation` keeping
    /// `requested` solutions within `tolerance_3d`, limited by
    /// `best_answer_so_far_sq`, with the operation's optional vectors
    /// (`opt_vectors`, empty for none). The operation category is set from the
    /// operation (the minimising, maximising and signed operations; everything
    /// else, [`SolverOperation::Rayfire`] included, is
    /// [`OperationCategory::Other`]); the directed, projected and signed
    /// directed / angle operations are projected.
    #[must_use]
    pub fn new(
        kernel: &'a Kernel,
        objects: &'a dyn TopologyObjects<O>,
        operation: SolverOperation,
        requested: SolutionRequested,
        tolerance_3d: f64,
        best_answer_so_far_sq: f64,
        opt_vectors: &[Vec3],
    ) -> Self {
        let mut base = GlobalSolver::new();
        base.operation = operation;
        base.solution_requested = requested;
        base.tolerance_3d = tolerance_3d;
        base.best_answer_so_far_sq = best_answer_so_far_sq;
        base.opt_vectors = opt_vectors.to_vec();

        match operation {
            SolverOperation::DirectedMinimize | SolverOperation::ProjectedMinimize => {
                base.projected_operation = true;
                base.category = OperationCategory::Minimize;
            }
            SolverOperation::Minimize | SolverOperation::AngleMinimize => {
                base.category = OperationCategory::Minimize;
            }
            SolverOperation::DirectedMaximize | SolverOperation::ProjectedMaximize => {
                base.projected_operation = true;
                base.category = OperationCategory::Maximize;
            }
            SolverOperation::Maximize => base.category = OperationCategory::Maximize,
            SolverOperation::SignedDirectedMinimize | SolverOperation::SignedAngleMinimize => {
                base.projected_operation = true;
                base.category = OperationCategory::Signed;
            }
            SolverOperation::Signed3dDirectedMinimize => {
                base.category = OperationCategory::Signed;
            }
            _ => {
                base.category = OperationCategory::Other;
                base.projected_operation = false;
            }
        }

        Self {
            base,
            kernel,
            objects,
            local_solver: None,
        }
    }

    /// `SmTopologySolver::GetBestAnswerSoFar` (see [`best_answer_so_far`]).
    #[must_use]
    pub fn best_answer_so_far(&self) -> Option<f64> {
        best_answer_so_far(&self.base)
    }

    /// `SmTopologySolver::MayContainAnswer`: false when the boxes of two objects
    /// prove that the pair holds no answer for the operation.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a missing optional vector, and for
    /// [`SolverOperation::Rayfire`] a ray vector shorter than `SM_EFF_ZERO` or an
    /// undefined second box (upstream then reads uninitialised values).
    pub fn may_contain_answer(&self, box1: &Extent3d, box2: &Extent3d) -> Result<bool> {
        let s = &self.base;
        let tol = s.tolerance_3d;
        match s.operation {
            SolverOperation::Intersect => {
                let mut bbox = *box1;
                bbox.expand_absolute(tol);

                if bbox.are_disjoint(box2, 0.0) {
                    return Ok(false); // No answers in this branch
                }
                Ok(true) // possible answers in this branch
            }

            SolverOperation::Minimize => {
                let min_dist_sq = box1.min_distance_squared(box2);
                if min_dist_sq > s.best_answer_so_far_sq
                    && sqrt(min_dist_sq) - tol > sqrt(s.best_answer_so_far_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true)
            }

            SolverOperation::Maximize => {
                let max_dist_sq = box1.max_distance_squared(box2);
                if max_dist_sq < s.best_answer_so_far_sq
                    && sqrt(max_dist_sq) + tol < sqrt(s.best_answer_so_far_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true)
            }

            SolverOperation::AtDistance => {
                let max_dist_sq = box1.max_distance_squared(box2);
                if max_dist_sq < s.at_distance_sq
                    && sqrt(max_dist_sq) - tol < sqrt(s.at_distance_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                let min_dist_sq = box1.min_distance_squared(box2);
                if min_dist_sq > s.at_distance_sq
                    && sqrt(min_dist_sq) + tol > sqrt(s.at_distance_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true) // may be answers
            }

            SolverOperation::ProjectedMinimize | SolverOperation::ProjectedMaximize => {
                let s1 = box1.sphere_bound();
                let s2 = box2.sphere_bound();
                let proj_pnt = point_project_to_plane(s2.center, s1.center, opt_vector(s, 0)?);
                if s.operation == SolverOperation::ProjectedMinimize {
                    let min_proj_dist = proj_pnt.distance(s1.center) - s1.radius - s2.radius;
                    if min_proj_dist > 0.0
                        && min_proj_dist * min_proj_dist > s.best_answer_so_far_sq
                    {
                        return Ok(false);
                    }
                }
                if s.operation == SolverOperation::ProjectedMaximize {
                    let max_proj_dist = proj_pnt.distance(s1.center) + s1.radius + s2.radius;
                    if max_proj_dist * max_proj_dist < s.best_answer_so_far_sq {
                        return Ok(false);
                    }
                }

                // If we were unable to elimiate using above sphere tests then
                // we just skip out here with a TRUE.
                Ok(true)
            }

            // If we are doing a directed minimization we can use the bounding
            // box information to eliminate additional comparisons
            SolverOperation::DirectedMinimize
            | SolverOperation::SignedDirectedMinimize
            | SolverOperation::DirectedMaximize => {
                // This optimization can be done by creating a plane through the
                // center of one bounding box whose normal is perpendicular to the
                // direction vector and seeing if the distance to the
                // center of the other bounding box is greater than the sum of
                // the two radii.
                let v0 = opt_vector(s, 0)?;
                let v1 = opt_vector(s, 1)?;
                let plane_norm = v1.cross(v0);
                let s1 = box1.sphere_bound();
                let s2 = box2.sphere_bound();
                let proj_pnt = point_project_to_plane(s2.center, s1.center, plane_norm);
                let sum_radius = s1.radius + s2.radius;
                if proj_pnt.distance_squared(s1.center) > sum_radius * sum_radius + EFF_ZERO_SQRT {
                    return Ok(false); // These two nodes can not solve this directed minimization
                }

                if s.operation == SolverOperation::SignedDirectedMinimize {
                    let vec = proj_pnt - s1.center;
                    let mut dist = vec.length();
                    if vec.dot(v1) < 0.0 {
                        dist = -dist;
                    }
                    let min_proj_dist = dist - s1.radius - s2.radius;
                    if min_proj_dist * min_proj_dist.abs() > s.best_answer_so_far_sq {
                        return Ok(false);
                    }
                }

                if s.operation == SolverOperation::DirectedMinimize {
                    let min_proj_dist = proj_pnt.distance(s1.center) - s1.radius - s2.radius;
                    if min_proj_dist > 0.0
                        && min_proj_dist * min_proj_dist > s.best_answer_so_far_sq
                    {
                        return Ok(false);
                    }
                }

                if s.operation == SolverOperation::DirectedMaximize {
                    let max_proj_dist = proj_pnt.distance(s1.center) + s1.radius + s2.radius;
                    if max_proj_dist * max_proj_dist < s.best_answer_so_far_sq {
                        return Ok(false);
                    }
                }

                Ok(true)
            }

            // Doing a ray firing - minimization of intersections along a vector.
            SolverOperation::Rayfire => {
                // get the ray
                let line_pnt = box1.min();
                let line_vec = opt_vector(s, 0)?;

                // expand target bounding box by tolerance
                let mut object_box = *box2;
                object_box.expand_absolute(tol);

                // intersect ray with boundingbox + tol
                let (enter, exit) = match object_box.intersect_line(line_pnt, line_vec) {
                    // when there is no intersection
                    Ok(LineHit::Miss) => return Ok(false),
                    Ok(LineHit::Touch { t }) => (t, t),
                    Ok(LineHit::Through { enter, exit }) => (enter, exit),
                    Err(_) => {
                        return Err(null_deref(
                            OP_MAY,
                            "ray/box intersection failed (upstream reads uninitialised values)",
                        ))
                    }
                };

                // when there is no intersection on the right side of the ray
                if enter < 0.0 && exit < 0.0 {
                    return Ok(false);
                }

                // See if we are further out on the positive side of the ray than
                // the best answer so far.
                if enter > 0.0 && enter * enter > s.best_answer_so_far_sq {
                    return Ok(false);
                }

                Ok(true)
            }

            // If we are doing a 3D directed minimization we can use the bounding
            // box information to eliminate additional comparisons
            SolverOperation::Signed3dDirectedMinimize => {
                let s1 = box1.sphere_bound();
                let s2 = box2.sphere_bound();
                let line_pnt = s1.center;
                let line_vec = opt_vector(s, 0)?;
                let param = line_closest_point(line_pnt, line_vec, s2.center);
                let cl_pnt = line_pnt + param * line_vec;
                let dist = cl_pnt.distance(s2.center);
                let sum_radius = s1.radius + s2.radius;
                // If two spheres don't intersect as projected along the vector
                // return FALSE.
                if dist > sum_radius + tol {
                    return Ok(false);
                }

                let vec = cl_pnt - s1.center;
                let mut dist_to = vec.length();
                if vec.dot(line_vec) < 0.0 {
                    dist_to = -dist_to;
                }
                let min_dist = dist_to - s1.radius - s2.radius - tol;
                if min_dist * min_dist.abs() > s.best_answer_so_far_sq {
                    return Ok(false);
                }

                Ok(true)
            }

            // SM_SO_NORMALIZE, SM_SO_FIND: no optimization here yet; the rest are
            // not handled yet (upstream: ERR(SM_ERR), a message only).
            _ => Ok(true),
        }
    }

    /// `SmTopologySolver::SolveTrees`: runs the global solve over `tree1` and
    /// `tree2` from their top nodes, `local` solving each pair of objects whose
    /// boxes may hold answers; solutions accumulate in
    /// [`GlobalSolver::solutions`] of [`TopologySolver::base`]. `local` comes back
    /// with the state the solves left in it.
    ///
    /// # Errors
    ///
    /// Those of the traversal ([`GlobalSolve::solve_branch`]);
    /// Errors from local solves or their explicit unported callees.
    pub fn solve_trees(
        &mut self,
        tree1: &'t Tree<O>,
        tree2: &'t Tree<O>,
        local: &mut LocalSolver<'a, O>,
    ) -> Result<()> {
        self.local_solver = Some(core::mem::take(local));
        let result = self.solve_trees_inner(tree1, tree2);
        if let Some(back) = self.local_solver.take() {
            *local = back;
        }
        result
    }

    /// The body of [`TopologySolver::solve_trees`] once the local solver is in
    /// place.
    fn solve_trees_inner(&mut self, tree1: &'t Tree<O>, tree2: &'t Tree<O>) -> Result<()> {
        let s = &mut self.base;
        s.num_variables = 0;
        s.trees = [None; MAX_SOLVER_TREES];
        s.trees[0] = Some(tree1);
        s.trees[1] = Some(tree2);
        s.num_trees = 2;

        // set up maximize problems with no size constraints
        if s.best_answer_so_far_sq == BIG_DOUBLE && s.category == OperationCategory::Maximize {
            s.best_answer_so_far_sq = 0.0;
        }

        // (not upstream) a new solve: restart the work bound of solve_branch
        s.branches_visited = 0;

        // locals
        let start_branch: Branch = [tree1.top_node(), tree2.top_node(), None];

        // pass the call along
        self.solve_branch(&start_branch)
    }

    /// `SmTopologySolver::SolveTwoTrees`: solves every object of `tree1` (in
    /// [`Tree::object_list`] order) against `tree2` with
    /// [`TopologySolver::solve_trees`], each through a one-node object tree with
    /// the object's box.
    ///
    /// # Errors
    ///
    /// Those of [`TopologySolver::solve_trees`] and of reading `tree1`'s object
    /// lists.
    pub fn solve_two_trees(
        &mut self,
        tree1: &Tree<O>,
        tree2: &'t Tree<O>,
        local: &mut LocalSolver<'a, O>,
    ) -> Result<()> {
        let list_ids = tree1.object_list()?;

        // Use a modified point tree and put each object into it one
        // at a time and solve against the other tree.
        let mut object_tree: Tree<O> = Tree::from_point(Point3::new(0.0, 0.0, 0.0), 0.0, true);
        let node = object_tree
            .top_node()
            .ok_or(null_deref(OP_TWO_TREES, "object tree has no node"))?;
        let list = object_tree
            .node(node)?
            .list_head()
            .ok_or(null_deref(OP_TWO_TREES, "object tree has no object list"))?;

        self.base.num_variables = 0;
        self.base.trees = [None, Some(tree2), None];
        self.base.num_trees = 2;

        for id in list_ids {
            let item = *tree1.list(id)?;
            object_tree.node_mut(node)?.bbox = item.bbox;
            let entry = object_tree.list_mut(list)?;
            entry.bbox = item.bbox;
            entry.next = None;
            entry.object = item.object;

            // the object tree lives only in this call: solve with a solver state
            // borrowing it, then take the state back
            let mut scoped = TopologySolver {
                base: self.take_base(),
                kernel: self.kernel,
                objects: self.objects,
                local_solver: None,
            };
            let result = scoped.solve_trees(&object_tree, tree2, local);
            self.restore_base(scoped.base, tree2);
            result?;
        }
        Ok(())
    }

    /// Moves the solver state out (for a solve over trees borrowed for less than
    /// `'t`), leaving a default state behind.
    pub(super) fn take_base<'x>(&mut self) -> GlobalSolver<'x, O>
    where
        't: 'x,
    {
        core::mem::take(&mut self.base)
    }

    /// Moves a state taken by [`TopologySolver::take_base`] back; its trees are
    /// replaced by `tree2` in the second slot (the first, local object tree no
    /// longer exists).
    pub(super) fn restore_base(&mut self, s: GlobalSolver<'_, O>, tree2: &'t Tree<O>) {
        let GlobalSolver {
            operation,
            category,
            projected_operation,
            solution_requested,
            num_trees,
            num_variables,
            trees: _,
            tolerance_3d,
            best_answer_so_far_sq,
            at_distance,
            at_distance_sq,
            opt_vectors,
            solutions,
            solve_depth,
            branches_visited,
        } = s;
        self.base = GlobalSolver {
            operation,
            category,
            projected_operation,
            solution_requested,
            num_trees,
            num_variables,
            trees: [None, Some(tree2), None],
            tolerance_3d,
            best_answer_so_far_sq,
            at_distance,
            at_distance_sq,
            opt_vectors,
            solutions,
            solve_depth,
            branches_visited,
        };
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for TopologySolver<'t, '_, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn object_zone_tol_3d(&self, object: O) -> Result<f64> {
        self.objects.object_zone_tol_3d(self.kernel, object)
    }

    /// `SmTopologySolver::AreReadyForLocalSolve`: any node of a topology tree may
    /// hold objects, so every branch is solved locally.
    fn are_ready_for_local_solve(&self, _branch: &Branch) -> Result<bool> {
        Ok(true)
    }

    /// `SmTopologySolver::LocalSolve`.
    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        self.local_solve_branch(k, branch)
    }

    /// `SmGlobalSolver::IdenticalSolutions` (upstream does not override it); the
    /// parameter intervals of the solutions' objects, which upstream reads through
    /// their `SmObject*`, resolve through the solver's [`TopologyObjects`] when the
    /// handle itself cannot reach them.
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let domains = TopologyDomains {
            objects: self.objects,
        };
        self.base.identical_solutions_in(sol1, sol2, &domains)
    }
}

/// [`ParameterDomains`] of a topology solver's handles: the handle's own hooks, else
/// the edge interval, curve, face UV domain or surface its [`TopologyObjects`] give.
struct TopologyDomains<'a, O> {
    objects: &'a dyn TopologyObjects<O>,
}

impl<O: SolutionObject> ParameterDomains<O> for TopologyDomains<'_, O> {
    fn edge_interval(&self, edge: &O) -> Option<Extent1d> {
        edge.edge_interval()
            .or_else(|| self.objects.edge_interval(*edge).ok())
    }

    fn natural_interval(&self, curve: &O) -> Option<Extent1d> {
        curve
            .natural_interval()
            .or_else(|| self.objects.curve(*curve).ok().map(Curve::natural_interval))
    }

    fn face_uv_domain(&self, face: &O) -> Option<Extent2d> {
        face.face_uv_domain()
            .or_else(|| self.objects.face_uv_domain(*face).ok())
    }

    fn natural_uv_domain(&self, surface: &O) -> Option<Extent2d> {
        surface.natural_uv_domain().or_else(|| {
            self.objects
                .surface(*surface)
                .ok()
                // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
                .map(Surface::natural_uv_domain_if_built)
        })
    }
}

impl<O> Default for LocalSolver<'_, O> {
    /// An `SmTopoLocalSolver` (see [`LocalSolver::new`]).
    fn default() -> Self {
        Self::new(LocalSolverKind::Base, false, None)
    }
}

impl<'a, O> LocalSolver<'a, O> {
    /// A local solver of class `kind` (the constructors of the local solver
    /// classes): `swap_order` reverses the object order of
    /// [`LocalSolver::solve_it`]; `shape` limits edges and faces to its members.
    /// Every other member starts empty (null pointers, an empty interval).
    #[must_use]
    pub fn new(kind: LocalSolverKind, swap_order: bool, shape: Option<&'a Shape>) -> Self {
        Self {
            kind,
            swap_order,
            shape,
            point: Point3::new(0.0, 0.0, 0.0),
            point_vertex: None,
            curve_owner: None,
            interval: Extent1d::new(),
            surface_curve: None,
            curve: None,
            curve_edge: None,
            curve_face: None,
            curve2: None,
            interval2: Extent1d::new(),
            curve_edge2: None,
            curve_face2: None,
            surface_curve2: None,
            temporary_curves: false,
        }
    }

    /// `SmTLSPoint::SetPoint`.
    pub fn set_point(&mut self, point: Point3) {
        self.point = point;
    }

    /// `SmTLSPoint::SetPointVertex`.
    pub fn set_point_vertex(&mut self, vertex: Option<O>) {
        self.point_vertex = vertex;
    }

    /// `SmTLSCurve::SetCurve`.
    pub fn set_curve(&mut self, curve: Option<CurveRef<'a, O>>, interval: Extent1d) {
        self.curve = curve;
        self.interval = interval;
    }

    /// `SmTLSCurve::SetCurveEdge`.
    pub fn set_curve_edge(&mut self, edge: Option<O>) {
        self.curve_edge = edge;
    }
}

impl<'a, O: SolutionObject> LocalSolver<'a, O> {
    /// `SolveIt`: solves the pair `object1`, `object2` (in swapped roles when
    /// [`LocalSolver::swap_order`]) and adds the solutions to `ts` (the virtual
    /// `SolveIt` of the solver's class).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the abstract classes ([`LocalSolverKind::Base`],
    /// [`LocalSolverKind::Point`], [`LocalSolverKind::Curve`]);
    /// `SM_ERR_NULL_POINTER` for a null object upstream checks;
    /// [`SolidError::InvalidInput`] for a null object, curve, edge or face upstream
    /// dereferences; those of the geometric solves and of
    /// [`GlobalSolve::add_sorted_solution`]; [`SolidError::Unported`] from
    /// unported callees.
    pub fn solve_it(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        match self.kind {
            LocalSolverKind::Base | LocalSolverKind::Point | LocalSolverKind::Curve => {
                Err(SolidError::sm_err(OP_BASE))
            }
            LocalSolverKind::PointVertex => self.solve_point_vertex(ts, object1, object2),
            LocalSolverKind::VertexVertex => self.solve_vertex_vertex(ts, object1, object2),
            LocalSolverKind::PointCurve => self.solve_point_curve(k, ts, object1, object2),
            LocalSolverKind::PointEdge => self.solve_point_edge(k, ts, object1, object2),
            LocalSolverKind::VertexEdge => self.solve_vertex_edge(k, ts, object1, object2),
            LocalSolverKind::PointFace => self.solve_point_face(k, ts, object1, object2),
            LocalSolverKind::CurveCurve => self.solve_curve_curve(k, ts),
            LocalSolverKind::CurveEdge => self.solve_curve_edge(k, ts, object1, object2),
            LocalSolverKind::EdgeEdge => self.solve_edge_edge(k, ts, object1, object2),
            LocalSolverKind::VertexFace => self.solve_vertex_face(k, ts, object1, object2),
            LocalSolverKind::CurveVertex => self.solve_curve_vertex(k, ts, object1, object2),
            LocalSolverKind::EdgeVertex => self.solve_edge_vertex(k, ts, object1, object2),
            LocalSolverKind::CurveFace => self.solve_curve_face(k, ts, object1, object2),
            LocalSolverKind::EdgeFace => self.solve_edge_face(k, ts, object1, object2),
            LocalSolverKind::FaceFace => self.solve_face_face(k, ts, object1, object2),
        }
    }

    /// Whether `object` (an edge or face, `None` for a null pointer) is to be
    /// solved: always without a shape, else when it is in the shape
    /// (`m_pShape == NULL || m_pShape->IsInShape(object)`).
    pub(super) fn in_shape(&self, objects: &dyn TopologyObjects<O>, object: Option<O>) -> bool {
        match self.shape {
            None => true,
            Some(shape) => object.is_some_and(|o| shape.is_in_shape(objects.shape_member(o))),
        }
    }

    /// `SmTLSPointVertex::SolveIt`: the distance (or ray, directed or projected
    /// measure) between the point and a vertex as a single-value solution with no
    /// variables.
    fn solve_point_vertex(
        &mut self,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        // sol that when found is added to SmGlobalSolver::m_pSolutions list
        let mut sol: Solution<O> = Solution::new();

        // set sSol m_lNumObjects and m_apObjects array
        //   Normally the point comes first and the vertex second.
        //   when m_bSwapOrder == TRUE, switch the order.
        let mut count = 0;
        let vertex;
        if self.swap_order {
            vertex = object1.ok_or(SolidError::null_pointer(OP_PV))?;
            push_object(&mut sol.objects, &mut count, Some(vertex));
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol.objects, &mut count, self.point_vertex);
            }
        } else {
            vertex = object2.ok_or(SolidError::null_pointer(OP_PV))?;
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol.objects, &mut count, self.point_vertex);
            }
            push_object(&mut sol.objects, &mut count, Some(vertex));
        }
        sol.num_objects = count;

        let s = &ts.base;
        let op = s.operation;
        let vertex_point = objects.vertex_point(vertex)?;

        // locals
        let diff = self.point - vertex_point;
        let mut dist_sq = diff.length_squared();
        let mut sign = 1.0;

        // When projecting, project the difference vector
        if s.projected_operation {
            let plane_norm = opt_vector(s, 0)?;
            let vec_proj = plane_norm.cross(diff).cross(plane_norm);
            if matches!(
                op,
                SolverOperation::SignedDirectedMinimize
                    | SolverOperation::DirectedMinimize
                    | SolverOperation::DirectedMaximize
            ) && !vec_proj.is_parallel_to(opt_vector(s, 1)?, 2.0)
            {
                return Ok(()); // no answers in this branch
            }
            dist_sq = vec_proj.length_squared();
            if op == SolverOperation::SignedDirectedMinimize
                && vec_proj.dot(opt_vector(s, 1)?) < 0.0
            {
                sign = -1.0;
            }
        }

        // when order is reversed and doing a signed operation - reverse the sign of the result
        if self.swap_order && s.category == OperationCategory::Signed {
            sign = -sign;
        }

        // switch on operation type to set dSign and dDistSq of Sol = dSign * sqrt(dDistSq)
        match op {
            SolverOperation::Rayfire | SolverOperation::Signed3dDirectedMinimize => {
                if op == SolverOperation::Rayfire {
                    let ray = opt_vector(s, 0)?;
                    let mut param = line_closest_point(self.point, ray, vertex_point);
                    let tol = s.tolerance_3d + objects.vertex_tolerance(vertex)?;
                    let scale = self.point.max_dimension();

                    // No answer if we are out of tol on wrong side of ray
                    if param < -EFF_ZERO * scale {
                        return Ok(());
                    }

                    // When in tol on wrong side or ray - snap to end point
                    if param < 0.0 {
                        param = 0.0;
                    }

                    let ray_pnt = self.point + param * ray;
                    let dist_to_ray = ray_pnt.distance(vertex_point);

                    // no answer - ray too from from vertex
                    if dist_to_ray > tol {
                        return Ok(());
                    }

                    // (upstream sets dDistSq = dParam*dParam here; the fall
                    // through below overwrites it)
                } // end SM_SO_RAYFIRE (falls through)

                // SM_SO_3D_SIGNED_DIRECTED_MINIMIZE
                let dir = opt_vector(s, 0)?;
                let mut param = line_closest_point(self.point, dir, vertex_point);
                let tol = s.tolerance_3d + objects.vertex_tolerance(vertex)?;

                let ray_pnt = self.point + param * dir;
                let dist_to_ray = ray_pnt.distance(vertex_point);

                // no answer - ray too far from vertex
                if dist_to_ray > tol {
                    return Ok(());
                }

                if self.swap_order {
                    param = -param;
                }

                // The following assumes that the direction vector is unitized
                dist_sq = param * param;
                sign = 1.0;
                if param < 0.0 {
                    sign = -1.0;
                }
                // (falls through to SM_SO_INTERSECT: break)
            }

            SolverOperation::ProjectedMinimize
            | SolverOperation::Minimize
            | SolverOperation::DirectedMinimize => {
                if dist_sq > s.best_answer_so_far_sq
                    && sqrt(dist_sq) - s.tolerance_3d > sqrt(s.best_answer_so_far_sq)
                {
                    return Ok(()); // no answers in this branch
                }
            }

            SolverOperation::SignedDirectedMinimize => {
                if sign * dist_sq > s.best_answer_so_far_sq {
                    let mut best_answer = sqrt(s.best_answer_so_far_sq.abs());
                    if s.best_answer_so_far_sq < 0.0 {
                        best_answer = -best_answer;
                    }

                    // no answers in this branch
                    if sign * sqrt(dist_sq) - s.tolerance_3d > best_answer {
                        return Ok(());
                    }
                }
            }

            SolverOperation::ProjectedMaximize
            | SolverOperation::Maximize
            | SolverOperation::DirectedMaximize => {
                if dist_sq < s.best_answer_so_far_sq
                    && sqrt(dist_sq) + s.tolerance_3d < sqrt(s.best_answer_so_far_sq)
                {
                    return Ok(()); // no answers in this branch
                }
            }

            SolverOperation::AtDistance => {
                // GWC: added numerical tolerance
                if dist_sq < s.at_distance_sq - EFF_ZERO * (1.0 + dist_sq)
                    && sqrt(dist_sq) - s.tolerance_3d < sqrt(s.at_distance_sq)
                {
                    return Ok(()); // no answers in this branch
                }

                // no answers in this branch
                if dist_sq > s.at_distance_sq + EFF_ZERO * (1.0 + dist_sq)
                    && sqrt(dist_sq) + s.tolerance_3d > sqrt(s.at_distance_sq)
                {
                    return Ok(());
                }
            }

            // SM_SO_INTERSECT, SM_SO_NORMALIZE, SM_SO_FIND: nothing to check; the
            // rest are not handled yet (upstream: ERR(SM_ERR), a message only).
            _ => {}
        }

        // arrive here when solution was found
        //   dSign   = +/- 1 to handle swap object and directed solution values
        //   dDistSq = solution value squared
        sol.solution_type = SolutionType::SingleValue;
        sol.num_variables = 0;
        sol.start.solution_value = sign * sqrt(dist_sq);

        // accumulate unique solutions sorted by value into SmGlobalSolver::m_pSolutions
        ts.add_sorted_solution(&sol, SortKey::BySolutionValue, true)
    }

    /// Loads the vertex among `object1`, `object2` (the first unless swapped) as
    /// the point (`m_pPointVertex`, `m_vPoint`), as the vertex solvers do before
    /// passing the call to their point solver.
    fn load_vertex(
        &mut self,
        objects: &dyn TopologyObjects<O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        self.point_vertex = if self.swap_order { object2 } else { object1 };
        let vertex = self
            .point_vertex
            .ok_or(null_deref(OP_VERTEX, "null vertex object"))?;

        // Load the Point value
        self.point = objects.vertex_point(vertex)?;
        Ok(())
    }

    /// `SmTLSVertexVertex::SolveIt`: [`LocalSolverKind::PointVertex`] with the
    /// first vertex as the point.
    fn solve_vertex_vertex(
        &mut self,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        self.load_vertex(ts.objects, object1, object2)?;

        // pass the call along to the Point/Vertex solver
        self.solve_point_vertex(ts, object1, object2)
    }

    /// `SmTLSPointCurve::SolveIt`: the curve object is the second object (the
    /// first when swapped).
    fn solve_point_curve(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        let handle = if self.swap_order { object1 } else { object2 };
        let handle = handle.ok_or(null_deref(OP_PC, "null curve object"))?;
        let curve = objects.curve(handle)?;
        self.point_curve(k, ts, curve, Some(handle))
    }

    /// The body of `SmTLSPointCurve::SolveIt` for `curve` named by `handle`: the
    /// curve's solutions for the point over [`LocalSolver::interval`]
    /// (`SmCurve::GlobalPointSolve`), named by the point vertex and the curve (or
    /// its owner), with the curve parameter replaced by the surface curve's point
    /// when there is one.
    pub(super) fn point_curve(
        &self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        curve: &Curve,
        handle: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        // Normally the point comes first and the curve second.  If
        // we want we can switch the order.
        let mut sol_objects: [Option<O>; MAX_TREES] = [None; MAX_TREES];
        let mut count = 0;
        let curve_entry = if self.curve_owner.is_some() {
            self.curve_owner
        } else {
            handle
        };
        if self.swap_order {
            push_object(&mut sol_objects, &mut count, curve_entry);
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol_objects, &mut count, self.point_vertex);
            }
        } else {
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol_objects, &mut count, self.point_vertex);
            }
            push_object(&mut sol_objects, &mut count, curve_entry);
        }

        // locals for GlobalPointSolve() call
        let s = &ts.base;
        let op = s.operation;
        let best = best_answer_so_far(s);
        let mut sign = 1.0;
        let swapped_vecs;
        let vecs: Option<&[Vec3]> =
            if self.swap_order && op == SolverOperation::Signed3dDirectedMinimize {
                // (upstream copies only the first vector into its local array)
                swapped_vecs = [-opt_vector(s, 0)?];
                Some(&swapped_vecs)
            } else {
                if self.swap_order && s.category == OperationCategory::Signed {
                    // If we are doing a signed operation and the order is reversed then we have to
                    // reverse the sign of the result.
                    sign = -sign;
                }
                if s.opt_vectors.is_empty() {
                    None
                } else {
                    Some(&s.opt_vectors)
                }
            };
        let target = best.map(|b| b * sign);

        // make the PointSolve call
        let zone_tol = |o: O| objects.object_zone_tol_3d(k, o);
        let solutions = curve.global_point_solve(
            ts.kernel,
            &self.interval,
            op,
            self.point,
            s.tolerance_3d,
            SolutionRequested::All,
            &GlobalPointSolveOptions {
                target_distance: target,
                opt_vectors: vecs,
                handle,
                analytic_handle: handle.and_then(|h| objects.analytic_curve_handle(h)),
                zone_tol_3d: Some(ObjectZoneTol(&zone_tol)),
            },
        )?;

        // iter every solution building final solution list.
        //   Set the Solution Object values and
        //   Sort into m_rTopologySolver.m_pSolutions array
        for found in solutions.as_slice() {
            let mut sol = *found;

            // If we have a surface curve then put the corresponding,
            // surface parameters into the output instead of the curve parameters.
            sol.num_variables = 1;
            if let Some(surface_curve) = self.surface_curve {
                let pnt = surface_curve.evaluate_point(k, sol.start.param(0)?)?;
                *sol.start.param_mut(0)? = pnt.x;
                *sol.start.param_mut(1)? = pnt.y;
                sol.num_variables = 2;
            }
            sol.objects[0] = sol_objects[0];
            sol.objects[1] = sol_objects[1];
            sol.num_objects = count;
            sol.start.solution_value *= sign;
            if sol.solution_type == SolutionType::RangeOfValues {
                sol.end.solution_value *= sign;
            }
            ts.add_sorted_solution(&sol, SortKey::BySolutionValue, true)?;
        } // end iter every solution building final solution list

        Ok(())
    }

    /// `SmTLSPointEdge::SolveIt`: [`LocalSolverKind::PointCurve`] over the
    /// interval of the curve's edge, solutions naming the edge.
    fn solve_point_edge(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        // Normally the point comes first and the curve second.
        // m_bSwapOrder is set when that ordering is switched.
        let handle = if self.swap_order { object1 } else { object2 };
        let handle = handle.ok_or(null_deref(OP_PE, "null curve object"))?;

        // Remove Composites - replaced Edge array with single curve Edge
        let edge = objects.curve_edge(handle)?;

        // when using m_pShape (called frome ShapePointSolve(), ShapeCurveSolve(), or ShapeShapeSolve()) - only operate on Edges in Shape
        if self.in_shape(objects, edge) {
            let edge = edge.ok_or(null_deref(OP_PE, "curve without an edge"))?;
            self.curve_owner = Some(edge);
            self.interval = objects.edge_interval(edge)?;

            // arrive here after ensuring pEdge is part of the shape
            //  and loading m_vInterval with the limiting interval.
            //  m_vPoint is already loaded with the target point.

            // pass the call along to get solutions.
            //   The target point is stored in m_vPoint, not the input args.
            //   So - put curve in as both args makes sure this works
            //        no matter the value of m_bSwapOrder
            // the solve builds the edge curve's cache at the edge's tolerance
            // (SmTol::GetApproxTol3d(&m_crCurve) reads pEdge->GetTolerance() through the
            // owner)
            let mut curve = objects.curve(handle)?.clone_for_evaluation()?;
            curve.set_owner_zone_tol_3d(Some(objects.edge_tolerance(edge)?));
            self.point_curve(k, ts, &curve, Some(handle))?;
        }

        Ok(())
    }

    /// `SmTLSVertexEdge::SolveIt`: [`LocalSolverKind::PointEdge`] with the
    /// vertex as the point.
    fn solve_vertex_edge(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        self.load_vertex(ts.objects, object1, object2)?;
        self.solve_point_edge(k, ts, object1, object2)
    }

    /// `SmTLSPointFace::SolveIt`: ray and 3D signed solves intersect the face's
    /// surface with the line through the point, projected operations measure to the
    /// face's silhouette curves, the others solve the point against the surface;
    /// only solutions the face's trim classifies inside are kept.
    pub(super) fn solve_point_face(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        // locals
        let mut sol_objects: [Option<O>; MAX_TREES] = [None; MAX_TREES];
        let mut count = 0;

        // Normally the point comes first and the surface second.  If
        // we want we can switch the order.
        let surface;
        if self.swap_order {
            surface = object1.ok_or(SolidError::null_pointer(OP_PF))?;
            push_object(&mut sol_objects, &mut count, Some(surface));
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol_objects, &mut count, self.point_vertex);
            }
        } else {
            surface = object2.ok_or(SolidError::null_pointer(OP_PF))?;
            if self.point_vertex.is_some() {
                // If the point represents a vertex then save it in list
                push_object(&mut sol_objects, &mut count, self.point_vertex);
            }
            push_object(&mut sol_objects, &mut count, Some(surface));
        }

        // Remove Composites - replaced face array with single surface face
        let face = objects.surface_face(surface)?;

        // when using m_pShape (called frome ShapePointSolve(), ShapeCurveSolve(), or ShapeShapeSolve()) - skip pFace not in shape
        if !self.in_shape(objects, face) {
            return Ok(());
        }
        let face = face.ok_or(null_deref(OP_PF, "surface without a face"))?;

        // the solution objects, the surface replaced by its face
        let named = |sol: &mut Solution<O>| {
            sol.objects[0] = sol_objects[0];
            sol.objects[1] = sol_objects[1];
            if sol_objects[0] == Some(surface) {
                sol.objects[0] = Some(face);
            }
            if sol_objects[1] == Some(surface) {
                sol.objects[1] = Some(face);
            }
            sol.num_objects = count;
            sol.num_variables = 2;
        };

        // branch on m_eSolverOperation val to find and load solutions into output m_rTopologySolver.m_pSolutions
        let op = ts.base.operation;
        let tol = ts.base.tolerance_3d;
        if op == SolverOperation::Rayfire || op == SolverOperation::Signed3dDirectedMinimize {
            // (upstream fetches the best answer here without using it)
            let _best = best_answer_so_far(&ts.base);

            let ray = opt_vector(&ts.base, 0)?;
            let mut line_vec = ray;
            let mut ray_fire = true;
            if op == SolverOperation::Signed3dDirectedMinimize {
                ray_fire = false;
                if self.swap_order {
                    line_vec = -line_vec;
                }
            }
            let face_surface = objects.face_surface(face)?;
            let uv_domain = objects.face_uv_domain(face)?;
            let face_tol = objects.face_tolerance(face)?;
            // pFace->GetSurface()->GlobalLineIntersect(uv_domain, point, line_vec, NULL,
            // ray_fire, tol, solutions) (SER)
            let mut context = objects.surface_solve_context();
            let solutions = if let Some(context) = context.as_deref_mut() {
                objects
                    .surface(face_surface)?
                    .global_line_intersect_with_context(
                        ts.kernel,
                        Some(face_surface),
                        None,
                        context,
                        &uv_domain,
                        self.point,
                        line_vec,
                        None,
                        ray_fire,
                        tol + face_tol,
                    )?
            } else {
                objects.surface(face_surface)?.global_line_intersect(
                    ts.kernel,
                    face_surface,
                    &uv_domain,
                    self.point,
                    line_vec,
                    None,
                    ray_fire,
                    tol + face_tol,
                )?
            };

            for found in solutions.as_slice() {
                let mut sol = *found;
                let scale = self.point.max_dimension();
                if op == SolverOperation::Rayfire && sol.start.param(0)? < -EFF_ZERO * scale {
                    continue;
                }

                // We've found a result within the surface domain.
                // It must also be within the Face: this is a topology routine.
                // [060718 bd]
                let src_zone_tol = vertex_zone_tol_3d(ts.kernel, objects, self.point_vertex)?;
                let uv = Vec2::new(sol.start.param(1)?, sol.start.param(2)?);
                if face_point_classify(ts.kernel, objects, face, uv, src_zone_tol, true, true)?
                    == POINT_CLASS_UNKNOWN
                {
                    continue;
                }

                // We need to do something here if we have a coincident segment to
                // classify the answer by seeing if the start point is on the
                // surface.  If not then skip it.
                if sol.solution_type == SolutionType::RangeOfValues {
                    let start = self.point + sol.start.param(0)? * ray;
                    if !face_classifies_on_face(ts.kernel, objects, face, start, tol)? {
                        continue;
                    }
                }

                // Handle composite face situation
                if objects.surface_owner(face_surface)? != Some(face) {
                    let start = self.point + sol.start.param(0)? * ray;
                    if !face_classifies_on_face(ts.kernel, objects, face, start, tol)? {
                        continue;
                    }
                } // end composite face check

                // load the solution
                named(&mut sol);
                // Shift the results for rayfire.
                sol.start.solution_value = sol.start.param(0)?;
                *sol.start.param_mut(0)? = sol.start.param(1)?;
                *sol.start.param_mut(1)? = sol.start.param(2)?;

                // add solution to output m_rTopologySolver.m_pSolutions if appropriate
                ts.add_sorted_solution(&sol, SortKey::BySolutionValue, true)?;
            } // end for each solution
        } else if ts.base.projected_operation {
            let opt_vectors = ts.base.opt_vectors.clone();
            let (trim_3d_curves, trim_uv_curves) =
                face_silhouettes(ts.kernel, objects, face, &opt_vectors, tol)?;
            // When we reach here there should be silhouette curves in sTrim3DCurves
            // We can use these to do measurement to the surface.  Please note that
            // the edges bounding the face would have already been measured.
            for (curve, uv_curve) in trim_3d_curves.iter().zip(&trim_uv_curves) {
                let mut sub: LocalSolver<'_, O> =
                    LocalSolver::new(LocalSolverKind::PointCurve, self.swap_order, self.shape);
                sub.curve_owner = Some(face);
                sub.interval = curve.natural_interval();
                sub.surface_curve = Some(uv_curve);
                sub.point = self.point;
                sub.point_vertex = self.point_vertex;
                // (the temporary curve has no handle)
                sub.point_curve(k, ts, curve, None)?;
            }
        } else {
            let best = best_answer_so_far(&ts.base);
            let mut sign = 1.0;
            if self.swap_order && ts.base.category == OperationCategory::Signed {
                // If we are doing a signed operation and the order is reversed then we have to
                // reverse the sign of the result.
                sign = -sign;
            }
            let target = best.map(|b| b * sign);

            let face_surface = objects.face_surface(face)?;
            let uv_domain = objects.face_uv_domain(face)?;
            let face_tol = objects.face_tolerance(face)?;
            // (SER) the surface reaches its face's cache through the caller's
            // context (`smsurf_GetSurfaceCache`), when it provides one
            let mut context = objects.surface_solve_context();
            let solutions = surface_global_point_solve(
                ts.kernel,
                face_surface,
                objects.surface(face_surface)?,
                context.as_deref_mut(),
                &uv_domain,
                op,
                self.point,
                tol + face_tol,
                target,
            )?;

            for found in solutions.as_slice() {
                let mut sol = *found;

                // Check whether the point is within the Face's trimmed boundaries.
                let src_zone_tol = vertex_zone_tol_3d(ts.kernel, objects, self.point_vertex)?;
                let uv = Vec2::new(sol.start.param(0)?, sol.start.param(1)?);
                if face_point_classify(ts.kernel, objects, face, uv, src_zone_tol, true, true)?
                    == POINT_CLASS_UNKNOWN
                {
                    continue;
                }

                // (Note: Surface::GlobalPointSolve() does not put anything
                // into its 'objects' arrays; doesn't have Point ptr anyway.)
                named(&mut sol);
                sol.start.solution_value *= sign;
                if sol.solution_type == SolutionType::RangeOfValues {
                    sol.end.solution_value *= sign;
                }
                ts.add_sorted_solution(&sol, SortKey::BySolutionValue, true)?;
            }
        } // end not a ProjectedOperation branch

        // all done
        Ok(())
    }

    /// `SmTLSCurveEdge::SolveIt`: [`LocalSolverKind::CurveCurve`] with the
    /// second curve's edge as the second curve.
    fn solve_curve_edge(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        let handle = if self.swap_order { object1 } else { object2 };
        let handle = handle.ok_or(SolidError::null_pointer(OP_CE))?;

        // Remove Composites - replaced Edge array with single curve Edge
        let edge = objects.curve_edge(handle)?;

        // when using m_pShape (called frome ShapePointSolve(), ShapeCurveSolve(), or ShapeShapeSolve()) - skip pEdge not in shape
        if self.in_shape(objects, edge) {
            let edge = edge.ok_or(null_deref(OP_CE, "curve without an edge"))?;
            // locals
            self.curve2 = match objects.edge_curve(edge)? {
                Some(h) => Some(CurveRef {
                    curve: objects.curve(h)?,
                    handle: h,
                }),
                None => None,
            };
            self.interval2 = objects.edge_interval(edge)?;
            self.curve_edge2 = Some(edge);

            // Solve the Curve/Curve problem
            self.solve_curve_curve(k, ts)?;
        } // end iter all cureve->edges

        Ok(())
    }

    /// `SmTLSCurveCurve::SolveIt`: intersections (`SmCurve::GlobalCurveIntersect`)
    /// or other solves (`SmCurve::GlobalCurveSolve`) of the two curves over their
    /// intervals, named by their edges, faces (UV points replacing the parameters)
    /// or the curves themselves.
    pub(super) fn solve_curve_curve(
        &self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, '_, O>,
    ) -> Result<()> {
        let objects = ts.objects;
        let mut sol_objects: [Option<O>; MAX_TREES] = [None; MAX_TREES];
        let mut count = 0;
        let mut uv_curve1: Option<&Curve> = None;
        let mut uv_curve2: Option<&Curve> = None;

        let first = (
            self.curve_edge,
            self.curve_face,
            self.curve.map(|c| c.handle),
        );
        let second = (
            self.curve_edge2,
            self.curve_face2,
            self.curve2.map(|c| c.handle),
        );
        if self.swap_order {
            match second {
                (Some(edge), _, _) => push_object(&mut sol_objects, &mut count, Some(edge)),
                (None, Some(face), _) => {
                    push_object(&mut sol_objects, &mut count, Some(face));
                    uv_curve1 = self.surface_curve2;
                }
                (None, None, curve) => push_object(&mut sol_objects, &mut count, curve),
            }
            match first {
                (Some(edge), _, _) => push_object(&mut sol_objects, &mut count, Some(edge)),
                (None, Some(face), _) => {
                    push_object(&mut sol_objects, &mut count, Some(face));
                    uv_curve2 = self.surface_curve;
                }
                (None, None, curve) => push_object(&mut sol_objects, &mut count, curve),
            }
        } else {
            match first {
                (Some(edge), _, _) => push_object(&mut sol_objects, &mut count, Some(edge)),
                (None, Some(face), _) => {
                    push_object(&mut sol_objects, &mut count, Some(face));
                    uv_curve1 = self.surface_curve;
                }
                (None, None, curve) => push_object(&mut sol_objects, &mut count, curve),
            }
            match second {
                (Some(edge), _, _) => push_object(&mut sol_objects, &mut count, Some(edge)),
                (None, Some(face), _) => {
                    push_object(&mut sol_objects, &mut count, Some(face));
                    uv_curve2 = self.surface_curve2;
                }
                (None, None, curve) => push_object(&mut sol_objects, &mut count, curve),
            }
        }

        let s = &ts.base;
        let op = s.operation;
        let best = best_answer_so_far(s);
        let mut sign = 1.0;
        if self.swap_order && s.category == OperationCategory::Signed {
            // If we are doing a signed operation and the order is reversed then we have to
            // reverse the sign of the result.
            sign = -sign;
        }
        let target = best.map(|b| b * sign);

        let curve1 = self.curve.ok_or(null_deref(OP_CC, "null first curve"))?;
        let curve2 = self.curve2.ok_or(null_deref(OP_CC, "null second curve"))?;
        let (this, this_interval, other, other_interval) = if self.swap_order {
            (curve2, &self.interval2, curve1, &self.interval)
        } else {
            (curve1, &self.interval, curve2, &self.interval2)
        };
        // a temporary curve's solve names the curve objects (their natural intervals),
        // not the face standing in for it (`temporary_curves`)
        let temporary = self.temporary_curves;
        let solutions = if op == SolverOperation::Intersect {
            this.curve.global_curve_intersect(
                ts.kernel,
                this_interval,
                other.curve,
                other_interval,
                s.tolerance_3d,
                &CurveIntersectOptions {
                    skip_global_coincidence_check: false,
                    handles: if temporary {
                        [None, None]
                    } else {
                        [Some(this.handle), Some(other.handle)]
                    },
                    analytic_handles: if temporary {
                        [None, None]
                    } else {
                        [
                            objects.analytic_curve_handle(this.handle),
                            objects.analytic_curve_handle(other.handle),
                        ]
                    },
                },
            )?
        } else {
            curve_global_curve_solve(
                ts.kernel,
                this,
                this_interval,
                other,
                other_interval,
                temporary,
                &CurveSolveArgs {
                    operation: op,
                    tolerance_3d: s.tolerance_3d,
                    best_answer: target,
                    opt_vectors: if s.opt_vectors.is_empty() {
                        None
                    } else {
                        Some(&s.opt_vectors)
                    },
                    requested: SolutionRequested::All,
                },
            )?
        };

        for found in solutions.as_slice() {
            let mut sol = *found;
            let t1 = sol.start.param(0)?;
            let t2 = sol.start.param(1)?;
            sol.objects[0] = sol_objects[0];
            sol.objects[1] = sol_objects[1];
            sol.num_objects = count;
            sol.num_variables = 0;
            for (uv_curve, t) in [(uv_curve1, t1), (uv_curve2, t2)] {
                if let Some(uv_curve) = uv_curve {
                    let pnt = uv_curve.evaluate_point(k, t)?;
                    *sol.start.param_mut(sol.num_variables)? = pnt.x;
                    sol.num_variables += 1;
                    *sol.start.param_mut(sol.num_variables)? = pnt.y;
                    sol.num_variables += 1;
                } else {
                    *sol.start.param_mut(sol.num_variables)? = t;
                    sol.num_variables += 1;
                }
            }
            sol.start.solution_value *= sign;
            if sol.solution_type == SolutionType::RangeOfValues {
                sol.end.solution_value *= sign;
            }
            ts.add_sorted_solution(&sol, SortKey::BySolutionValue, true)?;
        }

        Ok(())
    }

    /// `SmTLSEdgeEdge::SolveIt`: [`LocalSolverKind::CurveEdge`] with the first
    /// curve's edge (and its curve and interval) as the first curve.
    fn solve_edge_edge(
        &mut self,
        k: &Kernel,
        ts: &mut TopologySolver<'_, 'a, O>,
        object1: Option<O>,
        object2: Option<O>,
    ) -> Result<()> {
        let objects = ts.objects;
        let handle = if self.swap_order { object2 } else { object1 };
        let handle = handle.ok_or(null_deref(OP_EE, "null curve object"))?;

        // Remove Composites - replaced Edge array with single curve Edge
        let edge = objects.curve_edge(handle)?;

        // when using m_pShape (called frome ShapePointSolve(), ShapeCurveSolve(), or ShapeShapeSolve()) - skip pEdge not in shape
        if self.in_shape(objects, edge) {
            let edge = edge.ok_or(null_deref(OP_EE, "curve without an edge"))?;
            self.curve_edge = Some(edge);
            self.interval = objects.edge_interval(edge)?;
            self.curve = match objects.edge_curve(edge)? {
                Some(h) => Some(CurveRef {
                    curve: objects.curve(h)?,
                    handle: h,
                }),
                None => None,
            };

            self.solve_curve_edge(k, ts, object1, object2)?;
        }

        Ok(())
    }
}

/// `sm_GetFaceSilhouettes`: the face's trimmed silhouette curves for the view
/// vector `opt_vectors[0]` (3D curves and their UV trim curves), taken from the
/// surface cache's alternate caches or made and cached there (silhouette curves of
/// the surface to ten times `tolerance_3d` and 20 degrees, trimmed by the face).
/// The surface cache is the one the caller's
/// [`TopologyObjects::surface_solve_context`] makes for the face's surface (else the
/// per-call [`surface_cache`]); either is this call's copy, so the alternate caches
/// it searches first hold only what this call adds. The trimming works on the face
/// [`TopologyObjects::face_entity`] resolves, which the objects lend read-only, so it
/// classifies on a copy of that B-rep and drops the lazy edits upstream's `const`
/// `CreateCurvesByTrimming` leaves on it; an adapter that resolves none returns
/// [`SolidError::Unported`] for `SmFace::CreateCurvesByTrimming`. A trimmed UV curve
/// that is not a B-spline (upstream caches it as NULL and `GetCopyOfCurves` fails
/// on it) is `SM_ERR_NULL_POINTER`.
pub(super) fn face_silhouettes<O: SolutionObject>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    face: O,
    opt_vectors: &[Vec3],
    tolerance_3d: f64,
) -> Result<(Vec<Curve>, Vec<Curve>)> {
    // locals and temporaries
    let surface = objects.face_surface(face)?;
    let srf = objects.surface(surface)?;
    let mut cache = match objects.surface_solve_context() {
        Some(mut ctx) => ctx.build_surface_cache(k, srf, Some(surface))?,
        None => surface_cache(k, srf, Some(surface))?,
    };
    let view = *opt_vectors
        .first()
        .ok_or(null_deref(OP_FACE_SIL, "no view vector"))?;

    let approx_tol = tolerance_3d * 10.0;
    let angle_tol = 20.0 * PI / 180.0;
    let key = AltCacheKey {
        vec: Some(view),
        boolean: Some(false),
        ..AltCacheKey::default()
    };

    // when silhouette curves to these tolerances have been made - get copies
    if let Some(copies) = cache.has_cached_curves(
        AltSrfCacheType::TrimmedSilhouette,
        approx_tol,
        angle_tol,
        &key,
        true,
        true,
    )? {
        return Ok((
            copies.curves_3d.unwrap_or_default(),
            copies.curves_uv.unwrap_or_default(),
        ));
    }

    // No cached trimmed silhouette curves in existance -- create them
    // (SM_CAST_PTR(SmBSplineSurface, pSurface): every kind derived from it; upstream
    // dereferences NULL for any other kind)
    if !srf.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
        return Err(null_deref(
            OP_FACE_SIL,
            "face surface is not a B-spline surface",
        ));
    }

    // Create the planar section curves from the brep and the plane
    let (curves_3d, curves_uv) = srf.create_silhouette_curves_with_cache(
        k,
        &mut cache,
        &objects.face_uv_domain(face)?,
        view,
        false,
        Some(approx_tol),
        Some(angle_tol),
    )?;

    // Now trim the curves.
    let Some((b, f)) = objects.face_entity(face) else {
        // DISPATCH GAP: SmFace::CreateCurvesByTrimming is ported (`topology::face::p2`),
        // but this adapter cannot resolve the face handle to its `Brep` and `FaceId`.
        return Err(SolidError::unported(
            "SmFace::CreateCurvesByTrimming",
            "topology.face_p2",
        ));
    };
    let curves_3d: Vec<Option<Curve>> = curves_3d.into_iter().map(Some).collect();
    let curves_uv: Vec<Option<Curve>> = curves_uv.into_iter().map(Some).collect();
    let trimmed = create_curves_by_trimming_snapshot(
        k,
        b,
        f,
        Some(approx_tol),
        Some(&curves_3d),
        Some(&curves_uv),
        true,
        true,
    )?;
    let trim_3d = trimmed.curves_3d.unwrap_or_default();
    // a NULL UV entry (an interval of a non-B-spline UV curve) has no Rust cache slot;
    // upstream's later GetCopyOfCurves stops at it with SM_ERR_NULL_POINTER
    let trim_uv = trimmed
        .uv_curves
        .unwrap_or_default()
        .into_iter()
        .collect::<Option<Vec<Curve>>>()
        .ok_or(SolidError::null_pointer(OP_FACE_SIL))?;

    // Register trimmed curves and create copy by getting them again
    cache.add_curves_to_cache(
        AltSrfCacheType::TrimmedSilhouette,
        approx_tol,
        angle_tol,
        &key,
        Some(trim_3d),
        Some(trim_uv),
    )?;

    // Don't clean up here we just use the curves that were created
    match cache.has_cached_curves(
        AltSrfCacheType::TrimmedSilhouette,
        approx_tol,
        angle_tol,
        &key,
        true,
        true,
    )? {
        Some(copies) => Ok((
            copies.curves_3d.unwrap_or_default(),
            copies.curves_uv.unwrap_or_default(),
        )),
        None => Err(SolidError::sm_err(OP_FACE_SIL)),
    }
}

/// `pFace->GetSurface()->GlobalPointSolve(uv_domain, op, point, tol,
/// pdBestAnswer, SM_SR_ALL, solutions)`. `context` is the caller's owning-face
/// service: the fast drop's trim test reaches the face's cache and classifies
/// through it.
#[allow(clippy::too_many_arguments)]
fn surface_global_point_solve<O: SolutionObject>(
    kernel: &Kernel,
    handle: O,
    surface: &Surface,
    context: Option<&mut (dyn crate::surface::surface::p1::SurfaceSolveContext<O> + '_)>,
    uv_domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    tol: f64,
    best_answer: Option<f64>,
) -> Result<SolutionArray<O>> {
    let Some(context) = context else {
        return surface.global_point_solve::<O>(
            kernel,
            Some(handle),
            uv_domain,
            op,
            point,
            tol,
            best_answer,
            SolutionRequested::All,
        );
    };
    let mut solutions = SolutionArray::new();
    surface.global_point_solve_into_with_context(
        kernel,
        Some(handle),
        None,
        context,
        uv_domain,
        op,
        point,
        tol,
        best_answer,
        SolutionRequested::All,
        &mut solutions,
    )?;
    Ok(solutions)
}

/// The arguments of `SmCurve::GlobalCurveSolve` after the two curves and their
/// intervals.
struct CurveSolveArgs<'v> {
    /// `eSolverOperation` (`m_eSolverOperation`).
    operation: SolverOperation,
    /// `d3dTolerance` (`m_d3dTolerance`).
    tolerance_3d: f64,
    /// `pdBestAnswer` (`NULL` is `None`), already multiplied by the swap sign.
    best_answer: Option<f64>,
    /// `cpOptVectors` (`m_cpOptVectors`, `NULL` is `None`).
    opt_vectors: Option<&'v [Vec3]>,
    /// `eSolutionRequested` (always `SM_SR_ALL` here).
    requested: SolutionRequested,
}

/// `this->GlobalCurveSolve(this_interval, *other, other_interval, op, tol,
/// pdBestAnswer, cpOptVectors, SM_SR_ALL, solutions)`.
fn curve_global_curve_solve<O: SolutionObject>(
    k: &Kernel,
    this: CurveRef<'_, O>,
    this_interval: &Extent1d,
    other: CurveRef<'_, O>,
    other_interval: &Extent1d,
    temporary: bool,
    args: &CurveSolveArgs<'_>,
) -> Result<SolutionArray<O>> {
    this.curve.global_curve_solve(
        k,
        this_interval,
        other.curve,
        other_interval,
        args.operation,
        args.tolerance_3d,
        &CurveSolveOptions {
            target_distance: args.best_answer,
            opt_vectors: args.opt_vectors,
            handles: if temporary {
                [None, None]
            } else {
                [Some(this.handle), Some(other.handle)]
            },
        },
        args.requested,
    )
}

/// `SmTol::GetZoneTol3d(m_pPointVertex)` (`SM_USE_OLDTOL`): the system zone
/// tolerance without a vertex, the vertex's `GetTolerance` otherwise.
pub(super) fn vertex_zone_tol_3d<O>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    vertex: Option<O>,
) -> Result<f64> {
    match vertex {
        Some(vertex) => objects.vertex_tolerance(vertex),
        None => Ok(zone_tol_3d(k, SmallTopoType::NormalSize)),
    }
}

/// `SmTol::GetZoneTol3d(m_pCurve)` (`SM_USE_OLDTOL`): the tolerance of the curve's
/// edge (`SmCurve::GetEdge`), the system zone tolerance without a curve or for a
/// curve no edge owns.
pub(super) fn curve_zone_tol_3d<O>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    curve: Option<O>,
) -> Result<f64> {
    let edge = match curve {
        Some(curve) => objects.curve_edge(curve)?,
        None => None,
    };
    match edge {
        Some(edge) => objects.edge_tolerance(edge),
        None => Ok(zone_tol_3d(k, SmallTopoType::NormalSize)),
    }
}

/// `pFace->PointClassify(uv, src_zone_tol, do_boundaries, use_uv, point_class)`
/// with a fresh `SmPointClassification(src_zone_tol, context)`: the point class
/// code (`SM_PC_*`).
///
/// Every caller (the `SmTLSPointFace::SolveIt` and `SmTLSCurveFace::SolveIt` paths)
/// ignores `PointClassify`'s status and reads only `GetPointClass()`, so a failing
/// `PointClassify` still returns `Ok` with the class the classification holds
/// afterwards (`SM_PC_UNKNOWN` when it set none). `Err` is only for
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (harness-level failures
/// upstream cannot have); the callers propagate it with `?`.
///
/// The solver only reads its objects, so this is the read-only
/// [`point_classify_prepared`]: a classification that would have to create a UV trim
/// curve or build a trim-cache tree first is [`SolidError::Unported`]. An object view
/// may also record the visited edge use for its mutable caller to prepare. No trimmed
/// surface cache is passed (the solver's objects expose none), so upstream's cache
/// fast path is skipped and the loops classify the point.
pub(super) fn face_point_classify<O: Copy>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    face: O,
    uv: Vec2,
    src_zone_tol: f64,
    do_boundaries: bool,
    use_uv: bool,
) -> Result<i32> {
    if let Some(result) =
        objects.classify_face_point(k, face, uv, src_zone_tol, do_boundaries, use_uv)
    {
        return result;
    }
    let Some((b, f)) = objects.face_entity(face) else {
        // DISPATCH GAP: SmFace::PointClassify is ported (`query::face::p2`, query.face_p2
        // partial; no UNPORTED marker), but this adapter cannot resolve the face handle
        // to its `Brep` and `FaceId`.
        return Err(SolidError::unported(
            "SmFace::PointClassify",
            "query.face_p2",
        ));
    };
    let mut pc = PointClassification::new(src_zone_tol);
    let options = PointClassifyOptions {
        src_zone_tol_3d: src_zone_tol,
        do_boundary_intersections: do_boundaries,
        use_uv_classification: use_uv,
        ..PointClassifyOptions::default()
    };
    let result = match objects.uv_trim_preparation() {
        Some(preparation) => point_classify_requested(k, b, f, uv, &mut pc, options, preparation),
        None => point_classify_prepared(k, b, f, uv, &mut pc, options),
    };
    match result {
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::PreparationRequired { .. }),
        ) => Err(e),
        // (status ignored, as upstream)
        Ok(()) | Err(_) => Ok(pc.point_class().code()),
    }
}

/// The `Point3DClassify` test of `SmTLSPointFace::SolveIt`: classify `point`
/// against the face within `tol` (boundary intersections on), retrying within
/// `100 * tol` when that fails; `false` when both fail or the class is not
/// `SM_PC_FACE`.
///
/// `SmFace::Point3DClassify` is [`crate::query::face::p1::point_3d_classify`] on a
/// copy of the B-rep [`TopologyObjects::face_entity`] names (the solver holds its
/// objects shared; upstream's `const` member may add UV trim curves and a trimmed
/// surface cache to the B-rep, which the copy does not keep). Both tries classify on
/// the same copy, as upstream's on the same face.
fn face_classifies_on_face<O: Copy>(
    k: &Kernel,
    objects: &dyn TopologyObjects<O>,
    face: O,
    point: Point3,
    tol: f64,
) -> Result<bool> {
    use crate::error::EntityRef;
    use crate::intersect::point_class::PointClassType;
    use crate::query::face::p1::{point_3d_classify, Point3dClassifyOptions};
    if let Some(result) = objects.classifies_on_face(k, face, point, tol) {
        return result;
    }
    let Some((b, f)) = objects.face_entity(face) else {
        // DISPATCH GAP: this adapter cannot resolve the face handle to its `Brep` and
        // `FaceId` (`TopologyObjects::face_entity`).
        return Err(SolidError::unported(
            "SmFace::Point3DClassify",
            "query.face_p1",
        ));
    };
    let mut copy = b.clone();
    let mut pc = PointClassification::<EntityRef>::new(tol);
    let mut classify = |tol: f64, pc: &mut PointClassification<EntityRef>| -> Result<bool> {
        match point_3d_classify(
            k,
            &mut copy,
            f,
            point,
            tol,
            true,
            pc,
            Point3dClassifyOptions::default(),
        ) {
            Ok(()) => Ok(true),
            Err(e @ SolidError::Unported { .. }) => Err(e),
            Err(_) => Ok(false),
        }
    };
    if !classify(tol, &mut pc)? && !classify(tol * 100.0, &mut pc)? {
        return Ok(false);
    }
    Ok(pc.point_class() == PointClassType::Face)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::geom::solution_array::ObjectKind;

    /// Free-standing test objects: vertices `(point, tolerance)` and curves.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) enum Obj {
        Vertex(usize),
        Curve(usize, Extent1d),
        Edge(usize),
        Analytic(usize),
        Face(usize),
        Surface(usize),
    }

    impl SolutionObject for Obj {
        fn kind(&self) -> ObjectKind {
            match self {
                Obj::Vertex(_) => ObjectKind::Vertex,
                Obj::Curve(..) | Obj::Analytic(_) => ObjectKind::Curve,
                Obj::Edge(_) => ObjectKind::Edge,
                Obj::Face(_) => ObjectKind::Face,
                Obj::Surface(_) => ObjectKind::Surface,
            }
        }

        fn natural_interval(&self) -> Option<Extent1d> {
            match self {
                Obj::Curve(_, i) => Some(*i),
                _ => None,
            }
        }
    }

    pub(crate) struct Scene {
        vertices: Vec<(Point3, f64)>,
        curves: Vec<Curve>,
        edges: Vec<(usize, Extent1d)>,
    }

    impl TopologyObjects<Obj> for Scene {
        fn shape_member_object(&self, member: ShapeMember) -> Result<Obj> {
            match member {
                ShapeMember::Vertex((_, key)) => Ok(Obj::Vertex(key.index() as usize)),
                ShapeMember::Edge((_, key)) => Ok(Obj::Edge(key.index() as usize)),
                ShapeMember::Face((_, key)) => Ok(Obj::Face(key.index() as usize)),
                ShapeMember::Other => {
                    Err(SolidError::invalid_input("test mapping", "not a member"))
                }
            }
        }
        fn curve(&self, object: Obj) -> Result<&Curve> {
            match object {
                Obj::Curve(i, _) => self.curves.get(i).ok_or(SolidError::sm_err("curve")),
                _ => Err(SolidError::sm_err("curve")),
            }
        }
        fn analytic_curve_handle(&self, curve: Obj) -> Option<Obj> {
            match curve {
                Obj::Curve(i, _) => Some(Obj::Analytic(i)),
                _ => None,
            }
        }
        fn vertex_point(&self, vertex: Obj) -> Result<Point3> {
            match vertex {
                Obj::Vertex(i) => Ok(self.vertices[i].0),
                _ => Err(SolidError::sm_err("vertex")),
            }
        }
        fn vertex_tolerance(&self, vertex: Obj) -> Result<f64> {
            match vertex {
                Obj::Vertex(i) => Ok(self.vertices[i].1),
                _ => Err(SolidError::sm_err("vertex")),
            }
        }
        fn curve_edge(&self, curve: Obj) -> Result<Option<Obj>> {
            Ok(match curve {
                Obj::Curve(i, _) => self.edges.iter().position(|e| e.0 == i).map(Obj::Edge),
                _ => None,
            })
        }
        fn edge_curve(&self, edge: Obj) -> Result<Option<Obj>> {
            match edge {
                Obj::Edge(i) => {
                    let c = self.edges[i].0;
                    Ok(Some(Obj::Curve(c, self.curves[c].natural_interval())))
                }
                _ => Ok(None),
            }
        }
        fn edge_interval(&self, edge: Obj) -> Result<Extent1d> {
            match edge {
                Obj::Edge(i) => Ok(self.edges[i].1),
                _ => Err(SolidError::sm_err("edge")),
            }
        }
        fn edge_tolerance(&self, edge: Obj) -> Result<f64> {
            // SmEdge's default tolerance (the point/edge solve reads it for the cache)
            match edge {
                Obj::Edge(_) => Ok(0.0),
                _ => Err(SolidError::sm_err("edge")),
            }
        }
        fn surface_face(&self, _surface: Obj) -> Result<Option<Obj>> {
            Ok(None)
        }
        fn face_surface(&self, face: Obj) -> Result<Obj> {
            match face {
                Obj::Face(index) => Ok(Obj::Surface(index)),
                _ => Err(SolidError::sm_err("face")),
            }
        }
        fn surface(&self, _surface: Obj) -> Result<&Surface> {
            Err(SolidError::sm_err("surface"))
        }
        fn face_uv_domain(&self, _face: Obj) -> Result<Extent2d> {
            Err(SolidError::sm_err("face"))
        }
        fn face_tolerance(&self, _face: Obj) -> Result<f64> {
            Err(SolidError::sm_err("face"))
        }
        fn surface_owner(&self, _surface: Obj) -> Result<Option<Obj>> {
            Ok(None)
        }
        fn shape_member(&self, _object: Obj) -> ShapeMember {
            ShapeMember::Other
        }
    }

    pub(crate) fn scene() -> Scene {
        let line = Line::bounded(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &Extent1d::from_min_max(0.0, 10.0),
            &BoundedLineOptions::default(),
        );
        Scene {
            vertices: vec![
                (Vec3::new(3.0, 4.0, 0.0), 0.0),
                (Vec3::new(0.0, 0.0, 0.0), 0.0),
            ],
            curves: vec![Curve::from(line)],
            edges: vec![(0, Extent1d::from_min_max(2.0, 8.0))],
        }
    }

    #[test]
    fn shape_topology_keys_resolve_to_the_stored_geometry() {
        let objects = scene();
        let brep = crate::topology::BrepId::from_index(0);
        let edge = ShapeMember::Edge((brep, crate::topology::EdgeId::from_index(0)));
        let face = ShapeMember::Face((brep, crate::topology::FaceId::from_index(0)));
        let vertex = ShapeMember::Vertex((brep, crate::topology::VertexId::from_index(0)));
        assert_eq!(objects.shape_member_object(edge).unwrap(), Obj::Edge(0));
        assert_eq!(
            objects.shape_geometry_object(edge).unwrap(),
            Obj::Curve(0, Extent1d::from_min_max(0.0, 10.0))
        );
        assert_eq!(objects.shape_member_object(face).unwrap(), Obj::Face(0));
        assert_eq!(
            objects.shape_geometry_object(face).unwrap(),
            Obj::Surface(0)
        );
        assert_eq!(
            objects.shape_geometry_object(vertex).unwrap(),
            Obj::Vertex(0)
        );
    }

    #[test]
    fn constructor_sets_category_and_projection() {
        let k = Kernel::default();
        let sc = scene();
        let ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::DirectedMaximize,
            SolutionRequested::All,
            1e-6,
            BIG_DOUBLE,
            &[],
        );
        assert_eq!(ts.base.category, OperationCategory::Maximize);
        assert!(ts.base.projected_operation);
        let ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Rayfire,
            SolutionRequested::All,
            1e-6,
            BIG_DOUBLE,
            &[],
        );
        assert_eq!(ts.base.category, OperationCategory::Other);
        assert!(!ts.base.projected_operation);
        assert_eq!(ts.best_answer_so_far(), None);
    }

    #[test]
    fn best_answer_for_signed_is_signed_root() {
        let k = Kernel::default();
        let sc = scene();
        let ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Signed3dDirectedMinimize,
            SolutionRequested::All,
            1e-6,
            -4.0,
            &[Vec3::new(0.0, 0.0, 1.0)],
        );
        assert_eq!(ts.best_answer_so_far(), Some(-2.0));
    }

    #[test]
    fn may_contain_answer_culls_far_boxes() {
        let k = Kernel::default();
        let sc = scene();
        let ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Intersect,
            SolutionRequested::All,
            0.1,
            BIG_DOUBLE,
            &[],
        );
        let a = Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
        let b = Extent3d::from_bounds(1.05, 0.0, 0.0, 2.0, 1.0, 1.0);
        let c = Extent3d::from_bounds(1.2, 0.0, 0.0, 2.0, 1.0, 1.0);
        assert!(ts.may_contain_answer(&a, &b).unwrap());
        assert!(!ts.may_contain_answer(&a, &c).unwrap());
    }

    #[test]
    fn point_vertex_minimize_adds_distance() {
        let k = Kernel::default();
        let sc = scene();
        let mut ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Minimize,
            SolutionRequested::All,
            1e-6,
            BIG_DOUBLE,
            &[],
        );
        let mut ls = LocalSolver::new(LocalSolverKind::PointVertex, false, None);
        ls.set_point(Vec3::new(0.0, 0.0, 0.0));
        ls.solve_it(&k, &mut ts, None, Some(Obj::Vertex(0)))
            .unwrap();
        assert_eq!(ts.base.solutions.len(), 1);
        let s = ts.base.solutions.get(0).unwrap();
        assert_eq!(s.start.solution_value, 5.0);
        assert_eq!(s.num_objects, 1);
        assert_eq!(s.objects[0], Some(Obj::Vertex(0)));
    }

    #[test]
    fn abstract_solver_and_null_face_inputs_fail() {
        let k = Kernel::default();
        let sc = scene();
        let mut ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Minimize,
            SolutionRequested::All,
            1e-6,
            BIG_DOUBLE,
            &[],
        );
        let mut ls = LocalSolver::new(LocalSolverKind::Curve, false, None);
        assert!(ls.solve_it(&k, &mut ts, None, None).is_err());
        let mut ls = LocalSolver::new(LocalSolverKind::FaceFace, false, None);
        assert!(matches!(
            ls.solve_it(&k, &mut ts, None, None),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn face_point_classify_reads_the_face_the_adapter_names() {
        use crate::geom::object::ObjectRef;
        use crate::surface::trim_srf_cache::{POINT_CLASS_FACE, POINT_CLASS_VERTEX};
        use crate::topology::brep_cache::{arena_identity, BrepObjects, CacheObject};
        use crate::topology::{Brep, BrepId, EdgeuseId};
        let k = Kernel::default();
        let gap = |r: Result<i32>| {
            matches!(
                r,
                Err(SolidError::Unported {
                    unit: "query.face_p2",
                    ..
                })
            )
        };
        // an adapter without B-rep faces
        let sc = scene();
        let uv = Vec2::new(0.25, 0.5);
        assert!(gap(face_point_classify(
            &k,
            &sc,
            Obj::Face(0),
            uv,
            1e-6,
            true,
            true
        )));

        let owner_of = |_: ObjectRef| None;
        let mut b = Brep::new();
        let f = crate::query::face::p2::tests::square(&mut b);
        let objects = BrepObjects {
            preparation: None,
            brep: &b,
            brep_id: BrepId::from_index(0),
            cache: None,
            keys: &arena_identity,
            owner_of: &owner_of,
        };
        let classify = |face, uv| face_point_classify(&k, &objects, face, uv, 1e-6, true, true);
        let face = CacheObject::Face(f);
        assert_eq!(classify(face, uv).unwrap(), POINT_CLASS_FACE);
        assert_eq!(
            classify(face, Vec2::new(1.0, 1.0)).unwrap(),
            POINT_CLASS_VERTEX
        );
        assert_eq!(
            classify(face, Vec2::new(2.0, 0.5)).unwrap(),
            POINT_CLASS_UNKNOWN
        );
        // a handle the adapter does not resolve to a face
        assert!(gap(classify(CacheObject::Surface(f), uv)));

        // the read-only classification cannot create a missing UV trim
        let mut b = Brep::new();
        let f = crate::query::face::p2::tests::square(&mut b);
        for i in 0..4 {
            b.edgeuse_mut(EdgeuseId::from_index(i))
                .unwrap()
                .uv_trim_curve = None;
        }
        let objects = BrepObjects {
            preparation: None,
            brep: &b,
            ..objects
        };
        assert!(matches!(
            face_point_classify(&k, &objects, CacheObject::Face(f), uv, 1e-6, true, true),
            Err(SolidError::Unported { .. })
        ));
    }

    #[test]
    fn point_edge_names_the_edge() {
        let k = Kernel::default();
        let sc = scene();
        let mut ts: TopologySolver<'_, '_, Obj> = TopologySolver::new(
            &k,
            &sc,
            SolverOperation::Minimize,
            SolutionRequested::All,
            1e-6,
            BIG_DOUBLE,
            &[],
        );
        let mut ls = LocalSolver::new(LocalSolverKind::VertexEdge, false, None);
        let curve = Obj::Curve(0, sc.curves[0].natural_interval());
        ls.solve_it(&k, &mut ts, Some(Obj::Vertex(0)), Some(curve))
            .unwrap();
        let s = ts.base.solutions.get(0).unwrap();
        assert_eq!(s.objects[0], Some(Obj::Vertex(0)));
        assert_eq!(s.objects[1], Some(Obj::Edge(0)));
        assert!((s.start.solution_value - 4.0).abs() < 1e-12);
        assert!((s.start.parameters[0] - 3.0).abs() < 1e-12);
    }
}

//! The global solver: a branch-and-bound traversal of one to three subdivision
//! trees that culls pairs of nodes by their boxes and hands the surviving leaf
//! combinations to a local solver, plus the sorted, de-duplicating and
//! range-combining solution accumulator shared by every derived solver.
//!
//! Upstream: `source/SMLib/src/SmGlobalSolver.cpp`, `source/SMLib/inc/SmGlobalSolver.h`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`MAX_SOLVER_TREES`]                         | `SM_GS_MAX_TREES`                                |
//! | [`SolverOperation`]                          | `SmSolverOperationType`                          |
//! | [`OperationCategory`]                        | `SmOperationCategory`                            |
//! | [`SolutionRequested`]                        | `SmSolutionRequestedType`                        |
//! | [`SolutionPairType`], [`PreferredSolution`]  | `SmSolutionPairType`, `IdenticalSolutions`' `pPreferredSol` |
//! | [`Branch`]                                   | `SmTreeNode* apBranch[SM_GS_MAX_TREES]`          |
//! | [`GlobalSolver`], [`GlobalSolver::new`]      | `SmGlobalSolver`, `SmGlobalSolver()`             |
//! | [`GlobalSolver::with_tree`]                  | `SmGlobalSolver(SmTree*, ULONG)`                 |
//! | [`GlobalSolver::with_trees`]                 | `SmGlobalSolver(SmTree*, SmTree*, ULONG)`        |
//! | `Drop` (implicit)                            | `~SmGlobalSolver`                                |
//! | [`GlobalSolver::solver_operation`]           | `GetSolverOperationType`                         |
//! | [`GlobalSolver::opt_vector`]                 | `GetOptVector`                                   |
//! | [`GlobalSolver::set_solver_operation`] .. [`GlobalSolver::set_at_distance`] | `SetSolverOperation` .. `SetAtDistance` |
//! | [`GlobalSolve`]                              | the virtual interface of `SmGlobalSolver`        |
//! | [`GlobalSolve::solve_it`], [`SolveRequest`]  | `SmGlobalSolver::SolveIt` (and its arguments)    |
//! | [`GlobalSolve::solve_branch`], [`solve_branch`] | `SmGlobalSolver::SolveBranch`                 |
//! | [`GlobalSolve::branch_may_contain_answers`], [`GlobalSolver::branch_may_contain_answers`] | `SmGlobalSolver::BranchMayContainAnswers` |
//! | [`GlobalSolve::are_ready_for_local_solve`], [`GlobalSolver::are_ready_for_local_solve`] | `SmGlobalSolver::AreReadyForLocalSolve` |
//! | `find_largest_non_leaf` (private)            | `sm_FindLargestNonLeaf`                          |
//! | [`GlobalSolve::find_best_subdivision_node`], [`GlobalSolver::find_best_subdivision_node`] | `SmGlobalSolver::FindBestSubdivisionNode` |
//! | [`GlobalSolve::subdivide`], [`subdivide`]    | `SmGlobalSolver::Subdivide`                      |
//! | [`GlobalSolve::local_solve`], [`GlobalSolver::local_solve`] | `SmGlobalSolver::LocalSolve`      |
//! | `combine_min_max` (private)                  | `sm_CombineMinMax`                               |
//! | [`GlobalSolve::add_sorted_solution`]         | `SmGlobalSolver::AddSortedSolution`              |
//! | [`GlobalSolve::identical_solutions`], [`GlobalSolver::identical_solutions`] | `SmGlobalSolver::IdenticalSolutions` |
//! | [`GlobalSolver::identical_solutions_in`]     | `SmGlobalSolver::IdenticalSolutions` (object domains read through a caller's resolver) |
//!
//! Deviations:
//! - **Virtual methods.** Derived solvers (curve/curve, surface/surface, ...)
//!   implement [`GlobalSolve`] and override its hooks; the base behaviour lives
//!   in inherent methods of [`GlobalSolver`] (and [`subdivide`]) so an override
//!   can still call "`SmGlobalSolver::X`". `SolveIt`, `SolveBranch` and
//!   `AddSortedSolution` are provided trait methods that dispatch to the hooks.
//! - **Trees and nodes.** The trees are borrowed [`Tree`]s; a branch holds one
//!   [`NodeId`] per tree (`None` = null pointer). Paths where upstream
//!   dereferences a null tree, top node, child or branch slot (a tree without a
//!   top node, a node with only one child, `SM_SO_RAYFIRE` subdividing a leaf
//!   second tree, two-tree operations run on one tree, more than
//!   [`MAX_SOLVER_TREES`] trees) or reads past the optional vectors return
//!   [`SolidError::InvalidInput`].
//! - **Solutions.** The solver owns its solution array
//!   ([`GlobalSolver::solutions`], upstream the `m_pSolutions` pointer).
//!   [`GlobalSolve::solve_it`] swaps the caller's array in for the duration of
//!   the solve and back out afterwards (also on error), so solutions added
//!   after `SolveIt` returns go to the solver's own array instead of the
//!   caller's.
//! - **Optional vectors** are copied into the solver (upstream keeps the
//!   caller's pointer).
//! - **Errors.** `AddSortedSolution` returns `Result` because the solution
//!   queries it makes can fail (counts past the fixed arrays, calls into
//!   unported geometry). `FindBestSubdivisionNode`'s `SM_ERR` is
//!   [`SolidError::Upstream`] with code `1001`.
//! - **Recursion bound.** [`GlobalSolve::solve_it`] first checks the trees
//!   ([`GlobalSolver::check_solve_depth`]): cyclic child links (unbounded
//!   recursion upstream) or trees whose heights add up to more than
//!   [`MAX_SOLVE_DEPTH`] (1000 levels) return [`SolidError::InvalidInput`], so
//!   the `SolveBranch` recursion depth is bounded.
//! - **Work bound.** Upstream visits every branch whose boxes cannot be culled;
//!   boxes of overflowing geometry (`1e300` control points or weights) cull
//!   nothing, so the visits grow with the product of the tree sizes (fuzz
//!   `curve.curve_p9` fuzz01043 ran for minutes). [`solve_branch`] counts its
//!   visits since the solve started ([`GlobalSolver::branches_visited`]) and returns [`SolidError::InvalidInput`]
//!   past [`MAX_SOLVE_BRANCHES`] (`2^23` = 8,388,608). The largest legitimate solve
//!   measured visits 3,408,795 branches: a tiny-scale box / torus boolean (box 1e-4,
//!   torus major 1e-4 minor 3e-5; test `max_solve_branches_tiny_torus`, `#[ignore]`;
//!   `plan/findings/geom.global_solver--tiny-torus-c-reaches-ub175-2026-10-26.md`);
//!   upstream fixtures and the committed difftest cases visit at most 56,753.
//!   `2^23` gives 2.46 times headroom over that while keeping a runaway input (fuzz) to
//!   about a minute per solve at ~8 µs a branch (`2^24` would be about two).
//! - **Owner objects.** A tree node's owner object is a solution handle, not the
//!   curve, so the `SM_SO_SURFACE_PROJECTED_INTERSECT` test
//!   ([`GlobalSolver::surface_projected_may_contain_answers`]) takes the second
//!   curve from its caller (the curve/curve solver); the base solver without one
//!   returns [`SolidError::InvalidInput`] where the owner object is set. The
//!   ignored statuses of its `EvaluatePoint`/`EvaluateSurfaceNormal` keep the
//!   previous sample's point or normal; a failure on the first sample (an
//!   uninitialised value upstream) is returned.
//! - The `ERR(SM_ERR)` of `BranchMayContainAnswers` for operations it does not
//!   handle only prints a message upstream and falls through to `TRUE`; here
//!   it is silently `true`. `SM_ASSERT`s, `Dump` and the `SM_DEBUG_CODE`
//!   drawing blocks are not ported.
//! - **Recursion counter.** The tree check above runs only in `solve_it`; overrides
//!   and direct callers of `SolveBranch` skip it. The base traversal therefore also
//!   counts its nesting in [`GlobalSolver::solve_depth`] and returns
//!   [`SolidError::InvalidInput`] past `MAX_SOLVE_DEPTH + 1` nested calls (the top
//!   call plus one per level, so every tree set the check accepts still runs).
//! - `SM_SO_SURFACE_PROJECTED_INTERSECT` samples an `SmCrvOnSurf` owner of the
//!   second tree, which is not ported yet: once the box tests cannot decide,
//!   a tree without an owner object answers `false` (as upstream) and any
//!   owner object returns [`SolidError::Unported`] (see the `UNPORTED` marker).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::error::{Result, SolidError};
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{line_closest_point, point_project_to_plane};
use crate::geom::solution_array::{
    EndClass, OwnDomains, ParameterDomains, Solution, SolutionArray, SolutionObject, SolutionType,
    SortKey,
};
use crate::geom::tol::is_zero;
use crate::geom::tree::{AuxDataType, NodeData, NodeId, Tree, TreeNode};
use crate::geom::vector3d::sqrt;
use crate::kernel::Kernel;
use crate::math::{max2, max3, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, UNDEF_DOUBLE};

/// `SM_GS_MAX_TREES`: most trees a global solver traverses together.
pub const MAX_SOLVER_TREES: usize = 3;

/// Most tree levels one solve may descend through: the heights of the
/// traversed trees (counted from their top nodes) must add up to at most this.
/// `SolveBranch` recurses once per level (each subdivision descends one tree by
/// one level), so this bounds the recursion; upstream trees are far shallower.
pub const MAX_SOLVE_DEPTH: usize = 1_000;

/// Most branches [`solve_branch`] visits in one solve (not upstream; see the module
/// deviations): `2^23`, 2.46 times the most a legitimate input is measured to visit
/// (3,408,795: a tiny-scale box / torus boolean, 2026-10-26; test
/// `max_solve_branches_tiny_torus` and
/// `plan/findings/geom.global_solver--tiny-torus-c-reaches-ub175-2026-10-26.md`).
pub const MAX_SOLVE_BRANCHES: u64 = 1 << 23;

/// One node per traversed tree (`SmTreeNode* apBranch[SM_GS_MAX_TREES]`);
/// `None` is a null pointer. Slots past the solver's tree count are `None`.
pub type Branch = [Option<NodeId>; MAX_SOLVER_TREES];

/// What a global solve looks for (`SmSolverOperationType`); the discriminants
/// are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum SolverOperation {
    /// `SM_SO_INTERSECT`: intersect the objects.
    #[default]
    Intersect = 0,
    /// `SM_SO_INTERSECT_WIREFRAME`: brep/curve intersections limited to
    /// vertices and edges (no longer used upstream).
    IntersectWireframe = 1,
    /// `SM_SO_INTERSECTION_TEST`: test two trees for possible intersection.
    IntersectionTest = 2,
    /// `SM_SO_MINIMIZE`: minimise the distance between the objects.
    Minimize = 3,
    /// `SM_SO_MAXIMIZE`: maximise the distance between the objects.
    Maximize = 4,
    /// `SM_SO_NORMALIZE`: points that project along curve and surface normals.
    Normalize = 5,
    /// `SM_SO_AT_DISTANCE`: points at a given distance.
    AtDistance = 6,
    /// `SM_SO_FIND`: plain search.
    Find = 7,
    /// `SM_SO_RAYFIRE`: minimise intersections along the ray direction
    /// `opt_vectors[0]`.
    Rayfire = 8,
    /// `SM_SO_3D_SIGNED_DIRECTED_MINIMIZE`: minimise the 3D signed distance
    /// along `opt_vectors[0]`.
    Signed3dDirectedMinimize = 9,
    /// `SM_SO_3D_SIGNED_DIRECTED_MAXIMIZE`: maximise the 3D signed distance
    /// along a vector.
    Signed3dDirectedMaximize = 10,
    /// `SM_SO_SIGNED_DIRECTED_MINIMIZE`: minimise the projected signed distance
    /// along `opt_vectors[1]` (projection plane normal `opt_vectors[0]`).
    SignedDirectedMinimize = 11,
    /// `SM_SO_DIRECTED_MINIMIZE`: minimise the projected absolute distance
    /// along `opt_vectors[1]`.
    DirectedMinimize = 12,
    /// `SM_SO_DIRECTED_MAXIMIZE`: maximise the projected absolute distance
    /// along `opt_vectors[1]`.
    DirectedMaximize = 13,
    /// `SM_SO_PROJECTED_MINIMIZE`: minimise the distance projected along
    /// `opt_vectors[0]`.
    ProjectedMinimize = 14,
    /// `SM_SO_PROJECTED_MAXIMIZE`: maximise the distance projected along
    /// `opt_vectors[0]`.
    ProjectedMaximize = 15,
    /// `SM_SO_PROJECTED_INTERSECT`: intersect the objects projected along the
    /// unit view-plane normal `opt_vectors[0]`.
    ProjectedIntersect = 16,
    /// `SM_SO_ROTATED_PROJECTED_INTERSECT`: intersect objects rotated about an
    /// axis (point `opt_vectors[0]`, unit direction `opt_vectors[1]`, unit
    /// plane x axis `opt_vectors[2]`).
    RotatedProjectedIntersect = 17,
    /// `SM_SO_PERSPECTIVE_INTERSECT`: intersect perspective projections (plane
    /// point `opt_vectors[0]`, unit normal `opt_vectors[1]`, eye point
    /// `opt_vectors[2]`).
    PerspectiveIntersect = 18,
    /// `SM_SO_SURFACE_PROJECTED_INTERSECT`: intersect the first curve projected
    /// onto the surface of the second (an `SmCrvOnSurf`).
    SurfaceProjectedIntersect = 19,
    /// `SM_SO_ANGLE_MINIMIZE`: minimum absolute angle to a plane (not
    /// implemented upstream).
    AngleMinimize = 20,
    /// `SM_SO_SIGNED_ANGLE_MINIMIZE`: minimum signed angle against the
    /// reference vector `opt_vectors[1]`.
    SignedAngleMinimize = 21,
    /// `SM_SO_PROJECTED_TANGENCY`: projected tangency points of two curves.
    ProjectedTangency = 22,
    /// `SM_SO_SIGNED_PIVOT_MINIMIZE`: minimum pivot angle about the point
    /// `opt_vectors[1]` (reference direction `opt_vectors[2]`).
    SignedPivotMinimize = 23,
    /// `SM_SO_PROJECTED_TANGENT_THROUGH_POINT`: curve points whose projected
    /// tangents pass through a point.
    ProjectedTangentThroughPoint = 24,
}

impl SolverOperation {
    /// Every operation, in upstream order.
    pub const ALL: [Self; 25] = [
        Self::Intersect,
        Self::IntersectWireframe,
        Self::IntersectionTest,
        Self::Minimize,
        Self::Maximize,
        Self::Normalize,
        Self::AtDistance,
        Self::Find,
        Self::Rayfire,
        Self::Signed3dDirectedMinimize,
        Self::Signed3dDirectedMaximize,
        Self::SignedDirectedMinimize,
        Self::DirectedMinimize,
        Self::DirectedMaximize,
        Self::ProjectedMinimize,
        Self::ProjectedMaximize,
        Self::ProjectedIntersect,
        Self::RotatedProjectedIntersect,
        Self::PerspectiveIntersect,
        Self::SurfaceProjectedIntersect,
        Self::AngleMinimize,
        Self::SignedAngleMinimize,
        Self::ProjectedTangency,
        Self::SignedPivotMinimize,
        Self::ProjectedTangentThroughPoint,
    ];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The operation with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.code() == code)
    }

    /// True for the operations whose best answer is the largest
    /// (`SM_SO_MAXIMIZE`, `SM_SO_DIRECTED_MAXIMIZE`, `SM_SO_PROJECTED_MAXIMIZE`).
    const fn is_plain_maximize(self) -> bool {
        matches!(
            self,
            Self::Maximize | Self::DirectedMaximize | Self::ProjectedMaximize
        )
    }
}

/// Summary of a [`SolverOperation`] (`SmOperationCategory`); the
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum OperationCategory {
    /// `SM_OPERATION_MINIMIZE`: some kind of minimisation.
    Minimize = 0,
    /// `SM_OPERATION_MAXIMIZE`: some kind of maximisation.
    Maximize = 1,
    /// `SM_OPERATION_SIGNED`: signed operations.
    Signed = 2,
    /// `SM_OPERATION_OTHER`: everything else.
    #[default]
    Other = 3,
}

impl OperationCategory {
    /// Every category, in upstream order.
    pub const ALL: [Self; 4] = [Self::Minimize, Self::Maximize, Self::Signed, Self::Other];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The category with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }
}

/// Which solutions a solve keeps (`SmSolutionRequestedType`); the
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum SolutionRequested {
    /// `SM_SR_SINGLE`: only the best solution.
    Single = 0,
    /// `SM_SR_ALL`: every solution within tolerance.
    #[default]
    All = 1,
    /// `SM_SR_FIND_AMBIGUITIES`: not used upstream.
    FindAmbiguities = 2,
    /// `SM_SR_NODES`: only the tree nodes where solutions may exist.
    Nodes = 3,
}

impl SolutionRequested {
    /// Every request type, in upstream order.
    pub const ALL: [Self; 4] = [Self::Single, Self::All, Self::FindAmbiguities, Self::Nodes];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The request type with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.code() == code)
    }
}

/// How two solutions relate (`SmSolutionPairType`); the discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SolutionPairType {
    /// `SM_SP_IDENTICAL`: same type, counts, objects and parameters.
    Identical = 0,
    /// `SM_SP_DIFFCOUNTS`: different type, object or variable count (or
    /// solution value).
    DiffCounts = 1,
    /// `SM_SP_DIFFOBJECTS`: different objects.
    DiffObjects = 2,
    /// `SM_SP_DIFFPARAMETERS`: only the parameters differ.
    DiffParameters = 3,
}

impl SolutionPairType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// Which of two identical solutions to keep (`IdenticalSolutions`'
/// `pPreferredSol`, pointing at its first or second argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreferredSolution {
    /// The first solution (`&crSol1`).
    First,
    /// The second solution (`&crSol2`).
    Second,
}

/// The arguments of [`GlobalSolve::solve_it`] (`SmGlobalSolver::SolveIt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveRequest<'v> {
    /// `eSolverOperation`: what to look for.
    pub operation: SolverOperation,
    /// `eSolutionRequested`: which solutions to keep.
    pub solution_requested: SolutionRequested,
    /// `d3dTolerance`: 3D distance tolerance.
    pub tolerance_3d: f64,
    /// `dBestAnswerSoFarSq`: best (squared) value known before the solve;
    /// `SM_BIG_DOUBLE` for none (reset to `0` for the maximising operations).
    pub best_answer_so_far_sq: f64,
    /// `cpOptVectors`: the operation's optional vectors (see
    /// [`SolverOperation`]).
    pub opt_vectors: &'v [Vec3],
}

impl Default for SolveRequest<'_> {
    /// [`SolverOperation::Intersect`], [`SolutionRequested::All`], zero
    /// tolerance, no best answer yet (`SM_BIG_DOUBLE`), no optional vectors.
    fn default() -> Self {
        Self {
            operation: SolverOperation::Intersect,
            solution_requested: SolutionRequested::All,
            tolerance_3d: 0.0,
            best_answer_so_far_sq: BIG_DOUBLE,
            opt_vectors: &[],
        }
    }
}

/// The caller's `SmTol::GetZoneTol3d(const SmObject*)` for the solution handles of a
/// solve: the handles are opaque to the solver, so a caller that can resolve them to
/// their B-rep entities (vertex, edge, face, or the edge of a curve, the face of a
/// surface) passes this to the curve and surface solves (see
/// [`GlobalSolve::object_zone_tol_3d`]).
#[derive(Clone, Copy)]
pub struct ObjectZoneTol<'a, O>(pub &'a dyn Fn(O) -> Result<f64>);

impl<O> core::fmt::Debug for ObjectZoneTol<'_, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ObjectZoneTol(..)")
    }
}

impl<O> ObjectZoneTol<'_, O> {
    /// The zone tolerance of `object`.
    ///
    /// # Errors
    ///
    /// The resolver's.
    pub fn get(&self, object: O) -> Result<f64> {
        (self.0)(object)
    }
}

/// [`GlobalSolve::object_zone_tol_3d`] for a solver given an optional resolver: the
/// resolver's answer, else the opaque-handle deferral.
///
/// # Errors
///
/// The resolver's; [`SolidError::Unported`] without one.
pub fn resolve_object_zone_tol_3d<O>(
    resolver: Option<ObjectZoneTol<'_, O>>,
    object: O,
) -> Result<f64> {
    match resolver {
        Some(r) => r.get(object),
        // UNPORTED(geom.tol): SmTol::GetZoneTol3d(const SmObject*) on a handle the
        // caller gave no resolver for
        // Proof (unreachable): only Rayfire solvers read this; the library ones
        // (offset_geometry_creation, brep p2) pass a resolver or override
        // object_zone_tol_3d; 0 hits in the green difftest sweep.
        None => Err(SolidError::Unported {
            upstream: "SmTol::GetZoneTol3d(const SmObject*)",
            unit: "geom.tol",
        }),
    }
}

/// The state of a global solve (`SmGlobalSolver`'s data members).
#[derive(Debug, Clone)]
pub struct GlobalSolver<'t, O> {
    /// `m_eSolverOperation`: behaviour of the local solves.
    pub operation: SolverOperation,
    /// `m_eOperationCategory`: summary of [`GlobalSolver::operation`], set by
    /// [`GlobalSolve::solve_it`].
    pub category: OperationCategory,
    /// `m_bProjectedOperation`: true for the projected and directed operations,
    /// set by [`GlobalSolve::solve_it`].
    pub projected_operation: bool,
    /// `m_eSolutionRequested`: which solutions
    /// [`GlobalSolve::add_sorted_solution`] keeps.
    pub solution_requested: SolutionRequested,
    /// `m_lNumTrees`: trees to traverse (at most [`MAX_SOLVER_TREES`]).
    pub num_trees: usize,
    /// `m_lNumVariables`: total parameters of a solution.
    pub num_variables: usize,
    /// `m_apTrees`; `None` is a null pointer.
    pub trees: [Option<&'t Tree<O>>; MAX_SOLVER_TREES],
    /// `m_d3dTolerance`: 3D distance tolerance.
    pub tolerance_3d: f64,
    /// `m_dBestAnswerSoFarSq`: best (squared) value found so far.
    pub best_answer_so_far_sq: f64,
    /// `m_dAtDistance`: target distance of [`SolverOperation::AtDistance`].
    pub at_distance: f64,
    /// `m_dAtDistanceSq`: `at_distance * at_distance`.
    pub at_distance_sq: f64,
    /// `m_cpOptVectors`: operation-specific vectors (see [`SolverOperation`]).
    pub opt_vectors: Vec<Vec3>,
    /// `*m_pSolutions`: the sorted solutions.
    pub solutions: SolutionArray<O>,
    /// Current nesting of the base [`solve_branch`] (not upstream): bounded by
    /// `MAX_SOLVE_DEPTH + 1`.
    pub solve_depth: usize,
    /// Branches [`solve_branch`] visited since the solve started (not upstream;
    /// reset by [`GlobalSolve::solve_it`] and the topology solver's `SolveTrees`):
    /// bounded by [`MAX_SOLVE_BRANCHES`].
    pub branches_visited: u64,
}

/// Height of the subtree of `tree` below `top` along the child links (a leaf
/// is 0), with an explicit stack; `InvalidInput` when the links are cyclic.
/// Each node is measured once, so shared children cost nothing extra.
fn subtree_height<O: Copy + PartialEq>(
    tree: &Tree<O>,
    top: NodeId,
    operation: &'static str,
) -> Result<usize> {
    const OPEN: u8 = 1;
    const DONE: u8 = 2;
    let n = tree.nodes().len();
    let mut state = vec![0u8; n];
    let mut height = vec![0usize; n];
    let mut stack = vec![(top, false)];
    let slot = |id: NodeId| {
        (id.0 < n).then_some(id.0).ok_or(SolidError::InvalidInput {
            operation,
            reason: "tree node id out of range",
        })
    };
    while let Some((id, expanded)) = stack.pop() {
        let k = slot(id)?;
        let node = tree.node(id)?;
        let children = [node.child1, node.child2];
        if expanded {
            let mut h = 0;
            for c in children.into_iter().flatten() {
                h = h.max(height.get(slot(c)?).copied().unwrap_or(0) + 1);
            }
            if let Some(x) = height.get_mut(k) {
                *x = h;
            }
            if let Some(x) = state.get_mut(k) {
                *x = DONE;
            }
            continue;
        }
        match state.get(k).copied() {
            Some(DONE) => continue,
            // still open: `id` is on the current path, so a child link loops back
            Some(OPEN) => {
                return Err(SolidError::InvalidInput {
                    operation,
                    reason: "tree child links are cyclic",
                })
            }
            _ => {}
        }
        if let Some(x) = state.get_mut(k) {
            *x = OPEN;
        }
        stack.push((id, true));
        for c in children.into_iter().flatten() {
            stack.push((c, false));
        }
    }
    Ok(height.get(slot(top)?).copied().unwrap_or(0))
}

/// Error for a branch slot, tree or child that upstream would dereference as
/// a null pointer.
fn null_node(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "null tree or tree node in solver branch",
    }
}

impl<'t, O: SolutionObject> GlobalSolver<'t, O> {
    /// A solver without trees (`SmGlobalSolver()`): operation
    /// [`SolverOperation::Intersect`], all solutions requested, undefined
    /// tolerance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            operation: SolverOperation::Intersect,
            category: OperationCategory::Other,
            projected_operation: false,
            solution_requested: SolutionRequested::All,
            num_trees: 0,
            num_variables: 0,
            trees: [None; MAX_SOLVER_TREES],
            tolerance_3d: UNDEF_DOUBLE,
            best_answer_so_far_sq: 0.0,
            at_distance: 0.0,
            at_distance_sq: 0.0,
            opt_vectors: Vec::new(),
            solutions: SolutionArray::new(),
            solve_depth: 0,
            branches_visited: 0,
        }
    }

    /// A solver over one tree (`SmGlobalSolver(SmTree*, ULONG)`), zero
    /// tolerance.
    #[must_use]
    pub fn with_tree(tree: &'t Tree<O>, num_variables: usize) -> Self {
        let mut s = Self::new();
        s.num_trees = 1;
        s.num_variables = num_variables;
        s.trees[0] = Some(tree);
        s.tolerance_3d = 0.0;
        s
    }

    /// A solver over two trees (`SmGlobalSolver(SmTree*, SmTree*, ULONG)`),
    /// zero tolerance.
    #[must_use]
    pub fn with_trees(tree1: &'t Tree<O>, tree2: &'t Tree<O>, num_variables: usize) -> Self {
        let mut s = Self::new();
        s.num_trees = 2;
        s.num_variables = num_variables;
        s.trees[0] = Some(tree1);
        s.trees[1] = Some(tree2);
        s.tolerance_3d = 0.0;
        s
    }

    /// The operation (`GetSolverOperationType`).
    #[must_use]
    pub const fn solver_operation(&self) -> SolverOperation {
        self.operation
    }

    /// Optional vector `index` (`GetOptVector`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index past the optional vectors.
    pub fn opt_vector(&self, index: usize) -> Result<Vec3> {
        self.opt_vectors
            .get(index)
            .copied()
            .ok_or(SolidError::InvalidInput {
                operation: "geom::global_solver::GlobalSolver::opt_vector",
                reason: "optional solver vector index out of range",
            })
    }

    /// Sets the operation (`SetSolverOperation`); the category is not updated.
    pub fn set_solver_operation(&mut self, operation: SolverOperation) {
        self.operation = operation;
    }

    /// Sets which solutions to keep (`SetSolutionRequested`).
    pub fn set_solution_requested(&mut self, requested: SolutionRequested) {
        self.solution_requested = requested;
    }

    /// Sets the 3D tolerance (`Set3DTolerance`).
    pub fn set_3d_tolerance(&mut self, tolerance: f64) {
        self.tolerance_3d = tolerance;
    }

    /// Sets the best squared answer so far (`SetBestAnswerSoFarSq`).
    pub fn set_best_answer_so_far_sq(&mut self, best_sq: f64) {
        self.best_answer_so_far_sq = best_sq;
    }

    /// Sets the tree count (`SetNumTrees`).
    pub fn set_num_trees(&mut self, num_trees: usize) {
        self.num_trees = num_trees;
    }

    /// Sets the variable count (`SetNumVariables`).
    pub fn set_num_variables(&mut self, num_variables: usize) {
        self.num_variables = num_variables;
    }

    /// Sets the operation category (`SetOperationCategory`).
    pub fn set_operation_category(&mut self, category: OperationCategory) {
        self.category = category;
    }

    /// Replaces the solution array (`SetSolutions`), returning the old one.
    pub fn set_solutions(&mut self, solutions: SolutionArray<O>) -> SolutionArray<O> {
        std::mem::replace(&mut self.solutions, solutions)
    }

    /// Sets the target distance and its square (`SetAtDistance`).
    pub fn set_at_distance(&mut self, at_distance: f64) {
        self.at_distance = at_distance;
        self.at_distance_sq = at_distance * at_distance;
    }

    /// Checks the tree count against the fixed arrays.
    fn check_num_trees(&self, operation: &'static str) -> Result<()> {
        if self.num_trees > MAX_SOLVER_TREES {
            return Err(SolidError::InvalidInput {
                operation,
                reason: "solver tree count exceeds SM_GS_MAX_TREES",
            });
        }
        Ok(())
    }

    /// Checks that the child links below the start nodes of `branch` are
    /// acyclic and that their heights add up to at most [`MAX_SOLVE_DEPTH`]
    /// (the bound on the `SolveBranch` recursion).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for cyclic child links (upstream recurses
    /// without end) or trees deeper than the bound.
    pub fn check_solve_depth(&self, branch: &Branch) -> Result<()> {
        const OP: &str = "geom::global_solver::GlobalSolver::check_solve_depth";
        let mut total = 0usize;
        for (i, slot) in branch.iter().enumerate() {
            if let Some(top) = *slot {
                total = total.saturating_add(subtree_height(self.tree(i)?, top, OP)?);
            }
        }
        if total > MAX_SOLVE_DEPTH {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "trees deeper than MAX_SOLVE_DEPTH",
            });
        }
        Ok(())
    }

    /// Tree `index` (`m_apTrees[index]`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a null or missing tree.
    pub fn tree(&self, index: usize) -> Result<&'t Tree<O>> {
        self.trees
            .get(index)
            .copied()
            .flatten()
            .ok_or(null_node("geom::global_solver::GlobalSolver::tree"))
    }

    /// The node of `branch` in tree `index` (`apBranch[index]`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a null slot or tree, or a node id the
    /// tree does not hold.
    pub fn branch_node(&self, branch: &Branch, index: usize) -> Result<&'t TreeNode<O>> {
        const OP: &str = "geom::global_solver::GlobalSolver::branch_node";
        let id = branch.get(index).copied().flatten().ok_or(null_node(OP))?;
        self.tree(index)?.node(id)
    }

    /// The bounding box of `branch`'s node in tree `index`
    /// (`apBranch[index]->m_sBBox`).
    fn bbox(&self, branch: &Branch, index: usize) -> Result<Extent3d> {
        Ok(self.branch_node(branch, index)?.bbox)
    }

    /// Child 1 or child 2 of `branch`'s node in tree `index`.
    fn child(&self, branch: &Branch, index: usize, second: bool) -> Result<&'t TreeNode<O>> {
        const OP: &str = "geom::global_solver::GlobalSolver::child";
        let node = self.branch_node(branch, index)?;
        let id = if second { node.child2 } else { node.child1 };
        self.tree(index)?.node(id.ok_or(null_node(OP))?)
    }

    /// `SmGlobalSolver::BranchMayContainAnswers`: false when the node boxes of
    /// `branch` prove that it holds no answer for the operation.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a null tree or node of `branch`, more than
    /// [`MAX_SOLVER_TREES`] trees or a missing optional vector;
    /// [`SolidError::Unported`] for a surface-projected intersection that needs
    /// the curve-on-surface owner.
    pub fn branch_may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        // No natural optimization for single tree operations.
        // You'll have to do this by subclassing.
        if self.num_trees == 1 {
            return Ok(true);
        }
        self.check_num_trees("geom::global_solver::GlobalSolver::branch_may_contain_answers")?;
        let tol = self.tolerance_3d;

        // for each kind of Solver operation - return either TRUE=There may be solutions in this branch or FALSE=No Solutions
        match self.operation {
            SolverOperation::Intersect | SolverOperation::IntersectionTest => {
                for i in 0..self.num_trees.saturating_sub(1) {
                    let mut bbox = self.bbox(branch, i)?;
                    bbox.expand_absolute(tol);

                    for j in i + 1..self.num_trees {
                        let bbox2 = self.bbox(branch, j)?;
                        if bbox.are_disjoint(&bbox2, 0.0) {
                            return Ok(false); // No answers in this branch
                        }
                    }
                }
                Ok(true) // possible answers in this branch
            }

            SolverOperation::Minimize => {
                let b0 = self.bbox(branch, 0)?;
                let b1 = self.bbox(branch, 1)?;
                let min_dist_sq = b0.min_distance_squared(&b1);
                if min_dist_sq > self.best_answer_so_far_sq
                    && sqrt(min_dist_sq) - tol > sqrt(self.best_answer_so_far_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true) // possible answers in this branch
            }

            SolverOperation::Maximize => {
                let b0 = self.bbox(branch, 0)?;
                let b1 = self.bbox(branch, 1)?;
                let max_dist_sq = b0.max_distance_squared(&b1);
                if max_dist_sq < self.best_answer_so_far_sq
                    && sqrt(max_dist_sq) + tol < sqrt(self.best_answer_so_far_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true) // possible answers in this branch
            }

            SolverOperation::AtDistance => {
                let b0 = self.bbox(branch, 0)?;
                let b1 = self.bbox(branch, 1)?;
                let max_dist_sq = b0.max_distance_squared(&b1);
                if max_dist_sq < self.at_distance_sq
                    && sqrt(max_dist_sq) - tol < sqrt(self.at_distance_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                let min_dist_sq = b0.min_distance_squared(&b1);
                if min_dist_sq > self.at_distance_sq
                    && sqrt(min_dist_sq) + tol > sqrt(self.at_distance_sq)
                {
                    return Ok(false); // no answers in this branch
                }
                Ok(true) // may be answers
            }

            SolverOperation::ProjectedMinimize
            | SolverOperation::ProjectedMaximize
            | SolverOperation::ProjectedIntersect
            | SolverOperation::RotatedProjectedIntersect
            | SolverOperation::PerspectiveIntersect => self.projected_may_contain_answers(branch),

            // If we are doing a directed minimization we can use the bounding
            // box information to eliminate additional comparisons
            SolverOperation::DirectedMinimize
            | SolverOperation::SignedDirectedMinimize
            | SolverOperation::DirectedMaximize => self.directed_may_contain_answers(branch),

            SolverOperation::SignedPivotMinimize => {
                // First, find projected distance from pivot pt to each center of bounding spheres.
                // If the difference of those two distances is greater than the sum of
                // the two radii, we can determine that no solutions will come from these two nodes.
                let s1 = self.bbox(branch, 0)?.sphere_bound();
                let s2 = self.bbox(branch, 1)?.sphere_bound();
                let v0 = self.opt_vector(0)?;
                let v1 = self.opt_vector(1)?;
                let vec1 = v0.cross(s1.center - v1).cross(v0);
                let vec2 = v0.cross(s2.center - v1).cross(v0);
                let sum_radius = s1.radius + s2.radius;
                if (vec1.length() - vec2.length()).abs() - tol > sum_radius {
                    return Ok(false); // These two nodes can not solve this pivot minimization
                }
                Ok(true) // possible answers in this branch
            }

            // Doing a ray firing - minimization of intersections along a vector.
            SolverOperation::Rayfire => self.rayfire_may_contain_answers(branch),

            SolverOperation::Signed3dDirectedMinimize => {
                let s1 = self.bbox(branch, 0)?.sphere_bound();
                let s2 = self.bbox(branch, 1)?.sphere_bound();
                let line_pnt = s1.center;
                let line_vec = self.opt_vector(0)?;
                let param = line_closest_point(line_pnt, line_vec, s2.center);
                let cl_pnt = line_pnt + param * line_vec;
                let dist = cl_pnt.distance(s2.center);
                let sum_radius = s1.radius + s2.radius;
                // If two spheres don't intersect as projected along the vector
                // return FALSE.
                if dist > sum_radius + tol {
                    return Ok(false); // No answers in this branch
                }

                let vec = cl_pnt - s1.center;
                let mut dist_to = vec.length();
                if vec.dot(line_vec) < 0.0 {
                    dist_to = -dist_to;
                }
                let min_dist = dist_to - s1.radius - s2.radius - tol;
                if min_dist * min_dist.abs() > self.best_answer_so_far_sq {
                    return Ok(false); // No answers in this branch
                }
                Ok(true) // possible answers in this branch
            }

            SolverOperation::SurfaceProjectedIntersect => {
                self.surface_projected_may_contain_answers(branch, None)
            }

            // SM_SO_NORMALIZE, SM_SO_FIND: no optimization here yet.
            // SM_SO_PROJECTED_TANGENT_THROUGH_POINT: the point could be arbitrarily
            // far from the curve, so no kind of boxing will work.
            // The rest are not handled yet (upstream: ERR(SM_ERR), a message only).
            // No special knowledge: can't rule out solutions in this branch.
            SolverOperation::Normalize
            | SolverOperation::Find
            | SolverOperation::ProjectedTangentThroughPoint
            | SolverOperation::IntersectWireframe
            | SolverOperation::Signed3dDirectedMaximize
            | SolverOperation::AngleMinimize
            | SolverOperation::SignedAngleMinimize
            | SolverOperation::ProjectedTangency => Ok(true),
        }
    }

    /// The projected-sphere tests of `BranchMayContainAnswers` for the
    /// projected, rotated and perspective operations.
    fn projected_may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        let tol = self.tolerance_3d;

        // get Sphere bounds for the branch bboxes
        let s1 = self.bbox(branch, 0)?.sphere_bound();
        let s2 = self.bbox(branch, 1)?.sphere_bound();
        let (sph1_center, sph2_center) = (s1.center, s2.center);
        let mut sph1_radius = s1.radius;
        let mut sph2_radius = s2.radius;

        // Factor 3D tolerance by adding it to just one sphere radii.
        sph1_radius += tol;

        // project SphereCenters to common plane circles that can be quickly checked no intersections
        let proj_pnt1;
        let proj_pnt2;
        if self.operation == SolverOperation::RotatedProjectedIntersect {
            // m_cpOptVectors[1] = unitRotAxis, m_cpOptVectors[2] = unitXAxis
            let v0 = self.opt_vector(0)?;
            let v1 = self.opt_vector(1)?;
            let v2 = self.opt_vector(2)?;

            // rotate project Sph1 and Sph2 CenterPts to same plane
            proj_pnt1 = sph1_center.rotate_project_point_to_plane(v0, v1, v2);
            proj_pnt2 = sph2_center.rotate_project_point_to_plane(v0, v1, v2);
        } else if self.operation == SolverOperation::PerspectiveIntersect {
            // m_cpOptVectors[0] = point on view plane
            // m_cpOptVectors[1] = unit normal to view plane
            // m_cpOptVectors[2] = eye point
            let v0 = self.opt_vector(0)?;
            let v1 = self.opt_vector(1)?;
            let v2 = self.opt_vector(2)?;

            // perspective project Sph1 and Sph2 CenterPts to same plane
            proj_pnt1 = sph1_center.perspective_project_point_to_plane(v0, v1, v2);
            proj_pnt2 = sph2_center.perspective_project_point_to_plane(v0, v1, v2);

            // spheres project to ellipses - find max radius of sphere projections
            let eye_vec1 = sph1_center - v2;
            let mut dir1 = eye_vec1.cross(v1).unitize_or_z(); // vec perp to eyevec and view norm
            let mut dir2 = dir1.cross(eye_vec1).unitize_or_z(); // vec perp to eyevec in view normal plane
            sph1_radius -= tol;
            let new_rad1_max_sq = max_projected_radius_sq(
                sph1_center,
                sph1_center,
                [dir1, dir2],
                sph1_radius,
                [v0, v1, v2],
            );

            let eye_vec2 = sph2_center - v2;
            dir1 = eye_vec2.cross(v1).unitize_or_z(); // vec perp to eyevec and view norm
            dir2 = dir1.cross(eye_vec2).unitize_or_z(); // vec perp to eyevec in view normal plane

            // (upstream offsets the second centre by the first radius and projects
            // relative to the first centre)
            let new_rad2_max_sq = max_projected_radius_sq(
                sph2_center,
                sph1_center,
                [dir1, dir2],
                sph1_radius,
                [v0, v1, v2],
            );

            // arrive here when sProjPnt1 and sProjPnt2 are the projected centers of the spheres and
            //   dNewRad1Max and dNewRad2Max are the maximum radii of the sphere projections onto the viewing plane
            sph1_radius = sqrt(new_rad1_max_sq) + tol;
            sph2_radius = sqrt(new_rad2_max_sq);
        } else {
            // project Sph2 CenterPt to same plane as Sph1 CenterPt
            proj_pnt2 = point_project_to_plane(sph2_center, sph1_center, self.opt_vector(0)?);
            proj_pnt1 = sph1_center;
        }

        // for each operation - see if spheres are properly positioned to exclude a possible solution
        match self.operation {
            SolverOperation::ProjectedIntersect
            | SolverOperation::RotatedProjectedIntersect
            | SolverOperation::PerspectiveIntersect => {
                let min_proj_dist = proj_pnt1.distance(proj_pnt2) - sph1_radius - sph2_radius;
                if min_proj_dist > 0.0 {
                    return Ok(false); // No answers in this branch
                }
            }
            SolverOperation::ProjectedMinimize => {
                let min_proj_dist = proj_pnt1.distance(proj_pnt2) - sph1_radius - sph2_radius;
                if min_proj_dist > 0.0 && min_proj_dist * min_proj_dist > self.best_answer_so_far_sq
                {
                    return Ok(false); // No answers in this branch
                }
            }
            SolverOperation::ProjectedMaximize => {
                let max_proj_dist = proj_pnt1.distance(proj_pnt2) + sph1_radius + sph2_radius;
                if max_proj_dist * max_proj_dist < self.best_answer_so_far_sq {
                    return Ok(false); // No answers in this branch
                }
            }
            _ => {}
        }

        // If we were unable to eliminate using above sphere tests then
        // we just skip out here with a TRUE.
        Ok(true) // possible answers in this branch
    }

    /// The directed (signed) minimise / maximise tests of
    /// `BranchMayContainAnswers`.
    fn directed_may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        // This optimization can be done by creating a plane through the
        // center of one bounding box whose normal is perpendicular to the
        // direction vector and seeing if the distance to the
        // center of the other bounding box is greater than the sum of
        // the two radii.
        let v0 = self.opt_vector(0)?;
        let v1 = self.opt_vector(1)?;
        let plane_norm = v1.cross(v0);
        let s1 = self.bbox(branch, 0)?.sphere_bound();
        let s2 = self.bbox(branch, 1)?.sphere_bound();
        let (sph1_cent, sph1_radius) = (s1.center, s1.radius);
        let (sph2_cent, sph2_radius) = (s2.center, s2.radius);

        let mut proj_pnt = point_project_to_plane(sph2_cent, sph1_cent, plane_norm);
        let sum_radius = sph1_radius + sph2_radius;
        if proj_pnt.distance_squared(sph2_cent) > sum_radius * sum_radius + EFF_ZERO_SQRT {
            return Ok(false); // These two nodes can not solve this directed minimization
        }

        // Now take projected point and project it again up to projection plane
        // as defined going through sSph1Center
        proj_pnt = point_project_to_plane(proj_pnt, sph1_cent, v0);

        if self.operation == SolverOperation::SignedDirectedMinimize {
            let vec = proj_pnt - sph1_cent;
            let mut dist = vec.length();
            if vec.dot(v1) < 0.0 {
                dist = -dist;
            }
            let min_proj_dist = dist - sph1_radius - sph2_radius;
            if min_proj_dist * min_proj_dist.abs() > self.best_answer_so_far_sq {
                return Ok(false); // No answers in this branch
            }
        }

        if self.operation == SolverOperation::DirectedMinimize {
            let min_proj_dist = proj_pnt.distance(sph1_cent) - sph1_radius - sph2_radius;
            if min_proj_dist > 0.0 && min_proj_dist * min_proj_dist > self.best_answer_so_far_sq {
                return Ok(false); // No answers in this branch
            }
        }

        if self.operation == SolverOperation::DirectedMaximize {
            let max_proj_dist = proj_pnt.distance(sph1_cent) + sph1_radius + sph2_radius;
            if max_proj_dist * max_proj_dist < self.best_answer_so_far_sq {
                return Ok(false); // No answers in this branch
            }
        }

        Ok(true) // possible answers in this branch
    }

    /// The ray tests of `BranchMayContainAnswers` for
    /// [`SolverOperation::Rayfire`].
    fn rayfire_may_contain_answers(&self, branch: &Branch) -> Result<bool> {
        let tol = self.tolerance_3d;
        let line_pnt = self.bbox(branch, 0)?.min();
        let line_vec = self.opt_vector(0)?;
        let bbox1 = self.bbox(branch, 1)?;
        let s1 = bbox1.sphere_bound();
        let (sph1_cent, sph1_radius) = (s1.center, s1.radius);
        let param = line_closest_point(line_pnt, line_vec, sph1_cent);
        let cl_pnt = line_pnt + param * line_vec;
        let dist = cl_pnt.distance(sph1_cent);
        // See if sphere does not intersect the infinite line
        if dist > sph1_radius + tol {
            return Ok(false); // No answers in this branch
        }
        // See if we are on wrong side of ray start point
        if param + sph1_radius < 0.0 {
            return Ok(false); // No answers in this branch
        }

        // See if we are further out on the ray than the best answer so far.
        let min_possible_ray_distance = param - sph1_radius - tol;
        if min_possible_ray_distance > 0.0
            && min_possible_ray_distance * min_possible_ray_distance > self.best_answer_so_far_sq
        {
            return Ok(false); // No answers in this branch
        }
        // Do some additional tests to see if the ray does not intersect with the
        // box of the object.
        let leng = 2.0 * (param.abs() + sph1_radius);
        let end_pnt = line_pnt + leng * line_vec;
        let start_pnt = line_pnt - leng * line_vec;
        let mut ray_box = Extent3d::from_point(start_pnt);
        ray_box.add_point(end_pnt);
        ray_box.expand_absolute(tol);
        if ray_box.are_disjoint(&bbox1, 0.0) {
            return Ok(false); // No answers in this branch
        }
        Ok(true) // possible answers in this branch
    }

    /// The box and offset-box tests of `BranchMayContainAnswers` for
    /// [`SolverOperation::SurfaceProjectedIntersect`].
    ///
    /// `owner` resolves the second tree's owner object (upstream
    /// `SM_CAST_PTR(SmCrvOnSurf, apBranch[1]->GetOwnerObject())`): the kernel and
    /// the second curve as a curve on a surface (`None` when it is not one). The
    /// base solver cannot reach the curve and passes `None`; the curve/curve
    /// solver passes its curves.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a null node, an owner object that `owner`
    /// does not resolve, or span data that is not curve data; the errors of the
    /// curve-on-surface evaluations upstream would leave its first sample
    /// undefined with; [`SolidError::Unported`] from those evaluations.
    pub fn surface_projected_may_contain_answers(
        &self,
        branch: &Branch,
        owner: Option<(&Kernel, Option<&CrvOnSurf>)>,
    ) -> Result<bool> {
        const OP: &str = "geom::global_solver::GlobalSolver::branch_may_contain_answers";
        let tol = self.tolerance_3d;

        // If the boxes actually intersect, then we can return True.
        // If they don't, we have to look at the surface offsets.
        let mut bbox1 = self.bbox(branch, 0)?;
        bbox1.expand_absolute(tol);
        let bbox2 = self.bbox(branch, 1)?;

        let min_dist = bbox1.min_distance(&bbox2);
        // If the min dist is 0, then they overlap.
        if min_dist < tol {
            return Ok(true);
        }

        // Now we have to see about offsetting sBBox2 along the surface normal.
        // We can find a reasonable offset distance by comparing BBoxes.
        let max_dist = bbox1.max_distance(&bbox2);

        // If we create a new box by offsetting the surface-curve points
        // along the normal by both the min and max distances,
        // then we will catch any case that could 'intersect'.
        // But we have to sample the surface curve.
        let node2 = branch.get(1).copied().flatten().ok_or(null_node(OP))?;
        if self.tree(1)?.node_owner_object(node2)?.is_none() {
            return Ok(false);
        }
        let Some((k, cos)) = owner else {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "the base solver cannot reach the second tree's owner curve",
            });
        };
        let Some(cos) = cos else {
            return Ok(false);
        };

        // Get the vector from the surface box to the 3d curve box.
        // We'll use that to decide which surface normal (+/-) to use.
        let diff_vec = bbox1.mid() - bbox2.mid();

        let tree2 = self.tree(1)?;
        let node = tree2.node(node2)?;
        if matches!(node.data, NodeData::None) {
            return Ok(false);
        }
        let ivl2 = *tree2.curve_aux(node)?.interval();

        let mut off_box = Extent3d::new();
        // Upstream's out-parameters keep their previous value when an
        // evaluation fails (its status is ignored).
        let mut srf_pt: Option<Point3> = None;
        let mut norm: Option<Vec3> = None;

        let num_samples: u32 = 10;
        let d_num_samples = f64::from(num_samples);
        for ii in 0..=num_samples {
            let frac = f64::from(ii) / d_num_samples;
            let t = ivl2.evaluate(frac);
            match cos.evaluate_point(k, t) {
                Ok(p) => srf_pt = Some(p),
                Err(e) if e.is_unported() || srf_pt.is_none() => return Err(e),
                Err(_) => {}
            }
            match cos.evaluate_surface_normal(k, t, false) {
                Ok(eval) => norm = Some(eval.normal),
                Err(e) if e.is_unported() || norm.is_none() => return Err(e),
                Err(_) => {}
            }
            let (Some(pt), Some(n)) = (srf_pt, norm.as_mut()) else {
                return Err(null_node(OP));
            };
            if n.dot(diff_vec) < 0.0 {
                *n *= -1.0;
            }
            let n = *n;

            off_box.add_point(pt + min_dist * n);
            off_box.add_point(pt + max_dist * n);
        }

        // Expand offset box to account for sampling.
        off_box.expand_absolute(2.0 * tol);

        if off_box.are_disjoint(&bbox1, 0.0) {
            return Ok(false);
        }

        Ok(true) // possible answers in this branch
    }

    /// `SmGlobalSolver::AreReadyForLocalSolve`: true when every node of
    /// `branch` is a leaf (a Bezier-surface node whose children are not
    /// Bezier-surface nodes counts as a leaf).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a null tree or node of `branch` or more than
    /// [`MAX_SOLVER_TREES`] trees.
    pub fn are_ready_for_local_solve(&self, branch: &Branch) -> Result<bool> {
        self.check_num_trees("geom::global_solver::GlobalSolver::are_ready_for_local_solve")?;
        for i in 0..self.num_trees {
            let node = self.branch_node(branch, i)?;
            // If we have children then are not ready
            if node.child1.is_some() {
                // False for all tree types except for surfaces
                //  which can be true when their children are gwc???
                if node.aux_type != AuxDataType::BezierSurface
                    || self.child(branch, i, false)?.aux_type == AuxDataType::BezierSurface
                {
                    return Ok(false);
                }
            }
        }
        Ok(true) // all must been leaf nodes
    }

    /// `sm_FindLargestNonLeaf`: the tree whose subdividable node of `branch`
    /// has the largest box diagonal (the last one on ties); `None` for none.
    fn find_largest_non_leaf(&self, branch: &Branch) -> Result<Option<usize>> {
        let mut found = None;
        let mut max_size = 0.0;
        for i in 0..self.num_trees {
            let node = self.branch_node(branch, i)?;
            if node.child1.is_none() {
                continue;
            }
            if node.aux_type == AuxDataType::BezierSurface
                && self.child(branch, i, false)?.aux_type != AuxDataType::BezierSurface
            {
                continue;
            }
            let vec = node.bbox.max() - node.bbox.min();
            let size = vec.length_squared();
            if size >= max_size {
                max_size = size;
                found = Some(i);
            }
        }
        Ok(found)
    }

    /// `SmGlobalSolver::FindBestSubdivisionNode`: the tree whose node of
    /// `branch` to subdivide, and whether to visit its second child first
    /// (rayfire, directed and plain minimise / maximise orderings).
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] (`SM_ERR`) when no node can be subdivided;
    /// [`SolidError::InvalidInput`] for a null tree or node of `branch`, a missing optional vector or a
    /// two-tree ordering applied to a third tree.
    pub fn find_best_subdivision_node(&self, branch: &Branch) -> Result<(usize, bool)> {
        const OP: &str = "geom::global_solver::GlobalSolver::find_best_subdivision_node";
        self.check_num_trees(OP)?;
        // check for failure
        let Some(best) = self.find_largest_non_leaf(branch)? else {
            return Err(SolidError::sm_err(OP));
        };

        if self.operation == SolverOperation::Rayfire {
            let sub1 = self.child(branch, 1, false)?.bbox;
            let sub2 = self.child(branch, 1, true)?.bbox;

            // Use mid point of box to determine which one is best choice
            let p_mid1 = sub1.evaluate(0.5, 0.5, 0.5);
            let p_mid2 = sub2.evaluate(0.5, 0.5, 0.5);

            let line_pnt = self.bbox(branch, 0)?.min();
            let line_vec = self.opt_vector(0)?;

            let param1 = line_closest_point(line_pnt, line_vec, p_mid1);
            let param2 = line_closest_point(line_pnt, line_vec, p_mid2);

            // Swap nodes in child branches when the second is nearer
            return Ok((best, param2 < param1));
        }

        if self.operation == SolverOperation::Signed3dDirectedMinimize {
            let other = self.bbox(branch, other_index(best)?)?;
            let sub1 = self.child(branch, best, false)?.bbox;
            let sub2 = self.child(branch, best, true)?.bbox;

            // Use mid point of box to determine which one is best choice
            let p_mid1 = sub1.evaluate(0.5, 0.5, 0.5);
            let p_mid2 = sub2.evaluate(0.5, 0.5, 0.5);

            let line_pnt = other.evaluate(0.5, 0.5, 0.5);
            let line_vec = self.opt_vector(0)?;

            let param1 = line_closest_point(line_pnt, line_vec, p_mid1);
            let param2 = line_closest_point(line_pnt, line_vec, p_mid2);

            // Swap nodes in child branches when the second is nearer
            return Ok((best, param2 < param1));
        }

        if self.num_trees == 1 {
            return Ok((best, false));
        }

        // See if we can optimize based on distance between boxes
        let minimize = matches!(
            self.operation,
            SolverOperation::Minimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::SignedDirectedMinimize
                | SolverOperation::ProjectedMinimize
        );
        if minimize || self.operation.is_plain_maximize() {
            let other = self.bbox(branch, other_index(best)?)?;
            let sub1 = self.child(branch, best, false)?.bbox;
            let sub2 = self.child(branch, best, true)?.bbox;

            // Use mid point of box to determine which one is best choice
            let p_mid1 = sub1.evaluate(0.5, 0.5, 0.5);
            let p_mid2 = sub2.evaluate(0.5, 0.5, 0.5);
            let p_other = other.evaluate(0.5, 0.5, 0.5);
            let v1 = p_mid1 - p_other;
            let v2 = p_mid2 - p_other;
            // Swap nodes in child branches when the second is the better choice
            let swap = if minimize {
                v2.length_squared() < v1.length_squared()
            } else {
                v2.length_squared() > v1.length_squared()
            };
            return Ok((best, swap));
        }

        Ok((best, false))
    }

    /// `SmGlobalSolver::LocalSolve`: with no context the best the base solver
    /// can do is record the nodes of `branch` as a
    /// [`SolutionType::NodesOnly`] solution. Returns whether the branch needs
    /// more subdivision (never).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for more than [`MAX_SOLVER_TREES`] trees.
    pub fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.check_num_trees("geom::global_solver::GlobalSolver::local_solve")?;
        let mut solution = Solution::new();
        solution.solution_type = SolutionType::NodesOnly;
        for (slot, node) in solution.nodes.iter_mut().zip(branch).take(self.num_trees) {
            *slot = *node;
        }
        self.solutions.add(&solution);

        // How can we need more subdivision with no context
        Ok(false)
    }

    /// `SmGlobalSolver::IdenticalSolutions`: whether two solutions are the
    /// same (same counts, type, objects, solution values within
    /// `SM_EFF_ZERO_SQRT` and parameters within a scaled zero or `1e-4` of the
    /// parameter interval), and for identical ones the one with the smaller
    /// solution value.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for counts past the fixed solution arrays;
    /// [`SolidError::Unported`] for parameter domains of unported objects.
    pub fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        self.identical_solutions_in(sol1, sol2, &OwnDomains)
    }

    /// [`GlobalSolver::identical_solutions`] reading the parameter intervals of the
    /// solutions' objects through `domains` ([`Solution::parameter_interval_in`]).
    ///
    /// # Errors
    ///
    /// As [`GlobalSolver::identical_solutions`].
    pub fn identical_solutions_in(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
        domains: &dyn ParameterDomains<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        const OP: &str = "geom::global_solver::GlobalSolver::identical_solutions";

        // different ObjectCounts, VariableCounts, SolutionValues, SolutionTypes
        if sol1.num_objects != sol2.num_objects
            || sol1.num_variables != sol2.num_variables
            || sol1.solution_type != sol2.solution_type
            || EFF_ZERO_SQRT < (sol1.start.solution_value - sol2.start.solution_value).abs()
        {
            return Ok((SolutionPairType::DiffCounts, None));
        }

        // unequal ObjectPointers
        for j in 0..sol1.num_objects {
            let (Some(a), Some(b)) = (sol1.objects.get(j), sol2.objects.get(j)) else {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "solution object count exceeds the fixed arrays",
                });
            };
            if a != b {
                return Ok((SolutionPairType::DiffObjects, None));
            }
        }

        // ParameterValues off by more than ScaledZero or more than 1/10000th of the parameter length
        for i in 0..sol1.num_variables {
            let ivl1 = sol1.parameter_interval_in(i, domains)?;
            let ivl2 = sol2.parameter_interval_in(i, domains)?;
            let length1 = ivl1.map_or(0.0, |ivl| 0.0001 * ivl.length());
            let length2 = ivl2.map_or(0.0, |ivl| 0.0001 * ivl.length());

            // (Note, SM_EFF_ZERO is too small to use here.)
            let s1 = sol1.start.param(i)?;
            let scaled_zero = max3((1.0 + s1.abs()).abs() * 100.0 * EFF_ZERO, length1, length2);

            if (s1 - sol2.start.param(i)?).abs() > scaled_zero
                || (sol1.solution_type == SolutionType::RangeOfValues
                    && (sol1.end.param(i)? - sol2.end.param(i)?).abs() > scaled_zero)
            {
                return Ok((SolutionPairType::DiffParameters, None));
            }
        }

        // All tests passed - identical Solutions

        // prefer the solution with the tighter solution
        let preferred = if sol1.start.solution_value < sol2.start.solution_value {
            PreferredSolution::First
        } else {
            PreferredSolution::Second
        };
        Ok((SolutionPairType::Identical, Some(preferred)))
    }
}

impl<O: SolutionObject> Default for GlobalSolver<'_, O> {
    fn default() -> Self {
        Self::new()
    }
}

/// `1 - index` of a two-tree branch; upstream wraps around (and then reads
/// out of bounds) for index 2.
fn other_index(index: usize) -> Result<usize> {
    1usize.checked_sub(index).ok_or(SolidError::InvalidInput {
        operation: "geom::global_solver::GlobalSolver::find_best_subdivision_node",
        reason: "two-tree operation subdivides a third tree",
    })
}

/// The largest squared length of the four offsets `center ± dir1 * radius`,
/// `center ± dir2 * radius`, each perspective projected relative to `base`
/// (the radius loops of the perspective test in `BranchMayContainAnswers`).
/// `view` is the plane point, the unit plane normal and the eye point.
fn max_projected_radius_sq(
    center: Point3,
    base: Point3,
    dirs: [Vec3; 2],
    radius: f64,
    view: [Vec3; 3],
) -> f64 {
    let [dir1, dir2] = dirs;
    let [plane_point, plane_normal, eye_point] = view;
    let mut new_rad_max_sq = 0.0;
    for offset in [dir1 * radius, -dir1 * radius, dir2 * radius, -dir2 * radius] {
        let rad_vec = (center + offset).perspective_project_to_plane(
            base,
            plane_point,
            plane_normal,
            eye_point,
        );
        let new_rad_sq = rad_vec.length_squared();
        if new_rad_sq > new_rad_max_sq {
            new_rad_max_sq = new_rad_sq;
        }
    }
    new_rad_max_sq
}

/// `SmGlobalSolver::Subdivide`: two branches that replace the node chosen by
/// [`GlobalSolve::find_best_subdivision_node`] with its first and second
/// child (in swapped order when it asks for that).
///
/// # Errors
///
/// Errors of [`GlobalSolve::find_best_subdivision_node`]; [`SolidError::InvalidInput`]
/// for a null node of `original`.
pub fn subdivide<'t, O, S>(solver: &S, original: &Branch) -> Result<(Branch, Branch)>
where
    O: SolutionObject + 't,
    S: GlobalSolve<'t, O> + ?Sized,
{
    let (best, swap_traversal_order) = solver.find_best_subdivision_node(original)?;
    let base = solver.solver();
    base.check_num_trees("geom::global_solver::subdivide")?;

    // Do default subdivision
    let mut child_branch1: Branch = [None; MAX_SOLVER_TREES];
    let mut child_branch2: Branch = [None; MAX_SOLVER_TREES];
    for i in 0..base.num_trees {
        if let (Some(c1), Some(c2), Some(o)) = (
            child_branch1.get_mut(i),
            child_branch2.get_mut(i),
            original.get(i),
        ) {
            *c1 = *o;
            *c2 = *o;
        }
    }

    // Now make two subbranches take the children of the subdivision node.
    let node = base.branch_node(original, best)?;
    let mut first = node.child1;
    let mut second = node.child2;

    // If we need to swap the traversal order to take Child2 first
    if swap_traversal_order {
        std::mem::swap(&mut first, &mut second);
    }
    if let (Some(c1), Some(c2)) = (child_branch1.get_mut(best), child_branch2.get_mut(best)) {
        *c1 = first;
        *c2 = second;
    }

    Ok((child_branch1, child_branch2))
}

/// `sm_CombineMinMax`: the union of two solutions when they combine (a range
/// absorbing a point at or inside it, overlapping or, when
/// `combine_contiguous_ranges`, touching ranges). For an intersection whose
/// point solution lies inside a range and is tighter than the range ends, the
/// two are instead rewritten in place into two contiguous ranges and `None` is
/// returned.
fn combine_min_max<O: SolutionObject>(
    operation: SolverOperation,
    min: &mut Solution<O>,
    max: &mut Solution<O>,
    combine_contiguous_ranges: bool,
) -> Result<Option<Solution<O>>> {
    // when 1st solution is rangeOfValues
    if min.solution_type == SolutionType::RangeOfValues {
        // and 2nd solution is a point - crMax->Start to crMin
        if max.solution_type == SolutionType::SingleValue {
            // get classification
            let class1 = min.classify_solution_end(max, true)?;

            // don't combine incompatible solutions
            if class1 == EndClass::Inconsistent {
                return Ok(None);
            }

            // merge points into ranges when appropriate
            if class1 == EndClass::AtStart
                || class1 == EndClass::AtEnd
                || (class1 == EndClass::Within && operation != SolverOperation::Intersect)
            {
                return Ok(Some(*min));
            } else if class1 == EndClass::Within && operation == SolverOperation::Intersect {
                // arrive here when inserting a intersection point solution into a range solution

                // when point solution is tight -
                // convert two solutions into two contiguous range solutions
                let pt_dev = max.start.solution_value;
                let min_dev = min.start.solution_value;
                let max_dev = min.end.solution_value;

                // when point solution is better than range endPts
                if pt_dev < min_dev * 100.0 || pt_dev < max_dev * 100.0 {
                    // change point solution into a range that starts at the point solution
                    max.solution_type = SolutionType::RangeOfValues;
                    max.end = min.end;

                    // change min solution into a range that stops at the point solution
                    min.end = max.start;
                    return Ok(None);
                }
                // combine point into range
                return Ok(Some(*min));
            }
        } else {
            // MaxSolution is a range: classify Max Start and End against Min
            let class1 = min.classify_solution_end(max, true)?;
            let class2 = min.classify_solution_end(max, false)?;

            // don't combine incompatible solutions
            if class1 == EndClass::Inconsistent || class2 == EndClass::Inconsistent {
                return Ok(None);
            }

            // don't combine disjoint intervals
            if class1 == EndClass::AboveEnd || class2 == EndClass::BelowStart {
                return Ok(None);
            }

            // don't combine contiguous intervals unless asked
            if !combine_contiguous_ranges
                && ((class1 == EndClass::AtEnd && class2 == EndClass::AboveEnd)
                    || (class1 == EndClass::BelowStart && class2 == EndClass::AtStart))
            {
                return Ok(None);
            }

            // Max is contained within Min
            if class1 == EndClass::Within && class2 == EndClass::Within {
                return Ok(Some(*min));
            }

            // The remaining cases require the combination of the intervals
            let mut combined = *min;
            if class1 == EndClass::BelowStart {
                combined.start = max.start;
            }
            if class2 == EndClass::AboveEnd {
                combined.end = max.end;
            }
            return Ok(Some(combined));
        }
    }

    // The only remaining case to worry about is the case of min being a single
    // value which falls into the crMax interval.
    if max.solution_type == SolutionType::RangeOfValues {
        let class1 = max.classify_solution_end(min, true)?;
        if class1.is_at_or_within() {
            return Ok(Some(*max));
        }
        // (upstream's following tests for -1, 1 and 0 can no longer succeed)
    }

    // all done - no combining
    Ok(None)
}

/// `SmGlobalSolver::SolveBranch`, the base behaviour of
/// [`GlobalSolve::solve_branch`] (callable from an override): culls `branch`,
/// locally solves it when ready and recurses into its subdivisions, all through
/// `solver`'s hooks.
///
/// # Errors
///
/// Those of the hooks; [`SolidError::InvalidInput`] past `MAX_SOLVE_DEPTH + 1`
/// nested calls.
pub fn solve_branch<'t, O, S>(solver: &mut S, branch: &Branch) -> Result<()>
where
    O: SolutionObject + 't,
    S: GlobalSolve<'t, O> + ?Sized,
{
    let depth = solver.solver().solve_depth;
    if depth > MAX_SOLVE_DEPTH {
        return Err(SolidError::invalid_input(
            "geom::global_solver::solve_branch",
            "solve nesting exceeds MAX_SOLVE_DEPTH",
        ));
    }
    solver.solver_mut().solve_depth = depth + 1;
    let result = solve_branch_body(solver, branch);
    solver.solver_mut().solve_depth = depth;
    result
}

/// The body of [`solve_branch`] at one nesting level.
fn solve_branch_body<'t, O, S>(solver: &mut S, branch: &Branch) -> Result<()>
where
    O: SolutionObject + 't,
    S: GlobalSolve<'t, O> + ?Sized,
{
    // (not upstream) bound the work: see MAX_SOLVE_BRANCHES
    let s = solver.solver_mut();
    s.branches_visited += 1;
    if s.branches_visited > MAX_SOLVE_BRANCHES {
        return Err(SolidError::invalid_input(
            "geom::global_solver::solve_branch",
            "solve visits more than MAX_SOLVE_BRANCHES branches",
        ));
    }

    // no more work - these nodes no longer contain possible answer
    if !solver.branch_may_contain_answers(branch)? {
        return Ok(());
    }

    // check for leaf nodes
    if solver.are_ready_for_local_solve(branch)? {
        // make the leafNode/leafNode call to Local solver.
        let needs_more_subdivision = solver.local_solve(branch)?;

        // done with this branch when LocalSolver is done
        if !needs_more_subdivision {
            return Ok(());
        }
    }

    // get the children to this solverNode
    let (child1, child2) = solver.subdivide(branch)?;

    // recurse for each childNode
    solver.solve_branch(&child1)?;
    solver.solve_branch(&child2)
}

/// The overridable interface of a global solver (the virtual methods of
/// `SmGlobalSolver`). Implementors expose their [`GlobalSolver`] state and
/// override the hooks they specialise; the base behaviour of each hook is
/// the inherent method of the same name on [`GlobalSolver`].
pub trait GlobalSolve<'t, O: SolutionObject + 't> {
    /// The solver state.
    fn solver(&self) -> &GlobalSolver<'t, O>;

    /// The solver state, mutable.
    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O>;

    /// `SmTol::GetZoneTol3d(const SmObject*)` of a non-null solution object (read by
    /// `SmSolution::GetObjectTolerance` in [`GlobalSolve::add_sorted_solution`] for
    /// [`SolverOperation::Rayfire`]). The handles are opaque here: a solver whose
    /// caller can resolve them to their B-rep entities (the topology solver, or a
    /// curve or surface solve given [`ObjectZoneTol`]) overrides this.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] by default (no resolver for the handle).
    fn object_zone_tol_3d(&self, object: O) -> Result<f64> {
        resolve_object_zone_tol_3d(None, object)
    }

    /// `BranchMayContainAnswers`: false when `branch` provably holds no
    /// answer.
    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        self.solver().branch_may_contain_answers(branch)
    }

    /// `AreReadyForLocalSolve`: true when `branch` is ready for
    /// [`GlobalSolve::local_solve`].
    fn are_ready_for_local_solve(&self, branch: &Branch) -> Result<bool> {
        self.solver().are_ready_for_local_solve(branch)
    }

    /// `FindBestSubdivisionNode`: the tree index to subdivide and whether to
    /// visit the second child first.
    fn find_best_subdivision_node(&self, branch: &Branch) -> Result<(usize, bool)> {
        self.solver().find_best_subdivision_node(branch)
    }

    /// `Subdivide`: the two child branches of `branch`.
    fn subdivide(&mut self, branch: &Branch) -> Result<(Branch, Branch)> {
        subdivide(self, branch)
    }

    /// `LocalSolve`: solve `branch` locally, adding solutions; returns whether
    /// the branch needs more subdivision.
    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.solver_mut().local_solve(branch)
    }

    /// `IdenticalSolutions`: how `sol1` and `sol2` relate and, when
    /// identical, which one to keep (`None` for no preference).
    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        self.solver().identical_solutions(sol1, sol2)
    }

    /// `SolveBranch`: recursively culls, subdivides and locally solves
    /// `branch`.
    fn solve_branch(&mut self, branch: &Branch) -> Result<()> {
        solve_branch(self, branch)
    }

    /// `SolveIt`: runs the solve described by `request` from the top nodes of
    /// the trees, accumulating into `solutions` (whose existing entries are
    /// kept).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for more than [`MAX_SOLVER_TREES`] trees, a
    /// missing tree or top node, or a null node the traversal would
    /// dereference; errors of the hooks are propagated.
    fn solve_it(
        &mut self,
        request: &SolveRequest<'_>,
        solutions: &mut SolutionArray<O>,
    ) -> Result<()> {
        const OP: &str = "geom::global_solver::GlobalSolve::solve_it";
        let SolveRequest {
            operation,
            solution_requested,
            tolerance_3d,
            best_answer_so_far_sq,
            opt_vectors,
        } = *request;
        let s = self.solver_mut();

        // m_bProjectedOperation value from eSolverOperation
        s.projected_operation = matches!(
            operation,
            SolverOperation::SignedDirectedMinimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::ProjectedMinimize
                | SolverOperation::SignedAngleMinimize
                | SolverOperation::DirectedMaximize
                | SolverOperation::ProjectedMaximize
                | SolverOperation::SignedPivotMinimize
                | SolverOperation::Signed3dDirectedMaximize
        );

        // m_eOperationCategory value from eSolverOperation
        s.category = match operation {
            SolverOperation::DirectedMinimize
            | SolverOperation::ProjectedMinimize
            | SolverOperation::Minimize
            | SolverOperation::AngleMinimize
            | SolverOperation::Rayfire => OperationCategory::Minimize,

            SolverOperation::DirectedMaximize
            | SolverOperation::ProjectedMaximize
            | SolverOperation::Maximize => OperationCategory::Maximize,

            SolverOperation::SignedDirectedMinimize
            | SolverOperation::SignedAngleMinimize
            | SolverOperation::SignedPivotMinimize
            | SolverOperation::Signed3dDirectedMinimize
            | SolverOperation::Signed3dDirectedMaximize => OperationCategory::Signed,

            _ => OperationCategory::Other,
        };

        // load input values into this SmGlobalSolver member values
        s.operation = operation;
        s.solution_requested = solution_requested;
        s.best_answer_so_far_sq = best_answer_so_far_sq;
        s.opt_vectors = opt_vectors.to_vec();
        s.tolerance_3d = tolerance_3d;

        // special case dBestAnswerSoFarSq
        if s.best_answer_so_far_sq == BIG_DOUBLE
            && (operation == SolverOperation::Maximize
                || operation == SolverOperation::DirectedMaximize)
        {
            s.best_answer_so_far_sq = 0.0;
        }

        // init start branches with tree top nodes
        s.check_num_trees(OP)?;
        let mut start_branch: Branch = [None; MAX_SOLVER_TREES];
        for (i, slot) in start_branch.iter_mut().enumerate().take(s.num_trees) {
            *slot = Some(s.tree(i)?.top_node().ok_or(null_node(OP))?);
        }

        // bound the SolveBranch recursion (not upstream: see MAX_SOLVE_DEPTH)
        s.check_solve_depth(&start_branch)?;

        // store the output array, execute the solver tree/tree iteration
        std::mem::swap(&mut s.solutions, solutions);
        s.branches_visited = 0;
        let result = self.solve_branch(&start_branch);
        std::mem::swap(&mut self.solver_mut().solutions, solutions);
        result
    }

    /// `AddSortedSolution`: inserts `solution` into the solutions in
    /// `sort_key` order unless it is a `SM_BIG_DOUBLE` solution, a duplicate
    /// (which may replace the stored one) or farther than the tolerance from
    /// the best answer; then combines overlapping (and, when
    /// `combine_contiguous_ranges`, touching) ranges and absorbed points,
    /// keeps only the best for [`SolutionRequested::Single`], and for the
    /// minimising and maximising operations drops trailing solutions not
    /// within tolerance of the first.
    fn add_sorted_solution(
        &mut self,
        solution: &Solution<O>,
        sort_key: SortKey,
        combine_contiguous_ranges: bool,
    ) -> Result<()> {
        // skip BigSolutions
        if solution.start.solution_value == BIG_DOUBLE {
            return Ok(());
        }

        let s = self.solver_mut();
        let op = s.operation;
        let tol = s.tolerance_3d;

        // locals
        let sol_value = solution.start.solution_value;
        let this_sol_sq = sol_value * sol_value;
        let mut better_than_best = false;
        let mut far_from_best = false;

        // Note, using the squared value for BestAnswer (to avoid sqrt operations?)
        // causes several extra sqrt operations below, so do only one here:
        let best_so_far = sqrt(s.best_answer_so_far_sq.abs());

        // When looking for a best solution - compare this solution to best
        if s.category == OperationCategory::Signed {
            // remember if solution is better than the current best
            let sgn = if sol_value >= 0.0 { 1.0 } else { -1.0 };
            better_than_best = (sgn * this_sol_sq) < s.best_answer_so_far_sq;

            // remember if solution is worse than current best + tolerance
            let sign = if s.best_answer_so_far_sq < 0.0 {
                -1.0
            } else {
                1.0
            };
            let best_ans_with_tol = sign * best_so_far + tol;
            far_from_best = sol_value > best_ans_with_tol;
        }

        // remember Best solution values - cull solutions not within tol of best solution
        if better_than_best {
            s.best_answer_so_far_sq = this_sol_sq;
        } else if far_from_best {
            return Ok(());
        }

        match op {
            SolverOperation::Minimize
            | SolverOperation::DirectedMinimize
            | SolverOperation::ProjectedMinimize
            | SolverOperation::Rayfire => {
                better_than_best = this_sol_sq < s.best_answer_so_far_sq;
                let best_ans_with_tol = best_so_far + tol;
                far_from_best = sol_value > best_ans_with_tol;
            }
            SolverOperation::Maximize
            | SolverOperation::DirectedMaximize
            | SolverOperation::ProjectedMaximize => {
                better_than_best = this_sol_sq > s.best_answer_so_far_sq;
                let best_ans_with_tol = best_so_far - tol;
                far_from_best = sol_value < best_ans_with_tol;
            }
            _ => {}
        }

        // remember Best solution values - cull solutions not within tol of best solution
        if better_than_best {
            s.best_answer_so_far_sq = this_sol_sq;
        } else if far_from_best {
            return Ok(());
        }

        // Now do insertion sort
        let mut found_place = false;
        let mut insertion_index = self.solver().solutions.len();

        // for every current solution
        let num_sols = self.solver().solutions.len();
        for ii in 0..num_sols {
            let test = self.solver().solutions.get_at(ii)?;

            // don't add duplicate solutions (SolValues within ZeroSqrt and ParamValues within ScaledZero)
            let (pair, preferred) = self.identical_solutions(solution, &test)?;
            if pair == SolutionPairType::Identical {
                // save the preferred solution if possible, else better solution - improve tolerances
                let keep = match preferred {
                    Some(PreferredSolution::First) => Some(solution),
                    Some(PreferredSolution::Second) => Some(&test),
                    None if solution.start.solution_value.abs()
                        < test.start.solution_value.abs() =>
                    {
                        Some(solution)
                    }
                    None => None,
                };
                if let Some(keep) = keep {
                    self.solver_mut().solutions.set_at(ii, keep)?;
                }
                return Ok(());
            }

            let mut by_parameters = false;
            match sort_key {
                SortKey::BySolutionValue => {
                    if op.is_plain_maximize() {
                        // Best answer is biggest
                        if test.start.solution_value < solution.start.solution_value {
                            insertion_index = ii;
                            found_place = true;
                        }
                    } else if test.start.solution_value > solution.start.solution_value {
                        // Best answer is the smallest
                        insertion_index = ii;
                        found_place = true;
                    }
                }

                SortKey::ByFirstTwoParameters => {
                    if test.start.param(0)? > solution.start.param(0)?
                        && test.start.param(1)? > solution.start.param(1)?
                    {
                        insertion_index = ii;
                        found_place = true;
                    }
                }

                SortKey::ByFirstParameter => {
                    let t0 = test.start.param(0)?;
                    let s0 = solution.start.param(0)?;
                    if t0 > s0 {
                        insertion_index = ii;
                        found_place = true;
                    }

                    // Do additional sorting based on intervals not just start and end values
                    // This should put single values in front of intervals with same start
                    // This will help us to process closed curves better
                    let scale = (1.0 + t0) * EFF_ZERO;
                    if (t0 - s0).abs() < scale && test.solution_type == SolutionType::RangeOfValues
                    {
                        insertion_index = ii;
                        found_place = true;
                    }
                }

                SortKey::ByValueWithParameter => {
                    // when values are different - sort by value else sort by parameter
                    if is_zero(test.start.solution_value - solution.start.solution_value) {
                        // else fall through to next case and sort by parameters
                        by_parameters = true;
                    } else if op.is_plain_maximize() {
                        // Best answer is biggest
                        if test.start.solution_value < solution.start.solution_value {
                            insertion_index = ii;
                            found_place = true;
                        }
                    } else if test.start.solution_value > solution.start.solution_value {
                        // best answer is smallest
                        insertion_index = ii;
                        found_place = true;
                    }
                }

                SortKey::ByAllParameters => by_parameters = true,
            }

            if by_parameters {
                // The following should sort consecutive things together even if there
                // is tangency and periodic cases.  Basically it says that if a tie
                // exists then look at the next parameter to settle the deal.
                let mut found_one = false;
                for j in 0..test.num_variables {
                    let tj = test.start.param(j)?;
                    let sj = solution.start.param(j)?;
                    // Take default answer for this parameter
                    if !found_one {
                        // keep first answer
                        if tj > sj {
                            found_one = true;
                            insertion_index = ii;
                            found_place = true;
                        } else if tj < sj {
                            found_one = true;
                            found_place = false;
                        }
                    }

                    // This tol is bigger than the IdenticalSolutions() tolerance.
                    let scale = (1.0 + tj) * EFF_ZERO_SQRT;
                    if (tj - sj).abs() > scale {
                        // If answers are close to being same continue looking until you find
                        // an answer with acceptable deviation or until we have compared all of them
                        break;
                    }

                    // If we make it here then we have a tie so far.
                    // Look to see if the next one exists differentiates the answers
                    if j + 1 < test.num_variables {
                        let tn = test.start.param(j + 1)?;
                        let sn = solution.start.param(j + 1)?;
                        let scale = (1.0 + tn) * EFF_ZERO_SQRT;
                        if (tn - sn).abs() > scale {
                            if tn > sn {
                                // answer differentiates and should be inserted
                                insertion_index = ii;
                                found_place = true;
                            } else {
                                // answer differentiates and should not be inserted
                                found_place = false;
                            }
                            break;
                        }
                    }
                }
            }

            // If found an answer get out of loop
            if found_place {
                break;
            }
        }

        let s = self.solver_mut();
        if found_place {
            // Insert it
            s.solutions.insert_at(insertion_index, solution, 1);
        } else {
            // Just add it to the end
            s.solutions.add(solution);
        }

        // Once done go back and combine sequential solutions
        // Note that because things are not necessarily sequential
        // we have to compare every thing in the list to every other
        // thing in the list.
        let mut done = false;
        while !done && s.solutions.len() < 100 && !s.solutions.is_empty() {
            done = true;

            // For every solution
            let mut k = 0;
            while k + 1 < s.solutions.len() {
                // for all other solutions
                let mut j = k + 1;
                while j < s.solutions.len() {
                    // try to combine the solutions.
                    //   contiguous and overlapping intervals get unioned.
                    //   points contained in intervals get absorbed.
                    //   disjoint intervals remain disjoint.
                    //   point solutions remain distinct.
                    let mut min = s.solutions.get_at(k)?;
                    let mut max = s.solutions.get_at(j)?;
                    let combined =
                        combine_min_max(op, &mut min, &mut max, combine_contiguous_ranges)?;
                    if let Some(combined) = combined {
                        s.solutions.set_at(k, &combined)?;
                        s.solutions.remove_at(j, 1);
                        done = false;
                        break;
                    }
                    // (the split of a range around a tight point is made in place)
                    s.solutions.set_at(k, &min)?;
                    s.solutions.set_at(j, &max)?;
                    j += 1;
                }

                // start over after combining solutions
                if !done {
                    break;
                }
                k += 1;
            }
        }

        // If requesting only a single answer just keep the best
        if s.solution_requested == SolutionRequested::Single {
            // This assumes that we sorted by a meaningful
            // value and that the first answer is the best.
            s.solutions.set_size(1);
        }

        // If we are minimizing or maximizing only keep those
        // solutions which are within tolerance of the best.
        if matches!(
            op,
            SolverOperation::Minimize
                | SolverOperation::Maximize
                | SolverOperation::Rayfire
                | SolverOperation::Signed3dDirectedMinimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::DirectedMaximize
                | SolverOperation::ProjectedMinimize
                | SolverOperation::ProjectedMaximize
                | SolverOperation::SignedDirectedMinimize
                | SolverOperation::SignedPivotMinimize
        ) {
            let first = s.solutions.get_at(0)?;
            let mut last_index = s.solutions.len().saturating_sub(1);
            let mut this_tol = tol;

            // To match old behavior, we check the distance between
            // intersection points on the ray.
            if op == SolverOperation::Rayfire {
                let last = s.solutions.get_at(last_index)?;
                let first_tol = first.object_tolerance_with(0, |o| self.object_zone_tol_3d(o))?;
                let last_tol = last.object_tolerance_with(0, |o| self.object_zone_tol_3d(o))?;
                this_tol = max2(tol, first_tol + last_tol);
            }
            let s = self.solver_mut();

            while s.solutions.len() > 1
                && (first.start.solution_value
                    - s.solutions.get_at(last_index)?.start.solution_value)
                    .abs()
                    > this_tol
            {
                s.solutions.set_size(s.solutions.len() - 1);
                last_index = s.solutions.len() - 1;
            }
        }

        Ok(())
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for GlobalSolver<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        self
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;
    use crate::geom::tree::TreeNode;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj;

    impl SolutionObject for Obj {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Other
        }
    }

    fn leaf(min: [f64; 3], max: [f64; 3]) -> TreeNode<Obj> {
        let mut n = TreeNode::new();
        n.bbox = Extent3d::from_points(Vec3::from_array(min), Vec3::from_array(max));
        n
    }

    /// A two-level tree: a top box over [x0, x0+2] split in two along x.
    fn split_tree(x0: f64) -> Tree<Obj> {
        let mut t = Tree::new(None);
        let top = t.add_node(leaf([x0, 0.0, 0.0], [x0 + 2.0, 1.0, 1.0]));
        let c1 = t.add_node(leaf([x0, 0.0, 0.0], [x0 + 1.0, 1.0, 1.0]));
        let c2 = t.add_node(leaf([x0 + 1.0, 0.0, 0.0], [x0 + 2.0, 1.0, 1.0]));
        if let Ok(n) = t.node_mut(top) {
            n.child1 = Some(c1);
            n.child2 = Some(c2);
        }
        for c in [c1, c2] {
            if let Ok(n) = t.node_mut(c) {
                n.parent = Some(top);
            }
        }
        t.set_top_node(Some(top));
        t
    }

    fn single(value: f64, p0: f64) -> Solution<Obj> {
        let mut s = Solution::new();
        s.solution_type = SolutionType::SingleValue;
        s.num_variables = 1;
        s.start.solution_value = value;
        s.start.parameters[0] = p0;
        s
    }

    fn range(v0: f64, p0: f64, v1: f64, p1: f64) -> Solution<Obj> {
        let mut s = single(v0, p0);
        s.solution_type = SolutionType::RangeOfValues;
        s.end.solution_value = v1;
        s.end.parameters[0] = p1;
        s
    }

    #[test]
    fn codes_round_trip() {
        for op in SolverOperation::ALL {
            assert_eq!(SolverOperation::from_code(op.code()), Some(op));
        }
        assert_eq!(SolverOperation::ProjectedTangentThroughPoint.code(), 24);
        assert_eq!(SolutionRequested::Nodes.code(), 3);
        assert_eq!(OperationCategory::Other.code(), 3);
        assert_eq!(SolutionPairType::DiffParameters.code(), 3);
    }

    #[test]
    fn solve_depth_is_restored_after_a_recursive_error() {
        struct Repeating {
            base: GlobalSolver<'static, Obj>,
            stop: bool,
        }
        impl GlobalSolve<'static, Obj> for Repeating {
            fn solver(&self) -> &GlobalSolver<'static, Obj> {
                &self.base
            }

            fn solver_mut(&mut self) -> &mut GlobalSolver<'static, Obj> {
                &mut self.base
            }

            fn branch_may_contain_answers(&mut self, _: &Branch) -> Result<bool> {
                Ok(!self.stop)
            }

            fn are_ready_for_local_solve(&self, _: &Branch) -> Result<bool> {
                Ok(false)
            }

            fn subdivide(&mut self, branch: &Branch) -> Result<(Branch, Branch)> {
                Ok((*branch, *branch))
            }
        }

        let mut solver = Repeating {
            base: GlobalSolver::new(),
            stop: false,
        };
        // Exercise the last two recursion levels without filling the test
        // thread's stack. A cyclic branch must return an error and unwind both.
        solver.base.solve_depth = MAX_SOLVE_DEPTH - 1;
        let branch = [None; MAX_SOLVER_TREES];
        assert!(matches!(
            solver.solve_branch(&branch),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(solver.base.solve_depth, MAX_SOLVE_DEPTH - 1);

        // The same solver remains usable after the error.
        solver.base.solve_depth = 0;
        solver.stop = true;
        solver.solve_branch(&branch).unwrap();
        assert_eq!(solver.base.solve_depth, 0);
    }

    #[test]
    fn intersect_collects_overlapping_leaf_pairs() {
        let a = split_tree(0.0);
        let b = split_tree(1.5);
        let mut solver = GlobalSolver::with_trees(&a, &b, 0);
        let mut sols = SolutionArray::new();
        solver
            .solve_it(
                &SolveRequest {
                    operation: SolverOperation::Intersect,
                    best_answer_so_far_sq: 0.0,
                    ..SolveRequest::default()
                },
                &mut sols,
            )
            .unwrap();
        // a = [0,1],[1,2]; b = [1.5,2.5],[2.5,3.5]: only a.c2 meets b.c1
        assert_eq!(sols.len(), 1);
        let s = sols.get(0).unwrap();
        assert_eq!(s.solution_type, SolutionType::NodesOnly);
        assert_eq!(s.nodes[0], Some(NodeId(2)));
        assert_eq!(s.nodes[1], Some(NodeId(1)));
        assert_eq!(solver.category, OperationCategory::Other);
        assert!(!solver.projected_operation);
    }

    #[test]
    fn maximize_resets_big_best_answer() {
        let a = split_tree(0.0);
        let b = split_tree(5.0);
        let mut solver = GlobalSolver::with_trees(&a, &b, 0);
        let mut sols = SolutionArray::new();
        solver
            .solve_it(
                &SolveRequest {
                    operation: SolverOperation::Maximize,
                    best_answer_so_far_sq: BIG_DOUBLE,
                    ..SolveRequest::default()
                },
                &mut sols,
            )
            .unwrap();
        assert_eq!(solver.best_answer_so_far_sq, 0.0);
        assert_eq!(sols.len(), 4);
        // maximize visits the far child first
        assert_eq!(sols.get(0).unwrap().nodes[0], Some(NodeId(1)));
    }

    #[test]
    fn missing_top_node_is_an_error() {
        let a: Tree<Obj> = Tree::new(None);
        let mut solver = GlobalSolver::with_tree(&a, 0);
        let mut sols = SolutionArray::new();
        assert!(solver
            .solve_it(
                &SolveRequest {
                    operation: SolverOperation::Find,
                    best_answer_so_far_sq: 0.0,
                    ..SolveRequest::default()
                },
                &mut sols,
            )
            .is_err());
    }

    /// Fuzz regression: a child link back to the top node made `SolveBranch`
    /// recurse until the stack overflowed; now the solve is refused up front.
    #[test]
    fn cyclic_or_too_deep_trees_are_errors() {
        let request = SolveRequest {
            operation: SolverOperation::Intersect,
            ..SolveRequest::default()
        };
        let mut cyclic = split_tree(0.0);
        if let Ok(n) = cyclic.node_mut(NodeId(1)) {
            n.child1 = Some(NodeId(0));
            n.child2 = Some(NodeId(2));
        }
        let mut solver = GlobalSolver::with_tree(&cyclic, 0);
        let mut sols = SolutionArray::new();
        assert!(matches!(
            solver.solve_it(&request, &mut sols),
            Err(SolidError::InvalidInput { .. })
        ));

        // a chain of MAX_SOLVE_DEPTH + 1 levels (shared leaf siblings)
        let mut deep = Tree::new(None);
        let leaf_id = deep.add_node(leaf([0.0; 3], [1.0; 3]));
        let mut below = leaf_id;
        for _ in 0..=MAX_SOLVE_DEPTH {
            let mut n = leaf([0.0; 3], [1.0; 3]);
            n.child1 = Some(below);
            n.child2 = Some(leaf_id);
            below = deep.add_node(n);
        }
        deep.set_top_node(Some(below));
        let mut solver = GlobalSolver::with_tree(&deep, 0);
        assert!(matches!(
            solver.solve_it(&request, &mut sols),
            Err(SolidError::InvalidInput { .. })
        ));

        // the ordinary two-level tree passes the check
        let ok = split_tree(0.0);
        let solver = GlobalSolver::with_tree(&ok, 0);
        assert!(solver
            .check_solve_depth(&[ok.top_node(), None, None])
            .is_ok());
    }

    #[test]
    fn add_sorted_solution_sorts_and_absorbs_points() {
        let mut solver: GlobalSolver<'_, Obj> = GlobalSolver::new();
        solver.set_solver_operation(SolverOperation::Find);
        solver.set_3d_tolerance(1e-6);
        solver
            .add_sorted_solution(&single(0.0, 3.0), SortKey::ByFirstParameter, true)
            .unwrap();
        solver
            .add_sorted_solution(&single(0.0, 1.0), SortKey::ByFirstParameter, true)
            .unwrap();
        assert_eq!(solver.solutions.len(), 2);
        assert_eq!(solver.solutions.get(0).unwrap().start.parameters[0], 1.0);
        // a range over both points absorbs them
        solver
            .add_sorted_solution(&range(0.0, 0.5, 0.0, 4.0), SortKey::ByFirstParameter, true)
            .unwrap();
        assert_eq!(solver.solutions.len(), 1);
        assert_eq!(
            solver.solutions.get(0).unwrap().solution_type,
            SolutionType::RangeOfValues
        );
        // duplicates and BIG_DOUBLE solutions are skipped
        solver
            .add_sorted_solution(&single(BIG_DOUBLE, 9.0), SortKey::ByFirstParameter, true)
            .unwrap();
        assert_eq!(solver.solutions.len(), 1);
    }

    #[test]
    fn intersect_point_splits_range() {
        let mut solver: GlobalSolver<'_, Obj> = GlobalSolver::new();
        solver.set_3d_tolerance(1e-6);
        solver
            .add_sorted_solution(
                &range(1e-3, 0.0, 1e-3, 2.0),
                SortKey::ByFirstParameter,
                true,
            )
            .unwrap();
        solver
            .add_sorted_solution(&single(1e-9, 1.0), SortKey::ByFirstParameter, true)
            .unwrap();
        assert_eq!(solver.solutions.len(), 2);
        let a = solver.solutions.get(0).unwrap();
        let b = solver.solutions.get(1).unwrap();
        assert_eq!(a.end.parameters[0], 1.0);
        assert_eq!(b.solution_type, SolutionType::RangeOfValues);
        assert_eq!(b.end.parameters[0], 2.0);
    }

    #[test]
    fn minimize_keeps_best_within_tolerance() {
        let mut solver: GlobalSolver<'_, Obj> = GlobalSolver::new();
        solver.set_solver_operation(SolverOperation::Minimize);
        solver.set_best_answer_so_far_sq(BIG_DOUBLE);
        solver.set_3d_tolerance(0.1);
        for (v, p) in [(1.0, 0.0), (0.5, 1.0), (0.55, 2.0), (3.0, 3.0)] {
            solver
                .add_sorted_solution(&single(v, p), SortKey::BySolutionValue, true)
                .unwrap();
        }
        let values: Vec<f64> = solver
            .solutions
            .as_slice()
            .iter()
            .map(|s| s.start.solution_value)
            .collect();
        assert_eq!(values, vec![0.5, 0.55]);
        assert_eq!(solver.best_answer_so_far_sq, 0.25);
    }

    #[test]
    fn identical_solutions_prefers_tighter() {
        let solver: GlobalSolver<'_, Obj> = GlobalSolver::new();
        let (pair, pref) = solver
            .identical_solutions(&single(2e-7, 1.0), &single(1e-7, 1.0))
            .unwrap();
        assert_eq!(pair, SolutionPairType::Identical);
        assert_eq!(pref, Some(PreferredSolution::Second));
        let (pair, _) = solver
            .identical_solutions(&single(0.0, 1.0), &single(0.0, 1.1))
            .unwrap();
        assert_eq!(pair, SolutionPairType::DiffParameters);
    }
}

//! The advanced surface/surface intersector (`SmAdvSurfaceIntersector`): the
//! basic intersector plus the tangency and singularity handling (the Dupin
//! indicatrix of both surfaces at a point where their normals are parallel) and a
//! global solve that finds interior intersection loops (one start point per
//! intersecting leaf pair).
//!
//! Upstream: `source/SMLib/src/SmAdvSurfaceIntersector.cpp`,
//! `source/SMLib/inc/SmAdvSurfaceIntersector.h`.
//!
//! | Rust                                                     | Upstream                                          |
//! |----------------------------------------------------------|---------------------------------------------------|
//! | [`AdvSurfaceIntersector`], [`AdvSurfaceIntersector::new`] | `SmAdvSurfaceIntersector`, its constructor       |
//! | `Drop` (implicit)                                        | `~SmAdvSurfaceIntersector`                        |
//! | [`AdvSurfaceIntersector::set_surface_caches`]            | `m_apTrees[i] = smsurf_GetSurfaceCache(..)->GetTree()` of the callers |
//! | `add_sing_dir` (private)                                 | `sm_AddSingDir` (static)                          |
//! | [`AdvSurfaceIntersector::compute_singularity_point`], [`AdvSurfaceIntersector::compute_singularity_point_on`] | `ComputeSingularityPoint` |
//! | [`AdvSurfaceIntersector::make_dupin`], [`Dupin`]         | `MakeDupin`                                       |
//! | [`AdvSurfaceIntersector::find_interior_curves`], [`InteriorCurves`] | `FindInteriorCurves`                   |
//! | [`GlobalSolve::solve_branch`] (impl)                     | `SolveBranch`                                     |
//! | [`GlobalSolve::local_solve`] (impl)                      | `LocalSolve`                                      |
//! | [`AdvSurfaceIntersector::branch_is_gauss_separate`]      | `BranchIsGaussSeparate`                           |
//! | `find_largest_non_leaf` (private)                        | `sm_FindLargestNonLeaf` (static)                  |
//! | [`GlobalSolve::find_best_subdivision_node`] (impl)       | `FindBestSubdivisionNode`                         |
//! | [`AdvSurfaceIntersector::is_kind_of`], [`TYPE`]          | `IsKindOf`, `SmAdvSurfaceIntersector_TYPE`        |
//! | [`NoSurfaceHandle`]                                      | (none: the handle type of an intersector without caches) |
//!
//! Deviations:
//! - **Bases.** The `SmSurfaceIntersector` base is the field
//!   [`AdvSurfaceIntersector::base`] and the `SmGlobalSolver` base the field
//!   [`AdvSurfaceIntersector::solver`], initialised as the `SmSurfaceIntersector`
//!   constructor sets it (operation `SM_SO_INTERSECT`, `SM_SR_NODES`, two trees,
//!   four variables, zero tolerance). The virtual solver hooks are the
//!   [`GlobalSolve`] implementation; `SmSurfaceIntersector::SolveBranch` (not
//!   overridden by that class) is [`crate::geom::global_solver::solve_branch`].
//! - **Trees and node data.** Upstream reaches a node's `SmBezierPatch` through
//!   `m_pData` and its surface through the tree's owner (asserted to be
//!   `m_cpSurface[i]`). Here the trees are those of the two surface caches given to
//!   [`AdvSurfaceIntersector::set_surface_caches`]; a node's patch is read from its
//!   cache and its surface is [`SurfaceIntersector::surface`]. A cache without a
//!   tree (upstream `GetTree` would build it) returns [`SolidError::Unported`]
//!   (`surface.surface_cache_p3`). Without caches the nodes have no data
//!   (`NER`, `SM_ERR_NULL_POINTER`).
//! - **Polar boxes.** `BranchIsGaussSeparate` builds a missing polar box lazily
//!   (`GetPolarBox`); the caches are shared here, so a missing box is built without
//!   storing it ([`SurfaceCache::polar_box_unstored`]; the values are the same). (`SolveBranch` resets
//!   `m_bSeperateGaussMaps` to `TRUE` on entry, so it never calls it.)
//! - **Points.** `ComputeSingularityPoint` takes intersection point ids of the
//!   base's arena ([`AdvSurfaceIntersector::compute_singularity_point`]) or a
//!   detached point ([`AdvSurfaceIntersector::compute_singularity_point_on`]).
//!   Its body reads only base members, so it lives on [`SurfaceIntersector`]
//!   (`adv_compute_singularity_point`), where the base's `ComputePointValues`
//!   calls it for an intersector made by [`AdvSurfaceIntersector::new`] (the
//!   virtual dispatch).
//!   Its unused UV argument is kept. The fixed array of eight singular directions
//!   is kept: a ninth direction (upstream writes past the array) is
//!   [`SolidError::InvalidInput`], as is nesting past the base solver's
//!   [`MAX_SOLVE_DEPTH`](crate::geom::global_solver::MAX_SOLVE_DEPTH).
//! - **Arguments.** Out-params become return values ([`Dupin`],
//!   [`InteriorCurves`], the `(index, swap)` pair). `MakeDupin`'s conics are owned
//!   [`Conic`]s instead of context-allocated pointers.
//! - **Statuses.** Statuses upstream ignores are ignored, except
//!   [`SolidError::Unported`], which always propagates (also where
//!   `FindInteriorCurves` skips a node pair whose `ComputePointValues` fails).
//! - **Base virtuals.** `FindInteriorCurves` calls the base `ComputePointValues` and
//!   `TraceCurve` ([`SurfaceIntersector`] parts 2 and 4); the only virtual they reach
//!   that this class overrides, `ComputeSingularityPoint`, dispatches through the
//!   base's `advanced` flag. The start point is a zero-filled arena point
//!   (`GetNewElement`, [`TsectPnt::zeroed`]).
//! - **Recursion.** `SolveBranch` nests at most once per tree node; deeper nesting
//!   (cyclic tree links, which recurse forever upstream) is
//!   [`SolidError::InvalidInput`], as is nesting past the base solver's
//!   [`MAX_SOLVE_DEPTH`](crate::geom::global_solver::MAX_SOLVE_DEPTH).
//! - **Tangential contact points** (not upstream, WR-74, source issue #394,
//!   opt-in `Kernel::fixes.ssi_tangent_contact_points`, off by default). The node
//!   pairs `FindInteriorCurves` drops (normals within 5 degrees, no local
//!   intersection, normals within 2 degrees at the solution) seed the source
//!   fork's search for isolated tangential contacts (`tangent_contacts`,
//!   source fork@c26edbc), run after the loop for a non-self intersection.
//!   Its Newton iterations are bounded loops (8 + 25 steps), no recursion.
//! - **Singular start direction** (not upstream, WR-512, source issue #1522,
//!   opt-in `Kernel::fixes.ssi_singular_start_inside_direction`, off by default). A
//!   curve starting at a singularity takes its first direction from the first singular
//!   direction that points into both domains, whose angle is the search's `dAngleMin`,
//!   before the later inside directions are searched for the one closest to the
//!   previous direction. Upstream leaves the output at `aSingDirections[0]`, so where
//!   the first inside direction is the nearest (or the only one) the curve starts
//!   along a direction that need not point into either domain. The flag is snapshotted
//!   by `SurfaceIntersector::new` when the intersector is built; a point computed later
//!   uses that value, not a later change to the kernel.
//! - **Not ported.** `Dump` and the `SM_DEBUG_CODE` drawing blocks; `SM_ASSERT`s.

use crate::curve::conic::{Conic, ConicType};
use crate::curve::curve::p6::DropPointCaches;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::point_project_to_plane;
use crate::geom::global_solver::{
    self, Branch, GlobalSolve, GlobalSolver, SolutionRequested, SolveRequest, SolverOperation,
};
use crate::geom::polar_box::PolarBox;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tree::{AuxDataType, NodeId, TreeNode};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::intersect::surface_intersector::p1::{
    surface_evaluate_normal, surface_local_surface_intersect, IntersectionPointType, SrfIndex,
    SurfaceIntersector, TsectId, TsectPnt,
};
use crate::kernel::Kernel;
use crate::math::{min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::{
    compute_surface_point_type, drop_vectors, evaluate_geometric, LocalSurfaceType, TsectCurveType,
};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::p1::{BezierPatch, SurfaceCache};

mod tangent_contacts;

const OP_SING: &str =
    "intersect::adv_surface_intersector::AdvSurfaceIntersector::compute_singularity_point";
const OP_LOCAL: &str = "intersect::adv_surface_intersector::AdvSurfaceIntersector::local_solve";
const OP_SOLVE: &str = "intersect::adv_surface_intersector::AdvSurfaceIntersector::solve_branch";
const OP_GAUSS: &str =
    "intersect::adv_surface_intersector::AdvSurfaceIntersector::branch_is_gauss_separate";

/// `SOLV_BASE_TYPE`.
const SOLV_BASE_TYPE: i64 = 19000;

/// `SmAdvSurfaceIntersector_TYPE` (`SOLV_BASE_TYPE + 48`).
pub const TYPE: i64 = SOLV_BASE_TYPE + 48;

/// `SmSurfaceIntersector_TYPE` (`SOLV_BASE_TYPE + 44`).
const SURFACE_INTERSECTOR_TYPE: i64 = SOLV_BASE_TYPE + 44;

/// `SmGlobalSolver_TYPE` (`SOLV_BASE_TYPE + 4`).
const GLOBAL_SOLVER_TYPE: i64 = SOLV_BASE_TYPE + 4;

/// Size of `aSingDirections` in `ComputeSingularityPoint`.
const MAX_SING_DIRS: usize = 8;

/// The Dupin indicatrix of a surface point ([`AdvSurfaceIntersector::make_dupin`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Dupin {
    /// `crAboveDupinCurves`: the curves of the positive principal curvatures
    /// (the side of the reference normal).
    pub above: Vec<Conic>,
    /// `crBelowDupinCurves`: the curves of the negative principal curvatures.
    pub below: Vec<Conic>,
    /// `rPrinK1Vec`: unit tangent in the first principal direction.
    pub prin_k1_vec: Vec3,
    /// `rPrinK2Vec`: unit tangent in the second principal direction.
    pub prin_k2_vec: Vec3,
    /// `rdPrinK1`: first principal curvature (signed by the reference normal,
    /// zeroed when 1000 times smaller than the other).
    pub prin_k1: f64,
    /// `rdPrinK2`: second principal curvature.
    pub prin_k2: f64,
    /// `rEFG`: the first fundamental form.
    pub efg: Vec3,
    /// `rLMN`: the second fundamental form.
    pub lmn: Vec3,
}

/// The surface handle of an intersector built without surface caches (its solver
/// never runs over trees).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoSurfaceHandle;

impl SolutionObject for NoSurfaceHandle {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        crate::geom::solution_array::ObjectKind::Surface
    }
}

/// The outputs [`AdvSurfaceIntersector::find_interior_curves`] appends to.
#[derive(Debug, Clone, Default)]
pub struct InteriorCurves {
    /// `r3DCurves`: the 3D intersection curves.
    pub curves_3d: Vec<Curve>,
    /// `rSurface1UVCurves`: their UV curves on the first surface.
    pub surface1_uv_curves: Vec<Curve>,
    /// `rSurface2UVCurves`: their UV curves on the second surface.
    pub surface2_uv_curves: Vec<Curve>,
    /// `rCurveTypes`: the type of each curve.
    pub curve_types: Vec<TsectCurveType>,
    /// `rDeviations`: the deviation of each curve.
    pub deviations: Vec<f64>,
}

/// The advanced surface/surface intersector (`SmAdvSurfaceIntersector`).
#[derive(Debug, Clone)]
pub struct AdvSurfaceIntersector<'s, 't, O> {
    /// Caller context for virtual evaluations.
    pub kernel: &'s Kernel,

    /// The `SmSurfaceIntersector` base.
    pub base: SurfaceIntersector<'s>,
    /// The `SmGlobalSolver` base.
    pub solver: GlobalSolver<'t, O>,
    /// The surface caches whose trees the solver traverses (the owners of
    /// `m_apTrees[0]`, `m_apTrees[1]`).
    caches: [Option<&'t SurfaceCache<O>>; 2],
    /// `m_dClosestValue`: closest leaf pair distance seen by the local solves.
    closest_value: f64,
    /// `m_sUVS`: the UV points of that pair.
    uvs: [Point2; 2],
    /// `m_apNodes`: the leaf nodes of that pair.
    nodes: [Option<NodeId>; 2],
    /// `m_bSeperateGaussMaps`.
    separate_gauss_maps: bool,
    /// Nesting depth of [`GlobalSolve::solve_branch`] (bounds the recursion on cyclic
    /// tree links).
    solve_depth: usize,
    /// Not upstream (the fork's `m_vTangentSeeds`, opt-in
    /// `Kernel::fixes.ssi_tangent_curve_tracer`, WR-75): the UV pairs where
    /// [`AdvSurfaceIntersector::find_interior_curves`] found the normals (nearly)
    /// parallel, seeds of the tangential curve pass.
    pub(crate) tangent_curve_seeds: Vec<[Point2; 2]>,
}

/// `sm_AddSingDir`: appends `sing_vec` to `dirs[..count]` unless a stored direction
/// is within `acos(0.9)` of it.
fn add_sing_dir(sing_vec: Vec3, count: &mut usize, dirs: &mut [Vec3; MAX_SING_DIRS]) -> Result<()> {
    // see if crSingVec is already in aSingDirections array
    let found = dirs.iter().take(*count).any(|d| d.dot(sing_vec) > 0.9);

    // when crSingVec is not in aSingDirections - add it
    if !found {
        let slot = dirs.get_mut(*count).ok_or_else(|| {
            SolidError::invalid_input(
                OP_SING,
                "more than 8 singular directions (upstream writes past its array)",
            )
        })?;
        *slot = sing_vec;
        *count += 1;
    }
    Ok(())
}

/// `pPrev->CrvDeriv().AngleBetween(dir, angle)` under `SER`.
fn angle(prev: Vec3, dir: Vec3) -> Result<f64> {
    prev.angle_between(dir)
}

/// The `ALIGN_DIR_WITH_PREVIOUS_POINT` macro: flips the curve direction when it is
/// within `min(PI/30, 5 * ang_tol)` of opposite to the previous one.
fn align_with_previous(point: &mut TsectPnt, prev: Option<Vec3>, ang_tol_rad: f64) {
    if let Some(p) = prev {
        // status ignored: a zero vector reads as angle 0
        let angle_radians = point.crv_deriv().angle_between(p).unwrap_or(0.0);
        if (angle_radians - PI).abs() < min2(PI / 30.0, 5.0 * ang_tol_rad) {
            *point.crv_deriv_mut() = -point.crv_deriv();
        }
    }
}

/// `sm_FindLargestNonLeaf`: the tree to subdivide (`1` when the first node is a
/// leaf, else `0`).
fn find_largest_non_leaf<O>(
    node1: &TreeNode<O>,
    child1_of_1: Option<&TreeNode<O>>,
    node2: &TreeNode<O>,
    child1_of_2: Option<&TreeNode<O>>,
) -> usize {
    let leaf = |n: &TreeNode<O>, c: Option<&TreeNode<O>>| match c {
        None => true,
        Some(c) => {
            n.aux_type == AuxDataType::BezierSurface && c.aux_type != AuxDataType::BezierSurface
        }
    };
    let leaf1 = leaf(node1, child1_of_1);
    let leaf2 = leaf(node2, child1_of_2);

    // If either are leaf nodes than return the other node to be split
    if leaf1 {
        return 1;
    }
    if leaf2 {
        return 0;
    }
    0
}

impl<'s, 't, O: SolutionObject + 't> AdvSurfaceIntersector<'s, 't, O> {
    /// The intersector of `surface1` over `uv_domain1` and `surface2` over
    /// `uv_domain2` (`SmAdvSurfaceIntersector(...)`; upstream default
    /// `from_fillet_intersector = FALSE`): the base intersector, no closest value
    /// yet (`SM_BIG_DOUBLE`), Gauss map separation on.
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceIntersector::new`].
    pub fn new(
        k: &'s Kernel,
        surface1: &'s Surface,
        uv_domain1: &Extent2d,
        surface2: &'s Surface,
        uv_domain2: &Extent2d,
        from_fillet_intersector: bool,
    ) -> Result<Self> {
        let mut base = SurfaceIntersector::new(
            k,
            surface1,
            uv_domain1,
            surface2,
            uv_domain2,
            from_fillet_intersector,
        )?;
        // the virtual ComputeSingularityPoint is this class's override
        base.advanced = true;

        // global solver members, as SmSurfaceIntersector's constructor sets them
        let mut solver = GlobalSolver::new();
        solver.operation = SolverOperation::Intersect;
        solver.projected_operation = false;
        solver.solution_requested = SolutionRequested::Nodes;
        solver.num_trees = 2;
        solver.num_variables = 4;
        solver.tolerance_3d = 0.0;
        solver.best_answer_so_far_sq = 0.0;
        solver.at_distance = 0.0;
        solver.at_distance_sq = 0.0;

        Ok(Self {
            kernel: k,
            base,
            solver,
            caches: [None, None],
            closest_value: BIG_DOUBLE,
            uvs: [Point2::UNDEF; 2],
            nodes: [None, None],
            separate_gauss_maps: true,
            solve_depth: 0,
            tangent_curve_seeds: Vec::new(),
        })
    }

    /// Uses the decomposition trees of `cache1` and `cache2` (the caches of the first
    /// and second surface) as the solver trees (upstream `m_apTrees[i] =
    /// smsurf_GetSurfaceCache(m_cpSurface[i])->GetTree()`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for a cache without tree (`SmSurfaceCache::GetTree`
    /// would build it, `surface.surface_cache_p3`).
    pub fn set_surface_caches(
        &mut self,
        cache1: &'t SurfaceCache<O>,
        cache2: &'t SurfaceCache<O>,
    ) -> Result<()> {
        let tree = |c: &'t SurfaceCache<O>| {
            c.tree().ok_or_else(|| {
                // UNPORTED(surface.surface_cache_p3): SmSurfaceCache::GetTree
                SolidError::unported("SmSurfaceCache::GetTree", "surface.surface_cache_p3")
            })
        };
        let t1 = tree(cache1)?;
        let t2 = tree(cache2)?;
        self.solver.trees[0] = Some(t1);
        self.solver.trees[1] = Some(t2);
        self.caches = [Some(cache1), Some(cache2)];
        Ok(())
    }

    /// `m_dClosestValue`.
    #[must_use]
    pub const fn closest_value(&self) -> f64 {
        self.closest_value
    }

    /// `m_sUVS`.
    #[must_use]
    pub const fn closest_uvs(&self) -> [Point2; 2] {
        self.uvs
    }

    /// `m_apNodes`.
    #[must_use]
    pub const fn closest_nodes(&self) -> [Option<NodeId>; 2] {
        self.nodes
    }

    /// `m_bSeperateGaussMaps`.
    #[must_use]
    pub const fn separate_gauss_maps(&self) -> bool {
        self.separate_gauss_maps
    }

    /// `IsKindOf`: `SmAdvSurfaceIntersector_TYPE`, `SmSurfaceIntersector_TYPE` or
    /// `SmGlobalSolver_TYPE`.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == TYPE || t == SURFACE_INTERSECTOR_TYPE || t == GLOBAL_SOLVER_TYPE
    }

    /// The node `id` of tree `index`.
    fn node(&self, index: usize, id: Option<NodeId>) -> Result<&'t TreeNode<O>> {
        let mut branch: Branch = [None; global_solver::MAX_SOLVER_TREES];
        if let Some(slot) = branch.get_mut(index) {
            *slot = id;
        }
        self.solver.branch_node(&branch, index)
    }

    /// The `SmBezierPatch` of node `id` of tree `index` (`(SmBezierPatch*)m_pData`).
    fn patch(&self, index: usize, id: Option<NodeId>) -> Result<&'t BezierPatch<O>> {
        let cache = self
            .caches
            .get(index)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_LOCAL))?;
        let id = id.ok_or(SolidError::null_pointer(OP_LOCAL))?;
        cache.patch(id)
    }

    // ---- singular points ----------------------------------------------------------------

    /// `ComputeSingularityPoint` on point `point` of the base's arena, with the
    /// optional previous point `previous` (only its curve tangent is read): see
    /// [`AdvSurfaceIntersector::compute_singularity_point_on`]. The point is
    /// written back also when an error is returned (upstream works in place).
    ///
    /// # Errors
    ///
    /// Unknown ids ([`SolidError::InvalidInput`]) and those of
    /// [`AdvSurfaceIntersector::compute_singularity_point_on`].
    pub fn compute_singularity_point(
        &mut self,
        uv_values: [Point2; 2],
        point: TsectId,
        previous: Option<TsectId>,
    ) -> Result<()> {
        let prev_deriv = match previous {
            Some(id) => Some(self.base.point(id)?.crv_deriv()),
            None => None,
        };
        let mut pnt = self.base.point(point)?.clone();
        let result = self.compute_singularity_point_on(uv_values, &mut pnt, prev_deriv);
        *self.base.point_mut(point)? = pnt;
        result
    }

    /// `ComputeSingularityPoint`: classifies `point`, an intersection point where
    /// the surface normals are parallel (its surface evaluations, normals included,
    /// are set), and sets its curve tangent to the direction to pursue.
    /// `prev_crv_deriv` is the curve tangent of the optional previous point; without
    /// it the last point of the curve being traced is used, if any.
    ///
    /// A planar point against a cylindrical one is a tangent curve along the
    /// cylinder's flat direction; against a saddle a singularity with the four
    /// asymptotic directions; else a tangent point. Otherwise the Dupin indicatrices
    /// decide: equal umbilical curvatures are a coincidence, a shared principal
    /// curvature (and direction) a tangent curve (or a singularity/coincidence when
    /// both match), and else the directions are the intersections of the two
    /// indicatrices: none is a tangent point, two a tangent curve, more a
    /// singularity (preferring a direction into both domains when starting a curve,
    /// else the one closest to the previous direction).
    ///
    /// # Errors
    ///
    /// Those of `smsurf_EvaluateGeometric`, [`Vec3::angle_between`],
    /// `smsurf_DropVectors` and [`Vec3::unitize`] where upstream checks them; `SM_ERR`
    /// for a single singular direction; [`SolidError::InvalidInput`] for more than
    /// eight.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub fn compute_singularity_point_on(
        &self,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
        prev_crv_deriv: Option<Vec3>,
    ) -> Result<()> {
        self.base
            .adv_compute_singularity_point(uv_values, point, prev_crv_deriv)
    }

    /// `MakeDupin`: the Dupin indicatrix curves of the surface point evaluated in
    /// `surface_eval` (see [`TsectPnt::surface_pv`] for the layout; the matrix is
    /// updated in place as `smsurf_EvaluateGeometric` does), with curvatures signed
    /// by `reference_normal`. A planar point has no curves; a parabolic point two
    /// half lines at `1/sqrt|k|` from the origin; an elliptic point one ellipse; a
    /// hyperbolic point four hyperbolas (upstream's bottom hyperbola uses `k2` for
    /// both axes). Curves go above for positive curvatures, below for negative.
    ///
    /// # Errors
    ///
    /// Those of `smsurf_EvaluateGeometric`.
    pub fn make_dupin(
        &self,
        surface_eval: &mut [[Vec3; 3]; 3],
        reference_normal: Vec3,
    ) -> Result<Dupin> {
        make_dupin(surface_eval, reference_normal)
    }

    // ---- interior curves -------------------------------------------------------------------

    /// `FindInteriorCurves`: runs the global solve over the surface trees
    /// ([`AdvSurfaceIntersector::set_surface_caches`]) for one candidate start point
    /// per intersecting leaf pair, and traces a curve from each candidate whose
    /// normals are at least 5 degrees apart, whose local intersection succeeds with
    /// normals at least 2 degrees apart and which is not on a curve found already.
    /// The curves are appended to `out`.
    ///
    /// # Errors
    ///
    /// Those of the solve and of the callees; [`SolidError::Unported`] (see the
    /// module deviations).
    pub fn find_interior_curves(&mut self, k: &Kernel, out: &mut InteriorCurves) -> Result<()> {
        let mut solutions = SolutionArray::with_capacity(130);

        // Find node pairs in the tree where intersections might exist
        let request = SolveRequest {
            operation: SolverOperation::Intersect,
            solution_requested: SolutionRequested::All,
            tolerance_3d: self.base.this_approx_tol_3d,
            best_answer_so_far_sq: BIG_DOUBLE,
            opt_vectors: &[],
        };
        self.solve_it(&request, &mut solutions)?;

        // The caches of the drops on `out.curves_3d` (upstream keeps them on the
        // curves): this loop only appends curves, so slot `i` stays curve `i`'s.
        let mut drop_caches = DropPointCaches::default();

        // Opt-in (WR-74, #394): the patch UV domains of the node pairs that cannot
        // start a crossing curve seed the tangential contact point search below.
        let mut tangent_seeds: Vec<[Extent2d; 2]> = Vec::new();

        // Now for each node pair do a test to see if produces a unique start point
        for sol in solutions.as_slice() {
            let uv_guess1 = Point2::new(sol.start.parameters[0], sol.start.parameters[1]);
            let uv_guess2 = Point2::new(sol.start.parameters[2], sol.start.parameters[3]);
            let node1 = sol.nodes[0];
            let node2 = sol.nodes[1];
            self.patch(0, node1)?;
            self.patch(1, node2)?;
            let bss1 = self.base.surface(SrfIndex::First);
            let bss2 = self.base.surface(SrfIndex::Second);

            // Skip cases where surface normals are very close
            {
                let norm1 = surface_evaluate_normal(k, bss1, uv_guess1)?;
                let norm2 = surface_evaluate_normal(k, bss2, uv_guess2)?;
                let mut angle_rad = norm1.angle_between(norm2)?;
                if angle_rad > PI / 2.0 {
                    angle_rad = PI - angle_rad;
                }
                if angle_rad * (180.0 / PI) < 5.0 {
                    self.push_tangent_seed(k, &mut tangent_seeds, node1, node2)?;
                    // (WR-75, opt-in) a seed of the tangential curve pass
                    if k.fixes.ssi_tangent_curve_tracer {
                        self.tangent_curve_seeds.push([uv_guess1, uv_guess2]);
                    }
                    continue;
                }
            }

            // Get an intersection curve start point
            let (found_answer, start) = surface_local_surface_intersect(
                k,
                bss1,
                &self.base.uv_domains[0],
                bss2,
                &self.base.uv_domains[1],
                self.base.this_approx_tol_3d / 100.0,
                uv_guess1,
                uv_guess2,
                None,
                None,
                false,
            )?;

            // for nodes that do not intersect - skip to next node pair
            if !found_answer {
                self.push_tangent_seed(k, &mut tangent_seeds, node1, node2)?;
                continue;
            }

            // Get solution 3d and UV points point
            let uv = Point2::new(start[0], start[1]);
            let uv2 = Point2::new(start[2], start[3]);
            let surf_pnt = bss1.evaluate_point(k, uv)?;

            // Skip cases where surface normals are very close
            {
                let norm1 = surface_evaluate_normal(k, bss1, uv)?;
                let norm2 = surface_evaluate_normal(k, bss2, uv2)?;
                let mut angle_rad = norm1.angle_between(norm2)?;
                if angle_rad > PI / 2.0 {
                    angle_rad = PI - angle_rad;
                }
                if angle_rad * (180.0 / PI) < 2.0 {
                    self.push_tangent_seed(k, &mut tangent_seeds, node1, node2)?;
                    // (WR-75, opt-in) a seed of the tangential curve pass
                    if k.fixes.ssi_tangent_curve_tracer {
                        self.tangent_curve_seeds.push([uv, uv2]);
                    }
                    continue;
                }
            }

            // skip start points that are already on xsect curves
            if self.base.is_point_on_curve_cached(
                k,
                surf_pnt,
                &out.curves_3d,
                None,
                &mut drop_caches,
            )? {
                continue;
            }

            // make a SmTSectPnt for the intersection solution
            let uv_pts = [uv, Point2::new(start[2], start[3])];
            let start_tsp = self.base.alloc_point(TsectPnt::zeroed());
            self.base.curve_trace_direction = 1.0;
            match compute_point_values(k, &mut self.base, uv_pts, start_tsp) {
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => continue,
                Ok(()) => {}
            }

            // init trace params
            let input_trace_direction = self.base.curve_trace_direction;
            self.base.start_point = Some(start_tsp);

            // do the trace
            trace_curve(k, &mut self.base, start_tsp, &out.curves_3d)?;

            // when appropriate - trace in the other direction as well
            if !self.base.curve_is_closed
                && !self.base.point(start_tsp)?.is_bounded()
                && input_trace_direction == self.base.curve_trace_direction
            {
                self.base.reverse_curve_direction()?;
                self.base.curve_trace_direction *= -1.0;
                trace_curve(k, &mut self.base, start_tsp, &out.curves_3d)?;

                // restore the curve direction (upstream multiplies by 1.0)
                self.base.reverse_curve_direction()?;
            }

            // turn m_vCurvePoints into an intersection curve
            flush_curve(k, &mut self.base, out)?;
            self.base.start_point = None;
        }

        // Opt-in (WR-74, #394): isolated tangential contact points
        if k.fixes.ssi_tangent_contact_points && !self.base.doing_self_intersection {
            tangent_contacts::find_tangent_contact_points(
                k,
                &self.base,
                &tangent_seeds,
                out,
                &mut drop_caches,
            )?;
        }

        Ok(())
    }

    /// Opt-in (WR-74, #394): records the patch UV domains of a node pair
    /// `FindInteriorCurves` drops as a seed of the tangential contact point search
    /// (nothing without `Kernel::fixes.ssi_tangent_contact_points`).
    fn push_tangent_seed(
        &self,
        k: &Kernel,
        seeds: &mut Vec<[Extent2d; 2]>,
        node1: Option<NodeId>,
        node2: Option<NodeId>,
    ) -> Result<()> {
        if k.fixes.ssi_tangent_contact_points {
            seeds.push([
                *self.patch(0, node1)?.aux.uv_domain(),
                *self.patch(1, node2)?.aux.uv_domain(),
            ]);
        }
        Ok(())
    }

    // ---- Gauss maps --------------------------------------------------------------------------

    /// `BranchIsGaussSeparate`: true when both nodes of `branch` are surface
    /// subdivision nodes (`SM_AD_BEZIER_SURFACE` or `_HEAD`) whose normal polar
    /// boxes are disjoint.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without node data (no caches); those of the lazy polar
    /// box build.
    pub fn branch_is_gauss_separate(&self, k: &Kernel, branch: &Branch) -> Result<bool> {
        let node1 = self.node(0, branch[0])?;
        let node2 = self.node(1, branch[1])?;
        let is_surface = |n: &TreeNode<O>| {
            n.aux_type == AuxDataType::BezierSurface || n.aux_type == AuxDataType::BezierSurfaceHead
        };
        if is_surface(node1) && is_surface(node2) {
            let box1 = self.polar_box(k, 0, branch[0])?;
            let box2 = self.polar_box(k, 1, branch[1])?;
            if box1.are_disjoint(&box2) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `((SmBezierAux2d*)m_pData)->GetPolarBox()` of node `id` of tree `index`.
    fn polar_box(&self, k: &Kernel, index: usize, id: Option<NodeId>) -> Result<PolarBox> {
        let cache = self
            .caches
            .get(index)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_GAUSS))?;
        let id = id.ok_or(SolidError::null_pointer(OP_GAUSS))?;
        // built lazily upstream; built without storing it here (the caches are
        // shared), which gives the same box
        cache.polar_box_unstored(k, id)
    }

    /// The body of [`GlobalSolve::solve_branch`] (`SolveBranch`).
    fn solve_branch_at_depth(&mut self, k: &Kernel, branch: &Branch) -> Result<()> {
        self.separate_gauss_maps = true;

        if !self.branch_may_contain_answers(branch)? {
            return Ok(());
        }

        // no work - SolverOperation != INTERSECT - pass the call along
        if self.solver.operation != SolverOperation::Intersect {
            return global_solver::solve_branch(self, branch);
        }

        let mut save_answer = false;

        let mut achieved_seperability = false;
        if !self.separate_gauss_maps {
            achieved_seperability = self.branch_is_gauss_separate(k, branch)?;
        }

        if achieved_seperability {
            // separability at the top nodes of both trees: no interior curves
            let node1 = self.node(0, branch[0])?;
            let node2 = self.node(1, branch[1])?;
            if node1.parent.is_none() && node2.parent.is_none() {
                return Ok(());
            }

            let saved = core::mem::replace(&mut self.separate_gauss_maps, true);
            self.closest_value = BIG_DOUBLE;
            save_answer = true;

            let result = if self.are_ready_for_local_solve(branch)? {
                self.local_solve(branch).map(|_| ())
            } else {
                self.subdivide(branch).and_then(|(child1, child2)| {
                    self.solve_branch(&child1)?;
                    self.solve_branch(&child2)
                })
            };
            self.separate_gauss_maps = saved;
            result?;
        } else if self.are_ready_for_local_solve(branch)? {
            let saved = core::mem::replace(&mut self.separate_gauss_maps, true);
            self.closest_value = BIG_DOUBLE;
            // If we are unable to achieve seperability at the leaf level than we
            // save the answer any how.
            save_answer = true;
            let result = self.local_solve(branch);
            self.separate_gauss_maps = saved;
            result?;
        } else {
            let (child1, child2) = self.subdivide(branch)?;
            self.solve_branch(&child1)?;
            self.solve_branch(&child2)?;
        }

        // some leaf nodes intersected: save the best one as an answer
        if save_answer && self.closest_value < BIG_DOUBLE {
            let mut solution = Solution::new();
            solution.solution_type = SolutionType::SingleValue;
            solution.num_variables = 4;
            solution.start.parameters[0] = self.uvs[0].x;
            solution.start.parameters[1] = self.uvs[0].y;
            solution.start.parameters[2] = self.uvs[1].x;
            solution.start.parameters[3] = self.uvs[1].y;
            solution.nodes[0] = self.nodes[0];
            solution.nodes[1] = self.nodes[1];
            self.solver.solutions.add(&solution);
        }

        Ok(())
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for AdvSurfaceIntersector<'_, 't, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.solver
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.solver
    }

    /// `SolveBranch`: for `SM_SO_INTERSECT`, subdivides to the leaf pairs and saves
    /// the closest midpoint pair of each leaf solve as a 4-variable single-value
    /// solution (`m_vStart = [u1, v1, u2, v2]`, the two leaf nodes); other
    /// operations run the base traversal.
    fn solve_branch(&mut self, branch: &Branch) -> Result<()> {
        // an acyclic pair of trees nests at most one level per node
        let limit = self
            .solver
            .trees
            .iter()
            .flatten()
            .map(|t| t.nodes().len())
            .sum::<usize>()
            .saturating_add(2)
            .min(global_solver::MAX_SOLVE_DEPTH + 1);
        if self.solve_depth >= limit {
            return Err(SolidError::invalid_input(OP_SOLVE, "tree links are cyclic"));
        }
        self.solve_depth += 1;
        let result = self.solve_branch_at_depth(self.kernel, branch);
        self.solve_depth -= 1;
        result
    }

    /// `LocalSolve`: for `SM_SO_INTERSECT`, when the patches' pseudo boxes (the
    /// first grown by the solver tolerance) meet, compares the distance of the
    /// patch midpoints (after one drop of the smaller patch's midpoint onto the
    /// patch more than four times bigger) with the closest so far; for
    /// `SM_SO_INTERSECTION_TEST`, accumulates the union of the overlapping leaf boxes
    /// and patch domains in the first solution (`m_vStart` 3D box and count,
    /// `m_vEnd` both UV domains). Never needs more subdivision.
    #[allow(clippy::too_many_lines)] // one upstream function
    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        let surface_node1 = self.node(0, branch[0])?;
        let surface_node2 = self.node(1, branch[1])?;
        let bez_patch1 = self.patch(0, branch[0])?;
        let bez_patch2 = self.patch(1, branch[1])?;
        let bsp1 = self.base.surface(SrfIndex::First);
        let bsp2 = self.base.surface(SrfIndex::Second);
        let mut uv_domain1 = *bez_patch1.aux.uv_domain();
        let mut uv_domain2 = *bez_patch2.aux.uv_domain();

        // Do interval tests and reject if disjoint
        if uv_domain1.are_disjoint(&self.base.uv_domains[0], 0.0) {
            return Ok(false);
        }
        if uv_domain2.are_disjoint(&self.base.uv_domains[1], 0.0) {
            return Ok(false);
        }

        // If we are doing self intersection then reject case where domains are not
        // disjoint.
        if self.base.doing_self_intersection && !uv_domain1.are_disjoint(&uv_domain2, 0.0) {
            return Ok(false);
        }

        if self.solver.operation == SolverOperation::Intersect {
            let mut tmp_pbox0 = *bez_patch1.pseudo_box();
            tmp_pbox0.expand_absolute(self.solver.tolerance_3d);
            if !tmp_pbox0.are_disjoint(bez_patch2.pseudo_box(), None) {
                let size1 = surface_node1.bbox.size();
                let size2 = surface_node2.bbox.size();
                let mut uv1 = uv_domain1.evaluate(0.5, 0.5);
                let mut uv2 = uv_domain2.evaluate(0.5, 0.5);
                let p1 = bsp1.evaluate_point(k, uv1)?;
                let p2 = bsp2.evaluate_point(k, uv2)?;
                let mut dist = p1.distance(p2);

                // If the sizes are very different - try taking one Newton step to
                // get a closer guess point on the big surface.
                if size1.length() > 4.0 * size2.length() {
                    let uv_points = drop_points_to_tangent_plane(k, bsp1, uv1, &[p2])?;
                    let uv1_new = uv_domain1.clamp_point(uv_points.first().copied().unwrap_or(uv1));
                    let p1_new = bsp1.evaluate_point(k, uv1_new)?;
                    let new_dist = p1_new.distance(p2);
                    if new_dist < dist {
                        uv1 = uv1_new;
                        dist = new_dist;
                    }
                } else if size2.length() > 4.0 * size1.length() {
                    let uv_points = drop_points_to_tangent_plane(k, bsp2, uv2, &[p1])?;
                    let uv2_new = uv_domain2.clamp_point(uv_points.first().copied().unwrap_or(uv2));
                    let p2_new = bsp2.evaluate_point(k, uv2_new)?;
                    let new_dist = p2_new.distance(p1);
                    if new_dist < dist {
                        uv2 = uv2_new;
                        dist = new_dist;
                    }
                }

                if dist < self.closest_value {
                    self.closest_value = dist;
                    self.uvs = [uv1, uv2];
                    self.nodes = [branch[0], branch[1]];
                }
            }
        } else if self.solver.operation == SolverOperation::IntersectionTest {
            let mut tmp_pbox0 = *bez_patch1.pseudo_box();
            let tmp_pbox1 = *bez_patch2.pseudo_box();
            tmp_pbox0.expand_absolute(self.solver.tolerance_3d);

            // when Patch1 and Patch2 PseudoBoxes intersect
            if !tmp_pbox0.are_disjoint(&tmp_pbox1, None) {
                // Update the bounding box information in the solution.
                let mut bbox = surface_node1.bbox;
                bbox.expand_absolute(self.solver.tolerance_3d);
                let mut result = bbox
                    .intersect(&surface_node2.bbox)
                    .unwrap_or_else(|_| Extent3d::new());

                let solutions = &mut self.solver.solutions;
                if solutions.is_empty() {
                    // the first solution
                    let mut sol = Solution::new();
                    sol.nodes[0] = branch[0];
                    sol.nodes[1] = branch[1];
                    sol.num_variables = 7;
                    sol.solution_type = SolutionType::SingleValue;
                    let (mn, mx) = (result.min(), result.max());
                    sol.start.parameters[..7]
                        .copy_from_slice(&[mn.x, mn.y, mn.z, mx.x, mx.y, mx.z, 1.0]);
                    let (a, b) = (uv_domain1.min(), uv_domain1.max());
                    let (c, d) = (uv_domain2.min(), uv_domain2.max());
                    sol.end.parameters[..8]
                        .copy_from_slice(&[a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y]);
                    solutions.add(&sol);
                } else if let Some(sol) = solutions.get_mut(0) {
                    // add this solution to the previous solutions
                    let s = &mut sol.start.parameters;
                    s[6] += 1.0; // Number of contributors to intersection

                    let old_box = Extent3d::from_points(
                        Vec3::new(s[0], s[1], s[2]),
                        Vec3::new(s[3], s[4], s[5]),
                    );
                    result = result.union(&old_box);
                    let (mn, mx) = (result.min(), result.max());
                    s[..6].copy_from_slice(&[mn.x, mn.y, mn.z, mx.x, mx.y, mx.z]);

                    let e = &mut sol.end.parameters;
                    let old_uv1 =
                        Extent2d::from_points(Point2::new(e[0], e[1]), Point2::new(e[2], e[3]));
                    let old_uv2 =
                        Extent2d::from_points(Point2::new(e[4], e[5]), Point2::new(e[6], e[7]));
                    uv_domain1 = old_uv1.union(&uv_domain1);
                    uv_domain2 = old_uv2.union(&uv_domain2);
                    let (a, b) = (uv_domain1.min(), uv_domain1.max());
                    let (c, d) = (uv_domain2.min(), uv_domain2.max());
                    e[..8].copy_from_slice(&[a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y]);
                }
            }
        }

        Ok(false)
    }

    /// `FindBestSubdivisionNode`: `sm_FindLargestNonLeaf` (subdivide the second
    /// tree when the first node is a leaf, else the first), never swapping.
    fn find_best_subdivision_node(&self, branch: &Branch) -> Result<(usize, bool)> {
        let node1 = self.node(0, branch[0])?;
        let node2 = self.node(1, branch[1])?;
        let child = |index: usize, n: &TreeNode<O>| -> Result<Option<&'t TreeNode<O>>> {
            match n.child1 {
                None => Ok(None),
                Some(c) => self.node(index, Some(c)).map(Some),
            }
        };
        let c1 = child(0, node1)?;
        let c2 = child(1, node2)?;
        Ok((find_largest_non_leaf(node1, c1, node2, c2), false))
    }
}

/// `pSrf->DropPointsToTangentPlane(uv, TRUE, TRUE, n, points, uvs)`.
fn drop_points_to_tangent_plane(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    points: &[Point3],
) -> Result<Vec<Point2>> {
    crate::surface::surface::p7::drop_points_to_tangent_plane(
        k,
        surface,
        uv,
        Side::Left,
        Side::Left,
        points,
    )
}

/// `ComputePointValues(uv_pts, *point, NULL)` on the arena point `point` (virtual:
/// the base body, which calls this class's `ComputeSingularityPoint` override through
/// the intersector's `advanced` flag).
fn compute_point_values(
    k: &Kernel,
    si: &mut SurfaceIntersector<'_>,
    uv_pts: [Point2; 2],
    point: TsectId,
) -> Result<()> {
    let mut p = si.point(point)?.clone();
    let r = si.compute_point_values(k, uv_pts, &mut p, None, None);
    *si.point_mut(point)? = p;
    r
}

/// `TraceCurve(*start, r3DCurves)`: the base `SmSurfaceIntersector::TraceCurve` (not
/// overridden); its only virtual call this class overrides, `ComputeSingularityPoint`
/// inside `ComputePointValues`, dispatches through the `advanced` flag.
fn trace_curve(
    k: &Kernel,
    si: &mut SurfaceIntersector<'_>,
    start: TsectId,
    curves_3d: &[Curve],
) -> Result<()> {
    si.trace_curve(k, start, curves_3d)
}

/// `FlushCurve(r3DCurves, rSurface1UVCurves, rSurface2UVCurves, rCurveTypes,
/// rDeviations)`: the base `SmSurfaceIntersector::FlushCurve` (not overridden). The
/// output arrays are moved in and back, not cleared: its uniqueness test reads the 3D
/// curves already flushed.
fn flush_curve(
    k: &Kernel,
    si: &mut SurfaceIntersector<'_>,
    out: &mut InteriorCurves,
) -> Result<()> {
    let mut curves = crate::intersect::surface_intersector::p2::TsectCurves {
        curves_3d: core::mem::take(&mut out.curves_3d),
        uv_curves: [
            core::mem::take(&mut out.surface1_uv_curves),
            core::mem::take(&mut out.surface2_uv_curves),
        ],
        curve_types: core::mem::take(&mut out.curve_types),
        deviations: core::mem::take(&mut out.deviations),
    };
    let r = si.flush_curve(k, &mut curves);
    let crate::intersect::surface_intersector::p2::TsectCurves {
        curves_3d,
        uv_curves: [surface1_uv_curves, surface2_uv_curves],
        curve_types,
        deviations,
    } = curves;
    *out = InteriorCurves {
        curves_3d,
        surface1_uv_curves,
        surface2_uv_curves,
        curve_types,
        deviations,
    };
    r
}

/// `MakeDupin` (see [`AdvSurfaceIntersector::make_dupin`]; it reads no member).
fn make_dupin(surface_eval: &mut [[Vec3; 3]; 3], reference_normal: Vec3) -> Result<Dupin> {
    let geom = evaluate_geometric(surface_eval)?;
    let mut out = Dupin {
        above: Vec::new(),
        below: Vec::new(),
        prin_k1_vec: geom.principle_curvature_vector1,
        prin_k2_vec: geom.principle_curvature_vector2,
        prin_k1: geom.principle_curvature1,
        prin_k2: geom.principle_curvature2,
        efg: geom.efg_of_first_fund_form,
        lmn: geom.lmn_of_second_fund_form,
    };

    // negate curvatures when surface normal is opposite reference normal
    if reference_normal.dot(surface_eval[2][2]) < 0.0 {
        out.prin_k1 = -out.prin_k1;
        out.prin_k2 = -out.prin_k2;
    }

    // no work - surfacePoint is planar
    if out.prin_k1.abs() < EFF_ZERO && out.prin_k2.abs() < EFF_ZERO {
        return Ok(out);
    }

    // treat a curvature 1000 times smaller than the other as zero (a cylinder)
    if out.prin_k1.abs() < out.prin_k2.abs() / 1000.00 {
        out.prin_k1 = 0.0;
    }
    if out.prin_k2.abs() < out.prin_k1.abs() / 1000.00 {
        out.prin_k2 = 0.0;
    }
    let (k1, k2) = (out.prin_k1, out.prin_k2);

    if k1.abs() < EFF_ZERO || k2.abs() < EFF_ZERO {
        // We have a parabolic point - create two parallel lines
        let mut prin_k = k1;
        // 90 degree rotation because curves are parallel to the local Y axis.
        let mut angle_rad = PI / 2.0;
        if prin_k.abs() < EFF_ZERO {
            prin_k = k2;
            angle_rad = 0.0;
        }

        // two lines leaving this point in opposite directions
        let a = 1.0 / sqrt(prin_k.abs());
        let mut line1 = Conic::new(ConicType::Line, a, 0.0);
        line1.transform_2d(angle_rad, Point2::new(0.0, 0.0));
        let mut line2 = Conic::new(ConicType::Line, a, 0.0);
        line2.transform_2d(PI + angle_rad, Point2::new(0.0, 0.0));

        if prin_k > 0.0 {
            out.above.push(line1);
            out.above.push(line2);
        } else {
            out.below.push(line1);
            out.below.push(line2);
        }
        return Ok(out);
    } else if k1 * k2 > 0.0 {
        // We have an ellipse
        let ellipse = Conic::new(
            ConicType::Ellipse,
            1.0 / sqrt(k1.abs()),
            1.0 / sqrt(k2.abs()),
        );
        if k1 > 0.0 {
            out.above.push(ellipse);
        } else {
            out.below.push(ellipse);
        }
    } else if k1 * k2 < 0.0 {
        // We have a hyperbolic point
        let mut push = |c: Conic, k: f64| {
            if k > 0.0 {
                out.above.push(c);
            } else {
                out.below.push(c);
            }
        };

        // First create hyperbola on right side
        let hyp = Conic::new(
            ConicType::Hyperbola,
            1.0 / sqrt(k1.abs()),
            1.0 / sqrt(k2.abs()),
        );
        push(hyp, k1);

        // Now do left side hyperbola
        let mut hyp = Conic::new(
            ConicType::Hyperbola,
            1.0 / sqrt(k1.abs()),
            1.0 / sqrt(k2.abs()),
        );
        hyp.transform_2d(PI, Point2::new(0.0, 0.0));
        push(hyp, k1);

        // Now do top hyperbola
        let mut hyp = Conic::new(
            ConicType::Hyperbola,
            1.0 / sqrt(k2.abs()),
            1.0 / sqrt(k1.abs()),
        );
        hyp.transform_2d(PI / 2.0, Point2::new(0.0, 0.0));
        push(hyp, k2);

        // Now do bottom hyperbola
        let mut hyp = Conic::new(
            ConicType::Hyperbola,
            1.0 / sqrt(k2.abs()),
            1.0 / sqrt(k2.abs()),
        );
        hyp.transform_2d(PI + PI / 2.0, Point2::new(0.0, 0.0));
        push(hyp, k2);
    }

    Ok(out)
}

/// `SmAdvSurfaceIntersector::ComputeSingularityPoint` on the base intersector's
/// state (it reads no member of the derived class), so that the base's
/// `ComputePointValues` reaches the override on an `SmAdvSurfaceIntersector`.
impl SurfaceIntersector<'_> {
    /// See [`AdvSurfaceIntersector::compute_singularity_point_on`].
    ///
    /// # Errors
    ///
    /// Those of [`AdvSurfaceIntersector::compute_singularity_point_on`].
    #[allow(clippy::too_many_lines)] // one upstream function
    pub(crate) fn adv_compute_singularity_point(
        &self,
        _uv_values: [Point2; 2],
        point: &mut TsectPnt,
        prev_crv_deriv: Option<Vec3>,
    ) -> Result<()> {
        let curve_trace_direction = self.curve_trace_direction;
        let ang_tol_rad = self.this_ang_tol_rad;

        // init curve direction when possible - NULL when given a start point
        if let Some(d) = prev_crv_deriv {
            *point.crv_deriv_mut() = d;
        }

        // The last node of m_vCurvePoints (point may be on that list itself).
        let last_node = self.curve_points().last().copied();
        let last_node_deriv = match last_node {
            Some(id) => Some(self.point(id)?.crv_deriv()),
            None => None,
        };

        // classify surface shape at given points
        let surf1_type = compute_surface_point_type(
            point.srf_norm(SrfIndex::First),
            point.srf_duu(SrfIndex::First),
            point.srf_dvv(SrfIndex::First),
            None,
            false,
        );
        let surf2_type = compute_surface_point_type(
            point.srf_norm(SrfIndex::Second),
            point.srf_duu(SrfIndex::Second),
            point.srf_dvv(SrfIndex::Second),
            None,
            false,
        );

        // get planar surface index (if any)
        let planar = if surf1_type == LocalSurfaceType::Planar {
            Some(SrfIndex::First)
        } else if surf2_type == LocalSurfaceType::Planar {
            Some(SrfIndex::Second)
        } else {
            None
        };

        // locals for multiple intersection directions for singular points
        let mut sing_dir_count = 0_usize;
        let mut sing_directions = [Vec3::UNDEF; MAX_SING_DIRS];

        // some cases project the previous point's CurveDir() to the current point's
        // tangency plane (in a scratch copy)
        let mut prev: Option<Vec3> = None;
        if let Some(prev_deriv) = prev_crv_deriv.or(last_node_deriv) {
            // project pPrev CrvDeriv() vector into rTsectPnt tangency plane
            let end_pt = point.crv_pos() + prev_deriv;
            let pt_on_plane =
                point_project_to_plane(end_pt, point.crv_pos(), point.srf_norm(SrfIndex::First));
            let dir = pt_on_plane - point.crv_pos();

            // when projection worked - update the pPrev->CrvDeriv direction to that
            prev = Some(dir.unitize().unwrap_or(prev_deriv));
        }

        // Process planar case -
        if let Some(srf) = planar {
            // get other surface index
            let other = match srf {
                SrfIndex::First => SrfIndex::Second,
                SrfIndex::Second => SrfIndex::First,
            };

            // get otherSurf curvature properties at point
            let geom = evaluate_geometric(point.srf_eval_matrix_mut(other))?;
            let prin_k1 = geom.principle_curvature1;
            let prin_k2 = geom.principle_curvature2;
            let mut prin_kv1 = geom.principle_curvature_vector1;
            let mut prin_kv2 = geom.principle_curvature_vector2;

            // If the normals do not agree then reverse the curvatures
            let mut opposite_normals = false;
            if point.srf_norm(other).dot(point.srf_norm(srf)) < EFF_ZERO {
                opposite_normals = true;
                prin_kv1 = -prin_kv1;
                prin_kv2 = -prin_kv2;
            }

            if prin_k1.abs() < EFF_ZERO_SQRT && prin_k2.abs() > EFF_ZERO_SQRT {
                // planar surfacePoint is kissing a cylindrical surfacePoint in 1st
                // principal curvature direction
                point.point_type = IntersectionPointType::TangentCurve;
                *point.crv_deriv_mut() = prin_kv1 * curve_trace_direction;
                align_with_previous(point, prev, ang_tol_rad);
                return Ok(());
            } else if prin_k2.abs() < EFF_ZERO_SQRT && prin_k1.abs() > EFF_ZERO_SQRT {
                // ... in 2nd principal curvature direction
                point.point_type = IntersectionPointType::TangentCurve;
                *point.crv_deriv_mut() = prin_kv2 * curve_trace_direction;
                align_with_previous(point, prev, ang_tol_rad);
                return Ok(());
            } else if prin_k1 * prin_k2 < 0.0 {
                // planar surfacePoint is intersecting a saddle surfacePoint
                point.point_type = IntersectionPointType::Singularity;

                // evaluate the hyperbola K1*x^2 + K2*y^2 = 1 at a large Y value
                let y = 10000.0;
                let mut tmp = (1.0 - prin_k1 * y * y) / prin_k2;
                if tmp < 0.0 {
                    tmp = 0.0;
                }
                let x = sqrt(tmp);

                let mut asymp_dir1 = y * prin_kv1 + x * prin_kv2;
                let mut asymp_dir_in1 = y * prin_kv1 - x * prin_kv2;

                // If the normals oppose then switch directions
                if opposite_normals {
                    core::mem::swap(&mut asymp_dir1, &mut asymp_dir_in1);
                }

                // unitize the assymptotic directions
                asymp_dir1 = asymp_dir1.unitize_or_z();
                asymp_dir_in1 = asymp_dir_in1.unitize_or_z();

                // the other start directions are in the negative assymptotic directions
                let asymp_dir2 = -asymp_dir1;
                let asymp_dir_in2 = -asymp_dir_in1;

                // Reverse vectors if tracing backwards
                sing_dir_count = 4;
                sing_directions[0] = asymp_dir1 * curve_trace_direction;
                sing_directions[1] = asymp_dir2 * curve_trace_direction;
                sing_directions[2] = asymp_dir_in1 * curve_trace_direction;
                sing_directions[3] = asymp_dir_in2 * curve_trace_direction;
            } else {
                // plane is kissing a hill or bowl (or a plane)
                point.point_type = IntersectionPointType::TangentPoint;
                return Ok(());
            }
        } else {
            // No planar points branch

            // Dupin indicatrex curves for 1st surface
            let normal0 = point.srf_norm(SrfIndex::First);
            let d1 = make_dupin(point.srf_eval_matrix_mut(SrfIndex::First), normal0)?;
            let (prin_k1, prin_k2, prin_kv1, prin_kv2) =
                (d1.prin_k1, d1.prin_k2, d1.prin_k1_vec, d1.prin_k2_vec);

            // Compute values of second surface based upon the normal of the first
            let normal0 = point.srf_norm(SrfIndex::First);
            let mut d2 = make_dupin(point.srf_eval_matrix_mut(SrfIndex::Second), normal0)?;
            let (prin_k1_2, prin_k2_2, prin_kv1_2, prin_kv2_2) =
                (d2.prin_k1, d2.prin_k2, d2.prin_k1_vec, d2.prin_k2_vec);

            // if Surf1 has an Umbilical (Spherical) surfacePoint
            let scale = EFF_ZERO * (1.0 + prin_k1.abs());
            if (prin_k1 - prin_k2).abs() < scale {
                // if Surf2->k1 = Surf1->k1
                if (prin_k1 - prin_k1_2).abs() < EFF_ZERO_SQRT {
                    // and Surf2  k2 is the same - coincident surfacePoints
                    if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                        point.point_type = IntersectionPointType::Coincidence;
                        return Ok(());
                    }
                    // only tangent in k1 direction
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv1_2 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }

                // else if Surf2->k2 = Surf1->k2
                if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                    // only tangent in k2 direction
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv2_2 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }
            }

            // if Surf2 has an Umbilical (Spherical) surfacePoint
            let scale = EFF_ZERO * (1.0 + prin_k1_2.abs());
            if (prin_k1_2 - prin_k2_2).abs() < scale {
                if (prin_k1 - prin_k1_2).abs() < EFF_ZERO_SQRT {
                    if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                        point.point_type = IntersectionPointType::Coincidence;
                        return Ok(());
                    }
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv1 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }

                // if Surf1->k2 == Surf2->k1 singular Point is TANGENT_CURVE
                if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv2 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }
            }

            // when surf1->k1_Direction = surf2->k1_Direction
            let cross = prin_kv1.cross(prin_kv1_2);
            if cross.length() < EFF_ZERO_SQRT * 100.0 {
                // when surf1->k1 == surf2->k1
                if (prin_k1 - prin_k1_2).abs() < EFF_ZERO_SQRT {
                    // and surf1->k2 == surf2->k2
                    if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                        self.pick_matching_direction(
                            point,
                            prev,
                            [prin_kv1, -prin_kv1, prin_kv2, -prin_kv2],
                            (prin_k1, prin_k2, prin_kv1, prin_kv2),
                        )?;
                        return Ok(());
                    }
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv1 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }

                // when surf1->k2 == surf2->k2 while surf1->k1 != surf2->k1
                if (prin_k2 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv2 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }
            }

            // when surf1->k1_Direction = +/- surf2->k2_Direction
            let cross2 = prin_kv1.cross(prin_kv2_2);
            if cross2.length() < EFF_ZERO_SQRT * 100.0 {
                // when surf1->k1 == surf2->k2
                if (prin_k1 - prin_k2_2).abs() < EFF_ZERO_SQRT {
                    // and when surf1->k2 == surf2->k1
                    if (prin_k2 - prin_k1_2).abs() < EFF_ZERO_SQRT {
                        self.pick_matching_direction(
                            point,
                            prev,
                            [prin_kv1, -prin_kv1, prin_kv2, -prin_kv2],
                            (prin_k1, prin_k2, prin_kv1, prin_kv2),
                        )?;
                        return Ok(());
                    }
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv1 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }

                // when surf1->k2 == surf2->k1 while surf1->k1 != surf2->k2
                if (prin_k2 - prin_k1_2).abs() < EFF_ZERO_SQRT {
                    point.point_type = IntersectionPointType::TangentCurve;
                    *point.crv_deriv_mut() = prin_kv2 * curve_trace_direction;
                    align_with_previous(point, prev, ang_tol_rad);
                    return Ok(());
                }
            }

            // intersect the properly oriented Dupin indicatrix curves

            // angle between surface's natural coordinate systems
            let angle_rad = point
                .srf_norm(SrfIndex::First)
                .ccw_angle_between(prin_kv1, prin_kv1_2)?;

            // Rotate curves from surface 2 into position on surface 1
            for c in d2.above.iter_mut().chain(d2.below.iter_mut()) {
                c.transform_2d(angle_rad, Point2::new(0.0, 0.0));
            }

            // for every surf1->AboveCurves - intersect with surf2->AboveCurves, then
            // the same for the BelowCurves
            for (curves1, curves2) in [(&d1.above, &d2.above), (&d1.below, &d2.below)] {
                for c1 in curves1 {
                    for c2 in curves2 {
                        // intersect the dupin indicatrix curves
                        let sols = c1.intersect_conic(c2, EFF_ZERO_SQRT * 100.0)?;

                        // for every dupin indicatrix curve/curve xSect solution
                        for sol in &sols {
                            // xSect Point in Surface1 natural coordinate system
                            let p1 = c1.evaluate_point(sol.param).unitize()?;

                            // 3D direction for vector = [sP1 - NaturalOrigin] = sP1
                            let sing_dir = (p1.x * prin_kv1 + p1.y * prin_kv2).unitize()?;

                            // add unique sSingDir vector to aSingDirections
                            add_sing_dir(sing_dir, &mut sing_dir_count, &mut sing_directions)?;
                        }
                    }
                }
            }
        }

        let dirs = sing_directions;
        let dir = |i: usize| dirs.get(i).copied().unwrap_or(Vec3::UNDEF);

        // no singular directions branch
        if sing_dir_count == 0 {
            point.point_type = IntersectionPointType::TangentPoint;
            return Ok(());
        }

        // when number of singular directions is 2
        if sing_dir_count == 2 {
            point.point_type = IntersectionPointType::TangentCurve;

            // set sing direct = 1st singDirection found
            *point.crv_deriv_mut() = dir(0);

            // when there is a PreviousPoint - select best matching direction
            if let Some(p) = prev {
                let angle_min = angle(p, dir(0))?;
                let angle_min2 = angle(p, dir(1))?;
                if angle_min2 < angle_min {
                    *point.crv_deriv_mut() = dir(1);
                }
            }

            align_with_previous(point, prev, ang_tol_rad);
            return Ok(());
        }

        // 3 or more singular directions branch
        if sing_dir_count > 2 {
            point.point_type = IntersectionPointType::Singularity;

            let used = dirs.get(..sing_dir_count).unwrap_or(&[]);

            if last_node.is_none() {
                // starting a curve: seek a singularity direction inside the domains

                // drop SingDirectons into Surf1 and Surf2 parameter space
                let sing_uv_surf1 = drop_vectors(
                    point.srf_du(SrfIndex::First),
                    point.srf_dv(SrfIndex::First),
                    used,
                )?;
                let sing_uv_surf2 = drop_vectors(
                    point.srf_du(SrfIndex::Second),
                    point.srf_dv(SrfIndex::Second),
                    used,
                )?;

                // init output 3DCurveDirection = 1st SingDirection Vector
                *point.crv_deriv_mut() = dir(0);

                // for every SingDirection vector - find those directions which point
                // inside surf1 and surf2
                let mut inside: Vec<usize> = Vec::new();
                for (ii, (uv1, uv2)) in sing_uv_surf1.iter().zip(&sing_uv_surf2).enumerate() {
                    let delta1 = point.uv_pos(SrfIndex::First) + EFF_ZERO_SQRT * *uv1;
                    if self.uv_domains[0].contains_point(delta1, EFF_ZERO_SQRT / 100_000.0) {
                        let delta2 = point.uv_pos(SrfIndex::Second) + EFF_ZERO_SQRT * *uv2;
                        if self.uv_domains[1].contains_point(delta2, EFF_ZERO_SQRT / 100_000.0) {
                            // found a singular direction inside both domains
                            inside.push(ii);
                        }
                    }
                }

                // Decide which of the aSingDirections to use.
                if let Some(p) = prev {
                    if let Some((&first, rest)) = inside.split_first() {
                        // source change (WR-512, #1522): start from the first inside
                        // direction, which is where the search's dAngleMin comes from.
                        // Upstream leaves the output at dir(0), so where the first inside
                        // direction is the closest (or the only one) the curve starts
                        // along a direction that need not point inside either domain.
                        if self.singular_start_inside_direction {
                            *point.crv_deriv_mut() = dir(first);
                        }
                        // Find the singDirection closest to Prev->3DCurve_Direction.
                        let mut angle_min = angle(p, dir(first))?;
                        for &ii in rest {
                            let angle_test = angle(p, dir(ii))?;
                            if angle_test < angle_min {
                                *point.crv_deriv_mut() = dir(ii);
                                angle_min = angle_test;
                            }
                        }
                    } else {
                        // None points inside both domains.  Just use the prev direction.
                        *point.crv_deriv_mut() = p;
                    }
                } else if let Some(&first) = inside.first() {
                    // one inside direction, or several: use the first one
                    *point.crv_deriv_mut() = dir(first);
                }
                // else: no direction was inside both; leave it as it is
            } else {
                // continuing the curve: the direction closest to the last one
                let p = prev.unwrap_or(Vec3::UNDEF);
                *point.crv_deriv_mut() = dir(0);
                let mut angle_min = angle(p, dir(0))?;
                for kk in 1..sing_dir_count {
                    // direction test
                    let angle_test = angle(p, dir(kk))?;
                    if angle_test < angle_min {
                        angle_min = angle_test;
                        *point.crv_deriv_mut() = dir(kk);
                    }

                    // neg direction test
                    let angle_test = angle(p, -dir(kk))?;
                    if angle_test < angle_min {
                        angle_min = angle_test;
                        *point.crv_deriv_mut() = -dir(kk);
                    }
                }
                // (a minimum angle above 20 degrees is only reported upstream)
            }

            return Ok(());
        }

        // a single singular direction is not a valid result
        Err(SolidError::sm_err(OP_SING))
    }

    /// The singularity/coincidence case of matching principal curvatures and
    /// directions: with a previous direction the closest of `dirs`
    /// (`SM_IP_SINGULARITY`), else the direction of the smaller curvature
    /// (`SM_IP_COINCIDENCE`).
    fn pick_matching_direction(
        &self,
        point: &mut TsectPnt,
        prev: Option<Vec3>,
        dirs: [Vec3; 4],
        (prin_k1, prin_k2, prin_kv1, prin_kv2): (f64, f64, Vec3, Vec3),
    ) -> Result<()> {
        let [d0, d1, d2, d3] = dirs;
        *point.crv_deriv_mut() = d0;

        // when there is a prevPoint or a LastNode of m_vCurvePoints
        if let Some(p) = prev {
            // select the direction most like the previous point direction
            let mut angle_min = angle(p, d0)?;
            for d in [d1, d2, d3] {
                let angle_test = angle(p, d)?;
                if angle_test < angle_min {
                    angle_min = angle_test;
                    *point.crv_deriv_mut() = d;
                }
            }
            point.point_type = IntersectionPointType::Singularity;
        } else {
            // select the direction with the smallest radius of curvature
            *point.crv_deriv_mut() = if prin_k2.abs() < prin_k1.abs() {
                prin_kv2 * self.curve_trace_direction
            } else {
                prin_kv1 * self.curve_trace_direction
            };
            point.point_type = IntersectionPointType::Coincidence;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector2d::Vec2;
    use crate::surface::plane::Plane;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H;
    impl SolutionObject for H {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Surface
        }
    }

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn eval(du: Vec3, dv: Vec3, duu: Vec3, duv: Vec3, dvv: Vec3) -> [[Vec3; 3]; 3] {
        let n = du.cross(dv).unitize().unwrap();
        let z = Vec3::new(0.0, 0.0, 0.0);
        [[z, dv, dvv], [du, duv, Vec3::UNDEF], [duu, Vec3::UNDEF, n]]
    }

    #[test]
    fn construct_defaults() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let si: AdvSurfaceIntersector<'_, '_, H> =
            AdvSurfaceIntersector::new(k, &s, &d, &s, &d, false).unwrap();
        assert_eq!(si.closest_value(), BIG_DOUBLE);
        assert!(si.separate_gauss_maps());
        assert_eq!(si.solver.num_trees, 2);
        assert_eq!(si.solver.solution_requested, SolutionRequested::Nodes);
        assert!(si.is_kind_of(TYPE));
        assert!(!si.is_kind_of(7));
    }

    #[test]
    fn solve_depth_guard_rejects_before_touching_branch_state() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut tree = crate::geom::tree::Tree::new(None);
        for _ in 0..global_solver::MAX_SOLVE_DEPTH {
            tree.add_node(TreeNode::new());
        }
        let k = &Kernel::default();
        let mut si: AdvSurfaceIntersector<'_, '_, H> =
            AdvSurfaceIntersector::new(k, &s, &d, &s, &d, false).unwrap();
        si.solver.trees[0] = Some(&tree);
        si.solve_depth = global_solver::MAX_SOLVE_DEPTH + 1;
        si.separate_gauss_maps = false;
        assert!(matches!(
            si.solve_branch(&[None; global_solver::MAX_SOLVER_TREES]),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(si.solve_depth, global_solver::MAX_SOLVE_DEPTH + 1);
        assert!(!si.separate_gauss_maps);
    }

    #[test]
    fn dupin_kinds() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let si: AdvSurfaceIntersector<'_, '_, H> =
            AdvSurfaceIntersector::new(k, &s, &d, &s, &d, false).unwrap();
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = Vec3::new(0.0, 0.0, 1.0);
        let zero = Vec3::new(0.0, 0.0, 0.0);
        // bowl: one ellipse above
        let mut e = eval(x, y, z, zero, z * 2.0);
        let dp = si.make_dupin(&mut e, z).unwrap();
        assert_eq!(dp.above.len(), 1);
        assert!(dp.below.is_empty());
        // saddle: 2 above, 2 below
        let mut e = eval(x, y, z, zero, -z);
        let dp = si.make_dupin(&mut e, z).unwrap();
        assert_eq!((dp.above.len(), dp.below.len()), (2, 2));
        // cylinder: two lines
        let mut e = eval(x, y, z, zero, zero);
        let dp = si.make_dupin(&mut e, -z).unwrap();
        assert_eq!((dp.above.len(), dp.below.len()), (0, 2));
        // plane: nothing
        let mut e = eval(x, y, zero, zero, zero);
        let dp = si.make_dupin(&mut e, z).unwrap();
        assert!(dp.above.is_empty() && dp.below.is_empty());
    }

    #[test]
    fn plane_against_saddle_is_singular() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        let mut si: AdvSurfaceIntersector<'_, '_, H> =
            AdvSurfaceIntersector::new(k, &s, &d, &s, &d, false).unwrap();
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = Vec3::new(0.0, 0.0, 1.0);
        let zero = Vec3::new(0.0, 0.0, 0.0);
        let mut p = TsectPnt::new();
        p.surface_pv[0] = eval(x, y, zero, zero, zero);
        p.surface_pv[1] = eval(x, y, z, zero, -z);
        *p.crv_pos_mut() = zero;
        *p.uv_pos_mut(SrfIndex::First) = Point2::new(0.0, 0.0);
        *p.uv_pos_mut(SrfIndex::Second) = Point2::new(0.0, 0.0);
        let id = si.base.alloc_point(p);
        si.compute_singularity_point([Point2::new(0.0, 0.0); 2], id, None)
            .unwrap();
        let p = si.base.point(id).unwrap();
        assert_eq!(p.point_type, IntersectionPointType::Singularity);
        assert!((p.crv_deriv().length() - 1.0).abs() < 1e-12);
    }

    /// The plane-vs-saddle singular directions of
    /// [`plane_against_saddle_is_singular`] for `curve_trace_direction = -1`, in the
    /// order `ComputeSingularityPoint` stores them: `-x - y`, `x + y`, `-x + y`,
    /// `x - y` (the two asymptotic directions of the saddle and their negations).
    const SING: f64 = core::f64::consts::FRAC_1_SQRT_2;

    /// Runs the production start-point path (`compute_singularity_point` with an arena
    /// previous point) using supplied plane-vs-saddle differential values at the origin.
    /// The surfaces provide the domains; their cached point derivatives are set below
    /// rather than obtained by evaluating a modeled saddle. Returns the point's type
    /// and selected curve derivative.
    fn singular_start(k: &Kernel, domain1: Extent2d, prev: Vec3) -> (IntersectionPointType, Vec3) {
        let s = plane();
        let domain2 = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        // `from_fillet_intersector` keeps the given domains: the plane's natural domain
        // is [0, 1]^2, and the point sits on the u = 0 and v = 0 edges the test varies.
        let mut si: AdvSurfaceIntersector<'_, '_, H> =
            AdvSurfaceIntersector::new(k, &s, &domain1, &s, &domain2, true).unwrap();
        // The stored order above is the one for -1.
        si.base.curve_trace_direction = -1.0;
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = Vec3::new(0.0, 0.0, 1.0);
        let zero = Vec3::new(0.0, 0.0, 0.0);
        let mut p = TsectPnt::new();
        p.surface_pv[0] = eval(x, y, zero, zero, zero);
        p.surface_pv[1] = eval(x, y, z, zero, -z);
        *p.crv_pos_mut() = zero;
        *p.uv_pos_mut(SrfIndex::First) = Point2::new(0.0, 0.0);
        *p.uv_pos_mut(SrfIndex::Second) = Point2::new(0.0, 0.0);
        let mut prev_pnt = TsectPnt::new();
        *prev_pnt.crv_deriv_mut() = prev;
        let prev_id = si.base.alloc_point(prev_pnt);
        let id = si.base.alloc_point(p);
        si.compute_singularity_point([Point2::new(0.0, 0.0); 2], id, Some(prev_id))
            .unwrap();
        let p = si.base.point(id).unwrap();
        (p.point_type, p.crv_deriv())
    }

    #[test]
    fn singular_start_uses_the_only_inside_direction_when_fixed() {
        // Only +x +y points into the plane's [0, 1] x [0, 1] domain, and it is the
        // second stored direction: upstream leaves the result at the first (-x -y),
        // which points into neither domain.
        let domain1 = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let inside = Vec3::new(SING, SING, 0.0);
        let outside = Vec3::new(-SING, -SING, 0.0);

        let off = singular_start(&Kernel::default(), domain1, inside);
        assert_eq!(off.0, IntersectionPointType::Singularity);
        assert!((off.1 - outside).length() < 1e-6, "upstream: {:?}", off.1);

        let mut k = Kernel::default();
        assert!(!k.fixes.ssi_singular_start_inside_direction);
        k.fixes.ssi_singular_start_inside_direction = true;
        let on = singular_start(&k, domain1, inside);
        assert_eq!(on.0, IntersectionPointType::Singularity);
        assert!((on.1 - inside).length() < 1e-6, "fixed: {:?}", on.1);
    }

    #[test]
    fn singular_start_uses_the_nearest_first_inside_direction_when_fixed() {
        // The plane's [0, 1] x [-1, 1] domain holds +x +y and +x -y; the first inside
        // direction is +x +y and it is the nearest to the previous one.
        let domain1 = Extent2d::from_bounds(0.0, -1.0, 1.0, 1.0);
        let nearest = Vec3::new(SING, SING, 0.0);
        let outside = Vec3::new(-SING, -SING, 0.0);

        let off = singular_start(&Kernel::default(), domain1, nearest);
        assert!((off.1 - outside).length() < 1e-6, "upstream: {:?}", off.1);

        let mut k = Kernel::default();
        k.fixes.ssi_singular_start_inside_direction = true;
        let on = singular_start(&k, domain1, nearest);
        assert!((on.1 - nearest).length() < 1e-6, "fixed: {:?}", on.1);
    }

    #[test]
    fn singular_start_still_takes_a_later_inside_direction_when_fixed() {
        // The first inside direction is +x +y, but +x -y is the nearest to the previous
        // one: the later-angle search must still replace the initial choice.
        let domain1 = Extent2d::from_bounds(0.0, -1.0, 1.0, 1.0);
        let later = Vec3::new(SING, -SING, 0.0);

        let mut k = Kernel::default();
        k.fixes.ssi_singular_start_inside_direction = true;
        let on = singular_start(&k, domain1, later);
        assert!((on.1 - later).length() < 1e-6, "fixed: {:?}", on.1);

        // Without the fix the same later direction wins: the default path is unchanged.
        let off = singular_start(&Kernel::default(), domain1, later);
        assert!((off.1 - later).length() < 1e-6, "upstream: {:?}", off.1);
    }

    #[test]
    fn singular_start_default_is_unchanged_when_the_first_direction_is_inside() {
        // With every direction inside, the first stored direction already is inside and
        // the nearest to the previous one: the fix changes nothing.
        let domain1 = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        let nearest = Vec3::new(-SING, -SING, 0.0);

        let off = singular_start(&Kernel::default(), domain1, nearest);
        let mut k = Kernel::default();
        k.fixes.ssi_singular_start_inside_direction = true;
        let on = singular_start(&k, domain1, nearest);
        assert_eq!(off.0, on.0);
        assert!((off.1 - on.1).length() < 1e-12);
        assert!((on.1 - nearest).length() < 1e-6, "fixed: {:?}", on.1);
    }

    #[test]
    fn add_sing_dir_dedupes_and_bounds() {
        let mut dirs = [Vec3::UNDEF; MAX_SING_DIRS];
        let mut n = 0;
        let x = Vec3::new(1.0, 0.0, 0.0);
        add_sing_dir(x, &mut n, &mut dirs).unwrap();
        add_sing_dir(x, &mut n, &mut dirs).unwrap();
        assert_eq!(n, 1);
        for i in 1..8 {
            let a = f64::from(i) * 0.7;
            add_sing_dir(Vec3::new(a.cos(), a.sin(), 0.0), &mut n, &mut dirs).unwrap();
        }
        assert_eq!(n, 8);
        assert!(add_sing_dir(Vec3::new(0.0, 0.0, 1.0), &mut n, &mut dirs).is_err());
    }
}

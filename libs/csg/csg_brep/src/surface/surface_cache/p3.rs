//! `SmSurfaceCache`, part 3: building the subdivision tree of a surface cache (the first
//! generation of Bezier patches, their recursive subdivision to the tessellation
//! tolerances, the top of the tree), splitting one node in two, and finding the nodes
//! whose domain meets a parameter domain.
//!
//! Upstream: `source/SMLib/src/SmSurfaceCache.cpp`.
//!
//! | Rust                                   | Upstream                                 |
//! |----------------------------------------|------------------------------------------|
//! | [`SurfaceCache::find_uv_nodes`], [`SurfaceCache::find_uv_nodes_built`] | `SmSurfaceCache::FindUVNodes` |
//! | [`SurfaceCache::subdivide_node`]       | `SmSurfaceCache::SubdivideNode`          |
//! | [`SurfaceCache::subdivide_to_tolerances`], [`SurfaceCache::subdivide_to_tolerances_with`] | `SmSurfaceCache::SubdivideToTolerances` |
//! | [`SurfaceCache::build_tree_base`], [`SurfaceCache::build_tree_base_with`], [`TreeBase`] | `SmSurfaceCache::BuildTreeBase` |
//! | [`SurfaceCache::build_tree`], [`SurfaceCache::build_tree_with`] | `SmSurfaceCache::BuildTree` |
//! | [`SurfaceCache::get_tree`]             | `SmSurfaceCache::GetTree`                |
//! | [`SurfaceCache::new`] (in part 1)      | `SmSurfaceCache::SmSurfaceCache`         |
//!
//! Deviations:
//! - **Constructor.** The field initialisation of `SmSurfaceCache::SmSurfaceCache` is
//!   [`SurfaceCache::new`] in [`crate::surface::surface_cache::p1`]. The rest of it sizes
//!   `SmMemBlockMgr` blocks (`m_lBezierSize`, `m_lTotalBezSize`) and allocates the empty
//!   continuity arrays: no counterpart.
//! - **Memory blocks.** `pBezierBlock` / `pOptBezierBlock` (the `gw_SURFACE` memory of
//!   the construction Bezier patches) have no counterpart: a patch owns its Bezier
//!   ([`BezierPatch::bezier`]). `BuildTree(pOptBezierBlock)` becomes
//!   [`SurfaceCache::build_tree`]`(for_derived_cache)`: `true` for a non-NULL block (a
//!   derived cache finishes the construction), `false` for NULL. New nodes and their data
//!   come from zero-filled block memory upstream: all-zero boxes, as in part 2.
//! - **Failed patch split.** `SubdivideNode` ignores the status of `sm_SplitSrf`
//!   (`GW_ERR` logs only): the children then are the zero-filled patches the block
//!   memory holds (`zeroed_like`), as for the approximation branch. Before PR #147
//!   round 4 the port returned the error, failing the cache build of a surface whose
//!   constructed knot vector is non-monotone, which upstream builds.
//! - **Split surfaces.** Splitting a node that holds a temporary subdivision surface
//!   (only `SmTessSrfCache` puts them on nodes; the tree build never does) snaps the
//!   split to a knot and splits the surface; the halves are named by the cache's
//!   subdivision handle maker ([`SurfaceCache::set_subdivision_handles`]): a cache
//!   without one returns [`SolidError::InvalidInput`] there.
//! - **Unused results.** `BuildTreeBase` reads the knots of a B-spline surface into
//!   split arrays that only the disabled tree vertex code uses (the status is ignored):
//!   not read. `SubdivideNode` fills split arrays for the same disabled code, and reads
//!   `GetFirstBezierPatch` in a branch whose two arms do the same (copy the parent box):
//!   both omitted.
//! - **Recursion.** `SubdivideToTolerances` recurses into both children without a
//!   bound; the port stops past [`MAX_SUBDIVISION_DEPTH`] levels with
//!   [`SolidError::InvalidInput`] (upstream fixtures stay far below it).
//! - **Node count.** Neither does upstream bound the number of nodes it makes: a surface
//!   much larger than the tessellation tolerances (e.g. control points at `1e4` with the
//!   default tolerances) is split into millions of patches, which upstream allocates for
//!   minutes and Rust aborts on (a 3.5 GB patch array in a `surface.surface_p11` run).
//!   The port refuses to split a node once the tree holds [`MAX_SUBDIVISION_NODES`]
//!   nodes, with [`SolidError::InvalidInput`]. The bound is `2^22` on 64-bit targets,
//!   twice the largest tree an upstream fixture reaches (2 014 255 nodes, the trimmed
//!   cache of a `1e-4`-scale face in the `topology.edgeuse_p3` adversarial fixtures,
//!   measured on the C++ build), and `2^20` on 32-bit targets (wasm32), whose 4 GB
//!   address space cannot hold a larger patch array.
//! - **Approximation arithmetic.** The base tolerance test refuses refinement driven
//!   by non-finite sampled angle lengths (see [`super::p2`]). The exact
//!   `surface.surface_point_solve_p3` hang input reaches this failure while its
//!   `1e300` control point is still finite. Generic traversal imposes no coordinate,
//!   box-size or tolerance-finiteness cutoff: a large translation, a custom tester,
//!   or an overflowing chord tolerance alone does not establish invalid refinement.
//! - **Cyclic trees.** `FindUVNodes` walks the child links of a tree whose links form a
//!   cycle forever (its stack grows without bound): the port stops after more node
//!   visits than a tree of that size has (twice the node count plus one) with
//!   [`SolidError::InvalidInput`].
//! - **Undefined behaviour.** Where upstream would dereference a null tree (a failed
//!   `GetTree`), a null `m_pNurb`, a node without data or a node that already has
//!   children, the port returns [`SolidError::InvalidInput`]. `SubdivideToTolerances`
//!   reads `mBA_pSurface` through an `SmBezierAux2d` cast of any node data, which every
//!   surface node has: the same field here.
//! - **Patch boxes.** The boxes of the construction Bezier patches are computed on
//!   B-spline surfaces made from bare patches (`SmBSplineSurface(pBezier, TRUE, ..)`),
//!   which have no owner: `CalculateBoundingBox` expands them by `SM_ZONE_TOL_3D`
//!   (`GetFace()` and `GetBrep()` are NULL). The port computes them unexpanded and expands
//!   them by [`ZONE_TOL_3D`] itself, since the B-spline port cannot ask
//!   `SmSurface::GetFace` (unported) whether a surface has an owner.
//! - **Surface kinds.** `IsKindOf(SmBSplineSurface_TYPE)` holds for planes too (an
//!   `SmPlane` is an `SmBSplineSurface`), as in part 1.
//! - **Ignored statuses** stay ignored (`CalculateBoundingBox` of the u split halves,
//!   `ImplantPolyEdge`, `ImplantEdgeuse`, `sm_ComputeBezierApproximation`), except
//!   [`SolidError::Unported`], which is always propagated.
//! - **Virtual calls.** `SubdivideToTolerances` calls the virtual
//!   `TestAgainstTolerances`: the `_with` variants
//!   ([`SurfaceCache::subdivide_to_tolerances_with`], [`SurfaceCache::build_tree_with`],
//!   [`SurfaceCache::build_tree_base_with`]) take the derived cache's override
//!   ([`ToleranceTester`]); the plain ones run the base class test. `FindUVNodes` starts
//!   with `GetTree`; a derived cache runs its own and then
//!   [`SurfaceCache::find_uv_nodes_built`]. `SubdivideNode` moves a boundary parent's edge uses and poly edges
//!   into the children through the virtual `ImplantEdgeuse` / `ImplantPolyEdge`:
//!   [`SurfaceCache::subdivide_node`] runs the base class's (which fail: the edge uses are
//!   dropped), and the trimmed surface cache splits through
//!   [`SurfaceCache::subdivide_node_split`] and its own implants. `GetTree` dispatches
//!   `BuildTree()` virtually, to `SmTrimSrfCache::BuildTree` on a trimmed surface cache.
//!   `SubdivideNode` also starts with `GetTree`; its build or rebuild is not reached
//!   there (the node is of the built tree, and the builds that subdivide are under
//!   construction), so [`SurfaceCache::subdivide_node_split`] reads the built tree. [`SurfaceCache::get_tree`] preserves standalone trimmed-cache
//!   flags and rejects a face-owned trimmed build without a [`CacheFace`](crate::surface::trim_srf_cache::CacheFace)
//!   adapter, instead of silently building a plain cache. [`SurfaceCache::subdivide_node`]
//!   does not call `GetTree` (it needs a built tree and fails without one).
//! - **Cache slot.** Filling the continuity arrays (`BuildTreeBase`) also marks the
//!   cache slot of the cache's surface built
//!   ([`CacheTreeState::Built`](crate::surface::surface::CacheTreeState)), so that a
//!   later cache made for the same surface object shows its continuities as upstream's
//!   reused cache does.

use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::tree::{
    AuxDataType, NodeClass, NodeData, NodeGeomType, NodeId, SplitDir, SurfaceAux, Tree, TreeNode,
    TreeOwner,
};
use crate::kernel::Kernel;
use crate::math::Point2;
use crate::nurbs::b_spline_surface::{BSplineSurface, BoundingBoxRequest, SurfParamType};
use crate::nurbs::nurbs_srf::split_srf;
use crate::nurbs::srf_basic::Direction;
use crate::nurbs::srf_basic::Surface as NurbSurface;
use crate::surface::surface::{CacheTreeState, Surface, SurfaceKind};

use crate::geom::geom_utility::p2::ContinuityType;

use super::p1::{
    bspline_of, compute_bezier_approximation_memo, invalid, BezierAux2d, BezierPatch,
    FaceContainment, SurfaceCache,
};
use super::p2::{
    create_approximate_bezier_patch_block, create_bezier_patch_block, ToleranceTester,
};

const OP_FIND: &str = "surface::surface_cache::p3::SurfaceCache::find_uv_nodes";
const OP_NODE: &str = "surface::surface_cache::p3::SurfaceCache::subdivide_node";
const OP_TOL: &str = "surface::surface_cache::p3::SurfaceCache::subdivide_to_tolerances";
const OP_BASE: &str = "surface::surface_cache::p3::SurfaceCache::build_tree_base";
const OP_GET: &str = "surface::surface_cache::p3::SurfaceCache::get_tree";

/// Deepest `SubdivideToTolerances` recursion the port follows (see the module
/// deviations).
pub const MAX_SUBDIVISION_DEPTH: u32 = 512;

/// Most tree nodes `SubdivideToTolerances` splits up to (see the module deviations):
/// `2^22` on 64-bit targets, twice the 2 014 255 nodes upstream makes for the trimmed
/// cache of a `1e-4`-scale face in the `topology.edgeuse_p3` fixtures (a generated
/// `surface.surface_p1` case at `1e-4` scale needs more than `2^18`).
#[cfg(target_pointer_width = "64")]
pub const MAX_SUBDIVISION_NODES: usize = 1 << 22;

/// Most tree nodes `SubdivideToTolerances` splits up to (see the module deviations):
/// `2^20` on 32-bit targets (wasm32), about 1.7 GB of Bezier patches, as much as a
/// 4 GB address space holds.
#[cfg(not(target_pointer_width = "64"))]
pub const MAX_SUBDIVISION_NODES: usize = 1 << 20;

/// `SM_SS_UMIN` .. `SM_SS_VMAX`: the singular sides of a surface.
const SS_UMIN: u32 = 1;
const SS_VMIN: u32 = 2;
const SS_UMAX: u32 = 4;
const SS_VMAX: u32 = 8;

/// The first generation of a tree ([`SurfaceCache::build_tree_base`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeBase {
    /// `rNodes`: the first generation nodes, v index fastest.
    pub nodes: Vec<NodeId>,
    /// `rl1stGenerationUCount`: their number of u entries.
    pub u_count: usize,
    /// `rl1stGenerationVCount`: their number of v entries.
    pub v_count: usize,
}

/// `SmBSplineSurface sBSS(pBezier, TRUE, ..)`: a borrowed B-spline over a Bezier patch.
fn bss_of(bezier: &NurbSurface) -> BSplineSurface {
    BSplineSurface::from_nurb(bezier.clone(), true)
}

/// `sBSS.CalculateBoundingBox(sBSS.GetNaturalUVDomain(), &box, &pseudo, NULL, guess)` of a
/// B-spline over a bare Bezier patch: the normal and pseudo boxes, expanded by the zone
/// tolerance. The surface has no owner, so `GetFace()` and `GetBrep()` are NULL and the
/// tolerance is `SM_ZONE_TOL_3D` (see the module deviations).
fn patch_boxes(
    k: &Kernel,
    bss: &BSplineSurface,
    pseudo_guess: Option<&crate::geom::pseudo_box::PseudoBox>,
    op: &'static str,
) -> Result<(
    crate::geom::extent3d::Extent3d,
    crate::geom::pseudo_box::PseudoBox,
)> {
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: true,
        polar: false,
        pseudo_guess,
        polar_guess: None,
        expand_by_zone_tol: false,
        expand_tolerance: None,
    };
    let boxes = bss.calculate_bounding_box(k, &bss.natural_uv_domain(), &req)?;
    let mut bbox = boxes.normal.ok_or_else(|| invalid(op, "no bounding box"))?;
    let mut pseudo = boxes.pseudo.ok_or_else(|| invalid(op, "no pseudo box"))?;
    bbox.expand_absolute(ZONE_TOL_3D);
    pseudo.expand_absolute(ZONE_TOL_3D);
    Ok((bbox, pseudo))
}

impl<O: Copy + PartialEq> SurfaceCache<O> {
    fn tree_or(&self, op: &'static str) -> Result<&Tree<O>> {
        self.tree().ok_or_else(|| invalid(op, "cache has no tree"))
    }

    fn tree_mut_or(&mut self, op: &'static str) -> Result<&mut Tree<O>> {
        self.tree_mut()
            .ok_or_else(|| invalid(op, "cache has no tree"))
    }

    /// The `SmBezierAux2d` fields kept on a node (`m_eSplitDir`, `m_bIsMarked`,
    /// `m_eNodeClass`, `m_pFace`, `mBA_pSurface`, the data slot).
    fn surface_aux(&self, node: NodeId, op: &'static str) -> Result<SurfaceAux<O>> {
        match self.tree_or(op)?.node(node)?.data {
            NodeData::Surface(a) => Ok(a),
            _ => Err(invalid(op, "node has no surface data")),
        }
    }

    fn surface_aux_mut(&mut self, node: NodeId, op: &'static str) -> Result<&mut SurfaceAux<O>> {
        match &mut self.tree_mut_or(op)?.node_mut(node)?.data {
            NodeData::Surface(a) => Ok(a),
            _ => Err(invalid(op, "node has no surface data")),
        }
    }

    // ---- FindUVNodes ----------------------------------------------------------------

    /// The nodes whose parametric domain meets `uv_domain` (just touching counts),
    /// leaves and interior nodes, in upstream's stack order (`FindUVNodes`). With
    /// `check_for_poles`, nodes along the singular sides of the surface that
    /// `uv_domain` reaches are kept too. Builds the tree first when needed
    /// ([`SurfaceCache::get_tree`]).
    ///
    /// # Errors
    ///
    /// As [`SurfaceCache::get_tree`] and `Surface::get_singularities`.
    pub fn find_uv_nodes(
        &mut self,
        k: &Kernel,
        uv_domain: &Extent2d,
        check_for_poles: bool,
    ) -> Result<Vec<NodeId>> {
        // locals
        self.get_tree(k)?;
        self.find_uv_nodes_built(k, uv_domain, check_for_poles)
    }

    /// [`SurfaceCache::find_uv_nodes`] once `GetTree` has run: searches the stored
    /// tree without building it (a derived cache runs its own `GetTree` first, the
    /// virtual `BuildTree` upstream).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without a tree; [`SolidError::Unported`] as
    /// [`SurfaceCache::find_uv_nodes`].
    pub fn find_uv_nodes_built(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        check_for_poles: bool,
    ) -> Result<Vec<NodeId>> {
        // init output
        let mut nodes = Vec::new();

        let top = self.tree_or(OP_FIND)?.top_node();
        let mut stack: Vec<Option<NodeId>> = Vec::new();

        // set up for poles
        let mut poles: u32 = 0;
        let (mut u_ivl, mut v_ivl) = (
            crate::geom::extent1d::Extent1d::new(),
            crate::geom::extent1d::Extent1d::new(),
        );
        let (mut u_tol, mut v_tol) = (0.0, 0.0);
        if check_for_poles {
            let srf_domain = self.surface().natural_uv_domain_with_nurb(k)?;

            // save natural domain sizes
            u_ivl = srf_domain.u_interval();
            v_ivl = srf_domain.v_interval();
            u_tol = u_ivl.length() / 100.0;
            v_tol = v_ivl.length() / 100.0;

            // list uv_domain boundaries near the surface's natural domain boundaries
            let (dmin, dmax) = (uv_domain.min(), uv_domain.max());
            poles = u32::from((dmin.x - u_ivl.min()).abs() < u_tol)
                + 2 * u32::from((dmin.y - v_ivl.min()).abs() < v_tol)
                + 4 * u32::from((dmax.x - u_ivl.max()).abs() < u_tol)
                + 8 * u32::from((dmax.y - v_ivl.max()).abs() < v_tol);

            // eliminate boundaries which are not poles
            if poles != 0 {
                poles &= self.surface().get_singularities(k, false, false)?.singular;
            }
        }

        // init a node stack
        stack.push(top);

        // a tree visits each node, and each missing second child, at most once
        let visit_limit = self
            .tree_or(OP_FIND)?
            .nodes()
            .len()
            .saturating_mul(2)
            .saturating_add(1);
        let mut visits = 0usize;

        // while nodes remain to be processed
        while let Some(entry) = stack.pop() {
            visits += 1;
            if visits > visit_limit {
                return Err(invalid(OP_FIND, "tree node links are cyclic"));
            }
            // get current node
            let Some(node) = entry else {
                // upstream: "Error: Surface cache: Null node"
                continue;
            };
            let aux_domain = match self.tree_or(OP_FIND)?.node(node)?.data {
                NodeData::Surface(SurfaceAux { slot: Some(_), .. }) => self.aux(node)?.uv_domain,
                _ => continue, // a null data pointer: upstream reports it and skips the node
            };

            // keep nodes which are not disjoint
            let keep_for_position = !aux_domain.are_disjoint(uv_domain, 0.0);

            // nodes near poles
            let mut keep_for_pole = false;
            if poles != 0 && !keep_for_position {
                let (amin, amax) = (aux_domain.min(), aux_domain.max());
                keep_for_pole = ((poles & SS_UMIN) != 0 && (amin.x - u_ivl.min()).abs() < u_tol)
                    || ((poles & SS_UMAX) != 0 && (amax.x - u_ivl.max()).abs() < u_tol)
                    || ((poles & SS_VMIN) != 0 && (amin.y - v_ivl.min()).abs() < v_tol)
                    || ((poles & SS_VMAX) != 0 && (amax.y - v_ivl.max()).abs() < v_tol);
            }

            // skip disjoint nodes not on a pole
            if !keep_for_position && !keep_for_pole {
                continue;
            }

            // arrive here when node domains intersect - add to output list
            nodes.push(node);

            // if node has children - add those to stack
            let n = self.tree_or(OP_FIND)?.node(node)?;
            if n.child1.is_some() {
                // Interior node
                stack.push(n.child1);
                stack.push(n.child2);
            }
        }

        Ok(nodes)
    }

    // ---- SubdivideNode --------------------------------------------------------------

    /// Splits the leaf `node` in two halves across `direction` ([`SurfParamType::U`]:
    /// child 1 left, child 2 right; [`SurfParamType::V`]: child 1 bottom, child 2 top)
    /// at the middle of its domain (`SubdivideNode`). A Bezier patch node
    /// (`SM_AD_BEZIER_SURFACE`) has its patch split (a B-spline cache) or approximated
    /// again over each half (any other surface) unless `only_aux_data`; otherwise the
    /// children get `SM_AD_AUX_DATA`, and when the node holds a subdivision surface the
    /// split moves to a nearby knot and the surface is split too (its halves are given
    /// handles from `new_handle`). Edge uses and poly edges of a node on the boundary
    /// are implanted in the children.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a missing tree, a node without surface data or with
    /// children, a patch node without its construction Bezier, or a needed handle with
    /// no `new_handle`; failures of the Bezier split and the patch boxes;
    /// [`SolidError::Unported`] for `SmSurface::SnapToKnots` (a node with a subdivision
    /// surface) and from the part 1 implants.
    pub fn subdivide_node(
        &mut self,
        k: &Kernel,
        node: NodeId,
        direction: SurfParamType,
        only_aux_data: bool,
    ) -> Result<()> {
        if let Some((node1, node2)) =
            self.subdivide_node_split(k, node, direction, only_aux_data)?
        {
            self.move_boundary_uses_base(node, node1, node2)?;
        }
        Ok(())
    }

    /// [`SurfaceCache::subdivide_node`] up to the edge uses and poly edges of a parent on
    /// the boundary: splits `node` and, when it is on the boundary, sets its children's
    /// class to unknown and returns them; the caller then moves the parent's edge uses
    /// and poly edges into them through its `ImplantEdgeuse` / `ImplantPolyEdge`
    /// (virtual upstream: the base class's in [`SurfaceCache::subdivide_node`], the
    /// trimmed surface cache's in
    /// [`TrimSrfCache`](crate::surface::trim_srf_cache::TrimSrfCache)).
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceCache::subdivide_node`] but the implants.
    pub fn subdivide_node_split(
        &mut self,
        k: &Kernel,
        node: NodeId,
        direction: SurfParamType,
        only_aux_data: bool,
    ) -> Result<Option<(NodeId, NodeId)>> {
        let mut only_aux_data = only_aux_data;
        // Upstream starts with GetTree(), which builds the tree when there is none or
        // rebuilds it for a face-owned plain cache outside a build (m_bHaveTSurfaceCache
        // FALSE, m_bFaceContainmentDone != 2). Neither is reached: `node` is a node of
        // this cache's tree, the builds that subdivide hold FaceContainment::
        // UnderConstruction (SmTemporaryChangeValue in BuildTree), and a face-owned
        // trimmed build without its face is refused before it subdivides. (A rebuild
        // there would free `node` upstream.) The port reads the built tree.
        let (aux_type, parent_bbox, has_children, is_top) = {
            let tree = self.tree_or(OP_NODE)?;
            let n = tree.node(node)?;
            (
                n.aux_type,
                n.bbox,
                n.child1.is_some() || n.child2.is_some(),
                tree.top_node() == Some(node),
            )
        };
        // check state - node is from a surface subdivision tree and is not yet a parent
        if !matches!(aux_type, AuxDataType::BezierSurface | AuxDataType::AuxData) {
            return Err(invalid(OP_NODE, "node is not a surface subdivision node"));
        }
        if has_children {
            return Err(invalid(OP_NODE, "node already has children"));
        }
        let parent_fields = self.surface_aux(node, OP_NODE)?;

        // don't try to split BezierPatch data for things that don't have Bezier patches
        if aux_type == AuxDataType::AuxData {
            only_aux_data = true;
        }
        let split_patch = aux_type == AuxDataType::BezierSurface && !only_aux_data;

        // the parent's SmBezierAux2d values the children inherit
        let parent_aux = self.aux(node)?;
        let node_class = parent_fields.node_class;
        let chord_ok = parent_aux.chord_height_satisfied;
        let angle_ok = parent_aux.angle_tol_satisfied;
        let uv_min = parent_aux.uv_domain.min();
        let uv_max = parent_aux.uv_domain.max();

        // allocate and initialize child1 and child2 node memory, set parent/child
        // relationships (zero-filled block memory: all-zero boxes)
        let (node1, node2) = {
            let tree = self.tree_mut_or(OP_NODE)?;
            let mut child = TreeNode::<O>::new();
            child.bbox = crate::geom::extent3d::Extent3d::from_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            child.parent = Some(node);
            let node1 = tree.add_node(child);
            let node2 = tree.add_node(child);
            let n = tree.node_mut(node)?;
            n.child1 = Some(node1);
            n.child2 = Some(node2);
            (node1, node2)
        };

        // next: allocate and initialize the data of each new child node, either
        // SmBezierPatch or SmBezierAux2d memory
        let mut subdivision_surface: Option<O> = None;
        let (slot1, slot2, child_type) = if split_patch {
            let parent_patch = self.patch(node)?;
            if parent_patch.bezier.is_none() {
                return Err(invalid(OP_NODE, "patch node without its Bezier"));
            }
            let (uc, vc) = (parent_patch.u_curve_conts, parent_patch.v_curve_conts);
            // init the new BezierPatch continuity values
            let child = BezierPatch {
                u_curve_conts: uc,
                v_curve_conts: vc,
                ..BezierPatch::default()
            };
            let slot1 = self.patches().len();
            self.patches_mut().push(child.clone());
            self.patches_mut().push(child);
            (slot1, slot1 + 1, AuxDataType::BezierSurface)
        } else {
            // depending on history mBA_pSurface is NULL, a subdivision surface the size of
            // the node or the root surface; remember a subdivision surface to split it
            if parent_fields.surface.is_some()
                && (is_top || self.surface_handle() != parent_fields.surface)
            {
                subdivision_surface = parent_fields.surface;
            }
            let slot1 = self.auxs().len();
            self.auxs_mut().push(BezierAux2d::default());
            self.auxs_mut().push(BezierAux2d::default());
            (slot1, slot1 + 1, AuxDataType::AuxData)
        };

        // set the children's remaining SmBezierAux2d data (not m_eSplitDir, the polar
        // box and the SmBezierPatch data)
        for (child, slot) in [(node1, slot1), (node2, slot2)] {
            {
                let n = self.tree_mut_or(OP_NODE)?.node_mut(child)?;
                n.aux_type = child_type;
                n.geom_type = NodeGeomType::Default;
                n.data = NodeData::Surface(SurfaceAux {
                    split_dir: SplitDir::U,
                    is_marked: false,
                    node_class,
                    face: None,
                    surface: None,
                    slot: Some(slot),
                });
            }
            let a = self.aux_mut(child)?;
            a.vertex_list.clear();
            a.edgeuse_list.clear();
            a.poly_edge_list.clear();
            a.owning_tree_node = Some(child);
            a.is_tess_only = false;
            a.chord_height_satisfied = chord_ok;
            a.angle_tol_satisfied = angle_ok;
        }

        // branch on split direction to build split child shapes and set child domains
        // and their bounding boxes
        let (dir, split_param) = if direction == SurfParamType::U {
            // let child1 = left, child2 = right
            self.surface_aux_mut(node, OP_NODE)?.split_dir = SplitDir::U;
            let split_param = (uv_max.x + uv_min.x) / 2.0;

            // when splitting the bezierPatch - mark child continuities across the split
            if split_patch {
                self.patch_mut_slot(slot1)?.u_curve_conts[1] = ContinuityType::C1C2;
                self.patch_mut_slot(slot2)?.u_curve_conts[0] = ContinuityType::C1C2;
            }

            if let Some(handle) = subdivision_surface {
                // if possible move the split parameter to a nearby existing knot, then
                // split the subdivision surface in two (child boxes from the halves)
                let split_param = self.split_subdivision_surface(
                    k,
                    handle,
                    [node1, node2],
                    SurfParamType::U,
                    split_param,
                    uv_min,
                    uv_max,
                )?;
                (Direction::U, split_param)
            } else {
                // either an SM_AD_BEZIER_SURFACE with a Bezier patch, or embedding an
                // edge use
                self.aux_mut(node1)?.uv_domain =
                    Extent2d::from_points(uv_min, Point2::new(split_param, uv_max.y));
                self.aux_mut(node2)?.uv_domain =
                    Extent2d::from_points(Point2::new(split_param, uv_min.y), uv_max);
                if !split_patch {
                    self.copy_parent_boxes(node1, node2, parent_bbox)?;
                }
                (Direction::U, split_param)
            }
        } else {
            // let child1 = bottom, child2 = top
            self.surface_aux_mut(node, OP_NODE)?.split_dir = SplitDir::V;
            let split_param = (uv_max.y + uv_min.y) / 2.0;

            // when splitting the bezierPatch - mark child continuities across the split
            if split_patch {
                self.patch_mut_slot(slot1)?.v_curve_conts[1] = ContinuityType::C1C2;
                self.patch_mut_slot(slot2)?.v_curve_conts[0] = ContinuityType::C1C2;
            }

            if let Some(handle) = subdivision_surface {
                // if possible move the split parameter to a nearby existing knot, then
                // split the subdivision surface in two (child boxes from the halves)
                let split_param = self.split_subdivision_surface(
                    k,
                    handle,
                    [node1, node2],
                    SurfParamType::V,
                    split_param,
                    uv_min,
                    uv_max,
                )?;
                (Direction::V, split_param)
            } else {
                self.aux_mut(node1)?.uv_domain =
                    Extent2d::from_points(uv_min, Point2::new(uv_max.x, split_param));
                self.aux_mut(node2)?.uv_domain =
                    Extent2d::from_points(Point2::new(uv_min.x, split_param), uv_max);
                if !split_patch {
                    self.copy_parent_boxes(node1, node2, parent_bbox)?;
                }
                (Direction::V, split_param)
            }
        };

        // still need bounding boxes when the node has a Bezier patch to split
        if split_patch {
            let parent_patch = self.patch(node)?;
            let parent_pseudo = parent_patch.pseudo_box;
            let parent_bezier = parent_patch
                .bezier
                .clone()
                .ok_or_else(|| invalid(OP_NODE, "patch node without its Bezier"))?;
            let (bez1, bez2) = if bspline_of(self.surface()).is_some() {
                // build child Beziers = split parent Bezier patch; upstream's `GW_ERR` only
                // logs a failed split, whose checks (`N_KnotVectorIsEndParam`, span and
                // multiplicity) fail before it writes, so the children keep their
                // zero-filled block memory (sized like the parent by
                // `sm_InitNurbSurfaceMemory`)
                match ignore(split_srf(&parent_bezier, split_param, dir))? {
                    Some(children) => children,
                    None => (zeroed_like(&parent_bezier)?, zeroed_like(&parent_bezier)?),
                }
            } else {
                // let child Bezier patches = Bezier approximations to the parent surface
                // subdomain (coarse for all but the final leaf nodes)
                // (zero-filled memory with the parent patch's sizes)
                let mut b1 = zeroed_like(&parent_bezier)?;
                let mut b2 = zeroed_like(&parent_bezier)?;
                let d1 = self.aux(node1)?.uv_domain;
                let d2 = self.aux(node2)?.uv_domain;
                let (surface, mut memo) = self.surface_and_pole_memo();
                ignore(compute_bezier_approximation_memo(
                    k,
                    surface,
                    &d1,
                    &mut b1,
                    memo.as_deref_mut(),
                ))?;
                ignore(compute_bezier_approximation_memo(
                    k, surface, &d2, &mut b2, memo,
                ))?;
                (b1, b2)
            };

            // get and store child 1 and 2 bounding and pseudo boxes
            for (child, slot, bez) in [(node1, slot1, bez1), (node2, slot2, bez2)] {
                let (bbox, pseudo) = patch_boxes(k, &bss_of(&bez), Some(&parent_pseudo), OP_NODE)?;
                self.tree_mut_or(OP_NODE)?.node_mut(child)?.bbox = bbox;
                let p = self.patch_mut_slot(slot)?;
                p.pseudo_box = pseudo;
                p.bezier = Some(bez);
            }
        }

        // when the parent is on the boundary, its edge uses (or poly edges) move into
        // the children
        if node_class == NodeClass::OnBoundary {
            // init the node classification
            self.surface_aux_mut(node1, OP_NODE)?.node_class = NodeClass::Unknown;
            self.surface_aux_mut(node2, OP_NODE)?.node_class = NodeClass::Unknown;
            return Ok(Some((node1, node2)));
        }

        Ok(None)
    }

    /// The subdivision surface branch of `SubdivideNode`: snaps `split_param` to a
    /// knot of the node's subdivision surface `handle` within a quarter of the node's
    /// side, splits that surface there, keeps the halves as new subdivision surfaces
    /// on the children `[node1, node2]` (`SM_CAST_PTR(SmBSplineSurface, ..)`: only
    /// B-spline halves are kept on the nodes), sets the children's domains and their
    /// boxes from the halves; returns the split parameter used. The u split ignores
    /// the box statuses, the v split returns them (as upstream).
    fn split_subdivision_surface(
        &mut self,
        k: &Kernel,
        handle: O,
        [node1, node2]: [NodeId; 2],
        direction: SurfParamType,
        split_param: f64,
        uv_min: Point2,
        uv_max: Point2,
    ) -> Result<f64> {
        let sub = self.node_surface(Some(handle))?.clone();
        let (orig, snap_dist) = if direction == SurfParamType::U {
            (
                Point2::new(split_param, uv_min.y),
                (uv_max.x - uv_min.x) / 4.0,
            )
        } else {
            (
                Point2::new(uv_min.x, split_param),
                (uv_max.y - uv_min.y) / 4.0,
            )
        };
        let snapped = sub.snap_to_knots(k, orig, snap_dist)?;
        let split_param = if direction == SurfParamType::U {
            snapped.x
        } else {
            snapped.y
        };

        // build two new split child surfaces from the original - don't modify the original
        let (half1, half2) = sub.split_at(k, split_param, direction)?;

        // store the child surfaces in the subdivision surface list and on the nodes
        let mut handles = [None, None];
        for (slot, half) in handles.iter_mut().zip([half1, half2]) {
            let h = self
                .new_subdivision_handle()
                .ok_or_else(|| invalid(OP_NODE, "cache cannot name new subdivision surfaces"))?;
            let is_bspline = matches!(half.kind(), SurfaceKind::BSpline(_));
            self.subdivision_surfaces_mut().push((h, half));
            *slot = is_bspline.then_some(h);
        }
        self.surface_aux_mut(node1, OP_NODE)?.surface = handles[0];
        self.surface_aux_mut(node2, OP_NODE)?.surface = handles[1];

        // set child domains
        let (d1, d2) = if direction == SurfParamType::U {
            (
                Extent2d::from_points(uv_min, Point2::new(split_param, uv_max.y)),
                Extent2d::from_points(Point2::new(split_param, uv_min.y), uv_max),
            )
        } else {
            (
                Extent2d::from_points(uv_min, Point2::new(uv_max.x, split_param)),
                Extent2d::from_points(Point2::new(uv_min.x, split_param), uv_max),
            )
        };
        self.aux_mut(node1)?.uv_domain = d1;
        self.aux_mut(node2)?.uv_domain = d2;

        // set the node bounding boxes from the halves
        let n = self.subdivision_surfaces_mut().len();
        for (child, domain, half_index) in [(node1, d1, n - 2), (node2, d2, n - 1)] {
            let r = {
                let half = &self
                    .subdivision_surfaces_mut()
                    .get(half_index)
                    .ok_or_else(|| invalid(OP_NODE, "subdivision surface missing"))?
                    .1;
                half_box(k, half, &domain)
            };
            let bbox = if direction == SurfParamType::U {
                ignore(r)?
            } else {
                Some(r?)
            };
            if let Some(b) = bbox {
                self.tree_mut_or(OP_NODE)?.node_mut(child)?.bbox = b;
            }
        }
        Ok(split_param)
    }

    /// The end of `SubdivideNode` for a parent on the boundary, with the base class's
    /// `ImplantEdgeuse` / `ImplantPolyEdge` (both fail, the status is ignored): the
    /// parent's edge uses are dropped, last first.
    fn move_boundary_uses_base(
        &mut self,
        node: NodeId,
        node1: NodeId,
        node2: NodeId,
    ) -> Result<()> {
        let poly_edges = self.aux(node)?.poly_edge_list.clone();
        if poly_edges.is_empty() {
            // for every edge use in the parent node, last first
            while let Some(eu) = self.aux_mut(node)?.edgeuse_list.last().copied() {
                // implant the edge use in both children without subsequent subdivision
                ignore(self.implant_edgeuse_in(eu, node1))?;
                ignore(self.implant_edgeuse_in(eu, node2))?;
                self.aux_mut(node)?.edgeuse_list.pop();
            }
        } else {
            for pe in poly_edges {
                ignore(self.implant_poly_edge(pe, node1))?;
                ignore(self.implant_poly_edge(pe, node2))?;
            }
        }
        Ok(())
    }

    /// The patch in slot `slot` of the patch arena.
    fn patch_mut_slot(&mut self, slot: usize) -> Result<&mut BezierPatch<O>> {
        self.patches_mut()
            .get_mut(slot)
            .ok_or_else(|| invalid(OP_NODE, "patch slot out of range"))
    }

    /// Children of an aux data split get their parent's box (upstream copies it whether
    /// or not the node has a Bezier shape: the sub-domain boxes are not computed yet).
    fn copy_parent_boxes(
        &mut self,
        node1: NodeId,
        node2: NodeId,
        parent_bbox: crate::geom::extent3d::Extent3d,
    ) -> Result<()> {
        let tree = self.tree_mut_or(OP_NODE)?;
        tree.node_mut(node1)?.bbox = parent_bbox;
        tree.node_mut(node2)?.bbox = parent_bbox;
        Ok(())
    }

    /// `ImplantEdgeuse(pEU, FALSE, sTempExt, pNode)` into one child.
    fn implant_edgeuse_in(&mut self, edgeuse: O, node: NodeId) -> Result<()> {
        let _ = node;
        self.implant_edgeuse(edgeuse, false)
    }

    // ---- SubdivideToTolerances ------------------------------------------------------

    /// Splits `node` until every leaf below it meets the tessellation tolerances
    /// (`SubdivideToTolerances`): a node failing a test is split in one pass along the
    /// tessellation splits ([`SurfaceCache::subdivide_by_block`], when the turning
    /// angle allows it on a node with a subdivision surface) or in two
    /// ([`SurfaceCache::subdivide_node`]) and its children are subdivided in turn; a
    /// node's box becomes the union of its children's, and for approximated patches the
    /// pseudo box too. `depth` is the recursion depth (0 at the call).
    ///
    /// # Errors
    ///
    /// `InvalidInput` past [`MAX_SUBDIVISION_DEPTH`] levels or [`MAX_SUBDIVISION_NODES`]
    /// nodes; the errors of the tolerance test, the block subdivision and
    /// [`SurfaceCache::subdivide_node`].
    pub fn subdivide_to_tolerances(&mut self, k: &Kernel, node: NodeId, depth: u32) -> Result<()> {
        self.subdivide_to_tolerances_with(k, node, depth, &mut |c, k, n| {
            c.test_against_tolerances(k, n)
        })
    }

    /// [`SurfaceCache::subdivide_to_tolerances`] with `tester` as the virtual
    /// `TestAgainstTolerances` (a derived cache's override, e.g.
    /// [`crate::query::tess::TessSrfCache`]).
    ///
    /// # Errors
    ///
    /// As [`SurfaceCache::subdivide_to_tolerances`], with `tester`'s errors. Arithmetic
    /// validation belongs to the tester; no coordinate or tolerance cutoff is imposed
    /// on custom testers.
    pub fn subdivide_to_tolerances_with(
        &mut self,
        k: &Kernel,
        node: NodeId,
        depth: u32,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<()> {
        // performance only: the outermost call memoises the singularity test's pole
        // data for the splits below it (see the deviations of `surface_cache::p1`)
        let owns_memo = self.begin_pole_memo();
        let r = self.subdivide_to_tolerances_body(k, node, depth, tester);
        if owns_memo {
            self.end_pole_memo();
        }
        r
    }

    fn subdivide_to_tolerances_body(
        &mut self,
        k: &Kernel,
        node: NodeId,
        depth: u32,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<()> {
        if depth > MAX_SUBDIVISION_DEPTH {
            return Err(invalid(
                OP_TOL,
                "subdivision deeper than MAX_SUBDIVISION_DEPTH",
            ));
        }

        // test to see if subdivision is required and pick a split direction
        let test = tester(self, k, node)?;
        // no work - patch currently within tolerance
        if !test.needs_subdivision {
            return Ok(());
        }

        // switch between subdivision strategies
        let node_surface = self.surface_aux(node, OP_TOL)?.surface;
        if self.switch_to_fast_subdivision
            && self.tessellation_algorithm == super::p1::TessAlgorithmType::FasterTessellation
            && node_surface.is_some()
        {
            // tessellate this node completely in a single pass
            self.subdivide_by_block(k, node)?;
            self.switch_to_fast_subdivision = false;
            return Ok(());
        }

        // subdivide the node in two - at mid-point or existing knot near mid-point
        // (bounded: see MAX_SUBDIVISION_NODES)
        if self.tree_or(OP_TOL)?.nodes().len() >= MAX_SUBDIVISION_NODES {
            return Err(invalid(
                OP_TOL,
                "subdivision tree exceeds MAX_SUBDIVISION_NODES",
            ));
        }
        self.subdivide_node(k, node, test.direction, false)?;

        // recursion - keep subdividing until we satisfy tolerances
        let (c1, c2) = {
            let n = self.tree_or(OP_TOL)?.node(node)?;
            (
                n.child1
                    .ok_or_else(|| invalid(OP_TOL, "split node has no child 1"))?,
                n.child2
                    .ok_or_else(|| invalid(OP_TOL, "split node has no child 2"))?,
            )
        };
        self.subdivide_to_tolerances_with(k, c1, depth + 1, tester)?;
        self.subdivide_to_tolerances_with(k, c2, depth + 1, tester)?;

        // a minimal bounding box: union the children's boxes
        {
            let tree = self.tree_mut_or(OP_TOL)?;
            let b1 = tree.node(c1)?.bbox;
            let b2 = tree.node(c2)?.bbox;
            tree.node_mut(node)?.bbox = b1.union(&b2);
        }

        // when Bezier patch approximations are being made for non B-spline surfaces
        let aux_type = self.tree_or(OP_TOL)?.node(node)?.aux_type;
        if aux_type == AuxDataType::BezierSurface && bspline_of(self.surface()).is_none() {
            // parent pseudo box = union of the child pseudo boxes in the parent's basis
            let p1 = self.patch(c1)?.pseudo_box;
            let p2 = self.patch(c2)?.pseudo_box;
            let slot = self
                .surface_aux(node, OP_TOL)?
                .slot
                .ok_or_else(|| invalid(OP_TOL, "node has no data slot"))?;
            let parent = self.patch_mut_slot(slot)?;
            let basis = *parent.pseudo_box.basis();
            p1.union_into(&p2, &basis, &mut parent.pseudo_box);
        }

        Ok(())
    }

    // ---- BuildTreeBase / BuildTree / GetTree ---------------------------------------

    /// Starts the tree: one first generation node per non-zero span pair of a
    /// B-spline surface (its exact Bezier patches), else per span of a grid of up to
    /// 5 x 5 bicubic approximations; sets their boxes and continuities, then
    /// subdivides each to the tolerances (`BuildTreeBase`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a B-spline without NURB; the patch block builders' errors;
    /// [`SolidError::Unported`] for `SmSurface::CalculateContinuities`; the box and
    /// subdivision errors.
    pub fn build_tree_base(&mut self, k: &Kernel) -> Result<TreeBase> {
        self.build_tree_base_with(k, &mut |c, k, n| c.test_against_tolerances(k, n))
    }

    /// [`SurfaceCache::build_tree_base`] with `tester` as the virtual
    /// `TestAgainstTolerances`.
    ///
    /// # Errors
    ///
    /// As [`SurfaceCache::build_tree_base`].
    pub fn build_tree_base_with(
        &mut self,
        k: &Kernel,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<TreeBase> {
        // store an allocated and empty decomposition tree
        let mut tree: Tree<O> = Tree::new(Some(TreeOwner::SurfaceCache));
        tree.set_owner_surface(self.surface_handle());
        self.set_tree(Some(tree));

        // first, decompose the surface into starter Bezier patches
        // (an SmPlane or SmSurfOfRevolution is an SmBSplineSurface; its NURB is made
        // when it has none, as GetOrCreateGwNurbPointer)
        let bspline: Option<std::borrow::Cow<'_, BSplineSurface>> = match self.surface().kind() {
            SurfaceKind::BSpline(b) => Some(std::borrow::Cow::Borrowed(b)),
            SurfaceKind::Plane(p) => Some(p.base_with_nurb()?),
            // GetOrCreateGwNurbPointer: SmSurfOfRevolution::MakeNurb when it has none,
            // kept on the surface (`Surface::kind_with_nurb`, status ignored)
            SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfRevolution(_) => self
                .surface()
                .revolution_base_with_nurb(k)?
                .map(std::borrow::Cow::Borrowed),
            SurfaceKind::SurfOfExtrusion(s) => {
                if s.base().gw_nurb().is_some() {
                    Some(std::borrow::Cow::Borrowed(s.base()))
                } else {
                    let mut tmp = (**s).clone();
                    tmp.make_nurb(k)?;
                    Some(std::borrow::Cow::Owned(tmp.base().clone()))
                }
            }
            SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::OffsetSurface(_)
            | SurfaceKind::SrfInVolume(_) => None,
        };
        let (u_count, v_count, beziers) = if let Some(b) = bspline {
            let nurb = b
                .gw_nurb()
                .ok_or_else(|| invalid(OP_BASE, "B-spline surface without NURB"))?;
            let block = create_bezier_patch_block(nurb)?;
            (block.num_u_spans, block.num_v_spans, block.beziers)
        } else {
            let block = create_approximate_bezier_patch_block(k, self.surface())?;
            (block.num_u_spans, block.num_v_spans, block.beziers)
        };
        // the patch memory manager is initialised with the block
        *self.patches_mut() = beziers
            .into_iter()
            .map(|b| BezierPatch {
                bezier: Some(b),
                ..BezierPatch::default()
            })
            .collect();
        self.auxs_mut().clear();

        // set the continuities (continuity is in the opposite direction to the query)
        let u_conts = calculate_continuities(k, self.surface(), SurfParamType::U)?;
        let v_conts = calculate_continuities(k, self.surface(), SurfParamType::V)?;
        self.set_continuities(u_conts, v_conts);
        // the surface's cache now has its continuity arrays (its cache slot, shared
        // with the surface the cache was made from)
        self.surface().set_cache_tree_state(CacheTreeState::Built);
        self.build_tree_base_nodes(k, u_count, v_count, tester)
    }

    /// Sets the continuity arrays a cache of this surface already has when its tree
    /// was built before (the surface's cache slot is
    /// [`CacheTreeState::Built`](crate::surface::surface::CacheTreeState)) but this cache
    /// is made without building one (during a Boolean operation): upstream reuses the
    /// built cache, whose `BuildTreeBase` computed them from the same surface.
    ///
    /// # Errors
    ///
    /// Those of `SmSurface::CalculateContinuities` ([`SolidError::Unported`] for kinds
    /// without iso curves).
    pub fn restore_continuities(&mut self, k: &Kernel) -> Result<()> {
        let u_conts = calculate_continuities(k, self.surface(), SurfParamType::U)?;
        let v_conts = calculate_continuities(k, self.surface(), SurfParamType::V)?;
        self.set_continuities(u_conts, v_conts);
        Ok(())
    }

    /// The rest of [`SurfaceCache::build_tree_base`] once the continuities are set.
    fn build_tree_base_nodes(
        &mut self,
        k: &Kernel,
        u_count: usize,
        v_count: usize,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<TreeBase> {
        let mut nodes = Vec::with_capacity(u_count.saturating_mul(v_count));
        let u_conts = self.u_continuities().to_vec();
        let v_conts = self.v_continuities().to_vec();
        let cont = |c: &[ContinuityType], i: usize| -> Result<ContinuityType> {
            c.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_BASE, "continuity index out of range"))
        };

        // connect every subdivided Bezier patch to a tree node, set all patch data
        for ii in 0..u_count {
            for jj in 0..v_count {
                let slot = ii
                    .checked_mul(v_count)
                    .and_then(|i| i.checked_add(jj))
                    .ok_or_else(|| invalid(OP_BASE, "patch index overflow"))?;
                let node = self.tree_mut_or(OP_BASE)?.add_node(TreeNode {
                    parent: None,
                    child1: None,
                    child2: None,
                    bbox: crate::geom::extent3d::Extent3d::from_bounds(
                        0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                    ),
                    aux_type: AuxDataType::BezierSurface,
                    data: NodeData::Surface(SurfaceAux {
                        split_dir: SplitDir::U,
                        is_marked: false,
                        node_class: NodeClass::Unknown,
                        face: None,
                        surface: None,
                        slot: Some(slot),
                    }),
                    geom_type: NodeGeomType::Default,
                });
                nodes.push(node);

                // build a B-spline surface for the Bezier patch geometry
                let bez = self
                    .patch_mut_slot(slot)?
                    .bezier
                    .clone()
                    .ok_or_else(|| invalid(OP_BASE, "patch without its Bezier"))?;
                let bss = bss_of(&bez);
                let domain = bss.natural_uv_domain();

                // compute and store the node's bounding box and the patch's pseudo box
                let (bbox, pseudo) = patch_boxes(k, &bss, None, OP_BASE)?;
                self.tree_mut_or(OP_BASE)?.node_mut(node)?.bbox = bbox;

                let (vc0, uc0, vc1, uc1) = (
                    cont(&v_conts, jj)?,
                    cont(&u_conts, ii)?,
                    cont(&v_conts, jj + 1)?,
                    cont(&u_conts, ii + 1)?,
                );
                let p = self.patch_mut_slot(slot)?;
                p.pseudo_box = pseudo;
                // set patch continuity to its neighbour values
                p.v_curve_conts = [vc0, vc1];
                p.u_curve_conts = [uc0, uc1];
                // initialize all remaining SmBezierPatch data
                p.aux.owning_tree_node = Some(node);
                p.aux.chord_height_satisfied = false;
                p.aux.angle_tol_satisfied = false;
                p.aux.uv_domain = domain;
                p.aux.vertex_list.clear();
                p.aux.edgeuse_list.clear();
                p.aux.poly_edge_list.clear();
            }
        }

        // subdivide each first generation node until all its children pass the tolerances
        for &node in &nodes {
            self.subdivide_to_tolerances_with(k, node, 0, tester)?;
        }

        Ok(TreeBase {
            nodes,
            u_count,
            v_count,
        })
    }

    /// Builds the subdivision tree: the first generation and its subdivision
    /// ([`SurfaceCache::build_tree_base`]) under parent nodes up to one top node
    /// ([`SurfaceCache::build_tree_top`]); unless `for_derived_cache`, the construction
    /// data is then dropped and every node refers to the cache's surface (`BuildTree`).
    /// Point testing is off and the face containment is marked under construction
    /// while it runs.
    ///
    /// # Errors
    ///
    /// The errors of [`SurfaceCache::build_tree_base`] and
    /// [`SurfaceCache::build_tree_top`].
    pub fn build_tree(&mut self, k: &Kernel, for_derived_cache: bool) -> Result<()> {
        self.build_tree_with(k, for_derived_cache, &mut |c, k, n| {
            c.test_against_tolerances(k, n)
        })
    }

    /// [`SurfaceCache::build_tree`] with `tester` as the virtual
    /// `TestAgainstTolerances`.
    ///
    /// # Errors
    ///
    /// As [`SurfaceCache::build_tree`].
    pub fn build_tree_with(
        &mut self,
        k: &Kernel,
        for_derived_cache: bool,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<()> {
        // to prevent bad recursion back into BuildTree - turn off point testing
        let saved_point_test = self.point_test_enabled;
        let saved_containment = self.face_containment_done;
        self.point_test_enabled = false;
        self.face_containment_done = FaceContainment::UnderConstruction;
        let r = self.build_tree_inner(k, for_derived_cache, tester);
        self.point_test_enabled = saved_point_test;
        self.face_containment_done = saved_containment;
        r
    }

    fn build_tree_inner(
        &mut self,
        k: &Kernel,
        for_derived_cache: bool,
        tester: &mut ToleranceTester<'_, O>,
    ) -> Result<()> {
        let base = self.build_tree_base_with(k, tester)?;

        // add parent nodes over the first generation
        self.build_tree_top(&base.nodes, base.u_count, base.v_count)?;

        // clean up the construction surfaces and Beziers
        if !for_derived_cache {
            self.subdivision_surfaces_mut().clear();
            let handle = self.surface_handle();
            for p in self.patches_mut() {
                p.bezier = None;
            }
            let tree = self.tree_mut_or(OP_BASE)?;
            for id in 0..tree.nodes().len() {
                if let NodeData::Surface(a) = &mut tree.node_mut(NodeId(id))?.data {
                    a.surface = handle;
                }
            }
        }
        Ok(())
    }

    /// The tree, built first when there is none, or when the surface belongs to a face
    /// whose boundary is not in the tree yet and no build is under way (`GetTree`).
    ///
    /// # Errors
    ///
    /// The errors of [`SurfaceCache::build_tree`] (upstream returns NULL).
    pub fn get_tree(&mut self, k: &Kernel) -> Result<&mut Tree<O>> {
        let face_owned = self.surface().face()?.is_some();
        if self.tree().is_none()
            || (!self.have_t_surface_cache
                && face_owned
                && self.face_containment_done != FaceContainment::UnderConstruction)
        {
            if self.trimmed_dispatch {
                if self.surface().face()?.is_some() {
                    return Err(SolidError::unported(
                        "TrimSrfCache::build_tree_with_face for a face-owned surface",
                        "surface.trim_srf_cache",
                    ));
                }
                // The standalone trimmed override has no topology to implant.
                // Preserve its construction guard and state rather than quietly
                // treating a lazy trimmed cache as a base cache.
                let saved = self.face_containment_done;
                self.face_containment_done = FaceContainment::UnderConstruction;
                let built = self.build_tree(k, false);
                self.face_containment_done = saved;
                built?;
                self.process_boundary_curves = true;
                self.point_test_enabled = false;
                self.have_t_surface_cache = false;
            } else {
                self.build_tree(k, false)?;
            }
        }
        self.tree_mut_or(OP_GET)
    }
}

/// `CalculateBoundingBox(domain, &box, NULL, NULL, NULL, NULL)` of a split half (the
/// B-spline override, boxes expanded by the zone tolerance); the base body's `SM_ERR`
/// for other kinds.
fn half_box(
    k: &Kernel,
    half: &Surface,
    domain: &Extent2d,
) -> Result<crate::geom::extent3d::Extent3d> {
    match half.kind() {
        SurfaceKind::BSpline(b) => b
            .calculate_bounding_box(
                k,
                domain,
                &BoundingBoxRequest {
                    normal: true,
                    expand_by_zone_tol: true,
                    ..BoundingBoxRequest::default()
                },
            )?
            .normal
            .ok_or_else(|| invalid(OP_NODE, "no bounding box")),
        _ => Err(SolidError::sm_err(OP_NODE)),
    }
}

/// `SmSurface::CalculateContinuities(eSurfParam, ..)`: the continuity at every unique
/// knot of the surface in `dir`.
fn calculate_continuities(
    k: &Kernel,
    surface: &Surface,
    dir: SurfParamType,
) -> Result<Vec<ContinuityType>> {
    Ok(surface.calculate_continuities(k, dir)?.1)
}

/// `sm_InitNurbSurfaceMemory` with the sizes of `like` on zero-filled block memory.
fn zeroed_like(like: &NurbSurface) -> Result<NurbSurface> {
    let (p, q) = like.degrees();
    let (knu, knv) = like.knot_vectors()?;
    crate::nurbs::nurbs_srf::allocate_nurb_surface(
        like.n(),
        like.m(),
        p,
        q,
        knu.last_index(),
        knv.last_index(),
    )
}

/// Ignores the status of a call, except [`SolidError::Unported`].
fn ignore<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::curve_bounded_surface::CurveBoundedSurface;
    use crate::surface::surface::p1::{
        cache_tolerances, make_surface_cache, make_trim_srf_cache, StepSurface,
    };
    use crate::surface::surface_cache::p1::SurfaceCacheOptions;

    /// The exact surface of `surface.surface_point_solve_p3`'s `fuzz00349_rust_hang`
    /// corpus input (`crates/difftest/tests/fuzz_corpus/surface.surface_point_solve_p3.json`),
    /// built without the difftest/FFI coordinate validators.
    fn corpus_surface() -> Surface {
        let net = vec![
            vec![
                CPoint::new(
                    0.000_282_115_039_950_964_9,
                    0.0,
                    0.0,
                    0.940_383_466_503_216_2,
                ),
                CPoint::new(
                    0.000_596_645_716_953_504_6,
                    0.000_198_881_905_651_168_2,
                    0.0,
                    1.988_819_056_511_682,
                ),
                CPoint::new(
                    0.000_413_727_792_694_957_6,
                    1e300,
                    0.0,
                    1.379_092_642_316_525_3,
                ),
                CPoint::new(
                    0.000_544_244_248_406_755,
                    0.000_544_244_248_406_755,
                    0.0,
                    1.814_147_494_689_183_1,
                ),
            ],
            vec![
                CPoint::new(
                    -0.000_254_573_732_578_935_63,
                    0.0,
                    0.000_190_172_254_527_736_9,
                    1.059_209_724_859_549_1,
                ),
                CPoint::new(
                    -0.000_222_818_281_601_881_3,
                    9.270_842_217_628_493e-5,
                    0.000_166_450_224_589_008_12,
                    0.927_084_221_762_849_2,
                ),
                CPoint::new(
                    -0.000_158_640_632_839_373_26,
                    0.000_132_011_813_912_682_52,
                    0.000_118_508_089_979_063_46,
                    0.660_059_069_563_412_6,
                ),
                CPoint::new(
                    -0.000_345_623_933_991_531_4,
                    0.000_431_413_203_930_455,
                    0.000_258_188_785_151_016_86,
                    1.438_044_013_101_516_6,
                ),
            ],
        ];
        let nurb = NurbSurface::new(
            net,
            1,
            3,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![-0.767_456_479_409_241_3; 4]
                .into_iter()
                .chain(vec![2.150_404_835_288_109_8; 4])
                .collect(),
        )
        .expect("corpus surface");
        Surface::from(
            CurveBoundedSurface::new(
                Surface::from(BSplineSurface::from_nurb(nurb, false)),
                Vec::new(),
                Vec::new(),
                true,
            )
            .expect("curve-bounded surface"),
        )
    }

    /// The corpus input wraps its surface in an `SmSTEPSurface` (`"wrap": true`): the
    /// approximation branch whose cache build hung.
    fn corpus_step_surface() -> Surface {
        Surface::from(StepSurface::new(corpus_surface(), true).expect("step surface"))
    }

    /// A cubic B-spline surface of ordinary shape, `scale` wide.
    fn scaled_surface(scale: f64) -> Surface {
        let net = (0..4)
            .map(|i| {
                (0..4)
                    .map(|j| {
                        CPoint::new(
                            f64::from(i) * scale,
                            f64::from(j) * scale,
                            f64::from(i * j) * 0.3 * scale,
                            1.0,
                        )
                    })
                    .collect()
            })
            .collect();
        let nurb = NurbSurface::new(
            net,
            3,
            3,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("scaled surface");
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    fn assert_angle_arithmetic_error(error: &SolidError) {
        assert!(
            matches!(
                error,
                SolidError::InvalidInput { operation, reason }
                    if *operation == "surface::surface_cache::p2::SurfaceCache::test_against_tolerances"
                        && *reason == "non-finite approximation angle segment length"
            ),
            "expected the approximation angle arithmetic error, got {error:?}"
        );
    }

    /// The exact corpus surface overflows sampled segment lengths in the angle
    /// test. That arithmetic error now propagates from both cache constructors,
    /// rather than driving a large approximation tree with zero "unit" directions.
    #[test]
    fn unmeasurable_corpus_surface_is_refused() {
        let k = Kernel::default();
        let surface = corpus_step_surface();

        for error in [
            make_trim_srf_cache::<i64>(&k, &surface, Some(0)).err(),
            make_surface_cache::<i64>(&k, &surface, Some(0)).err(),
        ] {
            assert_angle_arithmetic_error(
                &error.expect("invalid angle arithmetic must be refused"),
            );
        }
    }

    /// A separate synthetic rational net whose homogeneous coordinates normalize
    /// to ordinary geometry. This is NOT a substitute for the original HANG-20
    /// boundary fixtures, which include NOW sentinel and ordinary positive weights.
    #[test]
    fn huge_homogeneous_control_points_with_small_geometry_build() {
        let k = Kernel::default();
        let net = vec![
            vec![
                CPoint::new(-57.74e306, -4.79e306, -20.02e306, 1e306),
                CPoint::new(-9e306, 3.69e306, -2.39e306, 1e306),
            ],
            vec![
                CPoint::new(3.65e306, -7.33e306, -5.68e306, 1e306),
                CPoint::new(-9e306, 4.03e306, -8.00e305, 1e306),
            ],
            vec![
                CPoint::new(6.12e306, -7.88e305, 2.96e306, 1e306),
                CPoint::new(6.83e306, 4.25e306, 1.05e306, 1e306),
            ],
        ];
        let nurb = NurbSurface::new(
            net,
            2,
            1,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .expect("boundary surface");
        let surface = Surface::from(BSplineSurface::from_nurb(nurb, false));

        let cache = make_trim_srf_cache::<i64>(&k, &surface, Some(0))
            .expect("a normalizing huge control net must still build");
        let nodes = cache.base().tree().map_or(0, |t| t.nodes().len());
        assert!(nodes > 0);
    }

    fn translated_plane(x: f64) -> Surface {
        let net = [0.0, 1.0]
            .into_iter()
            .map(|y| {
                [0.0, 1.0]
                    .into_iter()
                    .map(|z| CPoint::new(x, y, z, NOW))
                    .collect()
            })
            .collect();
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let nurb = NurbSurface::new(net, 1, 1, knots.clone(), knots).unwrap();
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    #[test]
    fn translated_measurable_geometry_keeps_builtin_subdivision() {
        let k = Kernel::default();
        let mut counts = Vec::new();
        for x in [0.0, 1e155] {
            let mut cache: SurfaceCache<i64> = SurfaceCache::new(
                translated_plane(x),
                Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                SurfaceCacheOptions {
                    max_side_length_3d: 0.75,
                    ..SurfaceCacheOptions::default()
                },
            );
            cache
                .build_tree_base(&k)
                .expect("translated unit sides are measurable");
            counts.push(cache.tree().unwrap().nodes().len());
        }
        assert!(counts[0] > 1, "control must exercise subdivision");
        assert_eq!(counts[0], counts[1]);
    }

    #[test]
    fn custom_tester_can_split_translated_nodes_below_root() {
        let k = Kernel::default();
        let mut cache: SurfaceCache<i64> = SurfaceCache::new(
            translated_plane(1e155),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            SurfaceCacheOptions {
                chord_height_tolerance: f64::INFINITY,
                ..SurfaceCacheOptions::default()
            },
        );
        let mut calls = 0;
        cache
            .build_tree_base_with(&k, &mut |c, _, n| {
                calls += 1;
                Ok(super::super::p2::ToleranceTest {
                    needs_subdivision: c.aux(n)?.uv_domain.size().x > 0.3,
                    direction: SurfParamType::U,
                })
            })
            .expect("custom tester owns its measurement policy");
        assert_eq!(calls, 7);
        assert_eq!(cache.tree().unwrap().nodes().len(), 7);
    }

    /// Supported sizes keep their behaviour: an ordinary surface and one a hundred
    /// million units wide both build the same 31-node tree.
    #[test]
    fn supported_sizes_build() {
        let k = Kernel::default();
        for scale in [1.0, 1e8] {
            let surface =
                Surface::from(StepSurface::new(scaled_surface(scale), true).expect("step"));
            let tol = cache_tolerances(&k, &surface).expect("cache tolerances");
            assert!(tol.chord_height_tol.is_finite(), "scale {scale}: {tol:?}");
            let cache = make_trim_srf_cache::<i64>(&k, &surface, Some(0))
                .expect("a supported size must build");
            assert_eq!(
                cache.base().tree().map_or(0, |t| t.nodes().len()),
                31,
                "scale {scale} node count changed"
            );
        }
    }

    /// A large finite tolerance alone is not an arithmetic failure.
    #[test]
    fn finite_tolerance_is_not_refused() {
        let k = Kernel::default();
        let opts = SurfaceCacheOptions {
            chord_height_tolerance: f64::MAX,
            ang_tol_rad: 0.0,
            ..SurfaceCacheOptions::default()
        };
        let mut cache: SurfaceCache<i64> = SurfaceCache::new(
            scaled_surface(1.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            opts,
        );
        cache
            .build_tree_base(&k)
            .expect("a finite tolerance must still build");
    }
}

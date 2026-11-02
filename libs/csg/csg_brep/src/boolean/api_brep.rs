//! High-level boolean entry points (`SM_API`): two-B-rep booleans (union, difference,
//! intersection, merge, with or without merge options, non-manifold, with the
//! intersection edges, piecewise), planar 2D booleans, N-ary booleans over a list of
//! B-reps, booleans between lists of B-reps or surfaces, and CSG trees in postfix
//! notation.
//!
//! Upstream: `source/SM_API/src/SmApiBrep.cpp`, `source/SM_API/inc/SmApiBrep.h`.
//!
//! | Rust                                          | Upstream                                     |
//! |-----------------------------------------------|----------------------------------------------|
//! | `merge_breps_inputs_are_valid` (private)      | `SmApiMergeBrepsInputsAreValid` (static)     |
//! | `merge_breps_kernel_operation` (private)      | `SmApiMergeBrepsKernelOperation` (static)    |
//! | `validate_boolean_operands` (private)         | `SmApiValidateBooleanOperands` (static)      |
//! | `merge_boolean` (private)                     | the shared body of the two-B-rep entry points |
//! | [`boolean`], [`BooleanOutcome`]               | `SmApiBoolean`                               |
//! | [`boolean_union`]                             | `SmApiBooleanUnion`                          |
//! | [`boolean_difference`]                        | `SmApiBooleanDifference`                     |
//! | [`boolean_intersection`]                      | `SmApiBooleanIntersection`                   |
//! | [`boolean_merge`]                             | `SmApiBooleanMerge`                          |
//! | [`merge_breps`]                               | `SmApiMergeBreps`                            |
//! | [`boolean_with_options`]                      | `SmApiBooleanWithOptions`                    |
//! | [`boolean_with_curves`], [`BooleanWithCurves`] | `SmApiBooleanWithCurves`                    |
//! | [`boolean_2d`], [`Boolean2dOutcome`]          | `SmApiBoolean2d`                             |
//! | [`non_manifold_boolean`]                      | `SmApiNonManifoldBoolean`                    |
//! | [`piecewise_merge`], [`PiecewiseMergeOutcome`] | `SmApiPiecewiseMerge`                       |
//! | [`boolean_lists`], [`BooleanListsResult`]     | `SmApiBooleanLists`                          |
//! | [`boolean_trees`]                             | `SmApiBooleanTrees`                          |
//! | [`tessellate_boundaries`]                     | `SmApiTessellateBoundaries`                  |
//!
//! Deviations:
//! - **Ownership.** Upstream passes `SmBrep*` operands that the kernel modifies in
//!   place (the first becomes `rpResult`) or deletes (the second). The port takes the
//!   operands by value and returns the result; a second operand upstream keeps
//!   (`SM_BO_PARTIAL_MERGE`, `SM_BO_IMPRINT_CLASSIFY`, `bKeepOtherBrep`) is handed
//!   back in [`BooleanOutcome::other`]. A `NULL` operand, the same B-rep passed twice
//!   (`SmApiValidateBooleanOperands`) and a `NULL` or duplicate list entry
//!   (`SmApiMergeBrepsInputsAreValid`) cannot be expressed with owned values, so those
//!   checks reduce to the remaining ones. Operands rejected by a two-B-rep entry point
//!   are dropped (upstream leaves them with the caller); [`merge_breps`] takes the list
//!   `&mut` and leaves it untouched on rejection, as upstream. [`boolean_2d`] hands both
//!   operands back when upstream leaves them unassigned (an operand without faces);
//!   [`piecewise_merge`] always hands the second operand back (upstream imprints but
//!   does not delete it).
//! - **Intersection edges.** [`boolean_with_curves`] returns `SmMerge::m_vEdges` (the
//!   common edges `ManifoldBoolean` collected on the first B-rep, which
//!   `NonManifoldBoolean` also runs) as ids into the result, in that order. Upstream
//!   copies the non-null pointers without checking that the edges survived the rest of
//!   the merge; an id may likewise name an edge the result no longer has.
//! - **Out-params.** `rpResult`, `rResultBreps` / `rResultSurfaces` are return values;
//!   on failure there is none (upstream clears `rpResult`).
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`], taken by
//!   every entry point (the merge engine reads its tolerances).
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; `SM_ERR`
//!   is [`SolidError::Upstream`] with [`crate::error::SM_ERR`].
//! - **2D booleans.** [`boolean_2d`] runs `SmPrimitiveCreation::Boolean2D` (static
//!   upstream) on a [`PrimitiveCreation`] without primitive B-rep, as `merge::p3` does
//!   for planar list operands, with the operands' marks and an arena-only notification
//!   store ([`ArenaNotifyState`]).
//! - **Piecewise merge.** [`piecewise_merge`] builds `SmMerge(*pContext, pBrep1,
//!   pBrep2)` as a merge session of a [`PrimitiveModel`] over both operands and runs its
//!   `PiecewiseMerge(NULL, FALSE, bCreateNewBrepsForFaces, rpResult)`.
//! - **Dependencies.** List folding and tree evaluation call `merge::p3`; its
//!   construction and topology calls retain narrowly scoped dependency errors. The
//!   two-B-rep entry points build the merge (`boolean.merge_p2`, through
//!   `merge::p3`'s `DependencyMerge`, which keeps it as the `boolean.merge_p4` engine),
//!   then call `SmBrep::IsManifoldSolid` (`topology.brep_p2`) and `ManifoldBoolean`
//!   (`boolean.merge_p4`) or `NonManifoldBoolean` (`boolean.merge_p3`).
//! - **List context.** `boolean_lists` takes a mutable kernel for the connected
//!   component traversal's mark locks.
//! - **Nullable tree output.** Tree operands are `Option<Brep>` slots, preserving
//!   consumed entries on failure and defined null outputs of repeated leaves.
//! - **Attributes.** Upstream's attributes live on the operands' objects, which the merge
//!   edits in place. Here each [`Brep`] owns its attributes ([`Brep::add_attribute`]):
//!   the two-B-rep entry points move both operands' attributes into the merge's store
//!   under the merge's names of their objects, the merge's notifications propagate them
//!   with SMLib's handlers, and the attributes left on the result's (and a kept other
//!   B-rep's) objects go back to those B-reps; those of objects the merge deleted are
//!   cleared as upstream's destructors do. A reference attribute on objects of both
//!   returned B-reps becomes one attribute per B-rep. [`merge_breps`] and
//!   [`boolean_trees`] do the same for every merge they run. [`boolean_lists`] carries
//!   each list B-rep's attributes into its operand copies (`SM_NO_COPY`), through the
//!   merge and to every component B-rep it returns; a reference attribute shared by
//!   several output B-reps (or with the inputs, which keep theirs) becomes one per
//!   B-rep. `SM_BO_EXTRACT_SEPARATE`'s component B-reps are not returned by any entry
//!   point here (it is rejected), so their attributes stay in the merge's store.
//! - The debug drawing (`SM_DEBUG_CODE`) and `SM_ASSERT_VALID` checks are not ported.
//! - **`SmApiTessellateBoundaries`.** The B-rep is a reference (no `NULL` check); its
//!   out-arrays are the returned [`BoundaryTessellation`], whose `complete` flag is
//!   upstream's `SM_ERR` with partial output. An invalid angle tolerance is
//!   [`SolidError::InvalidInput`] (upstream clears the arrays).
//! - **Cached topology.** Every returned B-rep has the port's own cached fields
//!   ([`crate::topology::Edge::vertices`], `Face::shell`, ..., which topology edits
//!   inside the merge do not maintain) rewritten from its use links by
//!   [`Brep::refresh_cached_topology`]; nothing else changes.

use crate::boolean::merge::MergeOptions;
use crate::error::{Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::kernel::Kernel;
use crate::primitive::primitive_creation::model::PrimitiveModel;
use crate::primitive::primitive_creation::p1::PrimitiveCreation;
use crate::surface::surface::Surface;
use crate::topology::brep::p1::{self as brep_p1, BoundaryTessellation};
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::{Brep, EdgeId};

/// The planar 2D boolean operations of [`boolean_2d`] (`Sm2DBooleanOperationType`).
pub use crate::primitive::primitive_creation::p1::Boolean2d;

const OP_BOOLEAN: &str = "boolean::api_brep::boolean";
const OP_MERGE_BREPS: &str = "boolean::api_brep::merge_breps";
const OP_PIECEWISE: &str = "boolean::api_brep::piecewise_merge";

/// The outcome of a two-B-rep boolean ([`boolean`], [`boolean_with_options`]).
#[derive(Debug, Clone)]
pub struct BooleanOutcome {
    /// `rpResult`: the first operand, modified in place by the merge.
    pub result: Brep,
    /// The second operand when the operation keeps it (`SM_BO_PARTIAL_MERGE`,
    /// `SM_BO_IMPRINT_CLASSIFY`, or [`MergeOptions::keep_other_brep`]); `None` when
    /// upstream deletes it.
    pub other: Option<Brep>,
}

/// The outcome of [`boolean_with_curves`].
#[derive(Debug, Clone)]
pub struct BooleanWithCurves {
    /// The result and the second operand when kept, as [`boolean`]'s.
    pub outcome: BooleanOutcome,
    /// `rIntersectionEdges`: the merge's common edges on the first B-rep
    /// (`SmMerge::m_vEdges`), ids into [`BooleanOutcome::result`].
    pub intersection_edges: Vec<EdgeId>,
}

/// The outcome of a successful [`boolean_2d`].
#[derive(Debug, Clone)]
pub enum Boolean2dOutcome {
    /// `rpResult`: the first operand, modified; the second was consumed.
    Result(Brep),
    /// An operand has no faces: upstream succeeds without assigning `rpResult` or
    /// deleting the second operand, so both come back unchanged.
    Unassigned {
        /// The first operand.
        brep1: Brep,
        /// The second operand (boxed: keeps the variants of similar size).
        brep2: Box<Brep>,
    },
}

/// The outcome of [`piecewise_merge`].
#[derive(Debug, Clone)]
pub struct PiecewiseMergeOutcome {
    /// `rpResult`: the first operand, modified.
    pub result: Brep,
    /// The second operand, imprinted but not deleted.
    pub other: Brep,
}

/// The outputs of [`boolean_lists`].
pub use crate::boolean::merge::p3::BooleanListsResult;

/// Validates a public N-ary boolean input list without consuming it
/// (`SmApiMergeBrepsInputsAreValid`): at least two B-reps. Upstream also rejects a
/// `NULL` or repeated entry, which owned values cannot be.
fn merge_breps_inputs_are_valid(breps: &[Brep]) -> bool {
    breps.len() >= 2
}

/// Maps the public boolean operation to `SmMerge::merge_breps`' legacy zero-based
/// operation selector (`SmApiMergeBrepsKernelOperation`): union 0, intersection 1,
/// difference 2, merge 3; `None` for every other operation.
fn merge_breps_kernel_operation(operation: BooleanOperation) -> Option<u32> {
    match operation {
        BooleanOperation::Union => Some(0),
        BooleanOperation::Intersection => Some(1),
        BooleanOperation::Difference => Some(2),
        BooleanOperation::Merge => Some(3),
        _ => None,
    }
}

/// Validates the operands of a two-B-rep boolean before the kernel modifies or
/// deletes either of them (`SmApiValidateBooleanOperands`). `SM_BO_EXTRACT_SEPARATE`
/// has no single result (upstream's kernel deletes both inputs and leaves the pieces
/// in `SmMerge` members that nothing frees). Upstream also rejects a `NULL` operand
/// and the same B-rep twice, which owned values cannot be.
fn validate_boolean_operands(operation: BooleanOperation) -> Result<()> {
    if operation == BooleanOperation::ExtractSeparate {
        return Err(SolidError::invalid_input(
            OP_BOOLEAN,
            "SM_BO_EXTRACT_SEPARATE has no single result",
        ));
    }
    Ok(())
}

/// Which `SmMerge` method a two-B-rep entry point runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeMethod {
    /// `ManifoldBoolean` when both operands are manifold solids
    /// (`SmBrep::IsManifoldSolid`), else `NonManifoldBoolean`.
    ByOperands,
    /// `NonManifoldBoolean` (`SmApiNonManifoldBoolean`).
    NonManifold,
}

/// The body shared by the two-B-rep entry points after validation: build the merge
/// (`SmMerge sMerge(*pContext, pBrep1, pBrep2)`, or with `0, 0, &sOptions` when
/// `options` is given), then run `ManifoldBoolean` when both operands are manifold
/// solids (`SmBrep::IsManifoldSolid`) and `NonManifoldBoolean` otherwise.
fn merge_boolean(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
    options: Option<MergeOptions>,
) -> Result<BooleanOutcome> {
    merge_boolean_by(k, brep1, brep2, operation, options, MergeMethod::ByOperands)
        .map(|(outcome, _)| outcome)
}

/// [`merge_boolean`] running `method`, with the merge's common edges on the first
/// B-rep (`SmMerge::m_vEdges`) as it left them.
fn merge_boolean_by(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
    options: Option<MergeOptions>,
    method: MergeMethod,
) -> Result<(BooleanOutcome, Vec<EdgeId>)> {
    // SmMerge sMerge(*pContext, pBrep1, pBrep2) (or 0, 0, &sOptions): tolerance and
    // angle 0, both computed by the constructor.
    use crate::boolean::merge::p3::BooleanDriver;
    // (not upstream) the driver carries the operands' attributes through the merge, as
    // upstream's live on the objects the merge edits
    let mut driver = crate::boolean::merge::p3::DependencyMerge::carrying_attributes();
    let mut primary = Some(brep1);
    let mut other = Some(brep2);
    driver.construct(k, &mut primary, &mut other, 0.0, 0.0, options.as_ref())?;
    let manifold_solids = match (&primary, &other) {
        (Some(_), Some(_)) if method == MergeMethod::NonManifold => false,
        (Some(a), Some(b)) => is_manifold_solid(a)? && is_manifold_solid(b)?,
        _ => return Err(SolidError::invalid_input(OP_BOOLEAN, "null operand")),
    };
    // m_bManifoldBoolean and m_vTI's lamina-edge control of the constructed merge
    let mut manifold = true;
    let mut lamina_edges = 0;
    let result = if manifold_solids {
        driver.manifold_boolean(
            k,
            &mut primary,
            &mut other,
            &mut manifold,
            &mut lamina_edges,
            operation,
            None,
        )?
    } else {
        crate::boolean::merge::p3::non_manifold_boolean(
            k,
            &mut driver,
            &mut primary,
            &mut other,
            &mut manifold,
            &mut lamina_edges,
            operation,
        )?
    };
    let mut result = result.ok_or_else(|| SolidError::null_pointer(OP_BOOLEAN))?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    result.refresh_cached_topology();
    if let Some(o) = other.as_mut() {
        o.refresh_cached_topology();
    }
    let edges = driver
        .engine()
        .map(|engine| engine.state.edges.clone())
        .unwrap_or_default();
    Ok((BooleanOutcome { result, other }, edges))
}

fn is_manifold_solid(brep: &Brep) -> Result<bool> {
    crate::topology::brep::p2::is_manifold_solid(brep)
}

/// Booleans two B-reps (`SmApiBoolean`): `brep1` is modified into the result; `brep2`
/// is consumed (returned in [`BooleanOutcome::other`] for the operations upstream keeps
/// it for).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `SM_BO_EXTRACT_SEPARATE`; otherwise the merge's
/// error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
) -> Result<BooleanOutcome> {
    validate_boolean_operands(operation)?;
    merge_boolean(k, brep1, brep2, operation, None)
}

/// Unites two B-reps (`SmApiBooleanUnion`); `brep2` is consumed.
///
/// # Errors
///
/// The merge's error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean_union(k: &mut Kernel, brep1: Brep, brep2: Brep) -> Result<Brep> {
    validate_boolean_operands(BooleanOperation::Union)?;
    merge_boolean(k, brep1, brep2, BooleanOperation::Union, None).map(|o| o.result)
}

/// Subtracts `brep2` from `brep1` (`SmApiBooleanDifference`); `brep2` is consumed.
///
/// # Errors
///
/// The merge's error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean_difference(k: &mut Kernel, brep1: Brep, brep2: Brep) -> Result<Brep> {
    validate_boolean_operands(BooleanOperation::Difference)?;
    merge_boolean(k, brep1, brep2, BooleanOperation::Difference, None).map(|o| o.result)
}

/// Intersects two B-reps (`SmApiBooleanIntersection`); `brep2` is consumed.
///
/// # Errors
///
/// The merge's error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean_intersection(k: &mut Kernel, brep1: Brep, brep2: Brep) -> Result<Brep> {
    validate_boolean_operands(BooleanOperation::Intersection)?;
    merge_boolean(k, brep1, brep2, BooleanOperation::Intersection, None).map(|o| o.result)
}

/// Merges `brep2` into `brep1` (`SmApiBooleanMerge`); `brep2` is consumed.
///
/// # Errors
///
/// The merge's error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean_merge(k: &mut Kernel, brep1: Brep, brep2: Brep) -> Result<Brep> {
    validate_boolean_operands(BooleanOperation::Merge)?;
    merge_boolean(k, brep1, brep2, BooleanOperation::Merge, None).map(|o| o.result)
}

/// Applies one boolean operation across a list of at least two B-reps
/// (`SmApiMergeBreps`). The list is validated first and left untouched on rejection;
/// once validation passes the B-reps are consumed (the list is drained), as upstream.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for fewer than two B-reps or an operation other than
/// union, intersection, difference or merge; otherwise the merge's error
/// (including [`SolidError::Unported`] for missing merge-engine dependencies).
#[allow(clippy::ptr_arg)] // the list is drained on success, as upstream consumes it
pub fn merge_breps(
    k: &mut Kernel,
    breps: &mut Vec<Brep>,
    operation: BooleanOperation,
) -> Result<Brep> {
    // The output is valid only on success. Reject every unsafe list shape before the
    // kernel has a chance to consume it.
    let kernel_operation = match merge_breps_kernel_operation(operation) {
        Some(op) if merge_breps_inputs_are_valid(breps) => op,
        _ => {
            return Err(SolidError::invalid_input(
                OP_MERGE_BREPS,
                "fewer than two B-reps or an operation other than union, intersection, difference or merge",
            ))
        }
    };
    // `SmMerge::merge_breps(rBreps, lKernelOperation, rpResult)`, then `SM_ERR` when it
    // succeeds without a result.
    let mut result = crate::boolean::merge::p3::merge_breps(
        k,
        breps,
        kernel_operation,
        None,
        &mut crate::boolean::merge::p3::DependencyMerge::carrying_attributes(),
    )?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    result.refresh_cached_topology();
    Ok(result)
}

/// Booleans two B-reps with full merge-option control (`SmApiBooleanWithOptions`):
/// cookie cutter, imprinting, post-processing, imprint-and-classify and keeping the
/// second operand.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `SM_BO_EXTRACT_SEPARATE`; otherwise the merge's
/// error ([`SolidError::Unported`] until `SmMerge` is ported).
pub fn boolean_with_options(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
    options: MergeOptions,
) -> Result<BooleanOutcome> {
    validate_boolean_operands(operation)?;
    merge_boolean(k, brep1, brep2, operation, Some(options))
}

/// Booleans two B-reps and returns the intersection edges on the result
/// (`SmApiBooleanWithCurves`): as [`boolean`], plus `SmMerge::m_vEdges`, the common
/// edges the merge found on the first B-rep, in its order.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `SM_BO_EXTRACT_SEPARATE`; otherwise the merge's
/// error.
pub fn boolean_with_curves(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
) -> Result<BooleanWithCurves> {
    validate_boolean_operands(operation)?;
    let (outcome, edges) =
        merge_boolean_by(k, brep1, brep2, operation, None, MergeMethod::ByOperands)?;
    Ok(BooleanWithCurves {
        outcome,
        intersection_edges: edges,
    })
}

/// Booleans two B-reps with `NonManifoldBoolean` whatever their kind
/// (`SmApiNonManifoldBoolean`): `brep1` is modified into the result; `brep2` is
/// consumed (returned in [`BooleanOutcome::other`] for the operations upstream keeps it
/// for).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for `SM_BO_EXTRACT_SEPARATE`; otherwise the merge's
/// error.
pub fn non_manifold_boolean(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: BooleanOperation,
) -> Result<BooleanOutcome> {
    validate_boolean_operands(operation)?;
    merge_boolean_by(k, brep1, brep2, operation, None, MergeMethod::NonManifold)
        .map(|(outcome, _)| outcome)
}

/// Merges each face of `brep2` into `brep1` in turn (`SmApiPiecewiseMerge`,
/// `SmMerge::PiecewiseMerge(NULL, FALSE, bCreateNewBrepsForFaces, rpResult)`), each
/// through a stand-alone sheet B-rep when `create_new_breps_for_faces`. `brep2` is
/// imprinted, not deleted.
///
/// # Errors
///
/// The merge's error; `SM_ERR` if the merge's result were not `brep1` (upstream's
/// `PiecewiseMerge` always returns its first operand, so this does not happen).
pub fn piecewise_merge(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    create_new_breps_for_faces: bool,
) -> Result<PiecewiseMergeOutcome> {
    use crate::sweep::topology_sweep::access::SweepAccess;
    validate_boolean_operands(BooleanOperation::PartialMerge)?;
    let mut model = PrimitiveModel::new(k, brep1);
    let other_id = model.add_brep(brep2)?;
    // SmMerge sMerge(*pContext, pBrep1, pBrep2): tolerance and angle 0, both computed
    let merge = model.new_merge(
        PrimitiveModel::MAIN,
        PrimitiveModel::MAIN,
        other_id,
        0.0,
        0.0,
    )?;
    let merged = model.piecewise_merge(merge, false, create_new_breps_for_faces);
    model.delete_merge(merge);
    let result_id = merged?;
    if result_id != PrimitiveModel::MAIN {
        // PiecewiseMerge's result is its first operand
        return Err(SolidError::sm_err(OP_PIECEWISE));
    }
    let mut other = model.take_brep(other_id)?;
    let mut result = model.into_brep()?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    result.refresh_cached_topology();
    other.refresh_cached_topology();
    Ok(PiecewiseMergeOutcome { result, other })
}

/// The planar 2D boolean of two B-reps whose faces all lie on one plane
/// (`SmApiBoolean2d`, `SmPrimitiveCreation::Boolean2D`): union, intersection, A minus
/// B, exclusive or or merge of their regions. On success with faces on both sides the
/// first operand is modified into the result and the second consumed; an operand
/// without faces leaves both unchanged ([`Boolean2dOutcome::Unassigned`]).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for surfaces that are not coincident planes; those of
/// the boolean.
pub fn boolean_2d(
    k: &mut Kernel,
    brep1: Brep,
    brep2: Brep,
    operation: Boolean2d,
) -> Result<Boolean2dOutcome> {
    use crate::topology::topology_traverser::Marks;
    let mut primary = brep1;
    let mut other = brep2;
    let mut primitive = PrimitiveCreation::new(None, 0, false);
    let mut primary_marks = Marks::default();
    let mut other_marks = Marks::default();
    let mut result = None;
    {
        let mut state = ArenaNotifyState::new(&primary);
        state.seed(&other);
        primitive.boolean_2d(
            k,
            &mut primary,
            &mut other,
            operation,
            &mut result,
            (&mut primary_marks, &mut other_marks),
            &mut state.env(None),
        )?;
    }
    Ok(match result {
        Some(mut result) => {
            // (not upstream: the port's cached topology fields, true again for the caller)
            result.refresh_cached_topology();
            Boolean2dOutcome::Result(result)
        }
        None => Boolean2dOutcome::Unassigned {
            brep1: primary,
            brep2: Box::new(other),
        },
    })
}

/// Booleans two lists of B-reps or surfaces (`SmApiBooleanLists`, wrapping
/// `SmMerge::BooleanLists`). `operation` is `0` intersect, `1` union, `2` exclusive
/// or, `3` A minus B, `4` B minus A. The inputs are not modified.
///
/// # Errors
///
/// The first dependency error reached by `merge::p3::boolean_lists`.
pub fn boolean_lists(
    k: &mut Kernel,
    breps1: &[Brep],
    surfaces1: &[Surface],
    breps2: &[Brep],
    surfaces2: &[Surface],
    operation: i32,
) -> Result<BooleanListsResult> {
    let mut out = crate::boolean::merge::p3::boolean_lists(
        k, breps1, surfaces1, breps2, surfaces2, operation, None,
    )?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    for b in &mut out.breps {
        b.refresh_cached_topology();
    }
    Ok(out)
}

/// Combines B-reps by CSG trees in postfix notation (`SmApiBooleanTrees`, wrapping
/// `SmMerge::BooleanTrees`): a node `>= 0` is an index into `breps`, `-1` to `-6` are
/// union, intersection, difference, merge, exclusive or and slice. On success
/// `breps` holds one result per tree followed by the unused B-reps.
///
/// # Errors
///
/// Dependency errors, or `InvalidInput` for an out-of-range executed leaf or
/// a binary operation that would dereference a null operand upstream.
pub fn boolean_trees(
    k: &mut Kernel,
    breps: &mut Vec<Option<Brep>>,
    post_fix_trees: &[i64],
) -> Result<()> {
    crate::boolean::merge::p3::boolean_trees(
        k,
        breps,
        post_fix_trees,
        None,
        &mut crate::boolean::merge::p3::DependencyMerge::carrying_attributes(),
    )?;
    // (not upstream: the port's cached topology fields, true again for the caller)
    for b in breps.iter_mut().flatten() {
        b.refresh_cached_topology();
    }
    Ok(())
}

/// `SmApiTessellateBoundaries`: the edges of `b` as polylines within
/// `angle_tol_deg` ([`brep_p1::tessellate_boundaries`], `SmBrep::TessellateBoundaries`).
///
/// # Errors
///
/// Those of [`brep_p1::tessellate_boundaries`].
pub fn tessellate_boundaries(
    k: &Kernel,
    b: &Brep,
    angle_tol_deg: f64,
) -> Result<BoundaryTessellation> {
    brep_p1::tessellate_boundaries(k, b, angle_tol_deg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::context::MarkType;

    /// `TestSmTessellateBoundaries` (`tests/SM_API_test/src/TestSmBrep.cpp:1328`): a
    /// box's 12 edges are 2 points each; an edge without curve has none and makes the
    /// result incomplete; an invalid angle tolerance is refused.
    #[test]
    fn tessellate_boundaries_of_a_box() {
        let mut k = Kernel::default();
        let mut b = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::ZERO,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let t = tessellate_boundaries(&k, &b, 5.0).unwrap();
        assert!(t.complete);
        assert_eq!(t.edges.len(), 12);
        assert_eq!(t.vertex_counts, vec![2; 12]);
        assert_eq!(t.points.len(), 24);

        b.edge_mut(t.edges[0]).unwrap().curve = None;
        let t = tessellate_boundaries(&k, &b, 5.0).unwrap();
        assert!(!t.complete);
        assert_eq!(t.vertex_counts.len(), 12);
        assert_eq!(t.vertex_counts[0], 0);
        assert_eq!(t.points.len(), 22);

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                tessellate_boundaries(&k, &b, bad),
                Err(SolidError::InvalidInput { .. })
            ));
        }
    }

    const ALL: [BooleanOperation; 11] = [
        BooleanOperation::Unknown,
        BooleanOperation::Union,
        BooleanOperation::Intersection,
        BooleanOperation::Difference,
        BooleanOperation::ExclusiveOr,
        BooleanOperation::Merge,
        BooleanOperation::PartialMerge,
        BooleanOperation::Imprint,
        BooleanOperation::ImprintClassify,
        BooleanOperation::ExtractSeparate,
        BooleanOperation::Slice,
    ];

    #[test]
    fn kernel_operation_mapping() {
        for op in ALL {
            let expect = match op {
                BooleanOperation::Union => Some(0),
                BooleanOperation::Intersection => Some(1),
                BooleanOperation::Difference => Some(2),
                BooleanOperation::Merge => Some(3),
                _ => None,
            };
            assert_eq!(merge_breps_kernel_operation(op), expect, "{op:?}");
        }
    }

    #[test]
    fn extract_separate_is_rejected() {
        let mut k = Kernel::default();
        let e = boolean(
            &mut k,
            Brep::new(),
            Brep::new(),
            BooleanOperation::ExtractSeparate,
        )
        .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));
        let e = boolean_with_options(
            &mut k,
            Brep::new(),
            Brep::new(),
            BooleanOperation::ExtractSeparate,
            MergeOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));
    }

    /// Whether `call` reached the merge engine: it takes a mark (`SmNewMarkAndLock`),
    /// which the API's input validation never does. (On the empty operands of these
    /// tests its outcome is not compared with upstream.)
    fn reaches_merge<T>(k: &mut Kernel, call: impl FnOnce(&mut Kernel) -> Result<T>) -> bool {
        let next = k.context.current_mark(MarkType::NoMark);
        let _ = call(k);
        k.context.current_mark(MarkType::NoMark) != next
    }

    #[test]
    fn valid_operands_reach_the_merge() {
        let mut k = Kernel::default();
        for op in ALL {
            if op == BooleanOperation::ExtractSeparate {
                continue;
            }
            // The merge is built with a computed tolerance; the empty operands are not
            // manifold solids, and NonManifoldBoolean runs merge_p4's ManifoldBoolean on
            // the constructed engine.
            assert!(
                reaches_merge(&mut k, |k| boolean(k, Brep::new(), Brep::new(), op)),
                "{op:?}"
            );
        }
        assert!(reaches_merge(&mut k, |k| boolean_union(
            k,
            Brep::new(),
            Brep::new()
        )));
        assert!(reaches_merge(&mut k, |k| boolean_difference(
            k,
            Brep::new(),
            Brep::new()
        )));
        assert!(reaches_merge(&mut k, |k| boolean_intersection(
            k,
            Brep::new(),
            Brep::new()
        )));
        assert!(reaches_merge(&mut k, |k| boolean_merge(
            k,
            Brep::new(),
            Brep::new()
        )));
    }

    #[test]
    fn merge_breps_rejects_without_consuming() {
        let mut k = Kernel::default();
        for n in 0..2 {
            let mut breps = vec![Brep::new(); n];
            let e = merge_breps(&mut k, &mut breps, BooleanOperation::Union).unwrap_err();
            assert!(matches!(e, SolidError::InvalidInput { .. }));
            assert_eq!(breps.len(), n);
        }
        for op in ALL {
            let mut breps = vec![Brep::new(); 3];
            if merge_breps_kernel_operation(op).is_some() {
                // The merge is constructed and runs.
                assert!(
                    reaches_merge(&mut k, |k| merge_breps(k, &mut breps, op)),
                    "{op:?}"
                );
            } else {
                let e = merge_breps(&mut k, &mut breps, op).unwrap_err();
                assert!(matches!(e, SolidError::InvalidInput { .. }), "{op:?}");
                assert_eq!(breps.len(), 3);
            }
        }
    }

    #[test]
    fn lists_and_trees_reach_the_merge() {
        let mut k = Kernel::default();
        let e = boolean_lists(&mut k, &[Brep::new()], &[], &[Brep::new()], &[], 1).unwrap_err();
        // the list operands are copied (topology.brep_p3) and the merge engine runs
        // (topology.brep_p4 is ported); on empty operands it fails in the engine
        assert!(
            !e.is_unported() && e.operation().starts_with("boolean::merge"),
            "{e:?}"
        );
        let mut breps = vec![Some(Brep::new()), Some(Brep::new())];
        let tree = vec![0, 1, -1];
        // The merge is constructed and runs.
        assert!(reaches_merge(&mut k, |k| boolean_trees(
            k, &mut breps, &tree
        )));
    }
    #[test]
    fn tree_api_preserves_defined_null_outputs_and_failure_slots() {
        let mut k = Kernel::default();
        let mut breps = vec![Some(Brep::new()), Some(Brep::new())];
        boolean_trees(&mut k, &mut breps, &[0, 0]).unwrap();
        assert_eq!(breps.len(), 3);
        assert!(breps.first().unwrap().is_some());
        assert!(breps.get(1).unwrap().is_none());
        assert!(breps.get(2).unwrap().is_some());
        let mut breps = vec![Some(Brep::new()), Some(Brep::new())];
        assert!(boolean_trees(&mut k, &mut breps, &[0, -1]).is_err());
        assert!(breps.first().unwrap().is_none());
        assert!(breps.get(1).unwrap().is_some());
    }
}

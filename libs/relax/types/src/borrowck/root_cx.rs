// ported from N:rustc_borrowck/src/root_cx.rs @8d1a764
use super::check::{mir_borrowck, MirBorrowckResult};
use super::constraints::OutlivesConstraint;
use super::mir_nll::{compute_regions, MirRegionResult, RegionInput};
use crate::mir::{Body, Location};
use crate::mir::move_paths::MoveData;
use crate::program::Program;
use crate::tcx::Tcx;

/// An occurrence of a nested body, after C-NLL5 has instantiated its external
/// region arguments in the parent. Indices identify bodies, not DefIds: Relax
/// closures and promoted bodies can share the containing definition's identity.
pub struct NestedBodyUse {
    pub body: usize,
    pub external_regions: Vec<usize>,
    pub location: Location,
}
pub struct BodyInput<'a> {
    pub body: &'a Body,
    pub moves: &'a MoveData,
    pub regions: RegionInput,
    pub nested: Vec<NestedBodyUse>,
    pub has_opaque_types: bool,
}
pub struct CheckedBody { pub regions: MirRegionResult, pub result: MirBorrowckResult }
pub struct BorrowCheckRootCtxt<'a> { pub tcx: &'a Tcx, pub prog: &'a Program }
impl BorrowCheckRootCtxt<'_> {
    /// Inputs follow nightly's nested-before-parent order. The root is last.
    /// Region requirements retain child blame spans/categories, and acquire the
    /// parent's instantiation location before its region solve.
    pub fn do_mir_borrowck(&self, inputs: Vec<BodyInput<'_>>) -> Vec<CheckedBody> {
        let count = inputs.len();
        let mut checked: Vec<CheckedBody> = Vec::with_capacity(count);
        for (index, mut input) in inputs.into_iter().enumerate() {
            let mut stopped = vec![];
            let mut tainted = checked.iter().any(|body| !body.result.accepted());
            let locations = super::mir_nll::LocationMap::new(input.body);
            if input.has_opaque_types {
                stopped.push("opaque root scheduling needs definition-site storage/application from C0a/C-NLL5");
            }
            for nested in &input.nested {
                assert!(nested.body < index, "nested MIR bodies must precede their parent");
                let child = &checked[nested.body];
                if !child.result.unsupported.is_empty() { stopped.push("nested MIR body has an unsupported contract"); }
                // A failed nested body taints the root, as nightly's shared diagnostics buffer does.
                if !child.result.accepted() { tainted = true; }
                for requirement in &child.regions.regioncx.propagated_outlives_requirements {
                    let sup = nested.external_regions[requirement.sup];
                    let sub = nested.external_regions[requirement.sub];
                    assert!(sup < input.regions.definitions.len() && sub < input.regions.definitions.len());
                    input.regions.outlives.push(OutlivesConstraint {
                        sup, sub, location: Some(locations.point(nested.location)), from_closure: true,
                        ..*requirement
                    });
                }
                if !child.regions.regioncx.propagated_type_requirements.is_empty() {
                    stopped.push("closure type requirements need C-NLL5 type-outlives constraint conversion");
                }
            }
            let mut regions = compute_regions(self.tcx, self.prog, input.body, input.regions, index + 1 != count);
            let mut result = mir_borrowck(self.tcx, self.prog, input.body, &mut regions, input.moves);
            result.unsupported.extend(stopped);
            result.tainted_by_errors |= tainted;
            checked.push(CheckedBody { regions, result });
        }
        // Nightly finalizes through one shared taint cell for the entire root.
        // A child result cannot be reused as clean after a later sibling/parent fails.
        if checked.iter().any(|body| !body.result.accepted()) {
            for body in &mut checked { body.result.tainted_by_errors = true; }
        }
        checked
    }
}
// STOP: MIR has no captured-place/is_by_ref list or instantiated closure signature
// contract. used_mut_upvars and type-subject propagation cannot be reconstructed
// from BIR records. Opaque inputs cannot take the eager no-opaque path: nightly's
// preliminary requirements / shared hidden-type resolution / final requirements
// sequence remains blocked by C0a's explicitly incomplete opaque application API.

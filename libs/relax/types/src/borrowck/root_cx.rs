// ported from N:rustc_borrowck/src/root_cx.rs @1.97.1
//! The shared context of a typeck root and all its nested bodies (closures, coroutines, inline
//! consts): the two-phase schedule that applies nested bodies' region and type requirements and
//! the root-wide handling of opaque type uses, then solves and checks every body.
use super::check::{mir_borrowck_nested, MirBorrowckResult, NestedUsedMut};
use super::constraints::{OutlivesConstraint, RegionVid};
use super::mir_nll::{compute_regions, free_regions_in_order, LocationMap, MirRegionResult, MirTypeck, RegionDriverOps};
use super::region_infer::opaque_types::{self, DeferredOpaqueTypeError, HiddenTypes, OpaqueOps};
use super::region_infer::ClosureTypeRequirement;
use super::type_check::{ClauseOwner, DeferredClosureRequirement};
use super::universal_regions::UniversalRegions;
use crate::mir::{AggregateKind, Body, Rvalue, Span};
use crate::mir::move_paths::MoveData;
use crate::next_solver::type_op::{QueryRegionConstraint, QueryRegionConstraints, RegionConstraint};
use crate::program::Program;
use crate::tcx::{outlives::OutlivesPredicate, Tcx};
use crate::types::{RegionId, RegionKind};

/// The inference context of one body of the root, with everything borrowck does to it.
pub trait RootBodyOps: RegionDriverOps + OpaqueOps {}
impl<T: RegionDriverOps + OpaqueOps> RootBodyOps for T {}

/// `CollectRegionConstraintsResult`: one body whose MIR has been type checked, and whose region
/// constraints are not solved yet.
pub struct RootBody<'a> {
    /// Identifies the body in the `deferred_closure_requirements` of its parent.
    pub key: ClauseOwner,
    pub body: &'a Body,
    pub moves: &'a MoveData,
    pub infcx: &'a mut dyn RootBodyOps,
    pub typeck: MirTypeck,
    /// A closure, coroutine or inline const: universal region errors may be propagated to its creator.
    pub nested: bool,
}

/// `ClosureRegionRequirements`: what a nested body requires of the regions it was instantiated with.
#[derive(Clone, Debug)]
pub struct ClosureRequirements {
    pub num_external_vids: usize,
    pub outlives: Vec<OutlivesConstraint>,
    pub types: Vec<ClosureTypeRequirement>,
}

pub struct CheckedBody {
    pub regions: MirRegionResult,
    pub result: MirBorrowckResult,
    pub requirements: Option<ClosureRequirements>,
    /// Reported only when there are no region errors: with them these are likely consequences.
    pub deferred_opaque_type_errors: Vec<DeferredOpaqueTypeError>,
    /// What the closure-like bodies created in this body were written as, by closure expression: for
    /// `DiagCx::coroutine_kinds` (the noun in E0373 and friends).
    pub coroutine_kinds: Vec<(u32, crate::mir::CoroutineKind)>,
}

pub struct RootResult {
    /// In the order of the input: nested bodies before their parents, the root last.
    pub bodies: Vec<CheckedBody>,
    /// The final hidden type of every opaque defined in this root (or the error type).
    pub hidden_types: HiddenTypes,
}

pub struct BorrowCheckRootCtxt<'a> { pub tcx: &'a Tcx, pub prog: &'a Program }

/// What a parent needs to know about a nested body to instantiate its requirements.
struct NestedInfo { universals: UniversalRegions, closure_slots: Vec<RegionId>, closure_sig_erased_offset: usize }

impl BorrowCheckRootCtxt<'_> {
    /// `do_mir_borrowck`: `bodies` visit nested bodies before their parents. The schedule is not an
    /// unconditional children-first one-pass solve: computing the defining uses of opaques in a
    /// parent may depend on the propagated requirements of its nested bodies, while checking the
    /// non-defining uses in a nested body may rely on the defining uses in the parent. Requirements
    /// are therefore applied twice for bodies that depend on opaques: modulo opaques, and after.
    /// Bodies that do not depend on opaque types are checked eagerly.
    pub fn do_mir_borrowck(&self, bodies: Vec<RootBody<'_>>) -> RootResult { self.do_mir_borrowck_fv(bodies, None) }

    /// `do_mir_borrowck` with relaxed-Rust field views (S15) applied to every body's conflict check.
    pub fn do_mir_borrowck_fv(&self, bodies: Vec<RootBody<'_>>, fv: Option<&super::field_views::FieldViews<'_>>) -> RootResult {
        let n = bodies.len();
        let info: Vec<NestedInfo> = bodies.iter().map(|b| NestedInfo { universals: b.typeck.universals.clone(), closure_slots: b.typeck.closure_slots.clone(), closure_sig_erased_offset: b.typeck.closure_sig_erased_offset }).collect();
        let keys: Vec<ClauseOwner> = bodies.iter().map(|b| b.key).collect();
        // The nested bodies each body creates, and what they were written as.
        let created: Vec<Vec<usize>> = bodies.iter().map(|b| b.typeck.summary.deferred_closure_requirements.iter()
            .filter_map(|u| keys.iter().position(|k| *k == u.def)).collect()).collect();
        let kinds: Vec<Option<crate::mir::CoroutineKind>> = bodies.iter().map(|b| b.body.coroutine.as_ref().map(|c| c.kind)).collect();
        let nested_info = |i: usize, finished: &[Option<CheckedBody>]| {
            let mut used = NestedUsedMut::default();
            let mut coroutine_kinds = vec![];
            for &j in &created[i] {
                let (ClauseOwner::Closure(id) | ClauseOwner::Coroutine(id)) = keys[j] else { continue };
                if let Some(done) = &finished[j] { used.0.push((id, done.result.used_mut_upvars.clone())); }
                if let Some(kind) = kinds[j] { coroutine_kinds.push((id.expr, kind)); }
            }
            (used, coroutine_kinds)
        };
        let mut bodies: Vec<Option<RootBody<'_>>> = bodies.into_iter().map(Some).collect();
        let mut finished: Vec<Option<CheckedBody>> = (0..n).map(|_| None).collect();
        let mut modulo_opaques: Vec<Option<Option<ClosureRequirements>>> = vec![None; n];
        let mut deferred_errors: Vec<Vec<DeferredOpaqueTypeError>> = (0..n).map(|_| vec![]).collect();
        let mut hidden_types = HiddenTypes::new();

        // apply_closure_requirements_modulo_opaques
        for i in 0..n {
            let mut body = bodies[i].take().unwrap();
            // A body depends on opaque types if it has opaque type uses itself or a nested body which does.
            let mut depends_on_opaques = body.infcx.has_opaque_types_in_storage();
            let mut kept = vec![];
            for use_ in std::mem::take(&mut body.typeck.summary.deferred_closure_requirements) {
                // An inline constant the driver has not built as a body of its own imposes no requirement here
                // (rustc checks it as a nested body; the driver does not yet).
                let Some(j) = keys.iter().position(|k| *k == use_.def) else { continue };
                assert!(j < i, "nested MIR bodies must precede their parent");
                let requirements = match &finished[j] {
                    Some(done) => done.requirements.clone(),
                    None => {
                        // Only the requirements modulo opaques exist; the final ones are applied later.
                        depends_on_opaques = true;
                        kept.push(use_.clone());
                        modulo_opaques[j].clone().expect("a nested body depending on opaques has its requirements modulo opaques")
                    }
                };
                self.apply_closure_requirements(&mut body, &info[j], requirements.as_ref(), &use_);
            }
            body.typeck.summary.deferred_closure_requirements = kept;
            if depends_on_opaques {
                // A nested body needs its requirements modulo opaques for its parent.
                if i + 1 != n { modulo_opaques[i] = Some(self.requirements_modulo_opaques(&body)); }
                bodies[i] = Some(body);
            } else {
                assert!(body.typeck.summary.deferred_closure_requirements.is_empty());
                { let nested = nested_info(i, &finished); finished[i] = Some(self.check_region_constraints(body, vec![], nested, fv)); }
            }
        }

        // handle_opaque_type_uses: all bodies together.
        let mut unconstrained = vec![];
        let mut per_body: Vec<Option<(crate::infer::opaque_types::OpaqueTypeStorageEntries, Vec<_>)>> = (0..n).map(|_| None).collect();
        for i in 0..n {
            let Some(body) = bodies[i].as_mut() else { continue };
            let n_vars = body.infcx.num_region_vars();
            let relations = body.typeck.relations(n_vars);
            let (entries, opaque_types) = opaque_types::clone_and_resolve_opaque_types(self.tcx, &mut *body.infcx, &relations, &mut body.typeck.constraints);
            let definitions = body.typeck.definitions(&*body.infcx);
            let relations = body.typeck.relations(definitions.len());
            deferred_errors[i] = opaque_types::compute_definition_site_hidden_types(self.tcx, self.prog, i, &mut *body.infcx, definitions, &relations,
                &body.typeck.constraints, &mut hidden_types, &mut unconstrained, &opaque_types);
            per_body[i] = Some((entries, opaque_types));
        }
        {
            let mut errors: Vec<&mut Vec<DeferredOpaqueTypeError>> = deferred_errors.iter_mut().collect();
            opaque_types::handle_unconstrained_hidden_type_errors(self.tcx, &hidden_types, &mut unconstrained, &mut errors);
        }
        for i in 0..n {
            let Some(body) = bodies[i].as_mut() else { continue };
            let (entries, opaque_types) = per_body[i].take().unwrap();
            if deferred_errors[i].is_empty() {
                deferred_errors[i] = opaque_types::apply_definition_site_hidden_types(self.tcx, self.prog, &mut *body.infcx, &mut body.typeck.constraints, &mut hidden_types, &opaque_types);
            }
            opaque_types::detect_opaque_types_added_while_handling_opaque_types(self.tcx, self.prog, &mut *body.infcx, entries);
        }

        // Finish the bodies which depend on opaque types: first the final requirements of nested bodies.
        for i in 0..n {
            let Some(mut body) = bodies[i].take() else { continue };
            for use_ in std::mem::take(&mut body.typeck.summary.deferred_closure_requirements) {
                let Some(j) = keys.iter().position(|k| *k == use_.def) else { continue };
                let requirements = finished[j].as_ref().expect("nested bodies are finished before their parent").requirements.clone();
                self.apply_closure_requirements(&mut body, &info[j], requirements.as_ref(), &use_);
            }
            { let nested = nested_info(i, &finished); finished[i] = Some(self.check_region_constraints(body, std::mem::take(&mut deferred_errors[i]), nested, fv)); }
        }

        let mut bodies: Vec<CheckedBody> = finished.into_iter().map(|b| b.expect("every body is checked")).collect();
        // Nightly finalizes through one shared taint cell for the entire root: a child result cannot
        // be reused as clean after a later sibling or parent fails.
        if bodies.iter().any(|b| !b.result.accepted()) {
            for body in &mut bodies { body.result.tainted_by_errors = true; }
        }
        RootResult { bodies, hidden_types }
    }

    /// `compute_closure_requirements_modulo_opaques`: the requirements of a nested body before its
    /// opaque uses have been handled. Everything else of this solve is discarded.
    fn requirements_modulo_opaques(&self, body: &RootBody<'_>) -> Option<ClosureRequirements> {
        let input = body.typeck.region_input(&*body.infcx);
        let regions = compute_regions(self.tcx, self.prog, body.body, input, body.nested);
        requirements_of(&regions)
    }

    /// `borrowck_check_region_constraints`: solve the regions and check the body.
    fn check_region_constraints(&self, body: RootBody<'_>, deferred_opaque_type_errors: Vec<DeferredOpaqueTypeError>,
        (nested_used_mut, coroutine_kinds): (NestedUsedMut, Vec<(u32, crate::mir::CoroutineKind)>), fv: Option<&super::field_views::FieldViews<'_>>) -> CheckedBody {
        assert!(body.typeck.summary.deferred_closure_requirements.is_empty());
        let RootBody { body: mir, moves, infcx, typeck, nested, .. } = body;
        let input = typeck.into_region_input(&*infcx);
        let mut regions = compute_regions(self.tcx, self.prog, mir, input, nested);
        let mut result = mir_borrowck_nested(self.tcx, self.prog, mir, &mut regions, moves, fv, &nested_used_mut);
        if regions.regioncx.errors.is_empty() && !deferred_opaque_type_errors.is_empty() { result.tainted_by_errors = true; }
        let requirements = requirements_of(&regions);
        CheckedBody { regions, result, requirements, deferred_opaque_type_errors, coroutine_kinds }
    }

    /// `apply_closure_requirements`: instantiates the "extra" requirements computed for a nested
    /// body with the regions it is created with. This adds new outlives obligations to existing
    /// region variables of the parent, blamed on the nested body's original constraint.
    fn apply_closure_requirements(&self, parent: &mut RootBody<'_>, child: &NestedInfo, requirements: Option<&ClosureRequirements>, use_: &DeferredClosureRequirement) {
        let Some(requirements) = requirements else { return };
        let mapping = self.closure_mapping(parent, child, requirements.num_external_vids, use_);
        let tys = &self.tcx.tys;
        let point = use_.locations.from_location().map(|l| LocationMap::new(parent.body).point(l));
        for c in &requirements.outlives {
            parent.typeck.constraints.outlives_constraints.push(OutlivesConstraint {
                sup: mapping[c.sup], sub: mapping[c.sub], location: point, from_closure: true, ..*c });
        }
        // Type subjects are outlives obligations converted by the parent's inference context, like any other.
        let file = use_.locations.span(parent.body).file;
        for t in &requirements.types {
            let subject = tys.map_regions(t.subject, &|r, depth| match tys.region_kind(r) {
                RegionKind::ReBound(d, _) if d < depth => r,
                RegionKind::ReVar(v) => tys.intern_region(RegionKind::ReVar(mapping[v as usize] as u32)),
                _ => panic!("closure type requirement over a region that is not an external region variable"),
            }, 0);
            let outlived = tys.intern_region(RegionKind::ReVar(mapping[t.outlived_free_region] as u32));
            let query = QueryRegionConstraints { constraints: vec![QueryRegionConstraint {
                constraint: RegionConstraint::Outlives(OutlivesPredicate::Type(subject, outlived)),
                category: super::constraints::ConstraintCategory::Boring, visible_for_leak_check: false }] };
            let before = parent.typeck.constraints.outlives_constraints.outlives.len();
            let span = Span { file, lo: t.blame_span.lo, hi: t.blame_span.hi };
            parent.infcx.push_region_constraints(&mut parent.typeck.constraints, use_.locations, span, super::constraints::ConstraintCategory::Boring, &query);
            for c in &mut parent.typeck.constraints.outlives_constraints.outlives[before..] { c.from_closure = true; }
        }
    }

    /// `UniversalRegions::closure_mapping`: the regions of the parent that the nested body's
    /// external universal regions (global first, then external) stand for. Those the nested body
    /// inherits are the parent's own universal regions; the others sit in the regions of the closure
    /// type, at the same position in the parent's instantiation of it.
    fn closure_mapping(&self, parent: &mut RootBody<'_>, child: &NestedInfo, num_external_vids: usize, use_: &DeferredClosureRequirement) -> Vec<RegionVid> {
        let tys = &self.tcx.tys;
        let universals = &parent.typeck.universals;
        // The parent's instantiation of the closure type, one region per position.
        let instantiated: Vec<RegionVid> = match use_.def {
            ClauseOwner::Closure(id) | ClauseOwner::Coroutine(id) => {
                let kind = if matches!(use_.def, ClauseOwner::Closure(_)) { AggregateKind::Closure(id, use_.args.clone()) } else { AggregateKind::Coroutine(id, use_.args.clone()) };
                let ty = parent.infcx.rvalue_query_ty(parent.body, &Rvalue::Aggregate(Box::new(kind), vec![]));
                free_regions_in_order(self.tcx, ty).into_iter().map(|r| parent.infcx.to_region_vid(r)).collect()
            }
            ClauseOwner::Def(_) => vec![],
        };
        // the nested body numbers its own placeholders after those it inherits from this body (`UniversalRegionsBuilder::build`)
        let placeholder_base = super::universal_regions::placeholder_base(self.tcx, universals.indices.iter().map(|(r, _)| *r));
        (0..num_external_vids).map(|vid| {
            let identity = child.universals.indices.iter().find(|(_, v)| *v == vid).map(|x| x.0)
                .unwrap_or_else(|| panic!("external region {vid} of a nested body has no identity"));
            if tys.region_kind(identity) == RegionKind::ReStatic { return universals.fr_static; }
            if let Some(&(_, parent_vid)) = universals.indices.iter().find(|(r, _)| *r == identity) { return parent_vid; }
            // A region erased in the closure's signature stands for the signature's k-th erased region
            // (`replace_erased_regions_with_placeholders` numbers them in order); the creator's closure type
            // has a variable at that position.
            let erased_slot = match tys.region_kind(identity) {
                RegionKind::RePlaceholder { var, .. } => child.closure_slots.iter().enumerate().filter(|(_, r)| **r == RegionId::ERASED).nth(child.closure_sig_erased_offset + (var.0 - placeholder_base) as usize).map(|(i, _)| i),
                // an erased region of the environment type (`SignatureRegions::with_closure_env`)
                RegionKind::ReLateParam(_, crate::types::LateParamKind::Anon(n)) if n >= super::universal_regions::ENV_ERASED_BASE =>
                    child.closure_slots.iter().enumerate().filter(|(_, r)| **r == RegionId::ERASED).nth((n - super::universal_regions::ENV_ERASED_BASE) as usize).map(|(i, _)| i),
                _ => None,
            };
            if let Some(slot) = erased_slot { return instantiated[slot]; }
            let slot = child.closure_slots.iter().position(|r| *r == identity)
                .unwrap_or_else(|| panic!("external region {identity:?} = {:?} of a nested body is neither the parent's nor in its closure type (parent regions {:?}, closure slots {:?})", tys.region_kind(identity), universals.indices.iter().map(|(r, _)| tys.region_kind(*r)).collect::<Vec<_>>(), child.closure_slots.iter().map(|r| tys.region_kind(*r)).collect::<Vec<_>>()));
            instantiated[slot]
        }).collect()
    }
}

/// `opt_closure_req`: the outlives and type requirements a nested body propagates.
fn requirements_of(regions: &MirRegionResult) -> Option<ClosureRequirements> {
    let cx = &regions.regioncx;
    if cx.propagated_outlives_requirements.is_empty() && cx.propagated_type_requirements.is_empty() { return None; }
    Some(ClosureRequirements {
        num_external_vids: cx.universal_region_relations.universal_regions.num_global_and_external_regions(),
        outlives: cx.propagated_outlives_requirements.clone(),
        types: cx.propagated_type_requirements.clone(),
    })
}

// ported from N:rustc_borrowck/src/nll.rs @8d1a764 (compute_regions)
use super::constraints::OutlivesConstraintSet;
use super::dataflow::RegionScope;
use super::handle_placeholders::compute_sccs_applying_placeholder_outlives_constraints;
use super::region_infer::{RegionDefinition, RegionInferenceContext, TypeTest};
use super::region_infer::values::LivenessValues;
use super::type_check::free_region_relations::UniversalRegionRelations;
use crate::mir::{Body, Location, Region};
use crate::program::Program;
use crate::tcx::Tcx;

/// Dense MIR location numbering, including every terminator. Shared with C-NLL5.
#[derive(Clone)]
pub struct LocationMap { offsets: Vec<usize> }
impl LocationMap {
    pub fn new(body: &Body) -> Self {
        let mut offsets = vec![0];
        for block in &body.basic_blocks { offsets.push(offsets.last().unwrap() + block.statements.len() + 1); }
        Self { offsets }
    }
    pub fn num_points(&self) -> usize { *self.offsets.last().unwrap() }
    pub fn point(&self, at: Location) -> usize {
        let b = at.block.0 as usize;
        let point = self.offsets[b] + at.statement_index;
        assert!(point < self.offsets[b + 1]);
        point
    }
    pub fn location(&self, point: usize) -> Location {
        assert!(point < self.num_points());
        let block = self.offsets.partition_point(|&offset| offset <= point) - 1;
        Location { block: crate::mir::BasicBlock(block as u32), statement_index: point - self.offsets[block] }
    }
    pub fn successors(&self, body: &Body, point: usize) -> Vec<usize> {
        use crate::mir::visit::Visitor;
        let at = self.location(point);
        let block = &body.basic_blocks[at.block.0 as usize];
        if at.statement_index < block.statements.len() { return vec![point + 1]; }
        struct Targets<'a> { map: &'a LocationMap, points: Vec<usize> }
        impl Visitor for Targets<'_> {
            fn visit_target(&mut self, block: &crate::mir::BasicBlock, _: bool, _: Location) {
                self.points.push(self.map.point(Location { block: *block, statement_index: 0 }));
            }
        }
        let mut targets = Targets { map: self, points: vec![] };
        targets.visit_terminator(block.terminator.as_ref().unwrap(), at);
        targets.points
    }
    pub fn predecessors(&self, body: &Body) -> Vec<Vec<usize>> {
        let mut result = vec![vec![]; self.num_points()];
        for p in 0..self.num_points() { for q in self.successors(body, p) { result[q].push(p); } }
        result
    }
    pub fn matches(&self, body: &Body) -> bool { self.offsets == Self::new(body).offsets }
}
/// C-NLL5 supplies renumbered type-check/liveness output; no BIR producer is used.
pub struct RegionInput {
    pub polonius: super::polonius::PoloniusContext,
    /// Prior type-check/normalization failure taints the root even with no region errors.
    pub tainted_by_errors: bool,
    pub definitions: Vec<RegionDefinition>,
    pub outlives: OutlivesConstraintSet,
    pub relations: UniversalRegionRelations,
    pub liveness: LivenessValues,
    pub type_tests: Vec<TypeTest>,
    /// Why each universe was created (diagnostics of placeholder errors).
    pub universe_causes: Vec<(UniverseIndex, super::type_check::UniverseInfo)>,
}
pub struct MirRegionResult {
    pub polonius: super::polonius::PoloniusContext,
    pub nested: bool,
    pub tainted_by_errors: bool,
    pub regioncx: RegionInferenceContext,
    pub locations: LocationMap,
    pub type_tests: Vec<TypeTest>,
    pub universe_causes: Vec<(UniverseIndex, super::type_check::UniverseInfo)>,
}
impl RegionScope for MirRegionResult {
    fn is_loan_live_at(&self, loan: usize, at: Location) -> Option<bool> {
        self.regioncx.liveness_constraints().live_loans().map(|loans| loans.contains(self.locations.point(at), loan))
    }
    fn contains_point(&self, region: Region, at: Location) -> bool {
        self.regioncx.region_contains_point(region.0 as usize, self.locations.point(at))
    }
}
pub fn compute_regions(tcx: &Tcx, prog: &Program, body: &Body, input: RegionInput, nested: bool) -> MirRegionResult {
    let locations = LocationMap::new(body);
    let lowered = compute_sccs_applying_placeholder_outlives_constraints(
        input.definitions, input.outlives, input.relations.universal_regions.fr_static);
    let regioncx = RegionInferenceContext::solve(tcx, prog, lowered, input.relations, input.liveness,
        locations.num_points(), &input.type_tests, nested);
    MirRegionResult { polonius: input.polonius, nested, tainted_by_errors: input.tainted_by_errors, regioncx, locations, type_tests: input.type_tests, universe_causes: input.universe_causes }
}

// ---------------------------------------------------------------------------------------------
// The MIR front half of N:nll.rs `compute_regions`: universal regions from the native signature,
// N:type_check/free_region_relations.rs `create`, renumbering and `type_check`, assembled into the
// `RegionInput` that `compute_regions` solves. The body is the builder's analysis MIR.
// ---------------------------------------------------------------------------------------------
use super::constraints::RegionVid;
use super::implied_bounds::ImpliedBounds;
use super::region_infer::NllRegionVariableOrigin;
use super::type_check::{self, BorrowckTypeOps, MirTypeckRegionConstraints, MirTypeckResults, TypeckInputs};
use super::polonius::PoloniusContext;
use super::type_check::liveness::DropckOverflow;
use super::universal_regions::{SignatureRegions, UniversalRegions};
use super::borrowck_errors::BorrowckError;
use crate::mir::move_paths::MoveData;
use crate::mir::{BodyKind, Span};
use crate::types::{RegionId, RegionKind, TyId, TyKind, UniverseIndex};

/// What the driver needs of N01's inference context beyond the type operations.
pub trait RegionDriverOps: BorrowckTypeOps {
    fn num_region_vars(&self) -> usize;
    /// The origin and universe `next_nll_region_var*` recorded for `vid`.
    fn region_var_info(&self, vid: RegionVid) -> (NllRegionVariableOrigin, UniverseIndex);
    /// `renumber_mir`: every free region of the body and of its promoted bodies becomes a fresh variable.
    fn renumber(&mut self, body: &mut Body);
    /// Called with the renumbered body before the type check: location-aware ops read its numbering,
    /// spans and allocations through the context, not through a reference to the body.
    fn observe_body(&mut self, _body: &Body) {}
    /// The universal regions and the region-bound pairs (`T: 'r` facts) that location-aware type ops convert against.
    fn install_universals(&mut self, universals: &UniversalRegions, region_bound_pairs: &[(TyId, RegionId)]);
    /// `mir_borrowck_implied_outlives_bounds` of the declaration (implied_bounds.rs:27-118): the implied
    /// bounds of the signature and the normalized inputs and output.
    fn implied_bounds(&mut self, sig: &SignatureRegions, nested: bool) -> (ImpliedBounds, Vec<TyId>);
    /// `DeeplyNormalize` of a signature type, constraints holding everywhere (`Locations::All(span)`, Internal).
    fn normalize_signature_ty(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, span: Span) -> TyId;
}

pub struct MirNllInput<'a> {
    pub sig: &'a SignatureRegions,
    /// The body is a closure/coroutine/inline const nested in a typeck root.
    pub nested: bool,
    /// `def_span` of the defining item: the location of the signature's constraints.
    pub def_span: Span,
    pub moves: &'a MoveData,
    /// `borrow_set.len() != 0`.
    pub has_loans: bool,
}

/// Everything the type check found besides the region constraints.
pub struct MirTypeckSummary {
    pub errors: Vec<BorrowckError>,
    pub mir_bugs: Vec<super::type_check::MirBug>,
    pub dropck_overflows: Vec<DropckOverflow>,
    pub deferred_closure_requirements: Vec<super::type_check::DeferredClosureRequirement>,
}

/// N:CollectRegionConstraintsResult without its inference context: the type check of one body,
/// before its region constraints are solved. The root applies nested bodies' requirements and
/// defining opaque uses to `constraints` first.
pub struct MirTypeck {
    pub universals: UniversalRegions,
    /// Relations known to hold between universals, as variables.
    pub known: Vec<(RegionVid, RegionVid)>,
    /// `known[..n_explicit_known]` are the param-env's outlives clauses.
    pub n_explicit_known: usize,
    pub constraints: MirTypeckRegionConstraints,
    pub polonius: PoloniusContext,
    pub tainted_by_errors: bool,
    pub summary: MirTypeckSummary,
    /// The free regions of a closure body's environment type (the closure type's region slots),
    /// as written before renumbering. A nested use maps them to its instantiation by position.
    pub closure_slots: Vec<RegionId>,
    /// How many of the erased slots precede the signature's regions in the closure type (those of its parent's
    /// arguments, which a coroutine's hidden type has erased): the signature's k-th erased region is the
    /// (offset + k)-th.
    pub closure_sig_erased_offset: usize,
}
impl MirTypeck {
    /// The inputs of region inference: the definitions of every variable that exists now.
    pub fn region_input(&self, infcx: &dyn RegionDriverOps) -> RegionInput { self.clone_for_regions().into_region_input(infcx) }
    fn clone_for_regions(&self) -> MirTypeck {
        MirTypeck { universals: self.universals.clone(), known: self.known.clone(), n_explicit_known: self.n_explicit_known, constraints: self.constraints.clone(),
            polonius: self.polonius.clone(), tainted_by_errors: self.tainted_by_errors,
            summary: MirTypeckSummary { errors: vec![], mir_bugs: vec![], dropck_overflows: vec![], deferred_closure_requirements: vec![] },
            closure_slots: vec![], closure_sig_erased_offset: 0 }
    }
    /// The definition of every region variable that exists now.
    pub fn definitions(&self, infcx: &dyn RegionDriverOps) -> Vec<RegionDefinition> {
        (0..infcx.num_region_vars()).map(|vid| {
            let (origin, universe) = infcx.region_var_info(vid);
            let external_name = match origin {
                NllRegionVariableOrigin::FreeRegion => self.universals.indices.iter().find(|(_, v)| *v == vid).map(|x| x.0),
                NllRegionVariableOrigin::Placeholder(p) => Some(p),
                NllRegionVariableOrigin::Existential => None,
            };
            RegionDefinition { origin, universe, external_name }
        }).collect()
    }
    /// The relations known between the universal regions, for `num_regions` variables.
    pub fn relations(&self, num_regions: usize) -> super::type_check::free_region_relations::UniversalRegionRelations {
        super::type_check::free_region_relations::UniversalRegionRelations::create_with(self.universals.clone(), num_regions, &self.known, self.n_explicit_known)
    }
    pub fn into_region_input(self, infcx: &dyn RegionDriverOps) -> RegionInput {
        let definitions = self.definitions(infcx);
        let relations = self.relations(definitions.len());
        let MirTypeckRegionConstraints { outlives_constraints, liveness_constraints, type_tests, universe_causes, .. } = self.constraints;
        RegionInput { polonius: self.polonius, tainted_by_errors: self.tainted_by_errors, definitions, outlives: outlives_constraints,
            relations, liveness: liveness_constraints, type_tests, universe_causes }
    }
}

/// Replace the regions of `ty` by the variables of the universal regions they name; a region
/// the signature does not declare (typeck's leftover inference variables, erased regions) is
/// a fresh existential variable, one per original region.
fn map_to_vids(tcx: &Tcx, universals: &UniversalRegions, infcx: &mut dyn BorrowckTypeOps, fresh: &mut Vec<(RegionId, RegionId)>, ty: TyId) -> TyId {
    let tys = &tcx.tys;
    let found = std::cell::RefCell::new(vec![]);
    tys.map_regions(ty, &|r, depth| { if !matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { found.borrow_mut().push(r); } r }, 0);
    let mut replacement: Vec<(RegionId, RegionId)> = vec![];
    for r in found.into_inner() {
        if replacement.iter().any(|x| x.0 == r) { continue; }
        let mapped = match universals.indices.iter().find(|(o, _)| *o == r) {
            Some(&(_, vid)) => tys.intern_region(RegionKind::ReVar(vid as u32)),
            None if tys.region_kind(r) == RegionKind::ReErased => RegionId::ERASED,
            None => match fresh.iter().find(|x| x.0 == r) {
                Some(&(_, v)) => v,
                None => { let v = infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, UniverseIndex::ROOT); fresh.push((r, v)); v }
            },
        };
        replacement.push((r, mapped));
    }
    let infcx = std::cell::RefCell::new(infcx);
    tys.map_regions(ty, &|r, depth| {
        if matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { return r; }
        match replacement.iter().find(|x| x.0 == r) {
            // Each erased occurrence is its own region.
            Some(&(_, m)) if m == RegionId::ERASED => infcx.borrow_mut().next_nll_region_var(NllRegionVariableOrigin::Existential, UniverseIndex::ROOT),
            Some(&(_, m)) => m,
            None => r,
        }
    }, 0)
}

/// N:nll.rs `compute_regions` up to the region inference solve: `UniversalRegions::new`,
/// `free_region_relations::create`, `renumber_mir`, and `type_check` with its liveness.
/// `body` is renumbered in place.
pub fn type_check_body(tcx: &Tcx, prog: &Program, infcx: &mut dyn RegionDriverOps, body: &mut Body, input: &MirNllInput<'_>) -> MirTypeck {
    let sig = input.sig;
    // Universal regions precede every other region variable.
    let universals = UniversalRegions::from_signature(tcx, sig, || {
        let r = infcx.next_nll_region_var(NllRegionVariableOrigin::FreeRegion, UniverseIndex::ROOT);
        infcx.to_region_vid(r)
    });
    // The signature as the body sees it: the implicit environment argument, the inputs, the output.
    let raw: Vec<TyId> = sig.implicit_input.into_iter().chain(sig.inputs.iter().copied()).chain([sig.output]).collect();
    let (implied, _normalized_by_query) = infcx.implied_bounds(sig, input.nested);
    // The region-bound pairs are over the universal regions' variables (rustc builds them from the NLL
    // universals): the signature's own region identities (a closure's typeck-written `ReVar`s among them)
    // are mapped here, so that a later `ReVar(v)` always means the variable `v` of this context.
    let region_bound_pairs: Vec<(TyId, RegionId)> = implied.type_outlives.iter().map(|&(t, r)| {
        let known = |r: RegionId| universals.indices.iter().find(|(o, _)| *o == r).map(|&(_, v)| tcx.tys.intern_region(RegionKind::ReVar(v as u32)));
        (tcx.tys.map_regions(t, &|r, _| known(r).unwrap_or(r), 0), known(r).unwrap_or(r))
    }).collect();
    infcx.install_universals(&universals, &region_bound_pairs);
    let mut constraints = MirTypeckRegionConstraints::new(LocationMap::new(body).num_points());
    let mut fresh = vec![];
    let mut normalized_inputs_and_output = vec![];
    for &ty in &raw {
        let mapped = map_to_vids(tcx, &universals, infcx, &mut fresh, ty);
        normalized_inputs_and_output.push(infcx.normalize_signature_ty(&mut constraints, mapped, input.def_span));
    }
    // The coroutine's universal yield and resume types, as the builder wrote them before renumbering.
    let universal_yield_resume = body.coroutine.as_ref().map(|c| (
        map_to_vids(tcx, &universals, infcx, &mut fresh, c.yield_ty), map_to_vids(tcx, &universals, infcx, &mut fresh, c.resume_ty)));
    // Relations known to hold between universals: implied bounds and where clauses.
    let mut n_explicit_known = 0;
    let known: Vec<(RegionVid, RegionVid)> = implied.outlives.iter().enumerate().filter_map(|(i, &(a, b))| {
        let vid = |r| universals.indices.iter().find(|(o, _)| *o == r).map(|x| x.1);
        let pair = (vid(a)?, vid(b)?);
        if i < implied.explicit_outlives { n_explicit_known += 1; }
        Some(pair)
    }).collect();
    let closure_slots = if matches!(body.source.kind, BodyKind::Closure(_)) { body.local_decls.get(1).map_or(vec![], |env| {
        let env = match tcx.tys.kind(env.ty) { TyKind::Ref(_, t, _) => *t, _ => env.ty };
        free_regions_in_order(tcx, env)
    }) } else { vec![] };

    let closure_sig_erased_offset = if matches!(body.source.kind, BodyKind::Closure(_)) { body.local_decls.get(1).map_or(0, |env| {
        let env = match tcx.tys.kind(env.ty) { TyKind::Ref(_, t, _) => *t, _ => env.ty };
        match tcx.tys.kind(env) {
            TyKind::Closure(_, _, args) => [args.parent_args, args.closure_kind_ty].iter().flat_map(|&t| free_regions_in_order(tcx, t)).filter(|&r| r == RegionId::ERASED).count(),
            _ => 0,
        }
    }) } else { 0 };

    infcx.renumber(body);
    infcx.observe_body(body);
    let results = type_check::type_check(tcx, prog, infcx, body, TypeckInputs {
        normalized_inputs_and_output: &normalized_inputs_and_output, universal_yield_resume,
        universals: &universals, moves: input.moves, has_loans: input.has_loans, constraints });
    let MirTypeckResults { constraints, polonius, tainted_by_errors, errors, mir_bugs, dropck_overflows, deferred_closure_requirements } = results;
    MirTypeck { universals, known, n_explicit_known, constraints, polonius, tainted_by_errors,
        summary: MirTypeckSummary { errors, mir_bugs, dropck_overflows, deferred_closure_requirements }, closure_slots, closure_sig_erased_offset }
}

/// `tcx.for_each_free_region`: the regions of `ty` not bound inside it, in visiting order.
pub fn free_regions_in_order(tcx: &Tcx, ty: TyId) -> Vec<RegionId> {
    let tys = &tcx.tys;
    let found = std::cell::RefCell::new(vec![]);
    tys.map_regions(ty, &|r, depth| { if !matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { found.borrow_mut().push(r); } r }, 0);
    found.into_inner()
}

impl RegionDriverOps for super::infer_ctxt::BorrowckInferCtxt<'_> {
    fn num_region_vars(&self) -> usize { self.infcx.num_region_vars() }
    fn region_var_info(&self, vid: RegionVid) -> (NllRegionVariableOrigin, UniverseIndex) {
        use crate::infer::region_constraints::RegionVariableOrigin;
        let origin = match self.infcx.region_var_origin(vid as u32) {
            RegionVariableOrigin::Nll(origin) => origin,
            RegionVariableOrigin::Misc => NllRegionVariableOrigin::Existential,
        };
        let region = self.tcx.tys.intern_region(RegionKind::ReVar(vid as u32));
        (origin, self.infcx.inner.borrow_mut().unwrap_region_constraints().universe(&self.tcx.tys, region))
    }
    fn renumber(&mut self, body: &mut Body) {
        let mut promoted = std::mem::take(&mut body.promoted);
        super::renumber::renumber_mir(self, body, &mut promoted);
        body.promoted = promoted;
    }
    fn observe_body(&mut self, body: &Body) { super::infer_ctxt::BorrowckInferCtxt::observe_body(self, body); }
    fn install_universals(&mut self, universals: &UniversalRegions, region_bound_pairs: &[(TyId, RegionId)]) {
        self.universal_regions = Some(universals.clone());
        self.region_bound_pairs = region_bound_pairs.to_vec();
    }
    fn implied_bounds(&mut self, sig: &SignatureRegions, nested: bool) -> (ImpliedBounds, Vec<TyId>) {
        let (tcx, prog) = (self.tcx, self.prog);
        // The environment's implicit argument only contributes bounds: it is last in the query.
        let raw: Vec<TyId> = sig.inputs.iter().copied().chain([sig.output]).chain(sig.implicit_input).collect();
        let (mut bounds, normalized) = super::implied_bounds::mir_borrowck_implied_outlives_bounds(
            tcx, prog, self.root_def_id, self.infcx.typing_mode.get(), &self.param_env_id(), nested, &sig.contract.predicates, &raw, &sig.contract.assumed_wf)
            .unwrap_or_else(|_| super::implied_bounds::query_failed(tcx, prog, self.root_def_id, &sig.contract.predicates, &raw));
        // The query's external variables are the closure's universal regions; every other placeholder is residual.
        if nested { bounds.retain_instantiated_query_bounds(tcx, &sig.query_placeholders); } else { bounds.instantiate_query_bounds(tcx, &[]); }
        (bounds, normalized)
    }
    fn normalize_signature_ty(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, span: Span) -> TyId {
        use crate::next_solver::type_op::{NormalizeSignature, ParamEnvAnd};
        let op = ParamEnvAnd { param_env: self.param_env_id(), value: NormalizeSignature { value: ty } };
        let universals = self.universal_regions.as_ref().expect("install_universals precedes signature normalization");
        match super::type_check::canonical::fully_perform_op_raw(self, universals, &self.region_bound_pairs, &self.param_env.clauses, out,
            super::type_check::Locations::All(span), span, super::constraints::ConstraintCategory::Internal, op) {
            Ok(ty) => ty,
            Err(_) => { self.tainted_by_errors.set(true); self.tcx.tys.error }
        }
    }
}

// ported from N:rustc_borrowck/src/type_check/liveness/{mod.rs,trace.rs,local_use_map.rs} @8d1a764
// (1.97.1 where the two differ: overflow spans at the first drop location, no polonius
// legacy facts). C1's Polonius hooks (deferred locals, live-region variances) and the
// shared use/drop trace; dropck runs through `LivenessOps`, whose constraints are
// registered eagerly at the drop locations.
use crate::borrowck::constraints::{ConstraintCategory, OutlivesConstraintSet, RegionVid};
use crate::borrowck::mir_nll::LocationMap;
use crate::borrowck::polonius::{self, PoloniusContext};
use crate::borrowck::def_use::{categorize, DefUse};
use crate::borrowck::universal_regions::UniversalRegions;
use crate::consts::GenericArg;
use crate::fxhash::HashMap;
use crate::mir::{*, visit::{Visitor, PlaceContext}, dataflow::{BitSet, MaybeInitializedPlaces, iterate_to_fixpoint}};
use crate::mir::move_paths::MoveData;
use crate::next_solver::type_op::QueryRegionConstraints;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{RegionId, TyId, variance::V};
use super::{Locations, MirTypeckRegionConstraints};

/// `DropckOutlivesResult` plus the region constraints of the `DropckOutlives` type op
/// (trace.rs `DropData`). Computed once per local type and environment.
#[derive(Clone, Debug, Default)]
pub struct DropckData {
    /// Everything the destructor may touch: these must be live where the local is dropped
    /// (`#[may_dangle]` parameters are not among them).
    pub kinds: Vec<GenericArg>,
    /// Types whose drop-glue computation overflowed the recursion limit.
    pub overflows: Vec<TyId>,
    /// The outlives constraints of the query, registered at every drop location.
    pub region_constraint_data: Option<QueryRegionConstraints>,
}

/// An overflow while adding drop-check rules for `ty` (E0320), reported at `span`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropckOverflow { pub span: Span, pub ty: TyId, pub overflow_ty: TyId }

/// The operations liveness needs from N01's inference context.
pub trait LivenessOps {
    /// trace.rs `compute_drop_data`: the `DropckOutlives` type op for `ty`, performed in
    /// the body's environment. A failing query reports its errors and yields
    /// `DropckData::default()` (no kinds, no constraints).
    fn dropck_outlives(&mut self, ty: TyId, span: Span) -> DropckData;
    /// `for_liveness::FreeRegionsVisitor`: the free regions of `value` that must be live
    /// where it is (components of aliases already implied by their environment are skipped).
    fn free_regions_for_liveness(&mut self, value: GenericArg) -> Vec<RegionId>;
    /// `TypeChecker::push_region_constraints`: converts the retained query constraints
    /// into `out` at `locations`.
    /// `span` is where they are blamed: the location's, or that of a nested body's requirement.
    fn push_region_constraints(&mut self, out: &mut MirTypeckRegionConstraints, locations: Locations, span: Span,
        category: ConstraintCategory, data: &QueryRegionConstraints);
}

#[derive(Default)]
struct LocalUses { defs: Vec<usize>, uses: Vec<usize>, drops: Vec<usize> }
/// The local-use map covers the relevant and the deferred locals. Initialization
/// dataflow is only solved if a drop-live query needs it.
pub struct LivenessComputation<'a> {
    prog: &'a Program, tcx: &'a Tcx, body: &'a Body, locations: &'a LocationMap, moves: &'a MoveData,
    uses: Vec<LocalUses>, predecessors: Vec<Vec<usize>>,
    init_states: Option<Vec<Option<(BitSet, BitSet)>>>,
}
/// One local's liveness: where its value may still be used, where it may only be dropped,
/// and the drops that start the latter (`drop_locations`).
pub struct LocalLiveness { pub use_live: BitSet, pub drop_live: BitSet, pub drop_locations: Vec<usize> }
impl<'a> LivenessComputation<'a> {
    /// The use map of every local.
    pub fn new(prog: &'a Program, tcx: &'a Tcx, body: &'a Body, locations: &'a LocationMap, moves: &'a MoveData) -> Self {
        Self::for_locals(prog, tcx, body, locations, moves, |_| true)
    }
    /// LocalUseMap::build: appearances are only recorded for the locals asked about.
    pub fn for_locals(prog: &'a Program, tcx: &'a Tcx, body: &'a Body, locations: &'a LocationMap, moves: &'a MoveData,
        wanted: impl Fn(Local) -> bool) -> Self {
        struct Uses<'a, F> { map: &'a LocationMap, wanted: F, locals: Vec<LocalUses> }
        impl<F: Fn(Local) -> bool> Visitor for Uses<'_, F> {
            fn visit_local(&mut self, local: &Local, context: PlaceContext, at: Location) {
                if !(self.wanted)(*local) { return; }
                let Some(kind) = categorize(context) else { return; };
                let row = &mut self.locals[local.0 as usize];
                let points = match kind { DefUse::Def => &mut row.defs, DefUse::Use => &mut row.uses, DefUse::Drop => &mut row.drops };
                points.push(self.map.point(at));
            }
        }
        let mut uses = Uses { map: locations, wanted, locals: (0..body.local_decls.len()).map(|_| LocalUses::default()).collect() };
        for (b, block) in body.basic_blocks.iter().enumerate() { uses.visit_basic_block_data(BasicBlock(b as u32), block); }
        Self { prog, tcx, body, locations, moves, uses: uses.locals, predecessors: locations.predecessors(body),
            init_states: None }
    }
    pub fn drops(&self, local: Local) -> &[usize] { &self.uses[local.0 as usize].drops }
    pub fn compute(&mut self, local: Local) -> (BitSet, BitSet) {
        let l = self.compute_full(local);
        (l.use_live, l.drop_live)
    }
    pub fn compute_full(&mut self, local: Local) -> LocalLiveness {
        let mut use_live = BitSet::new_empty(self.locations.num_points());
        let mut drop_live = use_live.clone();
        let mut drop_locations = vec![];
        let row = &self.uses[local.0 as usize];
        let mut defs = BitSet::new_empty(self.locations.num_points());
        for &p in &row.defs { defs.insert(p); }
        let mut stack = row.uses.clone();
        while let Some(p) = stack.pop() {
            if defs.contains(p) || !use_live.insert(p) { continue; }
            stack.extend(&self.predecessors[p]);
        }
        if let Some(path) = self.moves.rev_lookup.find_local(local) {
            // LocalUseMap's appearance lists are LIFO: nightly sees the last drop first.
            let drops = row.drops.clone();
            for p in drops.into_iter().rev() {
                if self.initialized_at(p, path, false) && drop_live.insert(p) { drop_locations.push(p); stack.push(p); }
            }
            while let Some(p) = stack.pop() {
                for q in self.predecessors[p].clone() {
                    if defs.contains(q) || use_live.contains(q) { continue; }
                    // Check initializedness only at predecessor block exits. A
                    // move within a block was already a use-live boundary.
                    if self.locations.location(p).statement_index == 0 && !self.initialized_at(q, path, true) { continue; }
                    if drop_live.insert(q) { stack.push(q); }
                }
            }
        }
        LocalLiveness { use_live, drop_live, drop_locations }
    }
    fn initialized_at(&mut self, point: usize, path: usize, after: bool) -> bool {
        let states = self.init_states.get_or_insert_with(|| {
            let analysis = MaybeInitializedPlaces::new(self.prog, self.tcx, self.body, self.moves);
            let results = iterate_to_fixpoint(self.body, &analysis);
            let mut cursor = results.cursor();
            self.body.basic_blocks.iter().enumerate().map(|(b, block)| {
                let block_id = BasicBlock(b as u32);
                if !results.is_reachable(block_id) { return None; }
                let at = Location { block: block_id, statement_index: block.statements.len() };
                cursor.seek_before_primary_effect(at); let before = cursor.get().clone();
                cursor.seek_after_primary_effect(at);
                Some((before, cursor.get().clone()))
            }).collect()
        });
        let at = self.locations.location(point);
        let Some((before, exit)) = &states[at.block.0 as usize] else { return false; };
        let state = if after { exit } else { before };
        let mut initialized = false;
        crate::mir::drop_flag_effects::on_all_children_bits(self.moves, path, &mut |p| initialized |= state.contains(p));
        initialized
    }
}

/// What `generate` works on: the type checker's constraints so far (N:mod.rs `typeck`).
pub struct LivenessCx<'a> {
    pub tcx: &'a Tcx,
    pub prog: &'a Program,
    pub infcx: &'a mut dyn LivenessOps,
    pub body: &'a Body,
    pub moves: &'a MoveData,
    pub universals: &'a UniversalRegions,
    pub locations: &'a LocationMap,
    /// `borrow_set.len() != 0`: loan liveness is the only consumer of deferred liveness.
    pub has_loans: bool,
    pub constraints: &'a mut MirTypeckRegionConstraints,
    pub polonius: &'a mut PoloniusContext,
    pub overflows: &'a mut Vec<DropckOverflow>,
}

/// liveness/mod.rs:29 `generate`: combines liveness analysis with initialization analysis to
/// determine which regions are live at which points, due to ordinary uses and to drops.
pub fn generate(mut cx: LivenessCx<'_>) {
    // Universal regions are live at every point.
    for region in cx.universals.universal_regions_iter() { cx.constraints.liveness_constraints.add_all_points(region); }

    let free_regions = regions_that_outlive_free_regions(cx.universals, &cx.constraints.outlives_constraints);
    let (relevant_live_locals, boring_locals) = compute_relevant_live_locals(&cx, &free_regions);

    // Under Polonius, locals containing regions that outlive universal regions are relevant and
    // only locals containing solely universal regions are boring; their liveness is only needed
    // when propagating loans, so it is deferred until the localized graph traversal reaches them.
    let deferred_locals: Vec<Local> = if !cx.has_loans { vec![] } else {
        let mut boring_nll_locals = BitSet::new_empty(cx.body.local_decls.len());
        for l in &boring_locals { boring_nll_locals.insert(l.0 as usize); }
        let deferred: Vec<Local> = cx.body.local_decls.iter().enumerate().filter_map(|(i, decl)| {
            let local = Local(i as u32);
            // The polonius-relevant locals are the ones whose types do not only contain universal regions.
            (boring_nll_locals.contains(i)
                && !all_free_regions_meet(&cx, decl.ty, &|r| cx.universals.is_universal_region(r))).then_some(local)
        }).collect();
        cx.polonius.boring_nll_locals = boring_locals.clone();
        deferred
    };

    trace(&mut cx, &relevant_live_locals, &boring_locals, &deferred_locals);

    // Regions that should be live where they appear within rvalues or within a call:
    // args, regions and types.
    record_regular_live_regions(&mut cx);
}

/// `tcx.all_free_regions_meet`: every free region of `ty` (as a vid) satisfies `pred`.
fn all_free_regions_meet(cx: &LivenessCx<'_>, ty: TyId, pred: &dyn Fn(RegionVid) -> bool) -> bool {
    polonius::regions_in(cx.tcx, cx.universals, ty).into_iter().all(pred)
}

/// The subset of locals for which we need to do a liveness computation: those with some region
/// in their type that is not known to outlive a free region (it may be valid for just a part of
/// the body). Returns (relevant, boring).
fn compute_relevant_live_locals(cx: &LivenessCx<'_>, free_regions: &[bool]) -> (Vec<Local>, Vec<Local>) {
    let (mut relevant, mut boring) = (vec![], vec![]);
    for (i, decl) in cx.body.local_decls.iter().enumerate() {
        if all_free_regions_meet(cx, decl.ty, &|r| free_regions.get(r).copied().unwrap_or(false)) { boring.push(Local(i as u32)) } else { relevant.push(Local(i as u32)) }
    }
    (relevant, boring)
}

/// All regions currently known to outlive free regions: the outlives constraints force them
/// to be live over the whole body, so no liveness is computed for them. A depth-first search
/// from the universal regions over the reverse constraint graph (`R1: R2` is the edge `R2 -> R1`).
fn regions_that_outlive_free_regions(universals: &UniversalRegions, constraints: &OutlivesConstraintSet) -> Vec<bool> {
    let num = constraints.outlives.iter().flat_map(|c| [c.sup, c.sub]).chain(universals.universal_regions_iter()).max().map_or(0, |m| m + 1);
    let graph = constraints.reverse_graph(num);
    let mut outlives_free_region = vec![false; num];
    let mut stack: Vec<_> = universals.universal_regions_iter().collect();
    for &r in &stack { outlives_free_region[r] = true; }
    while let Some(sub_region) = stack.pop() {
        for c in graph.outgoing_edges_from_graph(sub_region, constraints) {
            if !std::mem::replace(&mut outlives_free_region[c.sup], true) { stack.push(c.sup); }
        }
    }
    outlives_free_region
}

/// trace.rs:47 `trace`: for each variable X that requires a liveness computation, a reverse DFS
/// from its uses finds the USE-LIVE points (every region of its type is live there); a second
/// search from its drops, stopping at uses and definitions, finds the DROP-LIVE points, where
/// the regions of the `dropck_outlives` result (respecting `#[may_dangle]`) are live.
fn trace(cx: &mut LivenessCx<'_>, relevant: &[Local], boring: &[Local], deferred: &[Local]) {
    // The use map also covers the deferred locals: their liveness is computed later, from the
    // same map, when the loan liveness traversal first reaches one of their regions.
    let mut covered = vec![false; cx.body.local_decls.len()];
    for l in relevant.iter().chain(deferred) { covered[l.0 as usize] = true; }
    let mut comp = LivenessComputation::for_locals(cx.prog, cx.tcx, cx.body, cx.locations, cx.moves, |l| covered[l.0 as usize]);
    let mut drop_data: HashMap<TyId, DropckData> = HashMap::default();
    for &local in relevant { compute_for_local(cx, &mut comp, &mut drop_data, local); }
    for &local in boring {
        dropck_boring_local(cx, &mut comp, &mut drop_data, local, deferred.contains(&local));
    }
}

fn compute_for_local(cx: &mut LivenessCx<'_>, comp: &mut LivenessComputation<'_>, drop_data: &mut HashMap<TyId, DropckData>, local: Local) {
    let live = comp.compute_full(local);
    let local_ty = cx.body.local_decls[local.0 as usize].ty;
    // #160670: variance comes from the whole local type, also for drops.
    if !live.use_live.is_empty() || !live.drop_live.is_empty() {
        polonius::record_live_region_variance(cx.tcx, cx.prog, &mut cx.polonius.live_region_variances, cx.universals, local_ty);
    }
    if !live.use_live.is_empty() { make_all_regions_live(cx, GenericArg::Type(local_ty), &live.use_live); }
    if live.drop_live.is_empty() { return; }
    let local_span = cx.body.local_decls[local.0 as usize].source_info.span;
    let first_drop = comp.locations.location(live.drop_locations[0]);
    let drop_span = super::source_info(cx.body, first_drop).span;
    let data = dropck_local(cx, drop_data, local_ty, local_span);
    // The destructor's own outlives requirements hold at every drop.
    if let Some(constraints) = &data.region_constraint_data {
        for &p in &live.drop_locations {
            let at = comp.locations.location(p);
            cx.infcx.push_region_constraints(cx.constraints, Locations::Single(at), super::source_info(cx.body, at).span, ConstraintCategory::Boring, constraints);
        }
    }
    report_overflows(cx, &data, drop_span, local_ty);
    // All things in the `outlives` array may be touched by the destructor and must be live here.
    for &kind in &data.kinds { make_all_regions_live(cx, kind, &live.drop_live); }
}

/// Runs dropck for locals whose liveness isn't relevant. This is necessary to eagerly detect
/// unbounded recursion during drop glue computation. These are all the locals which do not
/// potentially reference a region local to this body.
fn dropck_boring_local(cx: &mut LivenessCx<'_>, comp: &mut LivenessComputation<'_>, drop_data: &mut HashMap<TyId, DropckData>,
    local: Local, is_deferred: bool) {
    let local_ty = cx.body.local_decls[local.0 as usize].ty;
    let local_span = cx.body.local_decls[local.0 as usize].source_info.span;
    // Had this been "relevant", `compute_for_local` would have skipped dropck entirely for a
    // local without drop-liveness; drops are cheap to look for.
    if is_deferred && comp.drops(local).is_empty() {
        let tcx = cx.tcx;
        cx.polonius.deferred_liveness.defer_local(tcx, cx.universals, local, local_ty, vec![]);
        return;
    }
    // Dropck runs for *all* boring locals because overflows are reported.
    let data = dropck_local(cx, drop_data, local_ty, local_span);
    report_overflows(cx, &data, local_span, local_ty);
    if !is_deferred { return; }
    // A deferred local whose dropck returned constraints registers them only where it is
    // drop-live, which requires its liveness now: it is treated as eager.
    if data.region_constraint_data.is_some() {
        compute_for_local(cx, comp, drop_data, local);
        return;
    }
    // Otherwise the local is deferred together with its drop components.
    let drop_regions = drop_regions_of(cx, &data);
    let tcx = cx.tcx;
    cx.polonius.deferred_liveness.defer_local(tcx, cx.universals, local, local_ty, drop_regions);
}

fn drop_regions_of(cx: &mut LivenessCx<'_>, data: &DropckData) -> Vec<RegionVid> {
    let mut vids = vec![];
    for &kind in &data.kinds {
        for r in cx.infcx.free_regions_for_liveness(kind) {
            let vid = cx.universals.to_region_vid(&cx.tcx.tys, r);
            if !vids.contains(&vid) { vids.push(vid); }
        }
    }
    vids
}

fn dropck_local(cx: &mut LivenessCx<'_>, cache: &mut HashMap<TyId, DropckData>, ty: TyId, span: Span) -> DropckData {
    cache.entry(ty).or_insert_with(|| cx.infcx.dropck_outlives(ty, span)).clone()
}

/// `DropckOutlivesResult::report_overflows`.
fn report_overflows(cx: &mut LivenessCx<'_>, data: &DropckData, span: Span, ty: TyId) {
    if let Some(&overflow_ty) = data.overflows.first() {
        let overflow = DropckOverflow { span, ty, overflow_ty };
        if !cx.overflows.contains(&overflow) { cx.overflows.push(overflow); }
    }
}

/// Stores the result that all regions in `value` are live for the points `live_at`
/// (mod.rs:280 `make_all_regions_live`), and their variances for Polonius.
fn make_all_regions_live(cx: &mut LivenessCx<'_>, value: GenericArg, live_at: &BitSet) {
    for r in cx.infcx.free_regions_for_liveness(value) {
        let vid = cx.universals.to_region_vid(&cx.tcx.tys, r);
        for p in live_at.iter() { cx.constraints.liveness_constraints.add_point(vid, p); }
    }
}

/// Some variables are "regular live" at a location -- they may be used later. All regions
/// appearing in args, regions and types of an rvalue or a call are live there.
fn record_regular_live_regions(cx: &mut LivenessCx<'_>) {
    struct Regular<'a> { tcx: &'a Tcx, prog: &'a Program, universals: &'a UniversalRegions,
        liveness: &'a mut crate::borrowck::region_infer::values::LivenessValues, polonius: &'a mut PoloniusContext,
        map: &'a LocationMap, point: usize }
    impl Visitor for Regular<'_> {
        fn visit_statement(&mut self, s: &Statement, at: Location) { self.point = self.map.point(at); self.super_statement(s, at); }
        fn visit_terminator(&mut self, t: &Terminator, at: Location) { self.point = self.map.point(at); self.super_terminator(t, at); }
        fn visit_ty(&mut self, ty: &TyId) { self.record(*ty, V::Co); }
        fn visit_aggregate_kind(&mut self, kind: &AggregateKind) {
            match kind {
                AggregateKind::Adt(_, _, args, _, _) | AggregateKind::Closure(_, args) | AggregateKind::Coroutine(_, args) | AggregateKind::CoroutineClosure(_, args) => {
                    for &arg in args { self.record_arg(arg); }
                }
                AggregateKind::Array(ty) | AggregateKind::RawPtr(ty, _) => self.record(*ty, V::Co),
                AggregateKind::Tuple => {}
            }
        }
        fn visit_const(&mut self, value: &Const, _: Location) {
            match value {
                Const::Val(_, ty) | Const::Ty(ty, _) => self.record(*ty, V::Co),
                Const::Unevaluated { args, .. } => { for &arg in args { self.record_arg(arg); } }
            }
        }
        fn visit_region(&mut self, region: &Region) {
            self.liveness.add_point(region.0 as usize, self.point);
            polonius::record_region_variance(&mut self.polonius.live_region_variances, region.0 as usize, V::Co);
        }
    }
    impl Regular<'_> {
        fn record_arg(&mut self, arg: GenericArg) {
            match arg {
                GenericArg::Type(ty) => self.record(ty, V::Inv),
                GenericArg::Const(c) => self.record(self.tcx.tys.intern(crate::types::TyKind::Const(c)), V::Inv),
                GenericArg::Lifetime(r) => {
                    if let Some(r) = polonius::region_vid(self.tcx, self.universals, r) {
                        self.liveness.add_point(r, self.point);
                        polonius::record_region_variance(&mut self.polonius.live_region_variances, r, V::Inv);
                    }
                }
            }
        }
        fn record(&mut self, ty: TyId, variance: V) {
            for r in polonius::regions_in(self.tcx, self.universals, ty) { self.liveness.add_point(r, self.point); }
            polonius::record_variance(self.tcx, self.prog, &mut self.polonius.live_region_variances, self.universals, ty, variance);
        }
    }
    let (tcx, prog, universals, locations, body) = (cx.tcx, cx.prog, cx.universals, cx.locations, cx.body);
    let mut regular = Regular { tcx, prog, universals, liveness: &mut cx.constraints.liveness_constraints, polonius: &mut *cx.polonius,
        map: locations, point: 0 };
    for (b, block) in body.basic_blocks.iter().enumerate() { regular.visit_basic_block_data(BasicBlock(b as u32), block); }
}


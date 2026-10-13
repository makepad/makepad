// ported from N:rustc_borrowck/src/type_check/liveness/{mod.rs,trace.rs,local_use_map.rs} @8d1a764
// C1's Polonius hooks and the shared use/drop trace. C-NLL5 supplies normalized
// dropck_outlives region vids and eagerly registers the associated constraints.
use crate::borrowck::{def_use::{categorize, DefUse}, mir_nll::{LocationMap, RegionInput}, polonius};
use crate::mir::{*, visit::{Visitor, PlaceContext}, dataflow::{BitSet, MaybeInitializedPlaces, iterate_to_fixpoint}};
use crate::mir::move_paths::MoveData;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{TyId, variance::V};

#[derive(Default)]
struct LocalUses { defs: Vec<usize>, uses: Vec<usize>, drops: Vec<usize> }
/// The local-use map covers both eager and deferred locals. Initialization
/// dataflow is only solved if a drop-live query needs it.
pub struct LivenessComputation<'a> {
    prog: &'a Program, tcx: &'a Tcx, body: &'a Body, locations: &'a LocationMap, moves: &'a MoveData,
    uses: Vec<LocalUses>, predecessors: Vec<Vec<usize>>,
    init_states: Option<Vec<Option<(BitSet, BitSet)>>>,
}
impl<'a> LivenessComputation<'a> {
    pub fn new(prog: &'a Program, tcx: &'a Tcx, body: &'a Body, locations: &'a LocationMap, moves: &'a MoveData) -> Self {
        struct Uses<'a> { map: &'a LocationMap, locals: Vec<LocalUses> }
        impl Visitor for Uses<'_> {
            fn visit_local(&mut self, local: &Local, context: PlaceContext, at: Location) {
                let Some(kind) = categorize(context) else { return; };
                let row = &mut self.locals[local.0 as usize];
                let points = match kind { DefUse::Def => &mut row.defs, DefUse::Use => &mut row.uses, DefUse::Drop => &mut row.drops };
                points.push(self.map.point(at));
            }
        }
        let mut uses = Uses { map: locations, locals: (0..body.local_decls.len()).map(|_| LocalUses::default()).collect() };
        for (b, block) in body.basic_blocks.iter().enumerate() { uses.visit_basic_block_data(BasicBlock(b as u32), block); }
        Self { prog, tcx, body, locations, moves, uses: uses.locals, predecessors: locations.predecessors(body),
            init_states: None }
    }
    pub fn compute(&mut self, local: Local) -> (BitSet, BitSet) {
        let mut use_live = BitSet::new_empty(self.locations.num_points());
        let mut drop_live = use_live.clone();
        let row = &self.uses[local.0 as usize];
        let mut defs = BitSet::new_empty(self.locations.num_points());
        for &p in &row.defs { defs.insert(p); }
        let mut stack = row.uses.clone();
        while let Some(p) = stack.pop() {
            if defs.contains(p) || !use_live.insert(p) { continue; }
            stack.extend(&self.predecessors[p]);
        }
        if let Some(path) = self.moves.rev_lookup.find_local(local) {
            let drops = row.drops.clone();
            for p in drops {
                if self.initialized_at(p, path, false) && drop_live.insert(p) { stack.push(p); }
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
        (use_live, drop_live)
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

/// Populate the MIR region input. `drop_regions` must be the normalized,
/// may_dangle-aware dropck result for EVERY local (empty for trivial drops).
/// C-NLL5 owns normalization and eager dropck constraints; no type reconstruction.
pub fn generate(tcx: &Tcx, prog: &Program, body: &Body, moves: &MoveData, input: &mut RegionInput, drop_regions: &[Vec<usize>]) {
    assert_eq!(drop_regions.len(), body.local_decls.len());
    let locations = LocationMap::new(body);
    let universals = &input.relations.universal_regions;
    let mut free = BitSet::new_empty(input.definitions.len());
    for r in universals.universal_regions_iter() { free.insert(r); input.liveness.add_all_points(r); }
    loop {
        let mut changed = false;
        for c in &input.outlives.outlives { if free.contains(c.sub) { changed |= free.insert(c.sup); } }
        if !changed { break; }
    }
    let mut comp = LivenessComputation::new(prog, tcx, body, &locations, moves);
    for (i, decl) in body.local_decls.iter().enumerate() {
        let regions = polonius::regions_in(tcx, universals, decl.ty);
        if regions.iter().all(|&r| free.contains(r)) {
            if regions.iter().any(|&r| !universals.is_universal_region(r)) {
                input.polonius.deferred_liveness.defer_local(tcx, universals, Local(i as u32), decl.ty, drop_regions[i].clone());
            }
            continue;
        }
        let (use_live, drop_live) = comp.compute(Local(i as u32));
        if !use_live.is_empty() || !drop_live.is_empty() {
            polonius::record_live_region_variance(tcx, prog, &mut input.polonius.live_region_variances, universals, decl.ty);
        }
        for r in regions { for p in use_live.iter() { input.liveness.add_point(r, p); } }
        for &r in &drop_regions[i] { for p in drop_live.iter() { input.liveness.add_point(r, p); } }
    }
    struct Regular<'a> { tcx: &'a Tcx, prog: &'a Program, input: &'a mut RegionInput, map: &'a LocationMap, point: usize }
    impl Visitor for Regular<'_> {
        fn visit_statement(&mut self, s: &Statement, at: Location) { self.point = self.map.point(at); self.super_statement(s, at); }
        fn visit_terminator(&mut self, t: &Terminator, at: Location) { self.point = self.map.point(at); self.super_terminator(t, at); }
        fn visit_ty(&mut self, ty: &TyId) { self.record(*ty, V::Co); }
        fn visit_aggregate_kind(&mut self, kind: &AggregateKind) {
            match kind {
                AggregateKind::Adt(_, _, args, _, _) | AggregateKind::Closure(_, args) | AggregateKind::Coroutine(_, args) => {
                    for &arg in args { self.record_arg(arg); }
                }
                AggregateKind::Array(ty) | AggregateKind::RawPtr(ty, _) => self.record(*ty, V::Co),
                AggregateKind::Tuple => {}
            }
        }
        fn visit_const(&mut self, value: &Const, _: Location) {
            match value {
                Const::Val(_, ty) => self.record(*ty, V::Co),
                Const::Unevaluated { args, .. } => { for &arg in args { self.record_arg(arg); } }
            }
        }
        fn visit_region(&mut self, region: &Region) {
            self.input.liveness.add_point(region.0 as usize, self.point);
            polonius::record_region_variance(&mut self.input.polonius.live_region_variances, region.0 as usize, V::Co);
        }
    }
    impl Regular<'_> {
        fn record_arg(&mut self, arg: crate::consts::GenericArg) {
            use crate::consts::GenericArg;
            match arg {
                GenericArg::Type(ty) => self.record(ty, V::Inv),
                GenericArg::Const(c) => self.record(self.tcx.tys.intern(crate::types::TyKind::Const(c)), V::Inv),
                GenericArg::Lifetime(r) => {
                    if let Some(r) = polonius::region_vid(self.tcx, &self.input.relations.universal_regions, r) {
                        self.input.liveness.add_point(r, self.point);
                        polonius::record_region_variance(&mut self.input.polonius.live_region_variances, r, V::Inv);
                    }
                }
            }
        }
        fn record(&mut self, ty: TyId, variance: V) {
            let universals = &self.input.relations.universal_regions;
            for r in polonius::regions_in(self.tcx, universals, ty) { self.input.liveness.add_point(r, self.point); }
            polonius::record_variance(self.tcx, self.prog, &mut self.input.polonius.live_region_variances, universals, ty, variance);
        }
    }
    let mut regular = Regular { tcx, prog, input, map: &locations, point: 0 };
    for (b, block) in body.basic_blocks.iter().enumerate() { regular.visit_basic_block_data(BasicBlock(b as u32), block); }
}

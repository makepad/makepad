//! rustc_mir_dataflow/src/impls/initialized.rs:21,139,229,315,367,504,624.
use super::*;
use crate::mir::drop_flag_effects::*;
use crate::mir::move_paths::{InitKind, InitLocation, LookupResult, MoveData};
use crate::tcx::Tcx;
use crate::types::TyKind;
use crate::program::Program;

/// rustc initialized.rs:25. Recognize exactly the MIR discriminant assignment
/// immediately preceding SwitchInt; do not infer enum facts from arbitrary integers.
struct SwitchData { enum_place: Place, variants: Vec<VariantIdx> }
impl SwitchData {
    fn new(prog: &Program, tcx: &Tcx, body: &Body, block: BasicBlock, targets: &SwitchTargets, discr: &Operand) -> Option<Self> {
        let (Operand::Copy(discr) | Operand::Move(discr)) = discr else { return None; };
        let stmt = body.basic_blocks[block.0 as usize].statements.last()?;
        let StatementKind::Assign(lhs, Rvalue::Discriminant(enum_place)) = &stmt.kind else { return None; };
        if lhs != discr { return None; }
        let enum_ty = enum_place.ty(body, tcx).expect("invalid enum place").ty;
        // Relax represents coroutine state in the closure type; it has no ADT variants.
        let TyKind::Adt(def, _, _) = tcx.tys.kind(enum_ty) else {
            assert!(matches!(tcx.tys.kind(enum_ty), TyKind::Closure(..)), "discriminant of non-enum");
            return None;
        };
        let adt = &tcx.adts[def];
        assert!(adt.is_enum());
        let discriminants = adt.discriminants(tcx, prog);
        let mut variants = Vec::new();
        let mut index = 0;
        for &value in &targets.values {
            while discriminants[index].1.val != value { index += 1; }
            variants.push(VariantIdx(index as u32));
        }
        Some(Self { enum_place: enum_place.clone(), variants })
    }
    fn inactive(&self, index: SwitchTargetIndex, otherwise: bool) -> Option<InactiveVariants<'_>> {
        match index {
            SwitchTargetIndex::Normal(i) => Some(InactiveVariants::Active(self.variants[i])),
            SwitchTargetIndex::Otherwise if otherwise => Some(InactiveVariants::Inactives(&self.variants)),
            _ => None,
        }
    }
}

pub struct MaybeInitializedPlaces<'a> {
    prog: &'a Program, tcx: &'a Tcx, body: &'a Body, move_data: &'a MoveData,
    precise_enum_drop_elaboration: bool, exclude_inactive_in_otherwise: bool, skip_unreachable_unwind: bool,
}
impl<'a> MaybeInitializedPlaces<'a> {
    pub fn new(prog: &'a Program, tcx: &'a Tcx, body: &'a Body, move_data: &'a MoveData) -> Self {
        Self { prog, tcx, body, move_data, precise_enum_drop_elaboration: true,
            exclude_inactive_in_otherwise: false, skip_unreachable_unwind: false }
    }
    /// Explicit analysis option replaces rustc's session option (default true).
    pub fn precise_enum_drop_elaboration(mut self, enabled: bool) -> Self { self.precise_enum_drop_elaboration = enabled; self }
    pub fn exclude_inactive_in_otherwise(mut self) -> Self { self.exclude_inactive_in_otherwise = true; self }
    pub fn skipping_unreachable_unwind(mut self) -> Self { self.skip_unreachable_unwind = true; self }
    pub fn is_unwind_dead(&self, place: &Place, state: &BitSet) -> bool {
        let LookupResult::Exact(path) = self.move_data.rev_lookup.find(place) else { return false; };
        let mut live = false;
        on_all_children_bits(self.move_data, path, &mut |child| live |= state.contains(child));
        !live
    }
    fn effects(&self, state: &mut BitSet, loc: Location) {
        drop_flag_effects_for_location(self.body, self.move_data, loc, &mut |path, flag| {
            match flag { DropFlagState::Present => { state.insert(path); }, DropFlagState::Absent => { state.remove(path); } }
        });
    }
}
impl Analysis for MaybeInitializedPlaces<'_> {
    fn bottom_is_unreachable(&self) -> bool { true }
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(self.move_data.move_paths.len()) }
    fn initialize_start_block(&self, _: &Body, state: &mut BitSet) {
        drop_flag_effects_for_function_entry(self.body, self.move_data, &mut |path, _| { state.insert(path); });
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, loc: Location) {
        self.effects(state, loc);
        if self.precise_enum_drop_elaboration {
            if let StatementKind::Assign(_, Rvalue::Ref(_, BorrowKind::Mut { .. }, place) | Rvalue::RawPtr(_, place)) = &stmt.kind {
                on_lookup_result_bits(self.move_data, self.move_data.rev_lookup.find(place), &mut |path| { state.insert(path); });
            }
        }
    }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, loc: Location) -> TerminatorEdges<'a> {
        let mut edges = TerminatorEdges::for_terminator(term);
        if self.skip_unreachable_unwind {
            if let TerminatorKind::Drop { place, target, unwind: UnwindAction::Cleanup(_), .. } = &term.kind {
                if self.is_unwind_dead(place, state) { edges = TerminatorEdges::Single(*target); }
            }
        }
        self.effects(state, loc);
        edges
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, _: BasicBlock, places: CallReturnPlaces<'_>) {
        let (CallReturnPlaces::Call(place) | CallReturnPlaces::Yield(place)) = places;
        on_lookup_result_bits(self.move_data, self.move_data.rev_lookup.find(place), &mut |path| { state.insert(path); });
    }
    fn has_switch_int_edge_effect(&self, block: BasicBlock, targets: &SwitchTargets, discr: &Operand) -> bool {
        self.precise_enum_drop_elaboration && SwitchData::new(self.prog, self.tcx, self.body, block, targets, discr).is_some()
    }
    fn apply_switch_int_edge_effect(&self, state: &mut BitSet, block: BasicBlock, targets: &SwitchTargets, discr: &Operand, index: SwitchTargetIndex) {
        let data = SwitchData::new(self.prog, self.tcx, self.body, block, targets, discr).unwrap();
        if let Some(inactive) = data.inactive(index, self.exclude_inactive_in_otherwise) {
            on_all_inactive_variants(self.move_data, &data.enum_place, inactive, &mut |path| { state.remove(path); });
        }
    }
}

pub struct MaybeUninitializedPlaces<'a> {
    prog: &'a Program, tcx: &'a Tcx, body: &'a Body, move_data: &'a MoveData,
    precise_enum_drop_elaboration: bool, mark_inactive_variants_as_uninit: bool,
    include_inactive_in_otherwise: bool, skip_unreachable_unwind: BitSet,
}
impl<'a> MaybeUninitializedPlaces<'a> {
    pub fn new(prog: &'a Program, tcx: &'a Tcx, body: &'a Body, move_data: &'a MoveData) -> Self {
        Self { prog, tcx, body, move_data, precise_enum_drop_elaboration: true,
            mark_inactive_variants_as_uninit: false, include_inactive_in_otherwise: false,
            skip_unreachable_unwind: BitSet::new_empty(body.basic_blocks.len()) }
    }
    pub fn precise_enum_drop_elaboration(mut self, enabled: bool) -> Self { self.precise_enum_drop_elaboration = enabled; self }
    pub fn mark_inactive_variants_as_uninit(mut self) -> Self { self.mark_inactive_variants_as_uninit = true; self }
    pub fn include_inactive_in_otherwise(mut self) -> Self { self.include_inactive_in_otherwise = true; self }
    pub fn skipping_unreachable_unwind(mut self, blocks: BitSet) -> Self {
        assert_eq!(blocks.domain_size(), self.body.basic_blocks.len());
        self.skip_unreachable_unwind = blocks; self
    }
    fn effects(&self, state: &mut BitSet, loc: Location) {
        drop_flag_effects_for_location(self.body, self.move_data, loc, &mut |path, flag| {
            match flag { DropFlagState::Present => { state.remove(path); }, DropFlagState::Absent => { state.insert(path); } }
        });
    }
}
impl Analysis for MaybeUninitializedPlaces<'_> {
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(self.move_data.move_paths.len()) }
    fn initialize_start_block(&self, _: &Body, state: &mut BitSet) {
        state.insert_all();
        drop_flag_effects_for_function_entry(self.body, self.move_data, &mut |path, _| { state.remove(path); });
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, _: &Statement, loc: Location) { self.effects(state, loc); }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, loc: Location) -> TerminatorEdges<'a> {
        self.effects(state, loc);
        if self.skip_unreachable_unwind.contains(loc.block.0 as usize) {
            let TerminatorKind::Drop { target, unwind: UnwindAction::Cleanup(_), .. } = &term.kind else { panic!("dead unwind must be a cleanup Drop"); };
            TerminatorEdges::Single(*target)
        } else { TerminatorEdges::for_terminator(term) }
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, _: BasicBlock, places: CallReturnPlaces<'_>) {
        let (CallReturnPlaces::Call(place) | CallReturnPlaces::Yield(place)) = places;
        on_lookup_result_bits(self.move_data, self.move_data.rev_lookup.find(place), &mut |path| { state.remove(path); });
    }
    fn has_switch_int_edge_effect(&self, block: BasicBlock, targets: &SwitchTargets, discr: &Operand) -> bool {
        self.precise_enum_drop_elaboration && self.mark_inactive_variants_as_uninit
            && SwitchData::new(self.prog, self.tcx, self.body, block, targets, discr).is_some()
    }
    fn apply_switch_int_edge_effect(&self, state: &mut BitSet, block: BasicBlock, targets: &SwitchTargets, discr: &Operand, index: SwitchTargetIndex) {
        let data = SwitchData::new(self.prog, self.tcx, self.body, block, targets, discr).unwrap();
        if let Some(inactive) = data.inactive(index, self.include_inactive_in_otherwise) {
            on_all_inactive_variants(self.move_data, &data.enum_place, inactive, &mut |path| { state.insert(path); });
        }
    }
}

/// The domain is Init indices, not MovePath indices. StorageDead ends the root's
/// initialization history; moving a value out does not erase its history.
pub struct EverInitializedPlaces<'a> { body: &'a Body, move_data: &'a MoveData }
impl<'a> EverInitializedPlaces<'a> {
    pub fn new(body: &'a Body, move_data: &'a MoveData) -> Self { Self { body, move_data } }
}
impl Analysis for EverInitializedPlaces<'_> {
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(self.move_data.inits.len()) }
    fn initialize_start_block(&self, _: &Body, state: &mut BitSet) {
        for (index, init) in self.move_data.inits.iter().enumerate() {
            if matches!(init.location, InitLocation::Argument(_)) { state.insert(index); }
        }
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, loc: Location) {
        for &init in &self.move_data.init_loc_map[loc] { state.insert(init); }
        if let StatementKind::StorageDead(local) = stmt.kind {
            if let Some(path) = self.move_data.rev_lookup.find_local(local) {
                for &init in &self.move_data.init_path_map[path] { state.remove(init); }
            }
        }
    }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, loc: Location) -> TerminatorEdges<'a> {
        for &index in &self.move_data.init_loc_map[loc] {
            if self.move_data.inits[index].kind != InitKind::NonPanicPathOnly { state.insert(index); }
        }
        TerminatorEdges::for_terminator(term)
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, block: BasicBlock, _: CallReturnPlaces<'_>) {
        let loc = Location { block, statement_index: self.body.basic_blocks[block.0 as usize].statements.len() };
        for &init in &self.move_data.init_loc_map[loc] { state.insert(init); }
    }
}

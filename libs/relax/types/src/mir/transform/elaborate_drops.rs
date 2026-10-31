//! Body drop elaboration: rustc_mir_transform/src/elaborate_drops.rs:48,99,
//! 169,276,295,346,433,445,473,489. Queries fail before the patch is applied.
use super::{DropError, DropProvider};
use super::elaborate_drop::*;
use super::patch::MirPatch;
use crate::mir::*;
use crate::mir::dataflow::*;
use crate::mir::drop_flag_effects::*;
use crate::mir::move_paths::{LookupResult, MoveData};
use std::cell::RefCell;
use crate::mir::visit::Visitor;

struct BodyElaborator<'a> {
    data: &'a MoveData,
    inits: &'a BitSet,
    uninits: &'a BitSet,
    flags: &'a [Option<Local>],
    bool_ty: TyId,
}
impl DropElaborator for BodyElaborator<'_> {
    fn drop_style(&self, path: usize, mode: DropFlagMode) -> DropStyle {
        let mut init = false; let mut uninit = false; let mut count = 0;
        let mut visit = |p| { init |= self.inits.contains(p); uninit |= self.uninits.contains(p); count += 1; };
        match mode { DropFlagMode::Shallow => visit(path), DropFlagMode::Deep => on_all_children_bits(self.data, path, &mut visit) }
        match (init, uninit, count > 1) {
            (false, _, _) => DropStyle::Dead,
            (true, false, _) => DropStyle::Static,
            (true, true, false) => DropStyle::Conditional,
            (true, true, true) => DropStyle::Open,
        }
    }
    fn drop_flag(&self, path: usize) -> Option<Local> { self.flags[path] }
    fn clear_drop_flag(&self, patch: &mut MirPatch, loc: Location, path: usize, mode: DropFlagMode) {
        let mut clear = |p| set_flag(patch, self.flags, self.bool_ty, loc, p, DropFlagState::Absent);
        match mode { DropFlagMode::Shallow => clear(path), DropFlagMode::Deep => on_all_children_bits(self.data, path, &mut clear) }
    }
    fn subpath(&self, path: usize, projection: &PlaceElem) -> Option<usize> {
        move_path_children_matching(self.data, path, &mut |elem| match (elem, projection) {
            (PlaceElem::Field(a, _), PlaceElem::Field(b, _)) => a == b,
            (PlaceElem::Downcast(_, a), PlaceElem::Downcast(_, b)) => a == b,
            (PlaceElem::Deref, PlaceElem::Deref) => true,
            (PlaceElem::ConstantIndex { offset: a, min_length: al, from_end: false },
             PlaceElem::ConstantIndex { offset: b, min_length: bl, from_end: false }) => { assert_eq!(al, bl); a == b }
            _ => false,
        })
    }
}
fn set_flag(patch: &mut MirPatch, flags: &[Option<Local>], bool_ty: TyId, loc: Location, path: usize, state: DropFlagState) {
    if let Some(flag) = flags[path] {
        patch.statement(loc, StatementKind::Assign(Place::local(flag), Rvalue::Use(integer(bool_ty,
            state.value() as u128, 1, Span::default()))));
    }
}
fn term_loc(body: &Body, bb: BasicBlock) -> Location {
    Location { block: bb, statement_index: body.basic_blocks[bb.0 as usize].statements.len() }
}

/// The input must have completed analysis cleanup (no borrowck-only operations).
/// Built bodies with no such operations are useful for hand-built MIR clients.
/// Remaining Drops mean calls to type glue, including virtual glue for dyn.
pub fn elaborate_drops(body: &mut Body, provider: &dyn DropProvider) -> Result<(), DropError> {
    if matches!(body.phase, MirPhase::Runtime(_)) { return Err(DropError::InvalidMir("drops already elaborated")); }
    if body.coroutine.is_some() { return Err(DropError::Unsupported("coroutine drop")); }
    if !validate::validate(body, provider.tcx()).is_empty() { return Err(DropError::InvalidMir("invalid input body")); }
    for block in &body.basic_blocks {
        for stmt in &block.statements {
            if matches!(stmt.kind, StatementKind::FakeRead(..) | StatementKind::AscribeUserType(..) | StatementKind::PlaceMention(_)
                | StatementKind::Assign(_, Rvalue::Ref(_, BorrowKind::Fake(_), _))) {
                return Err(DropError::InvalidMir("analysis cleanup required before drop elaboration"));
            }
        }
        if matches!(block.terminator.as_ref().unwrap().kind, TerminatorKind::FalseEdge { .. } | TerminatorKind::FalseUnwind { .. } | TerminatorKind::Yield { .. } | TerminatorKind::CoroutineDrop | TerminatorKind::InlineAsm(_)) {
            return Err(DropError::Unsupported("analysis-only or unsupported terminator"));
        }
    }
    // M4a2's type filter is infallible. Capture an error solely to abort the
    // whole gather; no partial result or assumed query answer escapes.
    let error = RefCell::new(None);
    let data = MoveData::gather_moves(body, provider.program(), provider.tcx(), provider.typing_env(), &|ty| {
        match provider.needs_drop(ty) {
            Ok(value) => value,
            Err(e) => { *error.borrow_mut() = Some(e); false }
        }
    });
    if let Some(e) = error.into_inner() { return Err(e); }
    let init = MaybeInitializedPlaces::new(provider.program(), provider.tcx(), body, &data).exclude_inactive_in_otherwise().skipping_unreachable_unwind();
    let initial = iterate_to_fixpoint(body, &init);
    let mut cursor = initial.cursor();
    let mut dead_unwinds = BitSet::new_empty(body.basic_blocks.len());
    for (i, block) in body.basic_blocks.iter().enumerate() {
        let bb = BasicBlock(i as u32);
        if !initial.is_reachable(bb) { continue; }
        if let TerminatorKind::Drop { place, unwind: UnwindAction::Cleanup(_), .. } = &block.terminator.as_ref().unwrap().kind {
            cursor.seek_before_primary_effect(term_loc(body, bb));
            if init.is_unwind_dead(place, cursor.get()) { dead_unwinds.insert(i); }
        }
    }
    let inits = initial;
    let uninit = MaybeUninitializedPlaces::new(provider.program(), provider.tcx(), body, &data)
        .include_inactive_in_otherwise().mark_inactive_variants_as_uninit().skipping_unreachable_unwind(dead_unwinds);
    let uninits = iterate_to_fixpoint(body, &uninit);
    let mut ic = inits.cursor(); let mut uc = uninits.cursor();
    let mut patch = MirPatch::new(body);
    let bool_ty = provider.tcx().tys.bool_;
    let mut flags = vec![None; data.move_paths.len()];
    for (i, block) in body.basic_blocks.iter().enumerate() {
        let bb = BasicBlock(i as u32);
        if !inits.is_reachable(bb) { continue; }
        let term = block.terminator.as_ref().unwrap();
        let TerminatorKind::Drop { place, .. } = &term.kind else { continue; };
        ic.seek_before_primary_effect(term_loc(body, bb)); uc.seek_before_primary_effect(term_loc(body, bb));
        match data.rev_lookup.find(place) {
            LookupResult::Exact(path) => on_all_children_bits(&data, path, &mut |p| {
                if ic.get().contains(p) && uc.get().contains(p) && flags[p].is_none() { flags[p] = Some(patch.temp(bool_ty, term.source_info)); }
            }),
            LookupResult::Parent(Some(parent)) if !matches!(body.local_decls[place.local.0 as usize].local_info, LocalInfo::DerefTemp) => {
                if uc.get().contains(parent) { return Err(DropError::InvalidMir("drop of uninitialized untracked place")); }
            }
            _ => (),
        }
    }
    for (i, block) in body.basic_blocks.iter().enumerate() {
        let bb = BasicBlock(i as u32);
        let term = block.terminator.as_ref().unwrap();
        let TerminatorKind::Drop { place, target, unwind, replace } = &term.kind else { continue; };
        let ty = place.ty(body, provider.tcx()).ok_or(DropError::InvalidMir("invalid drop place"))?.ty;
        if !inits.is_reachable(bb) || !provider.needs_drop(ty)? {
            patch.terminator(bb, TerminatorKind::Goto { target: *target }); continue;
        }
        match data.rev_lookup.find(place) {
            LookupResult::Exact(path) => {
                ic.seek_before_primary_effect(term_loc(body, bb)); uc.seek_before_primary_effect(term_loc(body, bb));
                let unwind = if block.is_cleanup { Unwind::InCleanup } else { Unwind::To(patch.exit(*unwind, term.source_info)) };
                let policy = BodyElaborator { data: &data, inits: ic.get(), uninits: uc.get(), flags: &flags, bool_ty };
                let mut cx = DropCtxt { body, provider, elaborator: &policy, patch: &mut patch };
                cx.elaborate(&DropSite { place: place.clone(), path, succ: *target, unwind, source: term.source_info }, bb)?;
            }
            LookupResult::Parent(Some(_)) => {
                if !replace || block.is_cleanup { return Err(DropError::InvalidMir("untracked drop must be a replacement")); }
                // Borrow checking guarantees the old pointee is initialized.
            }
            LookupResult::Parent(None) => (),
        }
    }
    let entry = Location { block: BasicBlock(0), statement_index: 0 };
    for flag in &flags {
        if let Some(flag) = flag { patch.statement(entry, StatementKind::Assign(Place::local(*flag),
            Rvalue::Use(integer(bool_ty, 0, 1, Span::default())))); }
    }
    // CriticalCallEdges (rustc add_call_guards.rs:31): return effects must
    // belong to the call's edge, not another predecessor of its successor.
    // These identity blocks can be queued after analysis in the same patch.
    let mut preds = Predecessors(vec![0; body.basic_blocks.len()]);
    for (i, block) in body.basic_blocks.iter().enumerate() {
        preds.visit_terminator(block.terminator.as_ref().unwrap(), term_loc(body, BasicBlock(i as u32)));
    }
    for (i, block) in body.basic_blocks.iter().enumerate() {
        let term = block.terminator.as_ref().unwrap();
        if let TerminatorKind::Call { destination, target: Some(target), unwind: UnwindAction::Cleanup(_), .. } = &term.kind {
            let mut target = *target;
            if preds.0[target.0 as usize] > 1 {
                target = patch.block(term.source_info, block.is_cleanup, Vec::new(), TerminatorKind::Goto { target });
                let mut kind = term.kind.clone();
                let TerminatorKind::Call { target: slot, .. } = &mut kind else { unreachable!() };
                *slot = Some(target);
                patch.terminator(BasicBlock(i as u32), kind);
            }
            let loc = Location { block: target, statement_index: 0 };
            on_lookup_result_bits(&data, data.rev_lookup.find(destination), &mut |p| set_flag(&mut patch, &flags, bool_ty, loc, p, DropFlagState::Present));
        }
    }
    drop_flag_effects_for_function_entry(body, &data, &mut |p, state| set_flag(&mut patch, &flags, bool_ty, entry, p, state));
    for (i, block) in body.basic_blocks.iter().enumerate() {
        let bb = BasicBlock(i as u32);
        for j in 0..=block.statements.len() {
            if j == block.statements.len() && matches!(block.terminator.as_ref().unwrap().kind, TerminatorKind::Drop { .. }) { continue; }
            let loc = Location { block: bb, statement_index: j };
            drop_flag_effects_for_location(body, &data, loc, &mut |p, state| set_flag(&mut patch, &flags, bool_ty, loc, p, state));
        }
        if let TerminatorKind::Call { destination, target: Some(_), unwind, .. } = &block.terminator.as_ref().unwrap().kind {
            if !matches!(unwind, UnwindAction::Cleanup(_)) {
                let loc = term_loc(body, bb);
                on_lookup_result_bits(&data, data.rev_lookup.find(destination), &mut |p| set_flag(&mut patch, &flags, bool_ty, loc, p, DropFlagState::Present));
            }
        }
    }
    patch.apply(body);
    body.phase = MirPhase::Runtime(RuntimePhase::Initial);
    Ok(())
}

struct Predecessors(Vec<usize>);
impl Visitor for Predecessors {
    fn visit_target(&mut self, target: &BasicBlock, _: bool, _: Location) { self.0[target.0 as usize] += 1; }
}

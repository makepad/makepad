//! rustc_mir_transform/src/coroutine.rs `mir_coroutine_witnesses` (coroutine.rs:1389) and
//! `locals_live_across_suspend_points` (coroutine.rs:~675): the witness of a coroutine is the types of
//! the locals live across any suspension point. Borrowck's dropck asks whether any of them needs drop.
use crate::mir::dataflow::{iterate_to_fixpoint, BitSet, MaybeBorrowedLocals, MaybeLiveLocals, MaybeRequiresStorage};
use crate::mir::*;
use crate::program::Program;
use crate::tcx::{Tcx, TypingEnv};

/// The locals live across any suspension point (`live_locals_at_any_suspension_point`), the coroutine
/// argument excluded. A local is saved if it is used after the suspension point, or if it is live and has
/// been previously borrowed; either way only while its storage is required.
pub fn locals_live_across_suspend_points(body: &Body, movable: bool) -> BitSet {
    let n = body.local_decls.len();
    // The MIR locals that have been previously borrowed (even if they are still active).
    let borrowed_locals = iterate_to_fixpoint(body, &MaybeBorrowedLocals);
    let mut borrowed_locals_cursor2 = borrowed_locals.cursor();
    // The MIR locals that need their storage kept around.
    let requires_storage_analysis = MaybeRequiresStorage::new(borrowed_locals.cursor());
    let requires_storage = iterate_to_fixpoint(body, &requires_storage_analysis);
    let mut requires_storage_cursor = requires_storage.cursor();
    // The liveness of MIR locals ignoring borrows.
    let liveness = iterate_to_fixpoint(body, &MaybeLiveLocals);
    let mut liveness_cursor = liveness.cursor();

    let mut live_locals_at_any_suspension_point = BitSet::new_empty(n);
    for (b, data) in body.basic_blocks.iter().enumerate() {
        let block = BasicBlock(b as u32);
        let Some(Terminator { kind: TerminatorKind::Yield { .. }, .. }) = &data.terminator else { continue };
        // (an unreachable suspension point has the bottom state: nothing is live there)
        if !liveness.is_reachable(block) { continue; }
        let loc = Location { block, statement_index: data.statements.len() };
        liveness_cursor.seek_to_block_end(block);
        let mut live_locals = liveness_cursor.get().clone();
        if !movable {
            // Borrows cannot live across suspension points of a movable coroutine; an immovable one
            // conservatively keeps every borrowed local until its StorageDead.
            borrowed_locals_cursor2.seek_before_primary_effect(loc);
            live_locals.union(borrowed_locals_cursor2.get());
        }
        requires_storage_cursor.seek_before_primary_effect(loc);
        live_locals.intersect(requires_storage_cursor.get());
        // The coroutine argument is ignored.
        live_locals.remove(1);
        live_locals_at_any_suspension_point.union(&live_locals);
    }
    live_locals_at_any_suspension_point
}

/// Whether any type of the coroutine's witness needs drop (dropck_outlives.rs: `witness.field_tys.iter().any(needs_drop)`
/// in the typing environment with erased regions). The body is the analysis MIR (after promotion).
pub fn coroutine_witness_needs_drop(tcx: &Tcx, prog: &Program, env: TypingEnv, body: &Body) -> bool {
    let movable = body.coroutine.as_ref().is_some_and(|c| c.kind.movability() == Movability::Movable);
    let saved = locals_live_across_suspend_points(body, movable);
    saved.iter().any(|local| tcx.needs_drop(prog, env, body.local_decls[local].ty))
}

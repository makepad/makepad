// ported from N:rustc_borrowck/src/used_muts.rs @8d1a764
use crate::mir::*;
use crate::mir::dataflow::BitSet;
use crate::mir::move_paths::MoveData;
use crate::mir::visit::{MutatingUseContext as W, PlaceContext, Visitor};

pub(super) fn gather_used_muts(body: &Body, moves: &MoveData, used: &mut BitSet) {
    let mut temporary = BitSet::new_empty(body.local_decls.len());
    let mut never_initialized = BitSet::new_empty(body.local_decls.len());
    for (i, decl) in body.local_decls.iter().enumerate() {
        let user = decl.kind == LocalKind::UserVar || matches!(decl.local_info,LocalInfo::User { .. });
        if used.contains(i) && !user { temporary.insert(i); }
        if !used.contains(i) && user && i > body.arg_count as usize && decl.mutability == Mutability::Mut {
            never_initialized.insert(i);
        }
    }
    struct Gather<'a> { temporary: BitSet, never: &'a mut BitSet, used: &'a mut BitSet, moves: &'a MoveData }
    impl Visitor for Gather<'_> {
        fn visit_statement(&mut self, stmt: &Statement, at: Location) {
            if let StatementKind::Assign(place, _) = &stmt.kind { self.never.remove(place.local.0 as usize); }
            self.super_statement(stmt, at);
        }
        fn visit_terminator(&mut self, term: &Terminator, at: Location) {
            if let TerminatorKind::Call { destination, .. } = &term.kind { self.never.remove(destination.local.0 as usize); }
            self.super_terminator(term, at);
        }
        fn visit_local(&mut self, local: &Local, ctx: PlaceContext, at: Location) {
            if matches!(ctx, PlaceContext::MutatingUse(W::Store | W::Call | W::Yield | W::AsmOutput))
                && self.temporary.contains(local.0 as usize) {
                for &index in &self.moves.loc_map[at] {
                    let place = &self.moves.move_paths[self.moves.moves[index].path].place;
                    if place.projection.is_empty() { self.used.insert(place.local.0 as usize); }
                }
            }
        }
    }
    let mut gather = Gather { temporary, never: &mut never_initialized, used, moves };
    // Promoted bodies have separate local indices.
    for (b, data) in body.basic_blocks.iter().enumerate() { gather.visit_basic_block_data(BasicBlock(b as u32), data); }
    used.union(&never_initialized);
}

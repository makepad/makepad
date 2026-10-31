// ported from N:rustc_borrowck/src/borrow_set.rs @8d1a764
// STOP: M0 lacks Rvalue::Reborrow and PlaceElem::PhantomDeref; gather_reborrows
// and the multi-borrow insertion path wait for arch's MIR representation.
use super::place_ext::ignore_borrow;
use crate::fxhash::HashMap;
use crate::mir::dataflow::BitSet;
use crate::mir::move_paths::MoveData;
use crate::mir::visit::{MutatingUseContext, NonUseContext, PlaceContext, Visitor};
use crate::mir::*;
use crate::tcx::Tcx;

pub type BorrowIndex = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwoPhaseActivation {
    NotTwoPhase,
    NotActivated,
    ActivatedAt(Location),
}

#[derive(Clone, Debug)]
pub struct BorrowData {
    pub reserve_location: Location,
    pub activation_location: TwoPhaseActivation,
    pub kind: BorrowKind,
    pub region: Region,
    pub borrowed_place: Place,
    pub assigned_place: Place,
}

pub enum LocalsStateAtExit {
    AllAreInvalidated,
    SomeAreInvalidated { has_storage_dead_or_moved: BitSet },
}

impl LocalsStateAtExit {
    fn build(invalidated: bool, body: &Body, move_data: &MoveData) -> Self {
        if invalidated {
            return Self::AllAreInvalidated;
        }
        struct HasStorageDead(BitSet);
        impl Visitor for HasStorageDead {
            fn visit_local(&mut self, local: &Local, ctx: PlaceContext, _: Location) {
                if ctx == PlaceContext::NonUse(NonUseContext::StorageDead) {
                    self.0.insert(local.0 as usize);
                }
            }
        }
        let mut visitor = HasStorageDead(BitSet::new_empty(body.local_decls.len()));
        // Relax's visit_body also descends into promoted bodies, which have separate locals.
        for (i, block) in body.basic_blocks.iter().enumerate() {
            visitor.visit_basic_block_data(BasicBlock(i as u32), block);
        }
        for moved in &move_data.moves {
            visitor.0.insert(move_data.base_local(moved.path).0 as usize);
        }
        Self::SomeAreInvalidated { has_storage_dead_or_moved: visitor.0 }
    }
}

pub struct BorrowSet {
    borrows: Vec<BorrowData>,
    // Location has no Hash impl in M0; use its two existing components as the key.
    location_map: HashMap<(BasicBlock, usize), Vec<BorrowIndex>>,
    activation_map: HashMap<(BasicBlock, usize), Vec<BorrowIndex>>,
    local_map: HashMap<Local, Vec<BorrowIndex>>,
    locals_state_at_exit: LocalsStateAtExit,
}

impl BorrowSet {
    pub fn build(tcx: &Tcx, body: &Body, locals_are_invalidated_at_exit: bool, move_data: &MoveData) -> Self {
        let mut visitor = GatherBorrows {
            tcx,
            body,
            pending_activations: HashMap::default(),
            worklist: vec![BasicBlock(0)],
            set: Self {
                borrows: Vec::new(),
                location_map: HashMap::default(),
                activation_map: HashMap::default(),
                local_map: HashMap::default(),
                locals_state_at_exit: LocalsStateAtExit::build(locals_are_invalidated_at_exit, body, move_data),
            },
        };
        // Inline rustc mir/traversal.rs preorder; successor enumeration uses M0's visitor.
        let mut visited = BitSet::new_empty(body.basic_blocks.len());
        while let Some(block) = visitor.worklist.pop() {
            if visited.insert(block.0 as usize) {
                visitor.visit_basic_block_data(block, &body.basic_blocks[block.0 as usize]);
            }
        }
        visitor.set
    }
    pub fn iter(&self) -> impl Iterator<Item = &BorrowData> {
        self.borrows.iter()
    }
    pub fn iter_enumerated(&self) -> impl Iterator<Item = (BorrowIndex, &BorrowData)> {
        self.borrows.iter().enumerate()
    }
    pub fn len(&self) -> usize {
        self.borrows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.borrows.is_empty()
    }
    pub fn locals_state_at_exit(&self) -> &LocalsStateAtExit {
        &self.locals_state_at_exit
    }
    pub fn activations_at_location(&self, loc: &Location) -> &[BorrowIndex] {
        self.activation_map.get(&(loc.block, loc.statement_index)).map_or(&[], Vec::as_slice)
    }
    pub fn borrows_at_location(&self, loc: &Location) -> Option<&[BorrowIndex]> {
        self.location_map.get(&(loc.block, loc.statement_index)).map(Vec::as_slice)
    }
    pub fn borrows_on_local(&self, local: Local) -> Option<&[BorrowIndex]> {
        self.local_map.get(&local).map(Vec::as_slice)
    }
}
impl std::ops::Index<BorrowIndex> for BorrowSet {
    type Output = BorrowData;
    fn index(&self, index: BorrowIndex) -> &BorrowData {
        &self.borrows[index]
    }
}

struct GatherBorrows<'a> {
    tcx: &'a Tcx,
    body: &'a Body,
    set: BorrowSet,
    pending_activations: HashMap<Local, BorrowIndex>,
    worklist: Vec<BasicBlock>,
}
impl Visitor for GatherBorrows<'_> {
    fn visit_statement(&mut self, statement: &Statement, location: Location) {
        if let StatementKind::Assign(assigned_place, Rvalue::Ref(region, kind, borrowed_place)) = &statement.kind {
            if ignore_borrow(borrowed_place, self.tcx, self.body, &self.set.locals_state_at_exit) {
                return;
            }
            assert_ne!(*region, Region::ERASED, "borrow regions must be renumbered before gathering");
            let two_phase = *kind == BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow };
            let idx = self.set.borrows.len();
            self.set.borrows.push(BorrowData {
                reserve_location: location,
                activation_location: if two_phase { TwoPhaseActivation::NotActivated } else { TwoPhaseActivation::NotTwoPhase },
                kind: *kind,
                region: *region,
                borrowed_place: borrowed_place.clone(),
                assigned_place: assigned_place.clone(),
            });
            assert!(self.set.location_map.insert((location.block, location.statement_index), vec![idx]).is_none());
            if two_phase {
                assert!(assigned_place.projection.is_empty(), "two-phase borrow must assign to a local");
                assert!(self.pending_activations.insert(assigned_place.local, idx).is_none(), "multiple two-phase borrows for temporary");
            }
            self.set.local_map.entry(borrowed_place.local).or_default().push(idx);
        }
        self.super_statement(statement, location);
    }
    fn visit_local(&mut self, temp: &Local, context: PlaceContext, location: Location) {
        if !context.is_use() {
            return;
        }
        let Some(&index) = self.pending_activations.get(temp) else {
            return;
        };
        let data = &mut self.set.borrows[index];
        if data.reserve_location == location && context == PlaceContext::MutatingUse(MutatingUseContext::Store) {
            return;
        }
        assert_eq!(data.activation_location, TwoPhaseActivation::NotActivated, "multiple uses of two-phase temporary");
        self.set.activation_map.entry((location.block, location.statement_index)).or_default().push(index);
        data.activation_location = TwoPhaseActivation::ActivatedAt(location);
    }
    fn visit_target(&mut self, target: &BasicBlock, _: bool, _: Location) {
        self.worklist.push(*target);
    }
    fn visit_terminator(&mut self, term: &Terminator, location: Location) {
        assert!(!matches!(term.kind, TerminatorKind::InlineAsm(_)), "M0 has no inline-asm operands");
        self.super_terminator(term, location);
    }
}

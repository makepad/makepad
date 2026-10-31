// ported from N:rustc_borrowck/src/{borrowck_errors,session_diagnostics}.rs @8d1a764
use crate::mir::{BorrowKind, Location, Place, Region, Span};

/// Only error kinds, primary spans and short notes. Rendering belongs to C-NLL6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Uninitialized, Moved, Reassigned, MultipleMutableBorrows, ConflictingBorrow,
    UseWhileMutablyBorrowed, MoveWhileBorrowed, AssignWhileBorrowed,
    AssignImmutable, BorrowImmutable, DoesNotLiveLongEnough, TemporaryDropped,
    MoveBorrowedContent, MoveIndexed, MoveDropType, BorrowAcrossYield,
    ThreadLocalEscapes, MutateInGuard, TwoClosures, ClosureRequiresUnique, AlreadyUniquelyBorrowed,
    BorrowAcrossDestructor, ReturnLocalReference, EscapingClosure, UnsizedMove, BorrowEscapes,
    SimdIntrinsicArgConst,
}
impl ErrorKind {
    pub fn code(self) -> &'static str {
        use ErrorKind::*;
        match self {
            Uninitialized => "E0381", Moved => "E0382", Reassigned => "E0384",
            MultipleMutableBorrows => "E0499", ConflictingBorrow => "E0502",
            UseWhileMutablyBorrowed => "E0503", MoveWhileBorrowed => "E0505",
            AssignWhileBorrowed => "E0506", AssignImmutable => "E0594", BorrowImmutable => "E0596",
            DoesNotLiveLongEnough => "E0597", TemporaryDropped => "E0716",
            MoveBorrowedContent => "E0507", MoveIndexed => "E0508", MoveDropType => "E0509",
            BorrowAcrossYield => "E0626", ThreadLocalEscapes => "E0712", MutateInGuard => "E0510",
            TwoClosures => "E0524", ClosureRequiresUnique => "E0500", AlreadyUniquelyBorrowed => "E0501",
            BorrowAcrossDestructor => "E0713", ReturnLocalReference => "E0515", EscapingClosure => "E0373", UnsizedMove => "E0161", BorrowEscapes => "E0521",
            // N:session_diagnostics.rs:655 SimdIntrinsicArgConst has no error code.
            SimdIntrinsicArgConst => "",
        }
    }
    pub fn note(self) -> &'static str {
        use ErrorKind::*;
        match self {
            Uninitialized => "use of a possibly uninitialized value", Moved => "use of a moved value",
            Reassigned => "cannot assign twice to an immutable variable",
            MultipleMutableBorrows => "cannot borrow as mutable more than once at a time",
            ConflictingBorrow => "cannot borrow with conflicting mutability",
            UseWhileMutablyBorrowed => "cannot use a mutably borrowed value",
            MoveWhileBorrowed => "cannot move out because the value is borrowed",
            AssignWhileBorrowed => "cannot assign because the value is borrowed",
            AssignImmutable => "cannot assign through an immutable place",
            BorrowImmutable => "cannot borrow an immutable place as mutable",
            DoesNotLiveLongEnough => "borrowed value does not live long enough",
            TemporaryDropped => "temporary value dropped while borrowed",
            MoveBorrowedContent => "cannot move out of borrowed content",
            MoveIndexed => "cannot move out of an indexed value", MoveDropType => "cannot move out of a type implementing Drop",
            BorrowAcrossYield => "borrow may still be in use when coroutine yields",
            ThreadLocalEscapes => "thread-local variable borrowed past end of function",
            MutateInGuard => "cannot mutate a value in an immutable section",
            TwoClosures => "two closures require unique access at the same time",
            ClosureRequiresUnique => "closure requires unique access to a borrowed value",
            AlreadyUniquelyBorrowed => "value is already uniquely borrowed by a closure",
            BorrowAcrossDestructor => "borrow may still be in use when destructor runs",
            ReturnLocalReference => "cannot return a reference to local data",
            EscapingClosure => "closure may outlive the borrowed value", UnsizedMove => "cannot move an unsized value", BorrowEscapes => "borrowed data escapes outside its scope",
            SimdIntrinsicArgConst => "a SIMD intrinsic argument is required to be a `const` item",
        }
    }
}
/// What the checker knew when it chose the error: rustc's arguments to the report_* fns in
/// diagnostics/{conflict_errors,move_errors,mutability_errors}.rs. Rendering (messages, labels,
/// the "borrow later used here" search) happens in `diagnostics` from this and the solved regions.
#[derive(Clone, Debug, Default)]
pub enum Detail {
    #[default]
    None,
    /// E0381/E0382 (lib.rs InitializationRequiringAction + report_use_of_moved_or_uninitialized args):
    /// `moved_place` has move path `mpi`; the move-outs reaching `location` (empty: never initialized).
    Moved { action: InitAction, moved_place: Place, used_place: Place, location: Location, mpi: usize, moves: Vec<MoveSite> },
    /// E0499/E0502/E0503/E0505/E0506/E0500/E0501/E0524/E0510: the access conflicts with `borrow`.
    Conflict { place: Place, access: AccessKind, location: Location, borrow: BorrowInfo },
    /// E0597/E0713/E0716/E0712: `borrow` outlives the storage of `borrow.borrowed_place`.
    Lifetime { borrow: BorrowInfo, dropped_place: Place, location: Location },
    /// E0384/E0594/E0596: the access and the place that is not mutable (lib.rs report_mutability_error args).
    Mutability { place: Place, the_place_err: Place, access: AccessKind, location: Location, init: Option<usize> },
    /// E0626: `borrow` is live across the yield at `yield_location` (lib.rs check_movable_place).
    BorrowAcrossYield { borrow: BorrowInfo, yield_location: Location },
    /// E0507/E0508/E0509: an illegal move of `place` (diagnostics/move_errors.rs MoveError).
    IllegalMove { place: Place, location: Location },
}
/// lib.rs:1127 InitializationRequiringAction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitAction { Borrow, MatchOn, Use, Assignment, PartialAssignment }
impl InitAction {
    pub fn as_noun(self) -> &'static str {
        match self { Self::Borrow => "borrow", Self::MatchOn | Self::Use => "use", Self::Assignment => "assign", Self::PartialAssignment => "assign to part" }
    }
    pub fn as_verb_in_past_tense(self) -> &'static str {
        match self { Self::Borrow => "borrowed", Self::MatchOn => "matched on", Self::Use => "used", Self::Assignment => "assigned", Self::PartialAssignment => "partially assigned" }
    }
    pub fn as_general_verb_in_past_tense(self) -> &'static str {
        match self { Self::Borrow | Self::MatchOn | Self::Use => "used", Self::Assignment => "assigned", Self::PartialAssignment => "partially assigned" }
    }
}
/// conflict_errors.rs MoveSite: a move-out and whether the search crossed a loop back edge to find it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveSite { pub moi: usize, pub traversed_back_edge: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind { Copy, Borrow(BorrowKind), MutableBorrow(BorrowKind), Move, Mutate, Replace, StorageDeadOrDrop }
/// A copy of the BorrowData fields diagnostics need (the borrow set is not kept alive).
#[derive(Clone, Debug)]
pub struct BorrowInfo {
    pub reserve_location: Location, pub kind: BorrowKind, pub region: Region,
    pub borrowed_place: Place, pub assigned_place: Place,
}
impl From<&super::borrow_set::BorrowData> for BorrowInfo {
    fn from(b: &super::borrow_set::BorrowData) -> Self {
        Self { reserve_location: b.reserve_location, kind: b.kind, region: b.region, borrowed_place: b.borrowed_place.clone(), assigned_place: b.assigned_place.clone() }
    }
}
/// Why a conflict was accepted or narrowed by field views (relaxed Rust A1/A3): the callee or
/// closure whose summary touches only some parts of the borrowed place.
#[derive(Clone, Debug)]
pub enum FvNote {
    /// accepted with extended two-phase borrows: the borrow reserved here is only used by the call, after
    TwoPhase { reserved: Span },
    /// accepted with field views: what the borrows' callees use
    Narrowed { why: Vec<FvWho> },
    /// the conflict remains through field views: what the callees touch
    Remaining { why: Vec<FvWho> },
    /// accepted under Polonius (relaxed Rust A2): the loan is not live where NLL thought it was
    Polonius,
    /// accepted with disjoint indices (relaxed Rust D3): proven distinct, or checked at run time
    Disjoint { proven: bool },
}
#[derive(Clone, Debug)]
pub struct FvWho { pub callee: Option<crate::program::DefId>, pub is_closure: bool, pub parts: Vec<(Place, bool)> }

/// Equality is the error's identity for de-duplication: kind and primary span.
#[derive(Clone, Debug)]
pub struct BorrowckError { pub kind: ErrorKind, pub span: Span, pub detail: Detail, pub fv: Option<Box<FvNote>> }
impl PartialEq for BorrowckError { fn eq(&self, o: &Self) -> bool { self.kind == o.kind && self.span == o.span } }
impl Eq for BorrowckError {}
impl BorrowckError {
    pub fn new(kind: ErrorKind, span: Span) -> Self { Self { kind, span, detail: Detail::None, fv: None } }
    pub fn with(kind: ErrorKind, span: Span, detail: Detail) -> Self { Self { kind, span, detail, fv: None } }
    pub fn code(&self) -> &'static str { self.kind.code() }
}

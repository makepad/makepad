// ported from N:rustc_borrowck/src/{borrowck_errors,session_diagnostics}.rs @8d1a764
use crate::mir::Span;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowckError { pub kind: ErrorKind, pub span: Span }
impl BorrowckError {
    pub fn code(&self) -> &'static str { self.kind.code() }
    pub fn note(&self) -> &'static str { self.kind.note() }
}

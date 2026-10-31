// ported from N:rustc_borrowck/src/def_use.rs @8d1a764
use crate::mir::visit::{MutatingUseContext as M, NonMutatingUseContext as N, NonUseContext as U, PlaceContext};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefUse {
    Def,
    Use,
    Drop,
}

pub fn categorize(context: PlaceContext) -> Option<DefUse> {
    use PlaceContext::*;
    match context {
        MutatingUse(M::Store | M::Call | M::AsmOutput | M::Yield) | NonUse(U::StorageLive | U::StorageDead) => Some(DefUse::Def),
        MutatingUse(M::Projection | M::Borrow | M::RawBorrow)
        | NonMutatingUse(
            N::Projection | N::SharedBorrow | N::FakeBorrow | N::PlaceMention | N::RawBorrow | N::Inspect | N::Copy | N::Move,
        )
        | NonUse(U::AscribeUserTy(_)) => Some(DefUse::Use),
        MutatingUse(M::Drop) => Some(DefUse::Drop),
        NonUse(U::VarDebugInfo | U::BackwardIncompatibleDropHint) => None,
        MutatingUse(M::SetDiscriminant | M::Retag) => panic!("statement not allowed in borrowck MIR phase"),
    }
}

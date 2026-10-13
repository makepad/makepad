//! Runtime preparation passes; no production consumer is switched here.
pub mod elaborate_drops;
pub(crate) mod elaborate_drop;
pub(crate) mod patch;

use crate::mir::*;
use crate::program::Program;
use crate::tcx::{Destructor, Tcx};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropError {
    Query(TyId),
    InvalidMir(&'static str),
    Unsupported(&'static str),
}

/// Layout-free query boundary. The production needs_drop query belongs to Q1b/T0.
/// rustc elaborate_drop.rs:540,694,973; rustc_middle/src/ty/util.rs:376.
pub trait DropProvider {
    fn tcx(&self) -> &Tcx;
    fn program(&self) -> &Program;
    fn typing_env(&self) -> crate::tcx::TypingEnv;
    fn needs_drop(&self, ty: TyId) -> Result<bool, DropError>;
    fn field_ty(&self, ty: TyId, variant: Option<VariantIdx>, field: FieldIdx) -> Result<TyId, DropError> {
        PlaceTy { ty, variant_index: variant }.field_ty(self.program(), self.tcx(), self.typing_env(), field)
            .ok_or(DropError::InvalidMir("invalid drop field"))
    }
    fn adt_destructor(&self, def: DefId) -> Option<Destructor> { self.tcx().adt_destructor(def) }
    fn is_box(&self, def: DefId) -> bool { self.tcx().adts[&def].is_box() }
}

#[cfg(test)]
mod tests;

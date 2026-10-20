//! Runtime preparation passes; no production consumer is switched here.
pub mod const_check;
pub mod coroutine;
pub mod elaborate_drops;
pub(crate) mod elaborate_drop;
pub(crate) mod patch;
pub mod promote_consts;
pub mod qualifs;
pub mod simplify;

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

/// What the passes before borrowck produce besides the rewritten body.
#[derive(Debug)]
pub struct PreBorrowck {
    /// The qualifs of the return place of a const context (rustc `mir_const_qualif`); the default for
    /// a body that is not one.
    pub qualifs: qualifs::ConstQualifs,
    /// Const-checking errors, in block order (primary ones; the secondary ones only when there is no
    /// primary one).
    pub const_errors: Vec<const_check::ConstError>,
}

/// rustc_mir_transform/src/lib.rs `mir_built` (the part after building: `SimplifyCfg::Initial`),
/// `mir_const_qualif`, and `mir_promoted` (`PromoteTemps`, `SimplifyCfg::PromoteConsts`): brings built
/// MIR to the analysis MIR borrowck runs on, with `body.promoted` filled and the phase
/// `Analysis(Initial)`. A constant the queries cannot answer blocks the body (`Blocked`): it is
/// unchecked, not rejected.
pub fn mir_promoted(
    prog: &Program,
    tcx: &Tcx,
    env: crate::tcx::TypingEnv,
    provider: &dyn qualifs::ConstQualifsProvider,
    body: &mut Body,
) -> Result<PreBorrowck, qualifs::Blocked> {
    simplify::simplify_cfg(body, false);

    // Ensure that we compute the `mir_const_qualif` for constants at this point, before promotion:
    // promotion can rely on all const checks having been done.
    let checked = {
        let ccx = const_check::ConstCx::new(prog, tcx, env, body, provider);
        if ccx.const_kind.is_some() { Some(const_check::mir_const_qualif(&ccx)?) } else { None }
    };

    body.promoted = promote_consts::promote_temps(prog, tcx, env, provider, body)?;
    simplify::simplify_cfg(body, false);
    body.phase = MirPhase::Analysis(AnalysisPhase::Initial);

    Ok(match checked {
        Some(check) => PreBorrowck { qualifs: check.qualifs, const_errors: check.errors },
        None => PreBorrowck { qualifs: qualifs::ConstQualifs::default(), const_errors: Vec::new() },
    })
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

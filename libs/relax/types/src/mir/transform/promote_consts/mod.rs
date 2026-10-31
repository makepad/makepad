//! A pass that promotes borrows of constant rvalues: rustc_mir_transform/src/promote_consts.rs
//! (1.97.1), with the promotion policy of today's BIR promote.rs.
//!
//! The rvalues considered constant are trees of temps, each with exactly one initialization, and
//! holding a constant value with no interior mutability. They are placed into a new MIR constant body
//! in `promoted` and the borrow rvalue is replaced with a `Const::Unevaluated` naming the promoted
//! (`promoted: Some(index)`, no generic arguments: they are the owner's identity arguments).
//!
//! This pass assumes that every use is dominated by an initialization and can otherwise silence
//! errors, if move analysis runs after promotion on broken MIR.
//!
//! Relax-specific: statics are thread-local by their `#[thread_local]` attribute; a constant is
//! evaluated for the promotion checks through `Tcx::const_eval_resolve`; `required_consts` do not
//! exist (the promoted's constants are checked as operands of the promoted body).
use super::const_check::ConstCx;
use crate::mir::visit::{MutatingUseContext, PlaceContext, Visitor};
use crate::mir::*;
use crate::program::{DefId, DefKind, Program};

mod extract;
mod validate;
#[cfg(test)]
mod tests;

/// State of a temporary during collection and promotion.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum TempState {
    /// No references to this temp.
    Undefined,
    /// One direct assignment and any number of direct uses. A borrow of this temp is promotable if
    /// the assigned value is qualified as constant.
    Defined { location: Location, uses: usize, valid: bool },
    /// Any other combination of assignments/uses.
    Unpromotable,
    /// This temp was part of an rvalue which got extracted during promotion and needs cleanup.
    PromotedOut,
}

/// A "root candidate" for promotion, which will become the returned value in a promoted MIR, unless
/// it's a subset of a larger candidate.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Candidate {
    pub location: Location,
}

/// The statement or terminator at a location.
pub(crate) enum StmtOrTerm<'a> {
    Stmt(&'a Statement),
    Term(&'a Terminator),
}

pub(crate) fn stmt_at(body: &Body, location: Location) -> StmtOrTerm<'_> {
    let block = &body.basic_blocks[location.block.0 as usize];
    match block.statements.get(location.statement_index) {
        Some(statement) => StmtOrTerm::Stmt(statement),
        None => StmtOrTerm::Term(block.terminator.as_ref().expect("unfinished MIR block")),
    }
}

/// `tcx.is_thread_local_static(def)`: a `#[thread_local]` static.
pub(crate) fn is_thread_local_static(prog: &Program, def: DefId) -> bool {
    let d = prog.def(def);
    if !matches!(d.kind, DefKind::Static | DefKind::ForeignStatic) {
        return false;
    }
    let f = &prog.files[d.file as usize];
    f.ast.list(f.ast.item(d.item).attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "thread_local")
}

struct Collector<'a> {
    ccx: &'a ConstCx<'a>,
    temps: Vec<TempState>,
    candidates: Vec<Candidate>,
}

impl Visitor for Collector<'_> {
    fn visit_local(&mut self, index: &Local, context: PlaceContext, location: Location) {
        // We're only interested in temporaries and the return place
        let decl = &self.ccx.body.local_decls[index.0 as usize];
        match decl.kind {
            LocalKind::Arg | LocalKind::UserVar => return,
            LocalKind::Temp if decl.is_user_variable() => return,
            LocalKind::Return | LocalKind::Temp => {}
        }

        // Ignore drops, if the temp gets promoted, then it's constant and thus drop is noop.
        // Non-uses are also irrelevant.
        if context.is_drop() || !context.is_use() {
            return;
        }

        let temp = &mut self.temps[index.0 as usize];
        *temp = match *temp {
            TempState::Undefined => match context {
                PlaceContext::MutatingUse(MutatingUseContext::Store | MutatingUseContext::Call) => TempState::Defined { location, uses: 0, valid: false },
                _ => TempState::Unpromotable,
            },
            TempState::Defined { ref mut uses, .. } => {
                // We always allow borrows, even mutable ones, as we need to promote mutable borrows
                // of some ZSTs e.g., `&mut []`.
                let allowed_use = match context {
                    PlaceContext::MutatingUse(MutatingUseContext::Borrow) | PlaceContext::NonMutatingUse(_) => true,
                    PlaceContext::MutatingUse(_) | PlaceContext::NonUse(_) => false,
                };
                if allowed_use {
                    *uses += 1;
                    return;
                }
                TempState::Unpromotable
            }
            TempState::Unpromotable | TempState::PromotedOut => TempState::Unpromotable,
        };
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        self.super_rvalue(rvalue, location);

        if let Rvalue::Ref(..) = *rvalue {
            self.candidates.push(Candidate { location });
        }
    }
}

fn collect_temps_and_candidates(ccx: &ConstCx<'_>) -> (Vec<TempState>, Vec<Candidate>) {
    let mut collector = Collector { temps: vec![TempState::Undefined; ccx.body.local_decls.len()], candidates: vec![], ccx };
    for bb in super::simplify::reverse_postorder(ccx.body) {
        collector.visit_basic_block_data(bb, &ccx.body.basic_blocks[bb.0 as usize]);
    }
    (collector.temps, collector.candidates)
}

/// Promotion is the extraction of promotable temps into separate MIR bodies so they can have
/// `'static` lifetime. Returns the MIR body corresponding to each newly created `Const::Unevaluated`
/// (`body.promoted` is NOT set: the caller stores them, then runs `simplify_cfg` on the parent).
///
/// `body` must not be a promoted body itself. A constant the queries cannot answer blocks the
/// promotion of the whole body: nothing is rewritten.
pub fn promote_temps(
    prog: &Program,
    tcx: &crate::tcx::Tcx,
    env: crate::tcx::TypingEnv,
    provider: &dyn super::qualifs::ConstQualifsProvider,
    body: &mut Body,
) -> Result<Vec<Body>, super::qualifs::Blocked> {
    // There's not really any point in promoting errorful MIR.
    //
    // This does not include MIR that failed const-checking, which we still try to promote.
    if tcx.tys.flags(body.local_decls[0].ty).intersects(crate::types::TypeFlags::HAS_ERROR) {
        return Ok(Vec::new());
    }
    if matches!(body.source.kind, BodyKind::Promoted(..)) {
        return Ok(Vec::new());
    }

    let (temps, promotable_candidates) = {
        let ccx = ConstCx::new(prog, tcx, env, body, provider);
        super::qualifs::check_body_types(&ccx)?;
        let (mut temps, all_candidates) = collect_temps_and_candidates(&ccx);
        let promotable_candidates = validate::validate_candidates(&ccx, &mut temps, all_candidates);
        if let Some(blocked) = ccx.take_blocked() {
            return Err(blocked);
        }
        (temps, promotable_candidates)
    };

    Ok(extract::promote_candidates(tcx, body, temps, promotable_candidates))
}

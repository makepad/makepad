// ported from N:rustc_middle/src/mir/interpret/queries.rs @8d1a764
use crate::consts::{GenericArg, ValTree};
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::TyKind;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConstEvalKey {
    pub typing_env: TypingEnv,
    pub def: DefId,
    pub args: Arc<[GenericArg]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorHandled { TooGeneric, Reported(ValTreeCreationError) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValTreeCreationError { NonSupportedType(crate::types::TyId), NodesOverflow, CyclicConst, ErrorHandled(&'static str) }

pub type EvalToValTreeResult = Result<ValTree, ErrorHandled>;

/// queries.rs:58,90: one query identity for folding and execution:
/// (typing_env, definition, complete declaration args).
pub(crate) fn eval_to_valtree(tcx: &Tcx, prog: &Program, key: &ConstEvalKey) -> EvalToValTreeResult {
    match fold_to_valtree(tcx, prog, key, false) {
        Folded::Value(value) => value,
        Folded::Execute(key) => match tcx.const_eval_provider {
            Some(provider) => (provider.evaluate)(provider.context, key.typing_env, key.def, &key.args),
            None => Err(evaluation_error()),
        },
    }
}

enum Folded { Value(EvalToValTreeResult), Execute(ConstEvalKey) }

/// RELOAD' (lead design, round 33): the value of `def` by const folding alone, never by the
/// executable provider -- named constants it reads are folded the same way, not through the
/// query cache (a cached value may have come from running code). None: the value needs the
/// executable provider. A live reload compares discriminants with it without running pending
/// code; where it succeeds, eval_to_valtree folds to the same value.
/// A cycle (which the query engine would report) is not folded here.
pub fn fold_without_execution(tcx: &Tcx, prog: &Program, typing_env: TypingEnv, def: DefId) -> Option<EvalToValTreeResult> {
    crate::gen_local! { static ACTIVE: std::cell::RefCell<Vec<DefId>>; }
    if ACTIVE.with(|a| a.borrow().contains(&def)) { return None; }
    ACTIVE.with(|a| a.borrow_mut().push(def));
    let folded = fold_to_valtree(tcx, prog, &ConstEvalKey { typing_env, def, args: Arc::from([]) }, true);
    ACTIVE.with(|a| a.borrow_mut().pop());
    match folded {
        Folded::Value(value) => Some(value),
        Folded::Execute(_) => None,
    }
}

fn fold_to_valtree(tcx: &Tcx, prog: &Program, key: &ConstEvalKey, fold_only: bool) -> Folded {
    use crate::consts::ScalarInt;
    let resolved;
    let key = if prog.def(key.def).kind == crate::program::DefKind::AssocConst {
        let mut compact = Vec::new();
        for arg in key.args.iter() {
            match *arg {
                GenericArg::Lifetime(_) => {},
                GenericArg::Type(t) => compact.push(t),
                GenericArg::Const(c) => compact.push(tcx.tys.intern(TyKind::Const(c))),
            }
        }
        let instance = match tcx.resolve_instance_raw(prog, key.typing_env, key.def, &compact) {
            Ok(instance) => instance,
            Err(crate::next_solver::codegen::CodegenObligationError::Ambiguity) => return Folded::Value(Err(ErrorHandled::TooGeneric)),
            Err(_) => return Folded::Value(Err(evaluation_error())),
        };
        let crate::instance::InstanceKind::Item(def) = instance.def else { return Folded::Value(Err(evaluation_error())); };
        resolved = ConstEvalKey { typing_env: key.typing_env, def, args: instance.args.into() };
        &resolved
    } else { key };
    let def = prog.def(key.def);
    let Some(body) = const_body(prog, key.def) else { return Folded::Value(Err(evaluation_error())) };
    let state = crate::tcx::EvalState::new();
    state.fold_only.set(fold_only);
    let cx = crate::tcx::EvalCx::new(prog, &tcx, key.typing_env, &state);
    let env = tcx.item_env(prog, key.def);
    let mut compact = Vec::new();
    for arg in key.args.iter() {
        match *arg {
            GenericArg::Lifetime(_) => {},
            GenericArg::Type(ty) => compact.push(ty),
            GenericArg::Const(ct) => compact.push(tcx.tys.intern(TyKind::Const(ct))),
        }
    }
    state.const_generic_frame.replace(Some((env, compact)));
    let Ok(ty) = tcx.type_of(prog, key.def) else { return Folded::Value(Err(evaluation_error())) };
    let ty = tcx.instantiate_ty(prog, key.def, &key.args, ty);
    let value = crate::const_fold::eval_query_expr(&cx, prog, tcx, def.file, def.scope, body, ty);
    let value = match value {
        Some(value) => value,
        None if state.const_too_generic.get() => return Folded::Value(Err(ErrorHandled::TooGeneric)),
        None => return Folded::Execute(key.clone()),
    };
    let size = match tcx.tys.kind(ty) {
        TyKind::Int(p) => tcx.tys.int_info(*p).0 as u8,
        TyKind::Bool => 1,
        TyKind::Char => 4,
        _ => return Folded::Value(Err(ErrorHandled::Reported(ValTreeCreationError::NonSupportedType(ty)))),
    };
    Folded::Value(Ok(ValTree::from_scalar_int(&tcx.tys, ScalarInt::truncate_from_uint(value as u128, size).0)))
}

/// The compiler registers the executable-body provider when the Unit is pinned.
/// The context remains valid for the lifetime of its Tcx.
#[derive(Clone, Copy)]
pub struct ConstEvalProvider {
    pub context: usize,
    pub evaluate: fn(usize, TypingEnv, DefId, &[GenericArg]) -> EvalToValTreeResult,
}

pub fn const_body(prog: &Program, def: DefId) -> Option<relax_front::ast::ExprId> {
    use crate::program::DefKind;
    use relax_front::def_collector::NodeId;
    let d = prog.def(def);
    match d.kind {
        DefKind::AnonConst => match prog.def_id_to_node_id.get(&def) {
            Some((_, NodeId::AnonConst(expr) | NodeId::InlineConst(expr))) => Some(*expr),
            _ => None,
        },
        DefKind::Const | DefKind::AssocConst | DefKind::Static => prog.const_body(d.file, d.item).ok().flatten(),
        _ => None,
    }
}

pub fn evaluation_error() -> ErrorHandled {
    ErrorHandled::Reported(ValTreeCreationError::ErrorHandled("constant evaluation failed"))
}

#[cfg(test)]
mod tests;
pub mod interp;

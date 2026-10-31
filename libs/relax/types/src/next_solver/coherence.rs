// ported from N:rustc_next_trait_solver/src/coherence.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use crate::types::{TyKind, TypeFlags};
use crate::program::{Program, NO_DEF};
use std::ops::ControlFlow;

/// rustc_next_trait_solver/src/coherence.rs: Compat mode (orphan.rs:29-50) lets uncovered
/// type parameters inside rigid projections pass, behind the UNCOVERED_PARAM_IN_PROJECTION lint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OrphanCheckMode { Proper, Compat }
#[derive(Clone, Copy)]
pub enum InCrate { Local { mode: OrphanCheckMode }, Remote }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict { Upstream, Downstream }
#[derive(Debug)]
pub enum OrphanCheckErr {
    NonLocalInputType(Vec<(TyId,bool)>),
    UncoveredTyParams { uncovered:TyId, local_ty:Option<TyId> },
}
enum OrphanCheckEarlyExit { NormalizationFailure(NoSolutionOrRerunNonErased), UncoveredTyParam(TyId), LocalTy(TyId) }

/// The orphan check over `tr` (coherence.rs:222-257). `lazily_normalize_ty` is nightly's
/// closure parameter: it returns the shallow-resolved type and its normalization. The solver
/// (trait_ref_is_knowable) structurally normalizes in its EvalCtxt; the HIR orphan check
/// (orphan.rs:331-343) normalizes aliases in an ObligationCtxt and keeps the written alias on
/// any error.
pub fn orphan_check_trait_ref(tcx: &crate::tcx::Tcx, prog: &Program, tr: &TraitRef, local_crate:u32, in_crate:InCrate,
    lazily_normalize_ty: &mut dyn FnMut(TyId) -> (TyId, Result<TyId, NoSolutionOrRerunNonErased>)) -> Result<Result<(),OrphanCheckErr>,NoSolutionOrRerunNonErased> {
    assert!(!tr.args.iter().any(|arg| match *arg { GenericArg::Type(t) => tcx.tys.flags(t).intersects(TypeFlags::HAS_TY_PARAM), _ => false }), "orphan check only expects inference variables");
    let mut checker = OrphanChecker { tcx,prog,lazily_normalize_ty,local_crate,in_crate,in_self_ty:true,search_first_local_ty:false,non_local_tys:Vec::new() };
    Ok(match checker.visit_trait_ref(tr) {
        ControlFlow::Continue(()) => Err(OrphanCheckErr::NonLocalInputType(checker.non_local_tys)),
        ControlFlow::Break(OrphanCheckEarlyExit::NormalizationFailure(e)) => return Err(e),
        ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(_)) => Ok(()),
        ControlFlow::Break(OrphanCheckEarlyExit::UncoveredTyParam(uncovered)) => {
            checker.search_first_local_ty=true; checker.in_self_ty=true;
            let local_ty = match checker.visit_trait_ref(tr) { ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(t))=>Some(t), _=>None };
            Err(OrphanCheckErr::UncoveredTyParams { uncovered,local_ty })
        }
    })
}
pub fn trait_ref_is_knowable(ecx:&mut EvalCtxt<'_, '_>,env:&ParamEnvId,tr:&TraitRef,local_crate:u32)->Result<Result<(),Conflict>,NoSolutionOrRerunNonErased> {
    let (tcx, prog) = (ecx.delegate.tcx, ecx.solver.prog);
    let mut normalize = |ty: TyId| {
        let ty = ecx.delegate.shallow_resolve(ty);
        let normalized = ecx.structurally_normalize_term(env, Term::Ty(ty)).map(|term| match term { Term::Ty(t) => t, _ => unreachable!() });
        (ty, normalized)
    };
    if orphan_check_trait_ref(tcx,prog,tr,local_crate,InCrate::Remote,&mut normalize)?.is_ok() { return Ok(Err(Conflict::Downstream)); }
    if prog.def(tr.def_id).krate==local_crate || fundamental(prog,tr.def_id) { return Ok(Ok(())); }
    if orphan_check_trait_ref(tcx,prog,tr,local_crate,InCrate::Local { mode: OrphanCheckMode::Proper },&mut normalize)?.is_ok() { Ok(Ok(())) } else { Ok(Err(Conflict::Upstream)) }
}
fn fundamental(prog:&Program,def:DefId)->bool {
    let d=prog.def(def); let ast=&prog.files[d.file as usize].ast;
    crate::lang_items::has_attr(prog,d.file,ast.list(ast.item(d.item).attrs),"fundamental")
}
struct OrphanChecker<'a> {
    tcx:&'a crate::tcx::Tcx, prog:&'a Program,
    lazily_normalize_ty:&'a mut dyn FnMut(TyId) -> (TyId, Result<TyId, NoSolutionOrRerunNonErased>),
    local_crate:u32, in_crate:InCrate,
    in_self_ty:bool, search_first_local_ty:bool, non_local_tys:Vec<(TyId,bool)>,
}
impl OrphanChecker<'_> {
    fn visit_trait_ref(&mut self,tr:&TraitRef)->ControlFlow<OrphanCheckEarlyExit> {
        for &arg in &tr.args { if let GenericArg::Type(t)=arg { self.visit_ty(t)?; } }
        ControlFlow::Continue(())
    }
    fn def_id_is_local(&self,def:DefId)->bool { matches!(self.in_crate,InCrate::Local { .. }) && self.prog.def(def).krate==self.local_crate }
    fn found_non_local_ty(&mut self,t:TyId)->ControlFlow<OrphanCheckEarlyExit> { self.non_local_tys.push((t,self.in_self_ty)); ControlFlow::Continue(()) }
    fn found_uncovered_ty_param(&mut self,t:TyId)->ControlFlow<OrphanCheckEarlyExit> {
        if self.search_first_local_ty { ControlFlow::Continue(()) } else { ControlFlow::Break(OrphanCheckEarlyExit::UncoveredTyParam(t)) }
    }
    fn visit_ty(&mut self,ty:TyId)->ControlFlow<OrphanCheckEarlyExit> {
        let tcx=self.tcx;
        let (ty, normalized)=(self.lazily_normalize_ty)(ty);
        let ty=match normalized {
            Ok(t) if matches!(tcx.tys.kind(t),TyKind::Infer(_))=>ty,
            Ok(t)=>t,
            Err(e)=>return ControlFlow::Break(OrphanCheckEarlyExit::NormalizationFailure(e)),
        };
        let result=match tcx.tys.kind(ty) {
            TyKind::Param(_)=>panic!("orphan check expects inference variables"),
            TyKind::Placeholder(_) | TyKind::Bound(..) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)=>match self.in_crate {
                InCrate::Local { .. }=>self.found_uncovered_ty_param(ty), InCrate::Remote=>ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(ty)),
            },
            TyKind::Alias(_,alias) if tcx.tys.flags(ty).intersects(TypeFlags::HAS_TY_PLACEHOLDER | TypeFlags::HAS_TY_BOUND | TypeFlags::HAS_TY_INFER)=>match self.in_crate {
                InCrate::Local { mode: OrphanCheckMode::Compat } if matches!(alias.kind, crate::types::AliasTyKind::Projection { .. })=>ControlFlow::Continue(()),
                InCrate::Local { .. }=>self.found_uncovered_ty_param(ty), InCrate::Remote=>ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(ty)),
            },
            TyKind::Ref(_,t,_)=>self.visit_ty(*t),
            TyKind::Adt(def,args,_)=>{
                if self.def_id_is_local(*def) { ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(ty)) }
                else if fundamental(self.prog,*def) {
                    if tcx.lang_items.get(crate::lang_items::LangItem::OwnedBox)==Some(*def) { self.visit_ty(tcx.tys.list(*args)[0]) }
                    else { for &t in tcx.tys.list(*args) { if !matches!(tcx.tys.kind(t),TyKind::Const(_)) { self.visit_ty(t)?; } } ControlFlow::Continue(()) }
                } else { self.found_non_local_ty(ty) }
            }
            TyKind::Dyn(principal,..) if *principal!=NO_DEF && self.def_id_is_local(*principal)=>ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(ty)),
            TyKind::Error=>ControlFlow::Break(OrphanCheckEarlyExit::LocalTy(ty)),
            TyKind::FnDef(..) | TyKind::Closure(..)=>panic!("unnameable type in coherence"),
            _=>self.found_non_local_ty(ty),
        };
        self.in_self_ty=false; result
    }
}
impl crate::tcx::Tcx {
    /// rustc_hir_analysis/src/coherence/orphan.rs:305-385 (orphan_check) in `mode`.
    pub fn orphan_check_impl(&self,prog:&Program,imp:DefId,mode:OrphanCheckMode)->Result<(),OrphanCheckErr> {
        let local_crate=prog.def(imp).krate;
        let mut infcx=crate::infer::InferCtxt::for_item(self, prog, crate::tcx::TypingMode::Coherence, imp);
        infcx.coherence_crate=Some(local_crate);
        let args=infcx.fresh_args_for_item(prog,imp);
        let tr=self.impl_trait_ref(prog,imp,&args);
        if prog.def(tr.def_id).krate==local_crate { return Ok(()); }
        // orphan.rs:331-343: aliases normalize in the empty environment; any error keeps the alias.
        let mut normalize = |user_ty: TyId| {
            let user_ty = infcx.shallow_resolve(user_ty);
            if !matches!(self.tys.kind(user_ty), TyKind::Alias(..)) { return (user_ty, Ok(user_ty)); }
            let mut ocx = super::fulfill::ObligationCtxt::new(&infcx, prog);
            let ty = ocx.normalize_ty(&ParamEnvId::EMPTY, user_ty);
            let ty = infcx.deeply_resolve_ignoring_regions(ty);
            if !ocx.try_evaluate_obligations().is_empty() { return (user_ty, Ok(user_ty)); }
            (user_ty, Ok(ty))
        };
        match orphan_check_trait_ref(self,prog,&tr,local_crate,InCrate::Local { mode },&mut normalize) {
            Ok(result)=>result,
            Err(_)=>unreachable!("the HIR orphan check's normalizer never fails"),
        }
    }
}

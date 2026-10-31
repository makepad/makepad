// ported from N:rustc_trait_selection/src/traits/coherence.rs @8d1a764
use super::*;
use super::fast_reject::DeepRejectCtxt;
use crate::infer::{InferCtxt, relate::{relate_args, solver_relating::SolverRelating}};
use crate::program::Program;
use crate::tcx::{Tcx, TypingMode};
use crate::types::{variance::V, UniverseIndex};

struct ImplHeader { self_ty: TyId, trait_ref: Option<TraitRef>, predicates: Vec<Predicate> }
pub struct OverlapResult { pub overflowing_predicates: Vec<Predicate> }

fn fresh_impl_header(infcx: &InferCtxt<'_>, prog: &Program, def: DefId, is_trait: bool) -> ImplHeader {
    let tcx=infcx.tcx;
    let args=infcx.fresh_args_for_item(prog,def);
    let self_ty=tcx.instantiate_ty(prog,def,&args,tcx.impl_self[&def]);
    let trait_ref=is_trait.then(|| tcx.impl_trait_ref(prog,def,&args));
    let mut predicates=Vec::new();
    for p in tcx.clauses_of(prog,def) { predicates.push(tcx.instantiate_predicate(prog,def,&args,&p)); }
    ImplHeader { self_ty, trait_ref, predicates }
}
fn equate_impl_headers(infcx: &InferCtxt<'_>, prog: &Program, a: &ImplHeader, b: &ImplHeader) -> Option<Vec<Goal>> {
    match (&a.trait_ref,&b.trait_ref) {
        (Some(a),Some(b)) => {
            if a.def_id!=b.def_id { return None; }
            let mut relation=SolverRelating::new(infcx,prog,V::Inv,ParamEnvId::EMPTY);
            relate_args(&mut relation,&a.args,&b.args).ok()?;
            Some(relation.goals)
        }
        (None,None) => infcx.at(prog,ParamEnvId::EMPTY).eq(a.self_ty,b.self_ty).ok(),
        _ => panic!("mismatched impl kinds"),
    }
}
/// coherence.rs:327-375: depth-eight probe, definite errors, then ambiguities.
/// Ambiguity (including overflow) cannot establish disjointness.
fn impl_intersection_has_impossible_obligation(infcx: &InferCtxt<'_>, prog: &Program, goals: &[Goal]) -> Option<Vec<Predicate>> {
    for goal in goals { if !infcx.root_goal_may_hold_with_depth(prog,8,goal.clone()) { return None; } }
    let mut ocx=fulfill::ObligationCtxt::new(infcx,prog);
    ocx.register_obligations(goals.to_vec());
    let errors=ocx.try_evaluate_obligations();
    for e in &errors { if matches!(e,fulfill::NextSolverError::TrueError(_)) { return None; } }
    let pending=ocx.engine.pending_obligations();
    let mut ocx=fulfill::ObligationCtxt::new(infcx,prog);
    for obligation in pending { ocx.engine.register_predicate_obligation(infcx,obligation); }
    let mut overflowing=Vec::new();
    for error in errors.into_iter().chain(ocx.engine.evaluate_obligations_error_on_ambiguity(infcx,prog)) {
        match error {
            fulfill::NextSolverError::TrueError(_) => panic!("second overlap pass produced a true error"),
            fulfill::NextSolverError::Overflow(o) => overflowing.push(o.goal.predicate),
            fulfill::NextSolverError::Ambiguity(o) => {
                // Minimal BestObligation diagnostics: retain overflow on the pending root.
                if infcx.evaluate_root_goal(prog,o.goal.clone(),None).is_ok_and(|r| r.certainty.is_overflow()) { overflowing.push(o.goal.predicate); }
            }
        }
    }
    Some(overflowing)
}
/// coherence.rs:88-246. Only OverlapMode::Stable is exposed: explicit negative
/// coherence and specialization are cut feature-gated mechanisms (round 34).
pub fn overlapping_impls(tcx: &Tcx, prog: &Program, a: DefId, b: DefId, krate: u32, skip_leak_check: bool, is_trait: bool) -> Option<OverlapResult> {
    let reject=DeepRejectCtxt::relate_infer_infer(&tcx.tys);
    if is_trait {
        let ar=tcx.impl_trait_ref(prog,a,&tcx.generic_indices_of(prog,a).identity_args(&tcx.tys));
        let br=tcx.impl_trait_ref(prog,b,&tcx.generic_indices_of(prog,b).identity_args(&tcx.tys));
        if !reject.args_may_unify(&ar.args,&br.args) { return None; }
    } else if !reject.types_may_unify(tcx.impl_self[&a],tcx.impl_self[&b]) { return None; }
    let mut infcx=InferCtxt::for_item(tcx,prog,TypingMode::Coherence,b);
    infcx.coherence_crate=Some(krate);
    infcx.skip_leak_check=skip_leak_check;
    let a=fresh_impl_header(&infcx,prog,a,is_trait);
    let b=fresh_impl_header(&infcx,prog,b,is_trait);
    let mut goals=equate_impl_headers(&infcx,prog,&a,&b)?;
    for p in a.predicates.into_iter().chain(b.predicates) { goals.push(Goal { param_env: ParamEnvId::default(), predicate:p }); }
    let overflowing_predicates=impl_intersection_has_impossible_obligation(&infcx,prog,&goals)?;
    infcx.leak_check(UniverseIndex::ROOT,None).ok()?;
    // The overlap header, placeholder note and intercrate proof-tree visitor only
    // enrich diagnostics; the minimal E0119/E0592 report retains overflow context.
    Some(OverlapResult { overflowing_predicates })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(source:&str,label:&str)->(Program,Tcx,Vec<String>) {
        let dir=std::env::temp_dir().join(format!("relax-round34-{label}-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();let path=dir.join("lib.rs");std::fs::write(&path,source).unwrap();
        let mut p=Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg=crate::cfg::CfgSet::host(&p.session,&[]);
        p.load_crate(label,path.to_str().unwrap(),2021,cfg,vec![]).unwrap();
        assert!(p.errors.is_empty(),"{:?}",p.errors);
        let mut t=Tcx::new();t.collect(&p);
        let mut errors=t.errors.clone();for e in t.dcx.drain_reported().0 { errors.push(e.message); }
        std::fs::remove_file(path).unwrap();std::fs::remove_dir(dir).unwrap();(p,t,errors)
    }
    #[test]
    fn round34_trait_overlap_and_disjoint_headers() {
        for (source,code,label) in [
            ("trait Tr {} impl<T> Tr for T {} impl Tr for u32 {}",Some("E0119"),"blanket"),
            ("trait Tr {} impl Tr for u32 {} impl Tr for u64 {}",None,"disjoint"),
            ("trait Tr {} impl<T> Tr for fn(T) {} impl<T> Tr for for<'a> fn(&'a T) {}",None,"leak_check"),
            ("trait Tr<T> {} struct S; impl Tr<u32> for S {} impl Tr<u64> for S {}",None,"args"),
            ("trait Bound {} trait Tr {} struct S; impl<T: Bound> Tr for T {} impl Tr for S {}",None,"impossible"),
            ("trait Bound {} trait Tr {} struct S; impl Bound for S {} impl<T: Bound> Tr for T {} impl Tr for S {}",Some("E0119"),"possible"),
            ("trait Tr {} impl Tr for [u32; 1] {} impl Tr for [u32; 2] {}",None,"const_disjoint"),
            ("trait Tr {} impl<const N: usize> Tr for [u32; N] {} impl Tr for [u32; 2] {}",Some("E0119"),"const_overlap"),
            ("trait Tr {} impl<'a> Tr for &'a u32 {} impl Tr for &'static u32 {}",Some("E0119"),"regions"),
        ] {
            let (_,_,errors)=check(source,label);
            if let Some(code)=code { assert!(errors.iter().any(|e|e.contains(code)),"{label}: {errors:?}"); }
            else { assert!(errors.is_empty(),"{label}: {errors:?}"); }
        }
    }
    #[test]
    fn round34_inherent_overlap_and_duplicate_items() {
        for (source,reject,label) in [
            ("struct S<T>(T); impl<T> S<T> { fn f() {} } impl S<u32> { fn f() {} }",true,"inherent"),
            ("struct S<T>(T); impl S<u32> { fn f() {} } impl S<u64> { fn f() {} }",false,"inherent_disjoint"),
            ("struct S; impl S { fn f() {} fn f() {} }",true,"duplicate"),
            ("struct S<T>(T); impl<T> S<fn(T)> { fn f() {} } impl<T> S<for<'a> fn(&'a T)> { fn f() {} }",true,"inherent_skip_leak"),
            ("struct S; impl S { fn f() {} } impl S { fn g() {} }",false,"different_names"),
            ("trait Bound {} struct S<T>(T); impl<T: Bound> S<T> { fn f() {} } struct X; impl S<X> { fn f() {} }",false,"inherent_impossible"),
        ] {
            let (_,_,errors)=check(source,label);
            assert_eq!(errors.iter().any(|e|e.contains("E0592")),reject,"{label}: {errors:?}");
            if !reject { assert!(errors.is_empty(),"{label}: {errors:?}"); }
        }
    }
    #[test]
    fn round34_large_inherent_components() {
        let mut source="struct S;".to_owned();
        for i in 0..500 { source.push_str(&format!("impl S {{ fn f{i}() {{}} }}")); }
        // Merge two existing components and add a new identifier in one block.
        source.push_str("impl S { fn f0() {} fn f1() {} fn extra() {} }");
        let (_,_,errors)=check(&source,"components");
        assert_eq!(errors.iter().filter(|e|e.contains("E0592")).count(),2,"{errors:?}");
    }
    #[test]
    fn round34_coherence_query_dedup_and_invalidation() {
        let (p,mut t,errors)=check("trait Tr {} impl<T> Tr for T {} impl Tr for u32 {}","query");
        assert_eq!(errors.len(),1,"{errors:?}");
        let count=t.dcx.err_count();t.coherence_check(&p);assert_eq!(t.dcx.err_count(),count);
        let concrete=p.impls[1].def;t.impl_trait.remove(&concrete);t.impl_self.remove(&concrete);
        t.publish_collected();
        let td=t.impl_trait[&p.impls[0].def];
        assert!(crate::type_queries::Providers {tcx:&t,prog:&p}.specialization_graph((0,td)).unwrap());
    }
}

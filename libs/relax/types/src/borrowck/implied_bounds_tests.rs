use crate::types::{AliasTy, AliasTyKind};
use super::implied_bounds::ImpliedBounds;
use crate::tcx::{Tcx, FnSigT, outlives::{OutlivesPredicate as P,RegionContract}};
use crate::types::*;
use crate::program::DefId;
fn region(tcx: &Tcx, i: u32) -> RegionId { tcx.tys.intern_region(RegionKind::ReEarlyParam(i)) }
fn reference(tcx: &Tcx, r: RegionId, t: TyId) -> TyId { tcx.tys.intern(TyKind::Ref(false,t,r)) }
fn bounds(tcx: &Tcx, raw: &[TyId], ps: &[P]) -> ImpliedBounds { ImpliedBounds::create(tcx,&crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())),ps,raw,&[],vec![]) }
fn signature(_tcx: &Tcx, ret: TyId) -> FnSigT {
    FnSigT { region_contract:Default::default(),generic_indices:Default::default(),bound_regions:0,
        params:vec![],ret,self_kind:0,n_generics:0,n_parent_generics:0,is_const:false,rustc_promotable:false,rustc_intrinsic:false,
        header:FnHeader::RUST,track_caller:false,target_features:vec![],safe_target_features:false }
}
#[test]
fn v4_function_pointer_components_include_free_but_not_bound_regions() {
    let tcx = Tcx::new(); let a = region(&tcx,0); let b = region(&tcx,1);
    let bound = tcx.tys.intern_region(RegionKind::ReBound(0,BoundVar(0)));
    let f = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[reference(&tcx,b,tcx.tys.unit),reference(&tcx,bound,tcx.tys.unit)]),tcx.tys.unit,FnHeader::RUST,1));
    let result = bounds(&tcx,&[reference(&tcx,a,f)],&[]);
    assert_eq!(result.outlives,[(b,a)]);
    assert_eq!(bounds(&tcx,&[],&[P::Type(f,a)]).outlives,[(b,a)]);
}
#[test]
fn v5_object_components_include_arguments_bindings_and_free_regions() {
    let tcx = Tcx::new(); let a = region(&tcx,0); let b = region(&tcx,1); let c = region(&tcx,2);
    let bound = tcx.tys.intern_region(RegionKind::ReBound(0,BoundVar(0)));
    for (args,bindings) in [(vec![reference(&tcx,b,tcx.tys.unit)],vec![]),
        (vec![],vec![(DefId(2),reference(&tcx,b,tcx.tys.unit),0)])] {
        let object = tcx.tys.intern(TyKind::Dyn(DefId(1),tcx.tys.mk_list(&args),tcx.tys.mk_list(&bindings),tcx.tys.intern_region_args(&[bound]),c,1,Default::default()));
        let result = bounds(&tcx,&[reference(&tcx,a,object)],&[]);
        assert!(result.outlives.contains(&(b,a)));
        assert!(result.outlives.contains(&(c,a)));
        assert!(!result.outlives.iter().any(|&(x,y)| x == bound || y == bound));
    }
}
#[test]
fn v6_function_items_exclude_shallow_lifetimes_but_keep_type_arguments() {
    let tcx = Tcx::new(); let a = region(&tcx,0); let b = region(&tcx,1);
    let rs = tcx.tys.intern_region_args(&[b]);
    let f = tcx.tys.intern(TyKind::FnDef(DefId(1),tcx.tys.mk_list(&[]),rs));
    assert!(bounds(&tcx,&[],&[P::Type(f,a)]).outlives.is_empty());
    let f = tcx.tys.intern(TyKind::FnDef(DefId(1),tcx.tys.mk_list(&[reference(&tcx,b,tcx.tys.unit)]),rs));
    assert_eq!(bounds(&tcx,&[],&[P::Type(f,a)]).outlives,[(b,a)]);
}
#[test]
fn v7_concrete_wf_walk_covers_functions_aliases_closures_and_objects() {
    let mut tcx = Tcx::new(); let a = region(&tcx,0); let b = region(&tcx,1);
    let inner = reference(&tcx,b,tcx.tys.unit); let nested = reference(&tcx,a,inner);
    let f = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[nested]),tcx.tys.unit,FnHeader::RUST,0));
    assert_eq!(bounds(&tcx,&[f],&[]).outlives,[(b,a)]);
    let d = DefId(50);
    tcx.insert_test_signature(d,signature(&tcx,nested));
    let item = tcx.tys.intern(TyKind::FnDef(d,tcx.tys.mk_list(&[]),RegionArgsId::EMPTY));
    assert_eq!(bounds(&tcx,&[item],&[]).outlives,[(b,a)]);
    let closure = tcx.tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: f, tupled_upvars_ty: tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[nested]))) }));
    assert_eq!(bounds(&tcx,&[closure],&[]).outlives,[(b,a)]);
    let closure = tcx.tys.intern(TyKind::Closure(0, 1, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: f, tupled_upvars_ty: tcx.tys.unit }));
    assert!(bounds(&tcx,&[closure],&[]).outlives.is_empty(),"closure signature is not an upvar");
    let contract = RegionContract { regions:vec![("'a".into(),a)],predicates:vec![P::Type(tcx.tys.intern(TyKind::Param(0)),a)],..Default::default() };
    tcx.nominal_region_contracts.insert(d,contract.clone());
    tcx.opaques.insert(d,crate::tcx::OpaqueDecl {region_contract:contract.clone(),..Default::default()});
    let rs = tcx.tys.intern_region_args(&[a]);
    for alias in [TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: d }, args: tcx.tys.mk_list(&[inner]), regions: rs }),TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Opaque { def_id: d }, args: tcx.tys.mk_list(&[inner]), regions: rs })] {
        let t = tcx.tys.intern(alias);
        assert_eq!(bounds(&tcx,&[t],&[]).outlives,[(b,a)]);
    }
    // Trait: 'a means the explicit object bound 'b must outlive 'a.
    let object = tcx.tys.intern(TyKind::Dyn(d,tcx.tys.mk_list(&[nested]),Default::default(),rs,b,0,Default::default()));
    assert_eq!(bounds(&tcx,&[object],&[]).outlives,[(b,a)]);
    tcx.nominal_region_contracts.clear();
    assert!(bounds(&tcx,&[object],&[]).outlives.is_empty(),"dyn generic arguments do not require WF");
    let bound = tcx.tys.intern_region(RegionKind::ReBound(0,BoundVar(0)));
    let higher = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[reference(&tcx,a,reference(&tcx,bound,tcx.tys.unit))]),tcx.tys.unit,FnHeader::RUST,1));
    assert!(bounds(&tcx,&[higher],&[]).outlives.is_empty());
}
#[test]
fn p4_filter_residual_placeholders_after_external_instantiation() {
    let tcx = Tcx::new(); let a = region(&tcx,0); let b = region(&tcx,1);
    let p = |u,i| tcx.tys.intern_region(RegionKind::RePlaceholder {universe:UniverseIndex(u),var:BoundVar(i)});
    let external = p(0,0); let residual = p(1,0); let root_residual = p(0,1);
    let t = tcx.tys.intern(TyKind::Param(0));
    let alias = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(1) }, args: tcx.tys.mk_list(&[reference(&tcx,residual,t)]), regions: RegionArgsId::EMPTY }));
    let mut result = bounds(&tcx,&[],&[P::Type(t,external),P::Type(t,residual),P::Type(alias,a),P::Region(residual,a),P::Region(root_residual,a),P::Region(external,a)]);
    result.instantiate_query_bounds(&tcx,&[(external,b)]);
    assert_eq!(result.outlives,[(b,a)]);
    assert_eq!(result.type_outlives,[(t,b)]);
}
#[test]
fn p9_variadic_signature_has_fresh_local_region_and_valist_input() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use super::universal_regions::{SignatureRegions,Identity,RegionClassification};
    let mut tcx = Tcx::new(); let d = DefId(1); let va = DefId(2);
    tcx.lang_items.items[crate::lang_items::LangItem::VaList as usize] = Some(va);
    let mut sig = signature(&tcx,tcx.tys.unit); sig.header.c_variadic = true;
    tcx.insert_test_signature(d,sig);
    let u = SignatureRegions::function(&tcx,&prog,d).unwrap();
    assert_eq!(u.inputs.len(),1);
    let TyKind::Adt(def,_,rs) = tcx.tys.kind(u.inputs[0]) else { panic!() };
    assert_eq!(*def,va);
    let r = tcx.tys.region_args(*rs)[0];
    assert!(u.regions.iter().any(|u| u.class == RegionClassification::Local && matches!(u.identity,Identity::Region(_,x) if x == r)));
    assert_eq!(u.fr_fn_body,2);
    assert_eq!(u.output,tcx.tys.unit);
    // Foreign-call interfaces never require or append the body-only VaList.
    tcx.lang_items.items[crate::lang_items::LangItem::VaList as usize] = None;
    let call = SignatureRegions::function_call(&tcx,&prog,d).unwrap();
    assert!(call.inputs.is_empty());
    assert_eq!(call.fr_fn_body,1);
}

fn collected(name: &str, source: &[u8]) -> (crate::program::Program,Tcx) {
    let dir = std::env::temp_dir().join(format!("cfix1-{name}-{}",std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lib.rs"); std::fs::write(&file,source).unwrap();
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = crate::program::Program::new(&session);
    prog.load_crate(name,file.to_str().unwrap(),2021,crate::cfg::CfgSet::host(&session,&[]),vec![]).unwrap();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(tcx.errors.is_empty(),"{:?}",tcx.errors);
    (prog,tcx)
}
#[test]
fn p3_compatibility_uses_only_registered_type_obligations_for_bevy_paramset() {
    let (prog,tcx) = collected("bevy_ecs",b"#![no_std]\nstruct ParamSet<T>(T); struct Other<T>(T); struct Query<T>(T);");
    let def = |name| *tcx.adts.keys().find(|&&d| prog.name(d) == name).unwrap();
    let t = tcx.tys.intern(TyKind::Param(0)); let a = region(&tcx,0); let b = region(&tcx,1);
    let paramset = tcx.tys.intern(TyKind::Adt(def("ParamSet"),tcx.tys.mk_list(&[t]),RegionArgsId::EMPTY));
    let other = tcx.tys.intern(TyKind::Adt(def("Other"),tcx.tys.mk_list(&[paramset]),RegionArgsId::EMPTY));
    let registered = [P::Type(t,a),P::Type(t,b),P::Region(a,b)];
    // Audit's ParamSet<Query<&'a u8>> witness: the selected SystemParam
    // obligation Q: 'static is registered by normalization, then decomposed.
    let q = reference(&tcx,a,tcx.tys.u8_);
    let query = tcx.tys.intern(TyKind::Adt(def("Query"),tcx.tys.mk_list(&[q]),RegionArgsId::EMPTY));
    let nested = tcx.tys.intern(TyKind::Adt(def("ParamSet"),tcx.tys.mk_list(&[query]),RegionArgsId::EMPTY));
    let static_region = tcx.tys.intern_region(RegionKind::ReStatic);
    let mut result = ImpliedBounds::default();
    result.consider_implied_bounds_compat(&tcx,&prog,nested,&[P::Type(q,static_region)]);
    assert_eq!(result.outlives,[(a,static_region)]);
    for ty in [paramset,reference(&tcx,a,reference(&tcx,b,paramset))] {
        let mut result = ImpliedBounds::default();
        result.consider_implied_bounds_compat(&tcx,&prog,ty,&registered[1..]);
        assert_eq!(result.type_outlives,[(t,b)]);
        assert!(result.outlives.is_empty());
    }
    for ty in [other,tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[paramset])))] {
        let mut result = ImpliedBounds::default();
        result.consider_implied_bounds_compat(&tcx,&prog,ty,&registered);
        assert!(result.type_outlives.is_empty());
    }
    let (prog,tcx) = collected("not_bevy",b"#![no_std]\nstruct ParamSet<T>(T);");
    let d = *tcx.adts.keys().next().unwrap();
    let t = tcx.tys.intern(TyKind::Param(0));
    let ty = tcx.tys.intern(TyKind::Adt(d,tcx.tys.mk_list(&[t]),RegionArgsId::EMPTY));
    let mut result = ImpliedBounds::default();
    result.consider_implied_bounds_compat(&tcx,&prog,ty,&[P::Type(t,region(&tcx,0))]);
    assert!(result.type_outlives.is_empty());
}
#[test]
fn v7_collected_object_supertraits_supply_wf_bounds() {
    let (prog,tcx) = collected("object_wf",br#"#![no_std]
        trait Base<'b>: 'b {} trait Derived<'b>: Base<'b> {}
        fn f<'a,'b>(_: &(dyn Derived<'b> + 'a)) {}
    "#);
    let d = (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.name(d) == "f").unwrap();
    let u = super::universal_regions::SignatureRegions::function(&tcx,&prog,d).unwrap();
    let TyKind::Ref(_,object,_) = tcx.tys.kind(u.inputs[0]) else { panic!() };
    let TyKind::Dyn(_,_,_,rs,a,_,_) = tcx.tys.kind(*object) else { panic!() };
    let b = tcx.tys.region_args(*rs)[0];
    assert!(bounds(&tcx,&[*object],&[]).outlives.contains(&(*a,b)));
}

use super::*;
use crate::infer::InferCtxt;
use crate::tcx::{EvalCx, EvalState, TypingEnv, TypingMode};
use crate::types::RegionKind;

fn program(source: &str, label: &str) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-native-delegate-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
    prog.load_crate(label, path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);

    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    (prog, tcx)
}

#[test]
fn round9_final_assoc_items_use_impl_or_trait_definition() {
    let (p, t) = program("trait Tr { type Out; const N: u8 = 2; fn f() {} } struct S; impl Tr for S { type Out = u32; }", "eligible");
    let imp = p.impls[0].def;
    let td = t.impl_trait[&imp];
    for &item in &p.traits[p.def(td).sub as usize] {
        let result = t.fetch_eligible_assoc_item(&p, item, imp);
        if p.name(item) == "Out" { assert_eq!(p.def(result).parent, imp); }
        else { assert_eq!(result, item); }
    }
}

#[test]
fn round9_impl_trait_ref_keeps_lifetime_and_const_arguments() {
    let (p, t) = program("trait Tr<'a, T, const N: usize> { type Out; } struct S; impl<'a, T, const N: usize> Tr<'a, T, N> for S { type Out = &'a T; }", "args");
    let imp = p.impls[0].def;
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let args = infcx.fresh_args_for_item(&p, imp);
    assert!(matches!(args.as_slice(), [GenericArg::Lifetime(_), GenericArg::Type(_), GenericArg::Const(_)]));
    let tr = t.impl_trait_ref(&p, imp, &args);
    assert_eq!(tr.args.len(), 4);
    assert_eq!(&tr.args[1..], args.as_slice());
    let indices = t.generic_indices_of(&p, imp);
    let compact = indices.identity_types_consts(&t.tys);
    let regions = t.tys.intern_region_args(&[t.tys.intern_region(RegionKind::ReStatic)]);
    let full = t.complete_args(&p, imp, &compact, regions);
    assert_eq!(full[0], GenericArg::Lifetime(t.tys.intern_region(RegionKind::ReStatic)));
    let raw = t.impl_assoc_ty(&EvalCx::new(&p, &t, TypingEnv::fully_monomorphized(&p), &EvalState::new()), &p, imp, p.syms.get("Out").unwrap()).unwrap();
    let value = t.instantiate_ty(&p, imp, &full, raw);
    assert!(matches!(t.tys.kind(value), TyKind::Ref(false, _, r) if t.tys.region_kind(*r) == RegionKind::ReStatic));
}

#[test]
fn round10_rpitit_has_synthesized_associated_type_definitions() {
    use crate::program::DefKind;
    let (p, t) = program("trait Copy {} #[lang = \"future_trait\"] trait Future { #[lang = \"future_output\"] type Output; } trait Tr { fn get(&self) -> impl Copy; async fn run(&self) -> u32; } struct S; impl Tr for S { fn get(&self) -> u32 { 1 } async fn run(&self) -> u32 { 2 } }", "rpitit");
    let td = t.impl_trait[&p.impls[0].def];
    let items = &p.traits[p.def(td).sub as usize];
    let mut opaques = 0;
    for d in &p.defs {
        if d.kind == DefKind::OpaqueTy && items.contains(&d.parent) { opaques += 1; }
    }
    assert_eq!(opaques, 2, "control: trait return opaque definitions exist");
    let trait_assocs: Vec<_> = items.iter().copied().filter(|d| p.def(*d).kind == DefKind::AssocTy).collect();
    let impl_assocs: Vec<_> = p.impls[0].items.iter().copied().filter(|d| p.def(*d).kind == DefKind::AssocTy).collect();
    assert_eq!((trait_assocs.len(), impl_assocs.len()), (2, 2), "nightly assoc.rs:24-46 synthesizes trait and impl associated types; trait={:?}, impl={:?}", trait_assocs, impl_assocs);
}

#[test]
fn round10_clauses_keep_higher_ranked_gat_bindings_and_nested_bounds() {
    let (p, t) = program("trait Bound<'a, T> {} trait Tr<'a> { type Out<'b, T>; } fn f<T>() where for<'a> T: Tr<'a, Out<'a, u32> = &'a u32> {} fn g<T>() where T: for<'a> Tr<'a, Out<'a, u32> = &'a u32> {} trait Iter { type Item; } fn nested<T: Iter<Item: for<'a> Bound<'a, u32>>>() {}", "clauses");
    for name in ["f", "g"] {
        let d = p.defs.iter().position(|d| p.syms.str(d.name) == name).unwrap();
        let ps = t.clauses_of(&p, DefId(d as u32));
        let proj = ps.iter().find(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::Projection(_)))).unwrap();
        assert!(matches!(proj.bound_vars.as_ref(), [BoundVariableKind::Region(crate::types::BoundRegionKind::Named(id))] if id.owner == DefId(d as u32)));
        let PredicateKind::Clause(ClauseKind::Projection(proj)) = &proj.kind else { unreachable!() };
        let AliasTerm::Ty(alias) = proj.projection_term else { panic!() };
        let TyKind::Alias(_, alias) = t.tys.kind(alias) else { panic!() };
        assert_eq!(t.tys.list(alias.args).len(), 2);
        let rs = t.tys.region_args(alias.regions);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0], rs[1]);
        assert!(matches!(t.tys.region_kind(rs[0]), RegionKind::ReBound(0, _)));
        let Term::Ty(value) = proj.term else { panic!() };
        assert!(matches!(t.tys.kind(value), TyKind::Ref(false, _, r) if *r == rs[0]));
    }
    let d = p.defs.iter().position(|d| p.syms.str(d.name) == "nested").unwrap();
    let ps = t.clauses_of(&p, DefId(d as u32));
    assert!(ps.iter().any(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(c)) if p.bound_vars.len() == 1 && matches!(c.trait_ref.args[0], GenericArg::Type(ty) if matches!(t.tys.kind(ty), TyKind::Alias(..))))));
}

#[test]
fn round10_supertraits_rebase_arguments_and_concatenate_binders() {
    let (p, t) = program("trait Base<'a, 'b, T> { type Out; } trait Sub<'a>: for<'b> Base<'a, 'b, u32> {}", "supertraits");
    let d = DefId(p.defs.iter().position(|d| p.syms.str(d.name) == "Sub").unwrap() as u32);
    let region = t.tys.intern_region(RegionKind::ReBound(0, crate::types::BoundVar(0)));
    let mut pred = super::super::collect::trait_predicate(d, vec![GenericArg::Type(t.tys.bool_), GenericArg::Lifetime(region)]);
    pred.bound_vars = vec![BoundVariableKind::Region(crate::types::BoundRegionKind::Anon)].into();
    let supers = t.supertraits(&p, pred);
    let base = supers.iter().find(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(c)) if c.trait_ref.def_id != d)).unwrap();
    assert_eq!(base.bound_vars.len(), 2);
    let PredicateKind::Clause(ClauseKind::Trait(c)) = &base.kind else { panic!() };
    assert_eq!(c.trait_ref.args[0], GenericArg::Type(t.tys.bool_));
    assert_eq!(c.trait_ref.args[1], GenericArg::Lifetime(region));
    let GenericArg::Lifetime(r) = c.trait_ref.args[2] else { panic!() };
    assert_eq!(t.tys.region_kind(r), RegionKind::ReBound(0, crate::types::BoundVar(1)));
    assert_eq!(c.trait_ref.args.len(), 4);
}

#[test]
fn round10_rpitit_lifetime_capture_generics_and_clauses() {
    use relax_front::assoc::ImplTraitInTraitData;
    let (p, t) = program("trait Copy {} trait Bound<'a> {} trait Tr<'a, T> { fn get<'b, U>(&'b self) -> impl Bound<'a>; fn precise<'b>(&'b self) -> impl Copy + use<Self, T>; } struct S; impl<'a, T> Tr<'a, T> for S { fn get<'b, U>(&'b self) -> u32 { 1 } fn precise<'b>(&'b self) -> u32 { 2 } }", "rpitit_lifetimes");
    for (&assoc, data) in &p.impl_trait_in_trait_data {
        if let ImplTraitInTraitData::Trait { fn_def_id, opaque_def_id } = *data {
            let sig = t.fn_sig(&p, fn_def_id).unwrap();
            let TyKind::Alias(_, alias) = t.tys.kind(sig.ret) else { panic!() };
            assert_eq!(alias.kind, crate::types::AliasTyKind::Projection { def_id: assoc });
            let generics = t.generic_indices_of(&p, assoc);
            let full = t.complete_args(&p, assoc, t.tys.list(alias.args), alias.regions);
            assert_eq!(full.len(), generics.lifetimes.len() + generics.types_consts.len());
            let data = t.opaque_generics(&p, opaque_def_id);
            if p.name(fn_def_id) == "get" {
                assert_eq!(data.captured_lifetimes.len(), 2);
                assert!(data.captured_lifetimes.iter().any(|&r| matches!(t.tys.region_kind(r), RegionKind::ReBound(0, _))));
                assert_eq!(t.clauses_of(&p, assoc).iter().filter(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::RegionOutlives(..)))).count(), 2);
                let bounds = t.item_bounds(&p, assoc);
                assert!(bounds.iter().any(|b| matches!(&b.kind, PredicateKind::Clause(ClauseKind::Trait(c)) if p.name(c.trait_ref.def_id) == "Bound")));
            } else { assert!(data.captured_lifetimes.is_empty()); }
            let imp_assoc = p.impl_trait_in_trait_data.keys().find(|d| p.trait_item_def_id.get(d) == Some(&assoc)).unwrap();
            let imp_generics = t.generic_indices_of(&p, *imp_assoc);
            assert_eq!(imp_generics.lifetimes.len(), generics.lifetimes.len());
            assert_eq!(imp_generics.types_consts.len() + 1, generics.types_consts.len());
            assert_eq!(t.clauses_of(&p, *imp_assoc).iter().filter(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::RegionOutlives(..)))).count(), if p.name(fn_def_id) == "get" { 2 } else { 0 });
        }
    }
}

#[test]
fn round10_nested_rpitit_and_async_bounds_keep_captured_regions() {
    use relax_front::assoc::ImplTraitInTraitData;
    let (p, t) = program("trait Bound<'a> {} trait Iter { type Item; } #[lang = \"future_trait\"] trait Future { #[lang = \"future_output\"] type Output; } trait Tr<'a> { fn get<'b>(&'b self) -> impl Iter<Item = impl Bound<'a>>; async fn run<'b>(&'b self, x: &'a u32) -> &'a u32; }", "nested_rpitit");
    let mut nested = false;
    let mut future = false;
    for (&assoc, data) in &p.impl_trait_in_trait_data {
        let ImplTraitInTraitData::Trait { fn_def_id, opaque_def_id } = *data else { continue; };
        let generics = t.generic_indices_of(&p, assoc);
        let opaque = t.opaque_generics(&p, opaque_def_id);
        assert_eq!(opaque.captured_lifetimes.len(), 2);
        assert_eq!(generics.lifetimes.len(), 3);
        for bound in t.item_bounds(&p, assoc) {
            if let PredicateKind::Clause(ClauseKind::Projection(proj)) = &bound.kind {
                let AliasTerm::Ty(projection) = proj.projection_term else { panic!() };
                let TyKind::Alias(_, a) = t.tys.kind(projection) else { panic!() };
                let Term::Ty(value) = proj.term else { panic!() };
                if p.name(fn_def_id) == "get" {
                    let TyKind::Alias(_, a) = t.tys.kind(value) else { panic!() };
                    assert!(matches!(a.kind, crate::types::AliasTyKind::Projection { def_id } if p.impl_trait_in_trait_data.contains_key(&def_id)));
                    nested = true;
                } else {
                    assert_eq!(p.name(a.kind.def_id()), "Output");
                    let TyKind::Ref(_, _, r) = t.tys.kind(value) else { panic!() };
                    let offset = (opaque.parent.lifetimes.len() + opaque.parent.types_consts.len()) as u32;
                    assert_eq!(t.tys.region_kind(*r), RegionKind::ReEarlyParam(offset));
                    future = true;
                }
            }
        }
    }
    assert!(nested && future);
}

#[test]
fn round11_opaque_variance_retains_trait_parent_lifetimes() {
    let (p,t) = program("trait Bound<'a> {} fn f<'a: 'a>(x: &'a u32) -> impl Bound<'a> { x } trait Tr<'a> { fn f() -> impl Bound<'a>; }", "opaque_variance");
    for (id,d) in p.defs.iter().enumerate() {
        if d.kind != crate::program::DefKind::OpaqueTy { continue; }
        let opaque = DefId(id as u32);
        t.fn_sig(&p, p.opaque_owner(opaque));
        let data = t.opaque_generics(&p, opaque);
        assert_eq!(data.parent.lifetimes.len(),1);
        let owner = p.opaque_owner(opaque); let in_trait = p.def(p.def(owner).parent).kind == crate::program::DefKind::Trait;
        let variances = t.variances_of(&p, opaque);
        assert_eq!(variances[data.parent.lifetimes[0] as usize] == crate::types::variance::V::Bi, !in_trait);
        assert!(variances[data.parent.lifetimes.len() + data.parent.types_consts.len()..].iter().all(|v| *v == crate::types::variance::V::Inv));
    }
}

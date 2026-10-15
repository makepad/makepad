// ported from N:rustc_hir_analysis/src/hir_ty_lowering/mod.rs @8d1a764
use super::*;
use super::resolve_bound_vars::{BinderId, LifetimeKey, ResolveBoundVars};
use crate::next_solver::BoundVariableKind;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct RegionEnv {
    pub(crate) owner: Option<DefId>,
    pub enabled: bool,
    pub names: Vec<(String, RegionKind)>,
    pub next_param: u32,
    pub(crate) anonymous_params: Vec<u32>,
    table: Option<Arc<ResolveBoundVars>>,
    binder: Option<BinderId>,
    pub(super) infer_fresh: bool,
}
impl RegionEnv {
    pub(crate) fn enter_body(&mut self) { self.enabled = true; self.infer_fresh = true; }
    pub fn identity_args(&self, tys: &Types) -> RegionArgsId {
        if !self.enabled { return RegionArgsId::EMPTY; }
        let mut indices = self.anonymous_params.clone();
        for (_, region) in &self.names { if let RegionKind::ReEarlyParam(i) = region { indices.push(*i); } }
        indices.sort_unstable(); indices.dedup();
        let args: Vec<_> = indices.into_iter().map(|i| tys.intern_region(RegionKind::ReEarlyParam(i))).collect();
        tys.intern_region_args(&args)
    }
    pub(crate) fn with_binder(&self, binder: Option<BinderId>) -> Self {
        let mut env = self.clone(); env.binder = binder; env
    }
    pub(crate) fn bound_vars(&self) -> Vec<BoundVariableKind> {
        if !self.enabled { return Vec::new(); }
        self.binder.and_then(|binder| self.table.as_ref()?.late_bound_vars_map.get(&binder).cloned()).unwrap_or_default()
    }
    pub fn bound_count(&self) -> u32 { self.bound_vars().len() as u32 }
    pub fn resolve(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, tys: &Types, _prog: &Program, _file: u32, name: Option<ast::Ident>) -> RegionId {
        match name {
            Some(name) => self.resolve_key(infcx, tys, LifetimeKey::Named(name.lo, name.hi)),
            None if self.infer_fresh => infcx.expect("body lifetime lowering requires inference context").next_region_var(),
            None => tys.intern_region(RegionKind::ReError),
        }
    }
    pub(super) fn resolve_key(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, tys: &Types, key: LifetimeKey) -> RegionId {
        if !self.enabled { return RegionId::ERASED; }
        let table = self.table.as_ref().expect("written lifetime lowering requires owner resolution");
        match table.region_for_use(key) {
            Some(region) => tys.intern_region(region),
            None => infcx.map_or_else(|| tys.intern_region(RegionKind::ReError), |infcx| infcx.next_region_var()),
        }
    }
    /// The object type `ty` has no resolved lifetime default (Scope::ObjectLifetimeDefault
    /// with `None`, resolve_bound_vars.rs:927-944 and :2290).
    pub(super) fn object_default_indeterminate(&self, ty: Option<ast::TyId>) -> bool {
        self.enabled && ty.is_some_and(|ty| self.table.as_ref().is_some_and(|table| table.region_for_use(LifetimeKey::ObjectDefault(ty.0)).is_none()))
    }
    pub(super) fn object_region(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, tys: &Types, ty: Option<ast::TyId>) -> RegionId {
        if !self.enabled { return RegionId::ERASED; }
        if let Some(ty) = ty { return self.resolve_key(infcx, tys, LifetimeKey::ObjectDefault(ty.0)); }
        infcx.map_or_else(|| tys.intern_region(RegionKind::ReStatic), |infcx| infcx.next_region_var())
    }
}
impl Tcx {
    pub(super) fn lifetime_path_is_parameter(&self, prog: &Program, mut owner: DefId, name: crate::program::Sym) -> bool {
        loop {
            let def = prog.def(owner);
            if def.kind == DefKind::Trait && prog.syms.str(name) == "Self" { return true; }
            let generics = super::resolve_bound_vars::owner_generics(prog, owner);
            let file = &prog.files[def.file as usize];
            for param in file.ast.list(generics.params) {
                if !matches!(param.kind, GenericParamKind::Lifetime(_)) && file.text(param.name) == prog.syms.str(name) { return true; }
            }
            if !matches!(def.kind, DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst) { return false; }
            owner = def.parent;
        }
    }

    /// Declaration path resolution for the lifetime passes. Generic parameters
    /// shadow module items; qualified associated paths resolve through their
    /// written trait, without lowering the self type or asking for a signature.
    pub(super) fn lifetime_path_def(&self, prog: &Program, owner: DefId, file: u32, module: u32, path: &ast::Path) -> Option<DefId> {
        let ast = &prog.files[file as usize].ast;
        let first = ast.segs(path.segs).first()?;
        if let Some(qself) = ast.qself(path) {
            let trait_path = qself.trait_path?;
            let td = self.resolve_type_path_def(prog, file, module, &trait_path)?;
            let name = prog.syms.get(prog.files[file as usize].text(first.name))?;
            return self.trait_assoc(prog, td, name);
        }
        if !path.global() && first.kind() == SegKind::Ident {
            let name = prog.syms.get(prog.files[file as usize].text(first.name));
            if name.is_some_and(|name| self.lifetime_path_is_parameter(prog, owner, name)) { return None; }
        }
        self.resolve_type_path_def(prog, file, module, path)
    }

    pub(crate) fn bound_region_env(&self, prog: &Program, owner: DefId) -> RegionEnv {
        let table = self.resolve_bound_vars(prog, owner);
        let mut env = RegionEnv { owner: Some(owner), enabled: true, next_param: (table.generic_indices.lifetimes.len() + table.generic_indices.types_consts.len()) as u32,
            table: Some(table.clone()), binder: Some(BinderId::Item), ..Default::default() };
        for declaration in table.names.inherited.iter().chain(&table.names.declarations) {
            if declaration.binder != BinderId::Item { continue; }
            let region = table.declaration_region(declaration.id);
            if let Some(name) = &declaration.name { env.names.push((name.clone(), region)); }
            else if let RegionKind::ReEarlyParam(i) = region { env.anonymous_params.push(i); }
        }
        env
    }
    pub(super) fn lower_fn_pointer(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &GenEnv, fp: &ast::FnPtr, binder: BinderId) -> TyId {
        let mut env = env.clone();
        env.regions = env.regions.with_binder(Some(binder));
        let mut params = Vec::new();
        for &ty in prog.files[file as usize].ast.list(fp.params) { params.push(self.lower_ty(cx, prog, file, module, &env, ty)); }
        let ret = fp.ret.map_or(self.tys.unit, |ty| self.lower_ty(cx, prog, file, module, &env, ty));
        let header = self.lower_fn_header(cx, prog, file, fp.unsafe_, fp.abi, fp.variadic);
        self.tys.intern(TyKind::FnPtr(self.tys.mk_list(&params), ret, header, env.regions.bound_count()))
    }
    pub(crate) fn lower_fn_trait_bound_args(&self, cx: &crate::tcx::EvalCx<'_>, prog: &Program, file: u32, module: u32, env: &mut GenEnv, params: ast::List<ast::TyId>, output: Option<ast::TyId>) -> (Vec<TyId>, TyId) {
        let mut inputs = Vec::new();
        for &ty in prog.files[file as usize].ast.list(params) { inputs.push(self.lower_ty(cx, prog, file, module, env, ty)); }
        let output = output.map_or(self.tys.unit, |ty| self.lower_ty(cx, prog, file, module, env, ty));
        (inputs, output)
    }
}

impl Tcx {
    /// rustc_hir_analysis/hir_ty_lowering/mod.rs:589: a resolved early parameter
    /// is instantiated by its declaration index, not its position in a type-only list.
    pub(crate) fn subst_decl_regions(&self, t: TyId, decl: &GenEnv, args: RegionArgsId) -> TyId {
        let args = self.tys.region_args(args);
        let bindings: Vec<_> = decl.regions.names.iter().filter_map(|(_, r)| match r {
            RegionKind::ReEarlyParam(i) => Some(*i), _ => None,
        }).zip(args.iter().copied()).collect();
        self.tys.map_regions(t, &|r, depth| match self.tys.region_kind(r) {
            RegionKind::ReEarlyParam(i) => bindings.iter().find(|(j, _)| *j == i).map_or(RegionId::ERASED, |(_, r)| {
                match self.tys.region_kind(*r) {
                    RegionKind::ReBound(d, v) => self.tys.intern_region(RegionKind::ReBound(d + depth, v)),
                    _ => *r,
                }
            }),
            _ => r,
        }, 0)
    }

    pub(crate) fn lower_region_args(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, prog: &Program, file: u32, module: u32, env: &GenEnv, p: &ast::Path) -> RegionArgsId {
        if !env.regions.enabled { return RegionArgsId::EMPTY; }
        let ast = &prog.files[file as usize].ast;
        let mut out = Vec::new();
        if let Some(GenericArgs::Angle(v)) = ast.segs(p.segs).last().and_then(|s| ast.gargs(s.args())) {
            for a in ast.list(*v) {
                if let GenericArg::Lifetime(lt) = a { out.push(env.regions.resolve(infcx, &self.tys, prog, file, Some(*lt))); }
            }
        }
        if out.is_empty() {
            if let Some(d) = self.resolve_type_path_def(prog, file, module, p) {
                let df = prog.def(d);
                if !matches!(df.kind, DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Trait | DefKind::AssocTy) { return RegionArgsId::EMPTY; }
                let a = &prog.files[df.file as usize].ast;
                let g = match &a.item(df.item).kind {
                    k @ (ItemKind::Struct(_) | ItemKind::Union(_) | ItemKind::Enum(_) | ItemKind::TypeAlias(_) | ItemKind::Trait(_)) => a.kind_generics(k),
                    _ => None,
                };
                if let Some(g) = g {
                    for (index, _) in a.list(g.params).iter().filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_))).enumerate() {
                        let segment = p.segs.lo + p.segs.len() as u32 - 1;
                        out.push(env.regions.resolve_key(infcx, &self.tys, LifetimeKey::Path(segment, index as u32)));
                    }
                }
            }
        }
        self.tys.intern_region_args(&out)
    }
}

impl Tcx {
    pub(super) fn object_region(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, prog: &Program, file: u32, env: &GenEnv, bs: &[ast::Bound], source_ty: Option<ast::TyId>) -> RegionId {
        for bound in bs {
            if let ast::Bound::Lifetime(name) = bound { return env.regions.resolve(infcx, &self.tys, prog, file, Some(*name)); }
        }
        env.regions.object_region(infcx, &self.tys, source_ty)
    }
}

impl Tcx {
    // rustc_hir_analysis::compute_object_lifetime_bound (hir_ty_lowering/mod.rs:3749)
    // elaborates Self: 'r predicates, including supertraits, before using a default.
    pub(super) fn derived_object_regions(&self, prog: &Program, td: DefId, args: RegionArgsId, seen: &mut Vec<DefId>, out: &mut Vec<RegionId>) {
        if seen.contains(&td) { return; }
        seen.push(td);
        let d = prog.def(td);
        let f = &prog.files[d.file as usize];
        if let Some(crate::ast::TraitItem { generics, supers, .. }) = f.ast.as_trait(&f.ast.item(d.item).kind) {
            let env = self.declaration_env(prog, td);
            let params = crate::types::variance::GenericIndices::from_env(&env).lifetimes;
            let rs = self.tys.region_args(args);
            let instantiate = |region| match self.tys.region_kind(region) {
                RegionKind::ReEarlyParam(index) => params.iter().position(|&p| p == index).and_then(|i| rs.get(i).copied()).unwrap_or(region),
                _ => region,
            };
            let mut all = f.ast.list(*supers).to_vec();
            for w in f.ast.list(generics.where_) {
                if let ast::WherePred::Bound { hrtb, ty, bounds } = w {
                    if hrtb.is_empty() && matches!(f.ast.ty(*ty), ast::Ty::Path(p) if p.no_qself() && p.segs.len() == 1 && f.ast.segs(p.segs)[0].kind() == SegKind::SelfType) {
                        all.extend_from_slice(f.ast.list(*bounds));
                    }
                }
            }
            for b in all {
                match b {
                    ast::Bound::Lifetime(n) => {
                        let r = instantiate(env.regions.resolve(None, &self.tys, prog, d.file, Some(n)));
                        if !out.contains(&r) { out.push(r); }
                    }
                    ast::Bound::Trait { hrtb, path, .. } if hrtb.is_empty() => {
                        if let Some(td) = self.resolve_type_path_def(prog, d.file, d.scope, &path) {
                            let inherited = self.lower_region_args(None, prog, d.file, d.scope, &env, &path);
                            let inherited: Vec<_> = self.tys.region_args(inherited).iter().map(|&r| instantiate(r)).collect();
                            self.derived_object_regions(prog, td, self.tys.intern_region_args(&inherited), seen, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        seen.pop();
    }

    pub(super) fn object_region_with_bounds(&self, infcx: Option<&crate::infer::InferCtxt<'_>>, prog: &Program, file: u32, env: &GenEnv, bounds: &[ast::Bound], td: DefId, rs: RegionArgsId, source_ty: Option<ast::TyId>) -> RegionId {
        if !env.regions.enabled { return RegionId::ERASED; }
        let explicit = bounds.iter().find_map(|b| match b { ast::Bound::Lifetime(n) => Some(*n), _ => None });
        if let Some(n) = explicit {
            if prog.files[file as usize].text(n) != "'_" { return env.regions.resolve(None, &self.tys, prog, file, Some(n)); }
        }
        let mut derived = Vec::new();
        self.derived_object_regions(prog, td, rs, &mut Vec::new(), &mut derived);
        // rustc_trait_selection::traits::wf::object_region_bounds: a region
        // escaping the existential predicate binder is not an object bound.
        derived.retain(|r| !matches!(self.tys.region_kind(*r), RegionKind::ReBound(..)));
        if let Some(r) = derived.iter().find(|r| self.tys.region_kind(**r) == RegionKind::ReStatic) { return *r; }
        if let Some(r) = derived.first() { return *r; }
        self.object_region(infcx, prog, file, env, bounds, source_ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BoundVar;

    // The UI suite still consumes AST-reconstructed regions in A2, so it cannot
    // observe incorrect region metadata. Check the actual collected item types.
    #[test]
    fn written_regions_and_inference_keys() {
        let source = br#"
            #![no_std]
            #[lang = "sized"] trait Sized {}
            #[lang = "unsize"] trait Unsize<T: ?Sized> {}
            struct S<'a, T>(&'a T);
            type A<'a> = &'a u32;
            type F<T> = fn(T);
            trait Tr<'a> { fn inherited(&'a self); }
            trait Hr<'a, T> { type Item; }
            #[lang = "fn_once"] trait FnOnce<T> { #[lang = "fn_once_output"] type Output; }
            #[lang = "fn"] trait Fn<T>: FnOnce<T> {}
            type Obj<T> = dyn for<'a> Hr<'a, T, Item = &'a T>;
            fn substituted<'a>(x: &'a u32, p: &Obj<&'a u32>) {}
            fn higher<'a>(x: &'a u32, p: &dyn for<'b> Hr<'b, &'a u32, Item = &'b u32>) {}
            fn elided_object(p: *const dyn Hr<'_, u32, Item = u32>) -> &u32 { loop {} }
            fn callable(p: &dyn for<'a> Fn(&'a u32) -> &'a u32) {}
            fn takes<F: for<'a> Fn(&'a u32) -> &'a u32>(f: F) {}
            struct O<'a, T: 'a + ?Sized>(&'a T);
            fn closure_annotations<'a>(z: &'a u32) {
                let a = |x: &u32, y: &u32| {};
                let b = |f: for<'b> fn(&'b u32), x: &'a u32| {};
                let c = |x: &u32| -> &u32 { x };
                let n = const { 1u32 };
            }
            fn takes_ref<F: for<'a> Fn(&'a u32) -> &'a u32>(f: &F) {}
            fn takes_mut<F: for<'a> Fn(&'a u32) -> &'a u32>(f: &mut F) {}
            fn takes_where<F>(f: F) where for<'a> F: Fn(&'a u32) -> &'a u32 {}
            fn takes_pointer(f: for<'a> fn(&'a u32) -> &'a u32) {}
            fn closure_expected() {
                let f: for<'unused, 'a> fn(&'a u32) -> &'a u32 = |x: &_| x;
                takes(|x: &_| x);
                takes_pointer(|x: &_| x);
                takes_where(|x: &_| x);
                takes_ref(&|x| x);
                takes_mut(&mut |x| x);
                takes_mut(&mut { (|x| x) });
                let object: &dyn for<'a> Fn(&'a u32) -> &'a u32 = &|x: &_| x;
            }
            fn deferred_closure_rejected() {
                let g;
                takes(g);
                g = |x| x;
            }
            type Callback = for<'a> fn(&'a u32) -> &'a u32;
            struct Factory { make: fn(Callback) }
            impl Factory { fn get(&self) -> fn(Callback) { takes_pointer } }
            fn make_pointer() -> fn(Callback) { takes_pointer }
            fn closure_routes(factory: Factory) {
                (factory.make)(|x: &_| x);
                make_pointer()(|x: &_| x);
                (factory.get())(|x: &_| x);
                let ptr: fn(Callback) = takes_pointer;
                let r = &ptr;
                r(|x: &_| x);
                let tuple: (Callback,) = (|x| x,);
                let array: [Callback; 1] = [|x| x];
            }
            fn named<'a>(x: &'a u32) -> &'a u32 { x }
            fn early<'a>(x: &'a u32) -> &'a u32 where 'a: 'static { x }
            fn output<'a>() -> &'a u32 { loop {} }
            fn elided(x: &u32) -> &u32 { x }
            fn alias<'a>(x: A<'a>) -> A<'a> { x }
            fn nested<'a>(x: &'a u32, f: F<&'a u32>) {}
            fn objects<'a>(x: &'a dyn Tr<'a>, y: O<'a, dyn Tr<'a>>) {}
            impl<'a, T> S<'a, T> {
                fn get(&self) -> &T { self.0 }
                fn parent<'b>(&'b self) -> &'b T where 'b: 'a { self.0 }
            }
        "#;
        let dir = std::env::temp_dir().join(format!("relax-written-regions-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, source).unwrap();
        let sess = &std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(sess);
        prog.load_crate("regions", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        // This self-contained fixture supplies its own Fn trait.
        prog.prelude_crate = Some(0);
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let named = |name: &str| (0..prog.defs.len() as u32).rev().map(DefId).find(|&d| prog.name(d) == name).unwrap();
        // Select the last declaration: both Factory and S have a `get` method.
        let sig = |name: &str| tcx.fn_sig(&prog, named(name)).unwrap();
        let region = |t| match tcx.tys.kind(t) {
            TyKind::Ref(_, _, r) => tcx.tys.region_kind(*r), _ => panic!("expected reference"),
        };
        for name in ["named", "elided", "alias", "get"] {
            let s = sig(name);
            assert_eq!(s.bound_regions, 1, "{name}");
            assert_eq!(region(s.params[0]), RegionKind::ReBound(0, BoundVar(0)), "{name}");
            assert_eq!(region(s.ret), RegionKind::ReBound(0, BoundVar(0)), "{name}");
            assert!(tcx.tys.eq_mod_regions(s.ret, tcx.tys.erase_all_regions(s.ret)));
        }
        assert_eq!(region(sig("early").ret), RegionKind::ReEarlyParam(0));
        assert_eq!(region(sig("output").ret), RegionKind::ReEarlyParam(0));
        assert_eq!(region(sig("parent").ret), RegionKind::ReEarlyParam(2));
        assert_eq!(region(sig("inherited").params[0]), RegionKind::ReEarlyParam(1));
        let nested = sig("nested");
        let TyKind::FnPtr(ps, _, _, n) = tcx.tys.kind(nested.params[1]) else { panic!("expected fn pointer") };
        assert_eq!(*n, 0);
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReBound(1, BoundVar(0)));
        let objects = sig("objects");
        let TyKind::Adt(_, args, rs) = tcx.tys.kind(objects.params[1]) else { panic!("expected ADT") };
        let r = tcx.tys.region_args(*rs)[0];
        let TyKind::Dyn(_, _, _, _, object_r, _, _) = tcx.tys.kind(tcx.tys.list(*args)[0]) else { panic!("expected dyn") };
        assert_eq!(r, *object_r);
        let elided = sig("elided_object");
        assert_eq!(elided.bound_regions, 1);
        assert_eq!(region(elided.ret), RegionKind::ReBound(0, BoundVar(0)));
        let TyKind::Ptr(_, object) = tcx.tys.kind(elided.params[0]) else { panic!("ptr") };
        let TyKind::Dyn(_, _, _, rs, _, n, _) = tcx.tys.kind(*object) else { panic!("dyn") };
        assert_eq!(*n, 0);
        assert_eq!(tcx.tys.region_kind(tcx.tys.region_args(*rs)[0]), RegionKind::ReBound(1, BoundVar(0)));
        let higher = sig("higher");
        assert_eq!(higher.bound_regions, 2);
        let TyKind::Ref(_, object, _) = tcx.tys.kind(higher.params[1]) else { panic!("ref") };
        let TyKind::Dyn(_, args, bindings, regions, _, n, _) = tcx.tys.kind(*object) else { panic!("dyn") };
        assert_eq!(*n, 1);
        assert_eq!(tcx.tys.region_kind(tcx.tys.region_args(*regions)[0]), RegionKind::ReBound(0, BoundVar(0)));
        assert_eq!(region(tcx.tys.list(*args)[0]), RegionKind::ReBound(1, BoundVar(0)));
        assert_eq!(tcx.tys.list(*bindings)[0].2, 1);
        assert_eq!(region(tcx.tys.list(*bindings)[0].1), RegionKind::ReBound(0, BoundVar(0)));
        let TyKind::Dyn(_, _, erased_bindings, _, _, erased_n, _) = tcx.tys.kind(tcx.tys.erase_all_regions(*object)) else { panic!("dyn") };
        assert_eq!(*erased_n, 0);
        assert_eq!(tcx.tys.list(*erased_bindings)[0].2, 0);
        let TyKind::Ref(_, object, _) = tcx.tys.kind(sig("callable").params[0]) else { panic!("ref") };
        let TyKind::Dyn(_, args, bindings, _, _, n, _) = tcx.tys.kind(*object) else { panic!("dyn") };
        assert_eq!(*n, 1);
        let TyKind::Tuple(ps) = tcx.tys.kind(tcx.tys.list(*args)[0]) else { panic!("Args tuple") };
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReBound(0, BoundVar(0)));
        assert_eq!(tcx.tys.list(*bindings)[0].2, 1);
        assert_eq!(region(tcx.tys.list(*bindings)[0].1), RegionKind::ReBound(0, BoundVar(0)));
        let TyKind::Ref(_, object, _) = tcx.tys.kind(sig("substituted").params[1]) else { panic!("ref") };
        let TyKind::Dyn(_, args, bindings, _, _, n, _) = tcx.tys.kind(*object) else { panic!("dyn") };
        assert_eq!(*n, 1);
        assert_eq!(region(tcx.tys.list(*args)[0]), RegionKind::ReBound(1, BoundVar(0)));
        assert_eq!(region(tcx.tys.list(*bindings)[0].1), RegionKind::ReBound(0, BoundVar(0)));
        let TyKind::Ref(_, inner, _) = tcx.tys.kind(tcx.tys.list(*bindings)[0].1) else { panic!("ref") };
        assert_eq!(region(*inner), RegionKind::ReBound(1, BoundVar(0)));
        let def = named("takes");
        let TyKind::FnPtr(ps, r, _, n) = tcx.tys.kind(tcx.callable_signatures(&prog, def)[0].signature) else { panic!("fn") };
        assert_eq!(*n, 1);
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReBound(0, BoundVar(0)));
        assert_eq!(region(*r), RegionKind::ReBound(0, BoundVar(0)));

        // Borrowck still reconstructs closure regions in A3. Assert the typeck
        // results themselves: the UI suite cannot observe binder corruption.
        let check = |name| {
            let d = named(name);
            tcx.typeck(&prog, d)
        };
        let body = check("closure_annotations");
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        assert_eq!(body.closure_sigs.len(), 3);
        assert_eq!(body.inline_const_types.len(), 1);
        let first = body.closure_sigs[&body.closures[0]];
        let TyKind::FnPtr(ps, _, _, n) = tcx.tys.kind(first.bound) else { panic!("fn") };
        assert_eq!(*n, 2);
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReBound(0, BoundVar(0)));
        assert_eq!(region(tcx.tys.list(*ps)[1]), RegionKind::ReBound(0, BoundVar(1)));
        let TyKind::FnPtr(ps, _, _, n) = tcx.tys.kind(first.liberated) else { panic!("fn") };
        assert_eq!(*n, 0);
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReLateParam(
            crate::types::RegionScope::Closure { owner: body.def, expr: body.closures[0] },
            crate::types::LateParamKind::Anon(0)));
        let second = body.closure_sigs[&body.closures[1]];
        let TyKind::FnPtr(ps, _, _, n) = tcx.tys.kind(second.bound) else { panic!("fn") };
        assert_eq!(*n, 0);
        assert!(matches!(region(tcx.tys.list(*ps)[1]), RegionKind::ReLateParam(crate::types::RegionScope::Item(_), _)));
        let TyKind::FnPtr(inner, _, _, n) = tcx.tys.kind(tcx.tys.list(*ps)[0]) else { panic!("fn") };
        assert_eq!(*n, 1);
        assert_eq!(region(tcx.tys.list(*inner)[0]), RegionKind::ReBound(0, BoundVar(0)));
        let explicit_output = body.closure_sigs[&body.closures[2]];
        let TyKind::FnPtr(ps, r, _, n) = tcx.tys.kind(explicit_output.bound) else { panic!("fn") };
        assert_eq!(*n, 1);
        assert_eq!(region(tcx.tys.list(*ps)[0]), RegionKind::ReBound(0, BoundVar(0)));
        assert!(matches!(region(*r), RegionKind::ReVar(_)));
        // rustc rejects this deferred closure as not general enough; it does
        // not acquire a higher-ranked signature from a previous use of `g`.
        assert!(check("deferred_closure_rejected").errors.iter().any(|error| error.contains("E0277")));
        for (name, count) in [("closure_expected", 8), ("closure_routes", 6)] {
            let body = check(name);
            assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
            assert_eq!(body.closure_sigs.len(), count);
            for sig in body.closure_sigs.values() {
                let TyKind::FnPtr(ps, r, _, n) = tcx.tys.kind(sig.bound) else { panic!("fn") };
                assert_eq!(*n, 1);
                let expected_region = RegionKind::ReBound(0, BoundVar(0));
                assert_eq!(region(tcx.tys.list(*ps)[0]), expected_region);
                assert_eq!(region(*r), expected_region);
                assert!(!tcx.tys.has_infer(sig.bound));
                let (supplied, expected) = sig.annotation_eq.unwrap();
                let TyKind::FnPtr(ps, _, _, _) = tcx.tys.kind(supplied) else { panic!("fn") };
                assert!(matches!(region(tcx.tys.list(*ps)[0]), RegionKind::ReVar(_) | RegionKind::ReLateParam(..)));
                assert!(tcx.tys.eq_mod_regions(supplied, expected));
            }
        }
    }
}

crate::collected_fields!(RegionEnv, owner, enabled, names, next_param, anonymous_params, table, binder, infer_fresh);

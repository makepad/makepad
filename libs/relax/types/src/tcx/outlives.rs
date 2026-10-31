//! Declaration-owned region contracts. Lowered during collection, never from
//! borrowck. These are assumptions in a definition and obligations at a use.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlivesPredicate {
    Region(RegionId, RegionId),
    Type(TyId, RegionId),
}

#[derive(Clone, Debug, Default)]
pub struct RegionContract {
    /// Bound callable I/O retained before selection erases regions. Closure
    /// typeck and borrowck need the declaration binder and its lifetime arguments.
    pub callable_signatures: Vec<super::callable::CallableSignature>,
    /// Includes unused declared parameters. Names are for diagnostics/adapters.
    pub regions: Vec<(String, RegionId)>,
    pub predicates: Vec<OutlivesPredicate>,
    pub caller_predicates: Vec<OutlivesPredicate>,
    pub assumed_wf: Vec<TyId>,
    pub type_names: Vec<String>,
    pub own_types: Vec<Option<u32>>,
    pub self_ty: Option<TyId>,
    pub trait_types: Vec<TyId>,
    pub trait_regions: Vec<RegionId>,
}

impl Tcx {
    /// Region contracts of the traits, associated types and type aliases among `defs`.
    pub(super) fn collect_nominal_region_contracts(&mut self, cx: &EvalCx<'_>, prog: &Program, defs: impl Iterator<Item = DefId>) {
        for d in defs {
            let def = prog.def(d);
            if !matches!(def.kind, DefKind::Trait | DefKind::AssocTy | DefKind::TypeAlias) { continue; }
            let mark = crate::program::missing_mark();
            let mut env = self.item_env(prog,d);
            // (item_env of a trait already holds `Self` first, then its parameters)
            env.regions.enabled = true;
            let mut contract = self.region_contract(cx,prog,d,&env);
            if let Some(crate::ast::TraitItem { supers, .. }) = { let a = &prog.files[def.file as usize].ast; a.as_trait(&a.item(def.item).kind) } {
                self.contract_type_bounds(cx,prog,def.file,def.scope,&env,self.tys.intern(TyKind::Param(0)),
                    prog.files[def.file as usize].ast.list(*supers),true,&mut contract);
            }
            if def.kind == DefKind::Trait {
                let mut object_bounds = vec![];
                self.derived_object_regions(prog,d,env.regions.identity_args(&self.tys),&mut vec![],&mut object_bounds);
                for r in object_bounds {
                    let p = OutlivesPredicate::Type(self.tys.intern(TyKind::Param(0)),r);
                    if !contract.predicates.contains(&p) { contract.predicates.push(p); }
                }
            }
            self.nominal_region_contracts.insert(d,contract);
            if crate::program::missing_since(mark).is_empty() { self.contracts_waiting.remove(&d); } else { self.contracts_waiting.insert(d); }
        }
    }

    pub(super) fn region_contract(
        &self,
        cx: &EvalCx<'_>,
        prog: &Program,
        d: DefId,
        env: &GenEnv,
    ) -> RegionContract {
        let pd = prog.def(d);
        let mut c = RegionContract::default();
        c.regions = env
            .regions
            .names
            .iter()
            .map(|(n, r)| (n.clone(), self.tys.intern_region(*r)))
            .collect();
        c.regions
            .extend(env.regions.anonymous_params.iter().map(|i| {
                (
                    "'_".into(),
                    self.tys.intern_region(RegionKind::ReEarlyParam(*i)),
                )
            }));
        c.type_names = env
            .names
            .iter()
            .map(|n| {
                if *n == u32::MAX {
                    "?".into()
                } else {
                    prog.syms.str(*n).into()
                }
            })
            .collect();
        c.self_ty = env.self_ty;
        let parent = pd.parent;
        if parent != NO_DEF {
            let pp = prog.def(parent);
            if matches!(pp.kind, DefKind::Impl | DefKind::Trait) {
                let caller = !self.impl_trait.contains_key(&parent);
                // collect/predicates_of.rs:96-101 + gather_explicit_predicates_of: the
                // parent's predicates are lowered in the parent's own item (its
                // resolve_bound_vars table, resolve_bound_vars.rs:253-285); the child's
                // table holds only the child's lifetime uses, so a lifetime written in the
                // impl/trait header (`T: 'static`, `E: 'a`) must be resolved through the
                // parent's. The parent's early parameters keep their indices in the child.
                let mut parent_env = env.clone();
                let enabled = env.regions.enabled;
                parent_env.regions = self.bound_region_env(prog, parent);
                parent_env.regions.enabled = enabled;
                self.contract_generics(cx, prog, parent, &parent_env, caller, &mut c);
                if pp.kind == DefKind::Impl {
                    if let Some(t) = self.impl_self.get(&parent) {
                        c.assumed_wf.push(*t);
                    }
                    c.trait_types = self
                        .impl_trait_args
                        .get(&parent)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(rs) = self.impl_trait_regions.get(&parent) {
                        c.trait_regions = self.tys.region_args(*rs).to_vec();
                    }
                } else {
                    let n = self.trait_generic_count(prog, parent);
                    c.trait_types = (1..=n as u32)
                        .map(|i| self.tys.intern(TyKind::Param(i)))
                        .collect();
                    // Trait Self occupies full generic index zero. Reuse the
                    // inherited entries in the item's already-lowered environment.
                    if let Some(crate::ast::TraitItem { generics, .. }) = {
                        let a = &prog.files[pp.file as usize].ast;
                        a.as_trait(&a.item(pp.item).kind)
                    } {
                        let count = prog.files[pp.file as usize]
                            .ast
                            .list(generics.params)
                            .iter()
                            .filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_)))
                            .count();
                        c.trait_regions = env
                            .regions
                            .names
                            .iter()
                            .take(count)
                            .map(|(_, r)| self.tys.intern_region(*r))
                            .collect();
                    }
                    if self.declared_static_trait(prog, parent, &mut Vec::new()) {
                        let p = OutlivesPredicate::Type(
                            self.tys.intern(TyKind::Param(0)),
                            self.tys.intern_region(RegionKind::ReStatic),
                        );
                        c.predicates.push(p);
                        c.caller_predicates.push(p);
                    }
                }
            }
        }
        self.contract_generics(cx, prog, d, env, true, &mut c);
        let f = &prog.files[pd.file as usize];
        if let ItemKind::Fn(s, ..) = &f.ast.item(pd.item).kind {
            for p in f.ast.list(f.ast.fn_sig(*s).generics.params) {
                match p.kind {
                    GenericParamKind::Type(..) => c.own_types.push(
                        env.names
                            .iter()
                            .position(|n| prog.syms.str(*n) == f.text(p.name))
                            .map(|i| i as u32),
                    ),
                    GenericParamKind::Const(..) => c.own_types.push(None),
                    _ => {}
                }
            }
        }
        for (at, pi) in &env.impl_params {
            if let ast::Ty::ImplTrait(bs) = f.ast.ty(ast::TyId(*at)) {
                self.contract_type_bounds(
                    cx,
                    prog,
                    pd.file,
                    pd.scope,
                    env,
                    self.tys.intern(TyKind::Param(*pi)),
                    f.ast.list(*bs),
                    true,
                    &mut c,
                );
                let name = f
                    .ast
                    .list(*bs)
                    .iter()
                    .find_map(|b| {
                        if let ast::Bound::Trait { path, .. } = b {
                            f.ast.segs(path.segs).last().map(|s| f.text(s.name))
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                c.type_names[*pi as usize] = format!("impl {name}");
            }
        }
        c
    }

    fn contract_generics(
        &self,
        cx: &EvalCx<'_>,
        prog: &Program,
        d: DefId,
        env: &GenEnv,
        caller: bool,
        c: &mut RegionContract,
    ) {
        let pd = prog.def(d);
        let f = &prog.files[pd.file as usize];
        let g = match &f.ast.item(pd.item).kind {
            ItemKind::TraitAlias(_) => return,
            k => match f.ast.kind_generics(k) { Some(g) => g, None => return },
        };
        let region = |n| env.regions.resolve(cx.infcx, &self.tys, prog, pd.file, Some(n));
        for p in f.ast.list(g.params) {
            match p.kind {
                GenericParamKind::Lifetime(bs) => {
                    for b in f.ast.list(bs) {
                        let p = OutlivesPredicate::Region(region(p.name), region(*b));
                        c.predicates.push(p);
                        if caller {
                            c.caller_predicates.push(p);
                        }
                    }
                }
                GenericParamKind::Type(bs, _) => {
                    if let Some(i) = env
                        .names
                        .iter()
                        .position(|n| *n != u32::MAX && prog.syms.str(*n) == f.text(p.name))
                    {
                        self.contract_type_bounds(
                            cx,
                            prog,
                            pd.file,
                            pd.scope,
                            env,
                            self.tys.intern(TyKind::Param(i as u32)),
                            f.ast.list(bs),
                            caller,
                            c,
                        );
                    }
                }
                _ => {}
            }
        }
        for w in f.ast.list(g.where_) {
            match w {
                ast::WherePred::Lifetime(a, bs) => {
                    for b in f.ast.list(*bs) {
                        let p = OutlivesPredicate::Region(region(*a), region(*b));
                        c.predicates.push(p);
                        if caller {
                            c.caller_predicates.push(p);
                        }
                    }
                }
                ast::WherePred::Bound { ty, bounds, hrtb } => {
                    if !f.ast.list(*bounds).iter().any(|b| matches!(b, ast::Bound::Lifetime(_)) || matches!(b, ast::Bound::Trait { path, maybe: false, .. } if self.resolve_type_path_def(prog,pd.file,pd.scope,path).is_some_and(|d| self.declared_static_trait(prog,d,&mut Vec::new())))) { continue; }
                    let mut e = env.clone();
                    // `for<'a> T: 'a` implies T: 'static. Other higher-ranked
                    // predicate forms await the shared solver's binder clauses.
                    for p in f.ast.list(*hrtb) {
                        if matches!(p.kind, GenericParamKind::Lifetime(_)) {
                            e.regions
                                .names
                                .push((f.text(p.name).into(), RegionKind::ReStatic));
                        }
                    }
                    let t = self.lower_ty(cx, prog, pd.file, pd.scope, &e, *ty);
                    self.contract_type_bounds(
                        cx,
                        prog,
                        pd.file,
                        pd.scope,
                        &e,
                        t,
                        f.ast.list(*bounds),
                        caller,
                        c,
                    );
                }
            }
        }
    }

    fn contract_type_bounds(
        &self,
        cx: &EvalCx<'_>,
        prog: &Program,
        file: u32,
        module: u32,
        env: &GenEnv,
        t: TyId,
        bounds: &[ast::Bound],
        caller: bool,
        c: &mut RegionContract,
    ) {
        for b in bounds {
            let r = match b {
                ast::Bound::Lifetime(n) => env.regions.resolve(cx.infcx, &self.tys, prog, file, Some(*n)),
                ast::Bound::Trait {
                    path, maybe: false, ..
                } if self
                    .resolve_type_path_def(prog, file, module, path)
                    .is_some_and(|d| self.declared_static_trait(prog, d, &mut Vec::new())) =>
                {
                    self.tys.intern_region(RegionKind::ReStatic)
                }
                _ => continue,
            };
            let p = OutlivesPredicate::Type(t, r);
            c.predicates.push(p);
            if caller {
                c.caller_predicates.push(p);
            }
        }
    }

    fn declared_static_trait(&self, prog: &Program, d: DefId, seen: &mut Vec<DefId>) -> bool {
        if seen.contains(&d) {
            return false;
        }
        seen.push(d);
        let pd = prog.def(d);
        let f = &prog.files[pd.file as usize];
        let (g, bs) = match &f.ast.item(pd.item).kind {
            ItemKind::Trait(r) => { let t = f.ast.row(*r); (&t.generics, t.supers) }
            ItemKind::TraitAlias(r) => { let t = f.ast.row(*r); (&t.generics, t.bounds) }
            _ => return false,
        };
        let mut bounds = f.ast.list(bs).to_vec();
        for w in f.ast.list(g.where_) {
            if let ast::WherePred::Bound { ty, bounds: bs, .. } = w {
                if matches!(f.ast.ty(*ty), ast::Ty::Path(p) if p.segs.len() == 1 && f.ast.segs(p.segs)[0].kind() == SegKind::SelfType)
                {
                    bounds.extend_from_slice(f.ast.list(*bs));
                }
            }
        }
        bounds.iter().any(|b| match b {
            ast::Bound::Lifetime(n) => f.text(*n) == "'static",
            ast::Bound::Trait {
                path, maybe: false, ..
            } => self
                .resolve_type_path_def(prog, pd.file, pd.scope, path)
                .is_some_and(|d| self.declared_static_trait(prog, d, seen)),
            _ => false,
        })
    }
}

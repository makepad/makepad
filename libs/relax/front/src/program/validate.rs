//! AST validation of the own crates' items (rustc_ast_passes ast_validation.rs): visibility
//! qualifiers where none are allowed (E0449), duplicate fields (E0124), items without a body.
use super::*;

impl Program {
    pub fn ast_validation_errors(&self, own: &[bool]) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.defs.len() {
            let d = DefId(i as u32);
            let def = self.def(d);
            if !own.get(def.krate as usize).copied().unwrap_or(false) || def.item.0 == u32::MAX {
                continue;
            }
            if self.item_defs.get(&(def.file, def.item.0)) != Some(&d) {
                continue;
            }
            let f = &self.files[def.file as usize];
            if f.token_input {
                continue;
            }
            let ast = &f.ast;
            let it = ast.item(def.item);
            let mut err = |pos: u32, msg: String| out.push(self.record_error(def.file, pos, "parse", msg));
            // `pub(in path)` on the item and on its fields
            self.check_vis_path(&mut err, ast, def, &it.vis, it.lo);
            let field_vis = |data: VariantData, out: &mut Vec<(Vis, u32)>| {
                if let VariantData::Tuple(l) | VariantData::Struct(l) = data {
                    for fd in ast.list(l) {
                        out.push((fd.vis, fd.ty_span.0));
                    }
                }
            };
            let mut fvs: Vec<(Vis, u32)> = Vec::new();
            if let Some(st) = ast.as_struct(&it.kind) {
                field_vis(st.data, &mut fvs);
            } else if let Some(u) = ast.as_union(&it.kind) {
                field_vis(VariantData::Struct(u.fields), &mut fvs);
            }
            for (v, pos) in fvs {
                self.check_vis_path(&mut err, ast, def, &v, pos);
            }
            // check_attr: attributes with a fixed list of targets (rustc_attr_parsing AllowedTargets) are not for generic parameters
            if let Some(g) = ast.kind_generics(&it.kind) {
                for gp in ast.list(g.params) {
                    for a in ast.list(gp.attrs) {
                        if a.toks.hi <= a.toks.lo || f.tok(a.toks.lo).lo >= f.src_len { continue; }
                        let name = f.tok_text(a.toks.lo);
                        let what = match gp.kind { GenericParamKind::Lifetime(_) => "lifetime parameters", GenericParamKind::Type(..) => "type parameters", GenericParamKind::Const(..) => "const parameters" };
                        match name {
                            "inline" | "cold" | "track_caller" | "no_mangle" | "export_name" | "used" | "target_feature" | "link_name" | "link_section" | "naked" | "non_exhaustive" =>
                                err(f.tok(a.toks.lo).lo, format!("`#[{}]` attribute cannot be used on {}", name, what)),
                            "repr" => err(f.tok(a.toks.lo).lo, "E0517: attribute should be applied to a struct, enum, or union".into()),
                            _ => {}
                        }
                    }
                }
            }
            // ast_validation: a where-clause bound may quantify lifetimes in the bounded type or in the bound, not both
            if let Some(g) = ast.kind_generics(&it.kind) {
                for w in ast.list(g.where_) {
                    let WherePred::Bound { hrtb, bounds, .. } = w else { continue };
                    if ast.list(*hrtb).is_empty() { continue; }
                    if ast.list(*bounds).iter().any(|b| matches!(b, Bound::Trait { hrtb, .. } if !ast.list(*hrtb).is_empty())) {
                        err(it.lo, "E0316: nested quantification of lifetimes".into());
                    }
                }
            }
            // ast_validation check_fn_decl / the limit on parameters of a function
            if let ItemKind::Fn(sig, ..) = it.kind {
                if ast.list(ast.fn_sig(sig).params).len() > 65535 {
                    err(it.lo, "function can not have more than 65535 arguments".into());
                }
            }
            // `impl Trait` nested in another (E0666) and in the output of `Fn` sugar (E0562), in a fn's signature
            if let ItemKind::Fn(sig, ..) = it.kind {
                let sig = ast.fn_sig(sig);
                let feats = self.crates[def.krate as usize].features(self);
                let fn_trait_return = feats.has("impl_trait_in_fn_trait_return");
                for p in ast.list(sig.params) { self.check_nested_impl_trait(&mut err, ast, p.ty, false, fn_trait_return, true, it.lo); }
                if let Some(r) = sig.ret { self.check_nested_impl_trait(&mut err, ast, r, false, fn_trait_return, false, it.lo); }
            }
            let not_permitted = |vis: &Vis| !matches!(vis, Vis::Private);
            match def.kind {
                DefKind::Impl => {
                    let Some(imp) = ast.as_impl(&it.kind) else { continue };
                    // type_of: `Self` is no type of the impl's own header
                    if self.ty_has_self(ast, imp.self_ty, 0) {
                        err(it.lo, "`Self` is not valid in the self type of an impl block".into());
                    }
                    if not_permitted(&it.vis) {
                        err(it.lo, "E0449: visibility qualifiers are not permitted here -- note: place qualifiers on individual impl items instead".into());
                    }
                    for &c in ast.list(imp.items) {
                        let ci = ast.item(c);
                        if imp.trait_.is_some() && not_permitted(&ci.vis) {
                            err(ci.lo, "E0449: visibility qualifiers are not permitted here -- note: trait items always share the visibility of their trait".into());
                        }
                        self.body_required(ast, &mut err, ci, Some("`impl`"));
                        self.assoc_const_name(&mut err, f, ci);
                        self.unresolved_assoc_macro(&mut err, f, ci);
                    }
                }
                DefKind::Trait => {
                    let Some(tr) = ast.as_trait(&it.kind) else { continue };
                    for &c in ast.list(tr.items) {
                        let ci = ast.item(c);
                        self.patterns_in_bodiless(ast, &mut err, ci);
                        if not_permitted(&ci.vis) {
                            err(ci.lo, "E0449: visibility qualifiers are not permitted here -- note: trait items always share the visibility of their trait".into());
                        }
                        self.assoc_const_name(&mut err, f, ci);
                        self.unresolved_assoc_macro(&mut err, f, ci);
                    }
                }
                DefKind::Struct => {
                    if let Some(s) = ast.as_struct(&it.kind) {
                        self.duplicate_fields(&mut err, ast, f, s.data);
                    }
                }
                DefKind::Union => {
                    if let Some(u) = ast.as_union(&it.kind) {
                        if ast.list(u.fields).is_empty() { err(it.lo, "unions cannot have zero fields".into()); }
                        self.duplicate_fields(&mut err, ast, f, VariantData::Struct(u.fields));
                    }
                }
                DefKind::Enum => {
                    let Some(e) = ast.as_enum(&it.kind) else { continue };
                    for v in ast.list(e.variants) {
                        if let VariantData::Tuple(fs) | VariantData::Struct(fs) = v.data {
                            for fd in ast.list(fs) {
                                if not_permitted(&fd.vis) {
                                    err(fd.ty_span.0, "E0449: visibility qualifiers are not permitted here -- note: enum variants and their fields always share the visibility of the enum".into());
                                }
                            }
                        }
                        self.duplicate_fields(&mut err, ast, f, v.data);
                    }
                }
                DefKind::Fn | DefKind::Const | DefKind::Static | DefKind::TypeAlias => {
                    self.body_required(ast, &mut err, it, None);
                }
                // an `extern` block's items (rustc_ast_passes check_foreign_item_*)
                DefKind::ForeignFn => {
                    if let ItemKind::Fn(sig, ..) = it.kind {
                        let sig = ast.fn_sig(sig);
                        if sig.konst {
                            err(it.lo, "functions in `extern` blocks cannot have `const` qualifier".into());
                        }
                        if sig.asyncness {
                            err(it.lo, "functions in `extern` blocks cannot have `async` qualifier".into());
                        }
                    }
                }
                DefKind::ForeignStatic => {
                    if let ItemKind::Static(_, _, e) = it.kind {
                        if e.get().is_some() {
                            err(it.lo, "incorrect `static` inside `extern` block".into());
                        }
                    }
                }
                DefKind::ForeignTy => {
                    if let ItemKind::TypeAlias(r) = it.kind {
                        if ast.row(r).ty.is_some() {
                            err(it.lo, "incorrect `type` inside `extern` block".into());
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// rustc_ast_passes ast_validation.rs `outer_impl_trait_span` / `with_impl_trait`: an `impl Trait` inside the
    /// generic arguments of another is E0666, except in an associated-type binding or the output of `Fn` sugar,
    /// which start afresh; rustc_ast_lowering path.rs: the output of `Fn` sugar may not hold `impl Trait`
    /// without `impl_trait_in_fn_trait_return` (E0562).
    fn check_nested_impl_trait(&self, err: &mut impl FnMut(u32, String), ast: &Ast, t: TyId, in_impl: bool, fn_ret_ok: bool, arg_pos: bool, pos: u32) {
        match ast.ty(t) {
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.check_nested_impl_trait(err, ast, *x, in_impl, fn_ret_ok, arg_pos, pos),
            Ty::Tuple(l) => for &x in ast.list(*l) { self.check_nested_impl_trait(err, ast, x, in_impl, fn_ret_ok, arg_pos, pos) },
            Ty::Fn(f) => {
                let f = ast.fn_ptr(*f);
                for &x in ast.list(f.params) { self.check_nested_impl_trait(err, ast, x, in_impl, fn_ret_ok, arg_pos, pos); }
                if let Some(r) = f.ret { self.check_nested_impl_trait(err, ast, r, in_impl, fn_ret_ok, arg_pos, pos); }
            }
            Ty::Path(p) => self.check_nested_in_segs(err, ast, ast.segs(p.segs), in_impl, fn_ret_ok, arg_pos, pos),
            Ty::ImplTrait(bs) => {
                if in_impl { err(pos, "E0666: nested `impl Trait` is not allowed".into()); }
                if !ast.list(*bs).iter().any(|b| matches!(b, Bound::Trait { .. })) { err(pos, "at least one trait must be specified".into()); }
                // rustc_ast_lowering: `use<..>` of an argument-position `impl Trait`
                if arg_pos && ast.list(*bs).iter().any(|b| matches!(b, Bound::Use(_))) {
                    err(pos, "`use<...>` precise capturing syntax not allowed in argument-position `impl Trait`".into());
                }
                self.check_nested_in_bounds(err, ast, *bs, true, fn_ret_ok, arg_pos, pos);
            }
            Ty::DynTrait(bs, _) => self.check_nested_in_bounds(err, ast, *bs, in_impl, fn_ret_ok, arg_pos, pos),
            _ => {}
        }
    }

    fn check_nested_in_bounds(&self, err: &mut impl FnMut(u32, String), ast: &Ast, bs: List<Bound>, in_impl: bool, fn_ret_ok: bool, arg_pos: bool, pos: u32) {
        for b in ast.list(bs) {
            if let Bound::Trait { path, .. } = b { self.check_nested_in_segs(err, ast, ast.segs(path.segs), in_impl, fn_ret_ok, arg_pos, pos); }
        }
    }

    fn check_nested_in_segs(&self, err: &mut impl FnMut(u32, String), ast: &Ast, segs: &[PathSeg], in_impl: bool, fn_ret_ok: bool, arg_pos: bool, pos: u32) {
        for seg in segs {
            let Some(a) = ast.gargs(seg.args()) else { continue };
            match a {
                GenericArgs::Angle(v) => for g in ast.list(*v) {
                    match g {
                        GenericArg::Type(x) => self.check_nested_impl_trait(err, ast, *x, in_impl, fn_ret_ok, arg_pos, pos),
                        // `Item = impl Debug` and `Item: Bound` start afresh
                        GenericArg::Binding(_, _, x) => self.check_nested_impl_trait(err, ast, *x, false, fn_ret_ok, arg_pos, pos),
                        GenericArg::Constraint(_, _, bs) => self.check_nested_in_bounds(err, ast, *bs, false, fn_ret_ok, arg_pos, pos),
                        _ => {}
                    }
                },
                GenericArgs::Paren(ins, out) => {
                    for &x in ast.list(*ins) { self.check_nested_impl_trait(err, ast, x, in_impl, fn_ret_ok, arg_pos, pos); }
                    if let Some(o) = out {
                        if !fn_ret_ok && ty_holds_impl_trait(ast, *o) {
                            err(pos, "E0562: `impl Trait` is not allowed in the return type of `Fn` trait bounds".into());
                        } else {
                            self.check_nested_impl_trait(err, ast, *o, false, fn_ret_ok, false, pos);
                        }
                    }
                }
            }
        }
    }

    /// rustc_resolve build_reduced_graph `resolve_visibility`: `pub(in path)` names an ancestor module.
    fn check_vis_path(&self, err: &mut impl FnMut(u32, String), ast: &Ast, def: &Def, vis: &Vis, pos: u32) {
        let Vis::In(row) = vis else { return };
        let p = ast.row(*row);
        let segs = path_segs(self, def.file, p);
        let Some(first) = segs.first() else { return };
        let edition = self.crates[def.krate as usize].edition;
        if edition >= 2018 && !p.global() && first.0 == SegKind::Ident {
            err(pos, "relative paths are not supported in visibilities in 2018 edition or later".into());
            return;
        }
        let Some(target) = self.resolve_mod_path(def.scope, p.global(), &segs, edition < 2018, false) else {
            // the first segment that does not resolve
            let k = (1..=segs.len()).find(|&k| self.resolve_mod_path(def.scope, p.global(), &segs[..k], edition < 2018, false).is_none()).unwrap_or(1) - 1;
            let name = self.syms.str(segs[k.min(segs.len() - 1)].1);
            err(pos, format!("E0433: failed to resolve: cannot find module or crate `{name}` in the crate root"));
            return;
        };
        let td = self.def(target);
        if td.kind != DefKind::Mod {
            err(pos, format!("E0577: expected module, found {} `{}`", self.def_descr(target), self.syms.str(td.name)));
        } else if td.krate != def.krate || !self.mod_within(def.scope, td.sub) {
            err(pos, "E0742: visibilities can only be restricted to ancestor modules".into());
        }
    }

    fn duplicate_fields(&self, err: &mut impl FnMut(u32, String), ast: &Ast, f: &ParsedFile, data: VariantData) {
        let (VariantData::Struct(fs) | VariantData::Tuple(fs)) = data else { return };
        let mut seen: Vec<&str> = Vec::new();
        for fd in ast.list(fs) {
            let Some(n) = fd.name else { continue };
            let name = f.text(n);
            if seen.contains(&name) {
                err(n.lo, format!("E0124: field `{name}` is already declared"));
            } else {
                seen.push(name);
            }
        }
    }

    /// A macro call among the members of an impl or trait that expanded to nothing because no macro
    /// of that name is in scope (the parser expands what resolves).
    fn unresolved_assoc_macro(&self, err: &mut impl FnMut(u32, String), f: &ParsedFile, it: &Item) {
        let ItemKind::Mac(m) = it.kind else { return };
        let ast = &f.ast;
        let Some(sg) = ast.segs(ast.row(m).path.segs).last() else { return };
        let name = f.text(sg.name).trim_start_matches("r#");
        if self.macros.defs.contains_key(name) || crate::builtins::macro_def(name).is_some() {
            return;
        }
        err(it.lo, format!("cannot find macro `{name}` in this scope"));
    }

    /// ast_validation check_decl_no_pat: a trait method without a body takes only plain identifier and `_`
    /// patterns (a `mut` identifier is the lint `patterns_in_fns_without_body`).
    fn patterns_in_bodiless(&self, ast: &Ast, err: &mut impl FnMut(u32, String), it: &Item) {
        let ItemKind::Fn(sig, None, _) = it.kind else { return };
        let sig = ast.fn_sig(sig);
        if sig.lazy_body.is_some() { return; }
        for prm in ast.list(sig.params) {
            match ast.pat(prm.pat) {
                Pat::Wild | Pat::Ident { by_ref: false, sub: None, .. } => {}
                _ => err(it.lo, "E0642: patterns aren't allowed in functions without bodies".into()),
            }
        }
    }

    /// Does a type name `Self` anywhere (rustc_hir `find_self_aliases`)?
    fn ty_has_self(&self, ast: &Ast, t: TyId, depth: u32) -> bool {
        if depth > 32 { return false; }
        let segs = |segs: SegList, depth: u32| -> bool {
            ast.segs(segs).iter().any(|s| match ast.gargs(s.args()) {
                Some(GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| match a {
                    GenericArg::Type(x) | GenericArg::Binding(_, _, x) => self.ty_has_self(ast, *x, depth + 1),
                    GenericArg::Constraint(_, _, bs) => self.bounds_have_self(ast, *bs, depth + 1),
                    _ => false,
                }),
                Some(GenericArgs::Paren(ins, out)) => ast.list(*ins).iter().any(|x| self.ty_has_self(ast, *x, depth + 1)) || out.is_some_and(|o| self.ty_has_self(ast, o, depth + 1)),
                None => false,
            })
        };
        match ast.ty(t) {
            Ty::Path(p) => {
                if let Some(q) = ast.qself(p) { if self.ty_has_self(ast, q.ty, depth + 1) { return true; } }
                ast.segs(p.segs).first().is_some_and(|s| s.kind() == SegKind::SelfType) || segs(p.segs, depth)
            }
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.ty_has_self(ast, *x, depth + 1),
            Ty::Tuple(l) => ast.list(*l).iter().any(|x| self.ty_has_self(ast, *x, depth + 1)),
            Ty::Fn(f) => { let f = ast.fn_ptr(*f); ast.list(f.params).iter().any(|x| self.ty_has_self(ast, *x, depth + 1)) || f.ret.is_some_and(|r| self.ty_has_self(ast, r, depth + 1)) }
            Ty::ImplTrait(bs) | Ty::DynTrait(bs, _) => self.bounds_have_self(ast, *bs, depth + 1),
            _ => false,
        }
    }

    fn bounds_have_self(&self, ast: &Ast, bs: List<Bound>, depth: u32) -> bool {
        ast.list(bs).iter().any(|b| match b {
            Bound::Trait { path, .. } => ast.segs(path.segs).iter().any(|s| match ast.gargs(s.args()) {
                Some(GenericArgs::Angle(v)) => ast.list(*v).iter().any(|a| matches!(a, GenericArg::Type(x) | GenericArg::Binding(_, _, x) if self.ty_has_self(ast, *x, depth + 1))),
                Some(GenericArgs::Paren(ins, out)) => ast.list(*ins).iter().any(|x| self.ty_has_self(ast, *x, depth + 1)) || out.is_some_and(|o| self.ty_has_self(ast, o, depth + 1)),
                None => false,
            }),
            _ => false,
        })
    }

    /// `const _` is only an item of a module or block (ast_validation `check_item_named`).
    fn assoc_const_name(&self, err: &mut impl FnMut(u32, String), f: &ParsedFile, it: &Item) {
        if matches!(it.kind, ItemKind::Const(..)) && f.text(it.name) == "_" {
            err(it.lo, "`const` items in this context need a name".to_string());
        }
    }

    /// A free item / an impl item must have its body (`fn f();`, `const C: u8;`, `type T;`).
    fn body_required(&self, ast: &Ast, err: &mut impl FnMut(u32, String), it: &Item, assoc_in: Option<&str>) {
        let (what, free_what, missing) = match it.kind {
            ItemKind::Fn(sig, None, _) => ("function", "function", ast.fn_sig(sig).lazy_body.is_none()),
            ItemKind::Const(_, e) => ("constant", "constant item", e.get().is_none()),
            ItemKind::Static(_, _, e) => ("static", "static item", e.get().is_none()),
            ItemKind::TypeAlias(r) => ("type", "type alias", ast.row(r).ty.is_none()),
            _ => return,
        };
        if !missing {
            return;
        }
        match assoc_in {
            Some(owner) => err(it.lo, format!("associated {what} in {owner} without body")),
            None if what == "function" => err(it.lo, "free function without a body".to_string()),
            None => err(it.lo, format!("free {free_what} without body")),
        }
    }
}

/// Whether a written type is, or directly wraps, an `impl Trait` (the output of `Fn` sugar).
fn ty_holds_impl_trait(ast: &Ast, t: TyId) -> bool {
    match ast.ty(t) {
        Ty::ImplTrait(_) => true,
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => ty_holds_impl_trait(ast, *x),
        Ty::Tuple(l) => ast.list(*l).iter().any(|&x| ty_holds_impl_trait(ast, x)),
        _ => false,
    }
}

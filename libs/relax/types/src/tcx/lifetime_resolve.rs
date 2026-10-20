// ported from N:rustc_resolve/src/late.rs @8d1a764
use super::*;
use super::resolve_bound_vars::{BinderId, LifetimeId, LifetimeKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LifetimeRes { Param(LifetimeId), Static, Infer, Error }

#[derive(Clone, Debug)]
pub(super) struct LifetimeDecl {
    pub id: LifetimeId,
    pub binder: BinderId,
    pub name: Option<String>,
    pub span: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LifetimeResolution {
    pub(super) inherited: Vec<LifetimeDecl>,
    pub(super) declarations: Vec<LifetimeDecl>,
    pub(super) uses: HashMap<LifetimeKey, LifetimeRes>,
    pub(super) binders: HashMap<BinderId, Vec<LifetimeId>>,
}

impl LifetimeResolution {
    /// The resolution of the elided lifetime of the reference type `ty` (`&T`).
    pub(crate) fn reference(&self, ty: u32) -> Option<LifetimeRes> { self.uses.get(&LifetimeKey::Reference(ty)).copied() }
    /// The resolution of the lifetime written at `lo..hi` (`'a`, `'_`, `'static`).
    pub(crate) fn named(&self, lo: u32, hi: u32) -> Option<LifetimeRes> { self.uses.get(&LifetimeKey::Named(lo, hi)).copied() }
    /// The lifetime declared by the parameter whose name is at `pos` (a `for<'a>` binder's).
    pub(crate) fn declared_at(&self, pos: u32) -> Option<LifetimeRes> { self.declarations.iter().find(|d| d.span == pos).map(|d| LifetimeRes::Param(d.id)) }
    /// The resolution of the elided lifetime of an `&self` receiver.
    pub(crate) fn self_reference(&self) -> Option<LifetimeRes> { self.uses.get(&LifetimeKey::SelfReference).copied() }
    /// The resolution of the `index`th hidden lifetime of path segment `seg` (`Foo` for `Foo<'a>`).
    pub(crate) fn path(&self, seg: u32, index: u32) -> Option<LifetimeRes> { self.uses.get(&LifetimeKey::Path(seg, index)).copied() }
    /// The lifetimes in scope of the item itself: inherited from its parents, declared by its own
    /// generics, and the anonymous ones its signature creates (a fn's late-bound lifetimes).
    pub(crate) fn in_scope_lifetimes(&self) -> Vec<LifetimeId> {
        self.inherited.iter().map(|d| d.id).chain(self.declarations.iter().filter(|d| d.binder == BinderId::Item).map(|d| d.id)).collect()
    }
    /// The named lifetimes in scope of the item: (id, name, position, declared by the item itself),
    /// inherited ones first.
    pub(crate) fn named_in_scope(&self) -> Vec<(LifetimeId, String, u32, bool)> {
        let inh = self.inherited.iter().filter_map(|d| d.name.clone().map(|n| (d.id, n, d.span, false)));
        let own = self.declarations.iter().filter(|d| d.binder == BinderId::Item).filter_map(|d| d.name.clone().map(|n| (d.id, n, d.span, true)));
        inh.chain(own).collect()
    }
    /// No lifetime declared, inherited or used anywhere in the owner (most items).
    fn is_empty(&self) -> bool { self.inherited.is_empty() && self.declarations.is_empty() && self.uses.is_empty() && self.binders.is_empty() }
    /// The answer to share: every empty resolution is the same value, kept once.
    pub(crate) fn share(self) -> std::sync::Arc<LifetimeResolution> {
        static EMPTY: std::sync::OnceLock<std::sync::Arc<LifetimeResolution>> = std::sync::OnceLock::new();
        if self.is_empty() { EMPTY.get_or_init(|| crate::zone_alloc::kept_with(Default::default)).clone() } else { std::sync::Arc::new(self) }
    }
    /// memory report: heap bytes owned (excluding the struct)
    pub(crate) fn heap_bytes(&self) -> usize {
        fn hm<K, V, S>(m: &std::collections::HashMap<K, V, S>) -> usize { m.capacity() * (std::mem::size_of::<(K, V)>() + 1) * 8 / 7 }
        self.inherited.capacity() * std::mem::size_of::<LifetimeDecl>() + self.declarations.capacity() * std::mem::size_of::<LifetimeDecl>()
            + hm(&self.uses) + hm(&self.binders) + self.binders.values().map(|v| v.capacity() * 4).sum::<usize>()
    }
}

pub(crate) fn resolve_lifetimes(tcx: &Tcx, prog: &Program, owner: DefId, parent: Option<&LifetimeResolution>) -> LifetimeResolution {
    let def = prog.def(owner);
    let mut resolver = LifetimeResolver {
        tcx, prog, owner, file: def.file, module: tcx.item_module(prog, owner),
        resolution: Default::default(), ribs: Vec::new(), candidates: None,
    };
    resolver.push(LifetimeRibKind::Item);
    if let Some(parent) = parent {
        resolver.resolution.inherited.extend_from_slice(&parent.inherited);
        for declaration in &parent.declarations {
            if declaration.binder == BinderId::Item { resolver.resolution.inherited.push(declaration.clone()); }
        }
        let mut bindings = Vec::new();
        for declaration in &resolver.resolution.inherited {
            if let Some(name) = &declaration.name { bindings.push((name.clone(), declaration.id)); }
        }
        resolver.ribs.push(LifetimeRib { kind: LifetimeRibKind::Generics { binder: None, poly_trait: false }, bindings });
    }
    resolver.visit_item();
    resolver.resolution
}

#[derive(Clone, Copy)]
enum LifetimeRibKind {
    Item,
    /// late.rs LifetimeRibKind::Generics; `poly_trait` is LifetimeBinderKind::PolyTrait, the
    /// (maybe implicit) `for<>` of a trait bound, where parenthesized sugar binds (late.rs:1366-1396).
    Generics { binder: Option<BinderId>, poly_trait: bool },
    AnonymousCreateParameter { binder: BinderId, report_in_path: bool },
    AnonymousReportError,
    Elided { res: LifetimeRes, error_in_path: bool },
    ElisionFailure,
    ConstParamTy,
    ConcreteAnonConst,
    ImplTrait,
}
struct LifetimeRib { kind: LifetimeRibKind, bindings: Vec<(String, LifetimeId)> }

pub(super) struct LifetimeResolver<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    owner: DefId,
    file: u32,
    module: u32,
    resolution: LifetimeResolution,
    ribs: Vec<LifetimeRib>,
    candidates: Option<Vec<LifetimeRes>>,
}

impl LifetimeResolver<'_> {
    fn error(&self, span: u32, message: &str) {
        self.tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { body_id: self.owner, span }, message.to_owned());
    }

    fn declare(&mut self, binder: BinderId, name: Option<ast::Ident>, span: u32) -> LifetimeId {
        let id = LifetimeId { owner: self.owner, local: self.resolution.declarations.len() as u32 };
        self.resolution.declarations.push(LifetimeDecl {
            id, binder, name: name.map(|n| self.prog.files[self.file as usize].lifetime_text(n).into_owned()), span,
        });
        self.resolution.binders.entry(binder).or_default().push(id);
        id
    }

    /// The where-predicate whose binder encloses the current position.
    fn pred(&self) -> Option<u32> {
        self.ribs.iter().rev().find_map(|rib| match rib.kind { LifetimeRibKind::Generics { binder: Some(BinderId::Where(n)), .. } => Some(n), _ => None })
    }

    fn record_lifetime_use(&mut self, key: LifetimeKey, res: LifetimeRes) {
        if let Some(pred) = self.pred() { self.resolution.uses.insert(key.scoped(Some(pred)), res); }
        self.resolution.uses.insert(key, res);
        if matches!(res, LifetimeRes::Param(_) | LifetimeRes::Static) {
            if let Some(candidates) = &mut self.candidates { candidates.push(res); }
        }
    }

    fn resolve_lifetime(&mut self, key: LifetimeKey, name: Option<ast::Ident>, span: u32, in_path: bool) {
        let text = name.map(|n| self.prog.files[self.file as usize].lifetime_text(n));
        let text = text.as_deref();
        if text == Some("'static") { self.record_lifetime_use(key, LifetimeRes::Static); return; }
        if text.is_none() || text == Some("'_") {
            self.resolve_anonymous_lifetime(key, span, in_path && name.is_none());
            return;
        }
        let text = text.unwrap();
        for rib in self.ribs.iter().rev() {
            if let Some((_, id)) = rib.bindings.iter().find(|(n, _)| n == text) {
                let id = *id;
                self.record_lifetime_use(key, LifetimeRes::Param(id));
                return;
            }
            match rib.kind {
                LifetimeRibKind::Item => break,
                LifetimeRibKind::ConstParamTy | LifetimeRibKind::ConcreteAnonConst => {
                    self.error(span, "non-static lifetime is not allowed in this constant context");
                    self.record_lifetime_use(key, LifetimeRes::Error);
                    return;
                }
                _ => {}
            }
        }
        self.error(span, &format!("E0261: use of undeclared lifetime name `{}`", text));
        self.record_lifetime_use(key, LifetimeRes::Error);
    }

    fn resolve_anonymous_lifetime(&mut self, key: LifetimeKey, span: u32, in_path: bool) {
        for i in (0..self.ribs.len()).rev() {
            match self.ribs[i].kind {
                LifetimeRibKind::AnonymousCreateParameter { binder, report_in_path } => {
                    if in_path && report_in_path {
                        self.error(span, "E0726: implicit elided lifetime not allowed here");
                        self.record_lifetime_use(key, LifetimeRes::Error);
                    } else {
                        let id = self.declare(binder, None, span);
                        self.record_lifetime_use(key, LifetimeRes::Param(id));
                    }
                    return;
                }
                LifetimeRibKind::Elided { res, error_in_path } => {
                    if in_path && error_in_path {
                        self.error(span, "E0726: implicit elided lifetime not allowed here");
                        self.record_lifetime_use(key, LifetimeRes::Error);
                    } else { self.record_lifetime_use(key, res); }
                    return;
                }
                LifetimeRibKind::AnonymousReportError | LifetimeRibKind::ElisionFailure => {
                    self.error(span, "E0106: missing lifetime specifier");
                    self.record_lifetime_use(key, LifetimeRes::Error);
                    return;
                }
                LifetimeRibKind::Item => break,
                LifetimeRibKind::Generics { .. } | LifetimeRibKind::ConstParamTy | LifetimeRibKind::ImplTrait => {}
                LifetimeRibKind::ConcreteAnonConst => panic!("anonymous constant has no inference lifetime rib"),
            }
        }
        self.error(span, "E0106: missing lifetime specifier");
        self.record_lifetime_use(key, LifetimeRes::Error);
    }

    fn push_generics(&mut self, binder: BinderId, params: ast::List<ast::GenericParam>) {
        let mut bindings = Vec::new();
        for p in self.prog.files[self.file as usize].ast.list(params) {
            if !matches!(p.kind, GenericParamKind::Lifetime(_)) { continue; }
            let name = self.prog.files[self.file as usize].lifetime_text(p.name).into_owned();
            if name == "'static" || name == "'_" { self.error(p.name.lo, &format!("E0262: invalid lifetime parameter name: `{}`", name)); }
            if bindings.iter().any(|(n, _)| *n == name) { self.error(p.name.lo, &format!("E0403: the name `{}` is already used for a generic parameter in this item's generic parameters", name)); }
            if self.ribs.iter().any(|r| r.bindings.iter().any(|(n, _)| *n == name)) {
                self.error(p.name.lo, &format!("E0496: lifetime name `{}` shadows a lifetime name that is already in scope", name));
            }
            let id = self.declare(binder, Some(p.name), p.name.lo);
            self.resolution.uses.insert(LifetimeKey::Named(p.name.lo, p.name.hi), LifetimeRes::Param(id));
            if let BinderId::Where(n) = binder { self.resolution.uses.insert(LifetimeKey::Named(p.name.lo, p.name.hi).scoped(Some(n)), LifetimeRes::Param(id)); }
            bindings.push((name, id));
        }
        self.ribs.push(LifetimeRib { kind: LifetimeRibKind::Generics { binder: Some(binder), poly_trait: matches!(binder, BinderId::Trait(_)) }, bindings });
    }

    fn push(&mut self, kind: LifetimeRibKind) { self.ribs.push(LifetimeRib { kind, bindings: Vec::new() }); }

    fn visit_item(&mut self) {
        let def = self.prog.def(self.owner);
        let ast = &self.prog.files[self.file as usize].ast;
        match ast.item(def.item).kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = ast.fn_sig(sig);
                self.push_generics(BinderId::Item, sig.generics.params);
                self.visit_generics(&sig.generics);
                let inputs: Vec<_> = ast.list(sig.params).iter().map(|p| p.ty).collect();
                self.resolve_fn_signature(BinderId::Item, sig.self_param, &inputs, sig.ret, sig.asyncness);
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                for param in ast.list(sig.params) { self.visit_pat(param.pat); }
                if let Some(body) = self.prog.fn_body(self.file, def.item).expect("collected function body") { self.visit_block(body); }
                self.ribs.pop();
                self.ribs.pop();
            }
            ItemKind::Struct(r) => {
                let ast::StructItem { generics, data } = *ast.row(r);
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                self.visit_variant_data(data);
                self.ribs.pop();
            }
            ItemKind::Union(r) => {
                let ast::UnionItem { generics, fields } = *ast.row(r);
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                self.visit_fields(fields);
                self.ribs.pop();
            }
            ItemKind::Enum(r) => {
                let ast::EnumItem { generics, variants } = *ast.row(r);
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                for variant in ast.list(variants) {
                    self.visit_variant_data(variant.data);
                    if let Some(expr) = variant.disc { self.visit_anon_const(expr); }
                }
                self.ribs.pop();
            }
            ItemKind::TypeAlias(r) => {
                let ast::TypeAliasItem { generics, bounds, ty } = *ast.row(r);
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                self.visit_bounds(bounds);
                if let Some(ty) = ty { self.visit_ty(ty); }
                self.ribs.pop();
            }
            ItemKind::Trait(_) | ItemKind::TraitAlias(_) => {
                let (generics, supers) = match ast.item(def.item).kind { ItemKind::Trait(r) => { let t = ast.row(r); (t.generics, t.supers) } ItemKind::TraitAlias(r) => { let t = ast.row(r); (t.generics, t.bounds) } _ => unreachable!() };
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                self.visit_bounds(supers);
                self.ribs.pop();
            }
            ItemKind::Impl(r) => {
                let ast::ImplItem { generics, trait_, self_ty, .. } = *ast.row(r);
                self.push_generics(BinderId::Item, generics.params);
                self.visit_generics(&generics);
                // late.rs:3549-3552: implicit elided path lifetimes in an impl header are an error.
                self.push(LifetimeRibKind::AnonymousCreateParameter { binder: BinderId::Item, report_in_path: true });
                if let Some(path) = trait_ { self.visit_path(&path, false); }
                self.visit_ty(self_ty);
                self.ribs.pop();
                self.ribs.pop();
            }
            ItemKind::Const(ty, _) => {
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Static, error_in_path: def.kind == DefKind::AssocConst });
                if let Some(ty) = ty.get() { self.visit_ty(ty); }
                self.ribs.pop();
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                if let Some(expr) = self.prog.const_body(self.file, def.item).expect("collected constant body") { self.visit_expr(expr); }
                self.ribs.pop();
            }
            ItemKind::Static(_, ty, _) => {
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Static, error_in_path: def.kind == DefKind::AssocConst });
                self.visit_ty(ty);
                self.ribs.pop();
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                if let Some(expr) = self.prog.const_body(self.file, def.item).expect("collected static body") { self.visit_expr(expr); }
                self.ribs.pop();
            }
            ItemKind::Mod(_) | ItemKind::ForeignMod(..) | ItemKind::ExternCrate(_) | ItemKind::Use { .. } | ItemKind::MacroRules(_) | ItemKind::Mac(_) => {}
        }
    }

    fn visit_variant_data(&mut self, data: VariantData) {
        match data { VariantData::Tuple(fields) | VariantData::Struct(fields) => self.visit_fields(fields), VariantData::Unit => {} }
    }

    fn visit_fields(&mut self, fields: ast::List<ast::FieldDef>) {
        for field in self.prog.files[self.file as usize].ast.list(fields) {
            self.visit_ty(field.ty);
            if let Some(expr) = field.default { self.visit_anon_const(expr); }
        }
    }

    fn visit_anon_const(&mut self, expr: ast::ExprId) {
        let outer = self.candidates.take();
        self.push(LifetimeRibKind::ConcreteAnonConst);
        self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
        self.visit_expr(expr);
        self.ribs.pop();
        self.ribs.pop();
        self.candidates = outer;
    }

    fn visit_block(&mut self, block: ast::BlockId) {
        let outer_module = self.module;
        if let Some(&module) = self.prog.block_mods.get(&(self.file, block.0)) { self.module = module; }
        let ast = &self.prog.files[self.file as usize].ast;
        for stmt in ast.list(ast.block(block).stmts) {
            match *stmt {
                ast::Stmt::Let(r) => {
                    let ast::LetStmt { pat, ty, init, else_, .. } = *ast.row(r);
                    self.visit_pat(pat);
                    if let Some(ty) = ty { self.visit_ty(ty); }
                    if let Some(expr) = init { self.visit_expr(expr); }
                    if let Some(block) = else_ { self.visit_block(block); }
                }
                ast::Stmt::Expr(expr, _) => self.visit_expr(expr),
                ast::Stmt::Attrs(r) => self.visit_expr(ast.row(r).expr),
                ast::Stmt::Item(_) | ast::Stmt::Empty => {}
            }
        }
        self.module = outer_module;
    }

    fn find_lifetime_for_self(&self, root: ast::TyId) -> Vec<LifetimeRes> {
        let ast = &self.prog.files[self.file as usize].ast;
        let parent = self.prog.def(self.prog.def(self.owner).parent);
        // late.rs:2730-2745: the impl self type counts as `Self` only when it unambiguously
        // names one type constructor: a struct, union, enum or primitive type.
        let primitive = |file: u32, path: &ast::Path| -> Option<&'static str> {
            let a = &self.prog.files[file as usize].ast;
            if !path.no_qself() || path.global() || path.segs.len() != 1 || a.gargs(a.segs(path.segs)[0].args()).is_some() { return None; }
            let text = self.prog.files[file as usize].text(a.segs(path.segs)[0].name);
            crate::program::PRIMS.iter().find(|(name, _)| *name == text).map(|(name, _)| *name)
        };
        let impl_self = if parent.kind == DefKind::Impl {
            let parent_ast = &self.prog.files[parent.file as usize].ast;
            if let Some(ast::ImplItem { self_ty, .. }) = parent_ast.as_impl(&parent_ast.item(parent.item).kind).copied() {
                if let ast::Ty::Path(path) = parent_ast.ty(self_ty) {
                    match self.tcx.resolve_type_path_def(self.prog, parent.file, parent.scope, path) {
                        Some(d) if matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Union | DefKind::Enum) => Some((Some(d), None)),
                        Some(_) => None,
                        None => primitive(parent.file, path).map(|name| (None, Some(name))),
                    }
                } else { None }
            } else { None }
        } else { None };
        let mut result = Vec::new();
        let mut todo = vec![root];
        while let Some(ty) = todo.pop() {
            if let ast::Ty::Ref(name, _, inner) = *ast.ty(ty) {
                let mut referent = vec![inner];
                let mut contains_self = false;
                while let Some(child) = referent.pop() {
                    match *ast.ty(child) {
                        ast::Ty::ImplicitSelf => { contains_self = true; break; }
                        ast::Ty::Path(path) if path.no_qself() => {
                            if (path.segs.len() == 1 && ast.segs(path.segs)[0].kind() == SegKind::SelfType) ||
                                impl_self.is_some_and(|(d, prim)| match d {
                                    Some(d) => self.tcx.resolve_type_path_def(self.prog, self.file, self.module, &path) == Some(d),
                                    None => self.tcx.resolve_type_path_def(self.prog, self.file, self.module, &path).is_none() && primitive(self.file, &path) == prim,
                                }) {
                                contains_self = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                    child_types(ast, child, &mut referent);
                }
                if contains_self {
                    let key = name.map_or(LifetimeKey::Reference(ty.0), |lt| LifetimeKey::Named(lt.lo, lt.hi));
                    let res = self.resolution.uses[&key];
                    if !result.contains(&res) { result.push(res); }
                }
            }
            child_types(ast, ty, &mut todo);
        }
        result
    }

    fn visit_lifetime(&mut self, name: ast::Ident) {
        self.resolve_lifetime(LifetimeKey::Named(name.lo, name.hi), Some(name), name.lo, false);
    }

    fn visit_generics(&mut self, generics: &ast::Generics) {
        let ast = &self.prog.files[self.file as usize].ast;
        self.push(LifetimeRibKind::AnonymousReportError);
        for p in ast.list(generics.params) {
            match p.kind {
                GenericParamKind::Lifetime(bounds) => for &lt in ast.list(bounds) { self.visit_lifetime(lt); },
                GenericParamKind::Type(bounds, default) => {
                    self.visit_bounds(bounds);
                    if let Some(ty) = default { self.visit_ty(ty); }
                }
                GenericParamKind::Const(ty, default) => {
                    self.push(LifetimeRibKind::ConstParamTy);
                    self.visit_ty(ty);
                    self.ribs.pop();
                    if let Some(expr) = default { self.visit_anon_const(expr); }
                }
            }
        }
        for (i, pred) in ast.list(generics.where_).iter().enumerate() {
            match *pred {
                ast::WherePred::Lifetime(lt, bounds) => {
                    self.visit_lifetime(lt);
                    for &lt in ast.list(bounds) { self.visit_lifetime(lt); }
                }
                ast::WherePred::Bound { hrtb, ty, bounds } => {
                    let outer = self.candidates.take();
                    self.push_generics(BinderId::Where(generics.where_.lo + i as u32), hrtb);
                    self.visit_ty(ty);
                    self.visit_bounds(bounds);
                    self.ribs.pop();
                    self.candidates = outer;
                }
            }
        }
        self.ribs.pop();
    }

    fn visit_bounds(&mut self, bounds: ast::List<ast::Bound>) {
        let ast = &self.prog.files[self.file as usize].ast;
        for bound in ast.list(bounds) {
            match *bound {
                ast::Bound::Lifetime(lt) => self.visit_lifetime(lt),
                ast::Bound::Use(args) => self.visit_generic_args(args),
                ast::Bound::Trait { hrtb, path, .. } => {
                    self.push_generics(BinderId::Trait(path.segs.lo), hrtb);
                    self.visit_path(&path, false);
                    let rib = self.ribs.pop().unwrap();
                    // late.rs:3304: preserve uses of outer lifetimes, removing
                    // only parameters introduced by this poly-trait binder.
                    if let Some(candidates) = &mut self.candidates {
                        candidates.retain(|res| !rib.bindings.iter().any(|(_, id)| *res == LifetimeRes::Param(*id)));
                    }
                }
            }
        }
    }

    fn visit_generic_args(&mut self, args: ast::GenericArgsId) {
        let ast = &self.prog.files[self.file as usize].ast;
        match ast.gargs(args) {
            Some(GenericArgs::Angle(arguments)) => {
                for arg in ast.list(*arguments) {
                    match *arg {
                        GenericArg::Lifetime(lt) => self.visit_lifetime(lt),
                        GenericArg::Type(ty) => self.visit_ty(ty),
                        GenericArg::Const(expr) => self.visit_anon_const(expr),
                        GenericArg::Binding(_, args, ty) => {
                            // late.rs:1341-1347: anonymous lifetimes in GAT arguments of a constraint are forbidden.
                            self.push(LifetimeRibKind::AnonymousReportError);
                            self.visit_generic_args(args);
                            self.ribs.pop();
                            self.visit_ty(ty);
                        }
                        GenericArg::Constraint(_, args, bounds) => {
                            self.push(LifetimeRibKind::AnonymousReportError);
                            self.visit_generic_args(args);
                            self.ribs.pop();
                            self.visit_bounds(bounds);
                        }
                    }
                }
            }
            Some(GenericArgs::Paren(..)) => panic!("parenthesized generic arguments require a trait binder"),
            None => {}
        }
    }

    /// late.rs:1366-1410 (visit_path_segment) + :2214-2425 (resolve_elided_lifetimes_in_path).
    /// `inferred`: the path is an expression or pattern path (PathSource::Expr/Pat/Struct/
    /// TupleStruct), whose implicit lifetimes are left to type checking.
    fn visit_path(&mut self, path: &ast::Path, inferred: bool) {
        let ast = &self.prog.files[self.file as usize].ast;
        if let Some(qself) = ast.qself(path) {
            self.visit_ty(qself.ty);
            if let Some(trait_path) = qself.trait_path { self.visit_path(&trait_path, inferred); }
        }
        self.resolve_elided_lifetimes_in_path(path, inferred);
        for segment in ast.segs(path.segs) {
            if let Some(GenericArgs::Paren(params, ret)) = ast.gargs(segment.args()) {
                // Probe the lifetime ribs: the nearest poly-trait binder introduces the
                // sugar's lifetimes; with no binder in between, walk the arguments normally.
                for i in (0..self.ribs.len()).rev() {
                    match self.ribs[i].kind {
                        LifetimeRibKind::Generics { binder: Some(binder), poly_trait: true } => {
                            let outer = self.candidates.take();
                            self.resolve_fn_signature(binder, None, ast.list(*params), *ret, false);
                            self.candidates = outer;
                            break;
                        }
                        LifetimeRibKind::Item | LifetimeRibKind::Generics { .. } => {
                            for &ty in ast.list(*params) { self.visit_ty(ty); }
                            if let Some(ty) = ret { self.visit_ty(*ty); }
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                self.visit_generic_args(segment.args());
            }
        }
    }

    /// late.rs:2214-2296: the resolved prefix (`proj_start`) picks the one segment whose
    /// declaration's lifetimes are elided; expression and pattern paths infer them.
    fn resolve_elided_lifetimes_in_path(&mut self, path: &ast::Path, inferred: bool) {
        let ast = &self.prog.files[self.file as usize].ast;
        let segs = ast.segs(path.segs);
        let mut proj_start = segs.len();
        let base = loop {
            if proj_start == 0 { return; }
            let prefix = ast::Path { segs: path.segs.prefix(proj_start), ..*path };
            if let Some(def) = self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, &prefix) { break def; }
            proj_start -= 1;
        };
        let has_generic_args = |i: usize| ast.gargs(segs[i].args()).is_some();
        for (i, segment) in segs.iter().enumerate() {
            if matches!(ast.gargs(segment.args()), Some(GenericArgs::Angle(args)) if ast.list(*args).iter().any(|a| matches!(a, GenericArg::Lifetime(_)))) { continue; }
            let type_def = match self.prog.def(base).kind {
                DefKind::AssocTy if i + 2 == proj_start => self.prog.def(base).parent,
                DefKind::Variant if i + 2 == proj_start && has_generic_args(i) => self.prog.def(base).parent,
                DefKind::Variant if i + 1 == proj_start && !(i > 0 && has_generic_args(i - 1)) => self.prog.def(base).parent,
                DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Trait if i + 1 == proj_start => base,
                _ => continue,
            };
            let d = self.prog.def(type_def);
            let a = &self.prog.files[d.file as usize].ast;
            let generics = match a.item(d.item).kind {
                ItemKind::Fn(..) | ItemKind::Impl(_) => continue,
                k => match a.kind_generics(&k) { Some(g) => *g, None => continue },
            };
            let count = a.list(generics.params).iter().filter(|param| matches!(param.kind, GenericParamKind::Lifetime(_))).count() as u32;
            for index in 0..count {
                let key = LifetimeKey::Path(path.segs.lo + i as u32, index);
                if inferred { self.record_lifetime_use(key, LifetimeRes::Infer); }
                else { self.resolve_lifetime(key, None, segment.name.lo, true); }
            }
        }
    }

    fn visit_ty(&mut self, ty: ast::TyId) {
        let ast = &self.prog.files[self.file as usize].ast;
        match *ast.ty(ty) {
            ast::Ty::Ref(name, _, inner) => {
                let key = name.map_or(LifetimeKey::Reference(ty.0), |lt| LifetimeKey::Named(lt.lo, lt.hi));
                self.resolve_lifetime(key, name, name.map_or(ast.item(self.prog.def(self.owner).item).lo, |lt| lt.lo), false);
                self.visit_ty(inner);
            }
            ast::Ty::Ptr(_, inner) | ast::Ty::Slice(inner) | ast::Ty::Paren(inner) => self.visit_ty(inner),
            ast::Ty::Array(inner, length) => { self.visit_ty(inner); self.visit_anon_const(length); }
            ast::Ty::Tuple(types) => for &ty in ast.list(types) { self.visit_ty(ty); },
            ast::Ty::Path(path) => {
                // late.rs:925-945: a path naming a trait is a bare trait object; its Fn-sugar
                // anonymous lifetimes are placed in a poly-trait rib of its own
                let bare_trait = path.no_qself() && self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, &path).is_some_and(|def| self.prog.def(def).kind == DefKind::Trait);
                if bare_trait {
                    self.push_generics(BinderId::Trait(path.segs.lo), ast::List::EMPTY);
                    self.visit_path(&path, false);
                    let rib = self.ribs.pop().unwrap();
                    if let Some(candidates) = &mut self.candidates {
                        candidates.retain(|res| !rib.bindings.iter().any(|(_, id)| *res == LifetimeRes::Param(*id)));
                    }
                } else {
                    self.visit_path(&path, false);
                }
            }
            ast::Ty::Fn(pointer) => {
                let pointer = ast.fn_ptr(pointer);
                let outer = self.candidates.take();
                self.push_generics(BinderId::FnPointer(ty.0), pointer.hrtb);
                self.resolve_fn_signature(BinderId::FnPointer(ty.0), None, ast.list(pointer.params), pointer.ret, false);
                self.ribs.pop();
                self.candidates = outer;
            }
            ast::Ty::DynTrait(bounds, _) => self.visit_bounds(bounds),
            ast::Ty::ImplTrait(bounds) => {
                let outer = self.candidates.take();
                self.push(LifetimeRibKind::ImplTrait);
                self.visit_bounds(bounds);
                self.ribs.pop();
                self.candidates = outer;
            }
            ast::Ty::Never | ast::Ty::Infer | ast::Ty::ImplicitSelf | ast::Ty::CVarArgs => {}
            ast::Ty::Mac(_) => self.error(0, "unexpanded type macro during lifetime resolution"),
        }
    }

    /// late.rs:2474-2647: a single distinct lifetime in a single input permits
    /// output elision. A receiver containing Self takes precedence.
    fn resolve_fn_signature(&mut self, binder: BinderId, receiver: Option<ast::SelfParam>, inputs: &[ast::TyId], output: Option<ast::TyId>, report_in_path: bool) {
        let outer = self.candidates.take();
        self.push(LifetimeRibKind::AnonymousCreateParameter { binder, report_in_path });
        let mut candidate = None;
        let mut ambiguous = false;
        let mut self_candidate = None;
        if let Some(receiver) = receiver {
            self.candidates = Some(Vec::new());
            match receiver {
                ast::SelfParam::Value(_) => {},
                ast::SelfParam::Ref(name, _) => {
                    let key = name.map_or(LifetimeKey::SelfReference, |lt| LifetimeKey::Named(lt.lo, lt.hi));
                    self.resolve_lifetime(key, name, self.prog.files[self.file as usize].ast.item(self.prog.def(self.owner).item).lo, false);
                    self_candidate = self.resolution.uses.get(&key).copied();
                }
                ast::SelfParam::Typed(_, ty) => {
                    self.visit_ty(ty);
                    let found = self.find_lifetime_for_self(ty);
                    if found.len() == 1 { self_candidate = Some(found[0]); }
                    else if found.len() > 1 { ambiguous = true; }
                }
            }
            self.candidates = None;
        }
        for &ty in inputs {
            self.candidates = Some(Vec::new());
            self.visit_ty(ty);
            let mut distinct = Vec::new();
            for res in self.candidates.take().unwrap() {
                if !distinct.contains(&res) { distinct.push(res); }
            }
            if !distinct.is_empty() {
                if candidate.is_some() || distinct.len() > 1 { ambiguous = true; }
                else { candidate = Some(distinct[0]); }
            }
        }
        let output_res = self_candidate.or(if ambiguous { None } else { candidate });
        self.push(output_res.map_or(LifetimeRibKind::ElisionFailure, |res| LifetimeRibKind::Elided { res, error_in_path: false }));
        if let Some(ty) = output { self.visit_ty(ty); }
        self.ribs.pop();
        self.ribs.pop();
        self.candidates = outer;
    }
    fn visit_pat(&mut self, p: ast::PatId) {
        match *self.prog.files[self.file as usize].ast.pat(p) {
            ast::Pat::Ident { sub: Some(p), .. } | ast::Pat::Ref(_, p) | ast::Pat::Paren(p) | ast::Pat::Box(p) => self.visit_pat(p),
            ast::Pat::Lit(e) => self.visit_expr(e),
            ast::Pat::Range(a, b, _) => { if let Some(e) = a { self.visit_expr(e); } if let Some(e) = b { self.visit_expr(e); } }
            ast::Pat::Path(p) => self.visit_path(&p, true),
            ast::Pat::TupleStruct(p, ps) => { self.visit_path(&p, true); for p in self.prog.files[self.file as usize].ast.list(ps) { self.visit_pat(*p); } }
            ast::Pat::Struct(p, fs, _) => { self.visit_path(&p, true); for f in self.prog.files[self.file as usize].ast.list(fs) { self.visit_pat(f.pat); } }
            ast::Pat::Tuple(ps) | ast::Pat::Slice(ps) | ast::Pat::Or(ps) => for p in self.prog.files[self.file as usize].ast.list(ps) { self.visit_pat(*p); },
            ast::Pat::Mac(m) => { let m = *self.prog.files[self.file as usize].ast.row(m); self.visit_mac(m) }
            ast::Pat::Wild | ast::Pat::Rest | ast::Pat::Ident { sub: None, .. } => {}
        }
    }
    fn visit_expr(&mut self, e: ast::ExprId) {
        match self.prog.files[self.file as usize].ast.expr(e).kind {
            ast::ExprKind::Repeat(v, n) => { self.visit_expr(v); self.visit_anon_const(n); }
            ast::ExprKind::ConstBlock(b) => {
                self.visit_block(b);
            }
            ast::ExprKind::Closure(row) => {
                let ast::ClosureRow { params, ret, body, .. } = *self.prog.files[self.file as usize].ast.row(row);
                let outer = self.candidates.take();
                // late.rs:1197-1212 + resolve_params :4067-4077: parameter patterns under
                // Elided(Infer), then the parameter types under AnonymousCreateParameter.
                self.push(LifetimeRibKind::AnonymousCreateParameter { binder: BinderId::Closure(e.0), report_in_path: false });
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                for p in self.prog.files[self.file as usize].ast.list(params) { self.visit_pat(p.pat); }
                self.ribs.pop();
                for p in self.prog.files[self.file as usize].ast.list(params) { if let Some(t) = p.ty { self.visit_ty(t); } }
                self.ribs.pop();
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                if let Some(t) = ret { self.visit_ty(t); } self.visit_expr(body); self.ribs.pop(); self.candidates = outer;
            }
            ast::ExprKind::Async(_, b) => {
                let outer = self.candidates.take();
                self.push(LifetimeRibKind::Elided { res: LifetimeRes::Infer, error_in_path: false });
                self.visit_block(b); self.ribs.pop(); self.candidates = outer;
            }
            ast::ExprKind::Path(p) => self.visit_path(&p, true),
            ast::ExprKind::Unary(_, x) | ast::ExprKind::Field(x, _) | ast::ExprKind::TupleField(x, _) |
            ast::ExprKind::AddrOf(_, _, x) | ast::ExprKind::Try(x) | ast::ExprKind::Await(x) | ast::ExprKind::Paren(x) | ast::ExprKind::Box(x) => self.visit_expr(x),
            ast::ExprKind::Binary(_, a, b) | ast::ExprKind::Assign(a, b) | ast::ExprKind::AssignOp(_, a, b) | ast::ExprKind::Index(a, b) => { self.visit_expr(a); self.visit_expr(b); }
            ast::ExprKind::Call(f, args) => { self.visit_expr(f); for e in self.prog.files[self.file as usize].ast.list(args) { self.visit_expr(*e); } }
            ast::ExprKind::MethodCall(recv, row) => {
                let ast::MethodCallRow { turbofish, args, .. } = *self.prog.files[self.file as usize].ast.row(row);
                self.visit_expr(recv); self.visit_generic_args(turbofish); for e in self.prog.files[self.file as usize].ast.list(args) { self.visit_expr(*e); }
            }
            ast::ExprKind::Cast(e, t) => { self.visit_expr(e); self.visit_ty(t); }
            ast::ExprKind::Block(b, _) | ast::ExprKind::Unsafe(b) | ast::ExprKind::Loop(b, _) => self.visit_block(b),
            ast::ExprKind::If(e, b, other) => { self.visit_expr(e); self.visit_block(b); if let Some(e) = other.get() { self.visit_expr(e); } }
            ast::ExprKind::Let(p, e) => { self.visit_pat(p); self.visit_expr(e); }
            ast::ExprKind::Match(e, arms) => {
                self.visit_expr(e); for a in self.prog.files[self.file as usize].ast.list(arms) { self.visit_pat(a.pat); if let Some(e) = a.guard { self.visit_expr(e); } self.visit_expr(a.body); }
            }
            ast::ExprKind::While(row) => { let ast::WhileRow { cond: e, body: b, .. } = *self.prog.files[self.file as usize].ast.row(row); self.visit_expr(e); self.visit_block(b); }
            ast::ExprKind::For(row) => { let ast::ForRow { pat: p, iter: e, body: b, .. } = *self.prog.files[self.file as usize].ast.row(row); self.visit_pat(p); self.visit_expr(e); self.visit_block(b); }
            ast::ExprKind::Break(_, v) | ast::ExprKind::Return(v) | ast::ExprKind::Yield(v) => if let Some(e) = v.get() { self.visit_expr(e); },
            ast::ExprKind::Tuple(es) | ast::ExprKind::Array(es) => for e in self.prog.files[self.file as usize].ast.list(es) { self.visit_expr(*e); },
            ast::ExprKind::Struct(row) => {
                let ast::StructRow { path: p, fields: fs, base, .. } = *self.prog.files[self.file as usize].ast.row(row);
                self.visit_path(&p, true); for f in self.prog.files[self.file as usize].ast.list(fs) { self.visit_expr(f.expr); }
                if let Some(e) = base.get() { self.visit_expr(e); }
            }
            ast::ExprKind::Range(a, b, _) => { if let Some(e) = a.get() { self.visit_expr(e); } if let Some(e) = b.get() { self.visit_expr(e); } }
            ast::ExprKind::Mac(m) => { let m = *self.prog.files[self.file as usize].ast.row(m); self.visit_mac(m) }
            ast::ExprKind::Lit(_) | ast::ExprKind::Continue(_) | ast::ExprKind::Underscore => {}
        }
    }
    fn visit_mac(&mut self, m: ast::MacCall) {
        if m.args == ast::NO_MAC_ARGS { return; }
        let args = *self.prog.files[self.file as usize].ast.mac_arg(m.args);
        for e in self.prog.files[self.file as usize].ast.list(args.exprs) { self.visit_expr(*e); }
        if let Some(p) = args.pat { self.visit_pat(p); } if let Some(e) = args.guard { self.visit_expr(e); }
        if let Some(asm) = args.asm {
            for op in self.prog.files[self.file as usize].ast.list(asm.ops) {
                if let Some(e) = op.input { if op.dir == ast::AsmDir::Const { self.visit_anon_const(e); } else { self.visit_expr(e); } }
                if let Some(e) = op.output { self.visit_expr(e); }
            }
        }
    }
}

fn child_types(ast: &ast::Ast, ty: ast::TyId, out: &mut Vec<ast::TyId>) {
    match *ast.ty(ty) {
        ast::Ty::Ref(_, _, ty) | ast::Ty::Ptr(_, ty) | ast::Ty::Slice(ty) | ast::Ty::Array(ty, _) | ast::Ty::Paren(ty) => out.push(ty),
        ast::Ty::Tuple(types) => out.extend_from_slice(ast.list(types)),
        ast::Ty::Fn(pointer) => {
            let pointer = ast.fn_ptr(pointer);
            out.extend_from_slice(ast.list(pointer.params));
            out.extend(pointer.ret);
        }
        ast::Ty::Path(path) => path_types(ast, &path, out),
        ast::Ty::DynTrait(bounds, _) | ast::Ty::ImplTrait(bounds) => bound_types(ast, bounds, out),
        _ => {}
    }
}
fn path_types(ast: &ast::Ast, path: &ast::Path, out: &mut Vec<ast::TyId>) {
    if let Some(qself) = ast.qself(path) {
        out.push(qself.ty);
        if let Some(path) = qself.trait_path { path_types(ast, &path, out); }
    }
    for segment in ast.segs(path.segs) { argument_types(ast, segment.args(), out); }
}
fn argument_types(ast: &ast::Ast, args: ast::GenericArgsId, out: &mut Vec<ast::TyId>) {
    match ast.gargs(args) {
        Some(GenericArgs::Angle(arguments)) => for arg in ast.list(*arguments) {
            match *arg {
                GenericArg::Type(ty) => out.push(ty),
                GenericArg::Binding(_, args, ty) => { argument_types(ast, args, out); out.push(ty); }
                GenericArg::Constraint(_, _, bounds) => bound_types(ast, bounds, out),
                _ => {}
            }
        },
        Some(GenericArgs::Paren(inputs, output)) => { out.extend_from_slice(ast.list(*inputs)); out.extend(output); }
        None => {}
    }
}
fn bound_types(ast: &ast::Ast, bounds: ast::List<ast::Bound>, out: &mut Vec<ast::TyId>) {
    for bound in ast.list(bounds) {
        match *bound {
            ast::Bound::Trait { path, .. } => path_types(ast, &path, out),
            ast::Bound::Use(args) => argument_types(ast, args, out),
            ast::Bound::Lifetime(_) => {}
        }
    }
}

crate::collected_enum!(LifetimeRes; 0 => Param(id), 1 => Static, 2 => Infer, 3 => Error);
crate::collected_fields!(LifetimeDecl, id, binder, name, span);
crate::collected_fields!(LifetimeResolution, inherited, declarations, uses, binders);

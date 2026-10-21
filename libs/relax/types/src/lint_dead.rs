//! `dead_code` (rustc_passes/src/dead.rs): which items of the checked crates are never used.
//!
//! Every body owner's references are collected while the bodies are type-checked (`collect_body`:
//! the resolved paths, method calls, field reads, patterns, types it mentions); the signatures'
//! and fields' references are read from the syntax (`TypeWalker`). `finish` then runs rustc's
//! worklist: seeds are `main`, the effectively public items (`#[allow(dead_code)]` items,
//! `#[no_mangle]` / `#[used]` / lang items, `const _`), and whatever a live item mentions is
//! live; trait impls and their items are live when the trait (item) and the implementing type
//! are. What stays dead is reported per module as rustc's `check_mod_deathness` does, grouped
//! by parent and lint level.
//!
//! Check-only (reports through the lint sink); `makepad run` never calls this.

use crate::ast::*;
use crate::ast_visit::{self, Visitor};
use crate::lint::{self, Level, Lint};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::TyKind;
use relax_session::sync::Mutex;
use std::collections::{HashMap, HashSet};

pub const DEAD_CODE: Lint = lint::lint("dead_code");
pub const DEAD_CODE_PUB_IN_BINARY: Lint = lint::lint("dead_code_pub_in_binary");

/// A field of a struct, union or enum variant: (ADT, variant index, field index).
type FieldKey = (DefId, u32, u32);

#[derive(Clone, Copy, Debug)]
enum Ref {
    /// `check_def_id`: the definition is live and explored
    Use(DefId),
    /// a field that is read
    Field(FieldKey),
}

/// What one body owner references, and the diagnostics it emits when it is live.
struct BodyInfo {
    refs: Vec<Ref>,
    /// (position, message, attributes in scope): `useless assignment ... to itself`
    notes: Vec<(u32, String, Vec<Attr>)>,
}

/// Positions (file, site) of the names patterns bind (they are definitions, not uses).
static BINDING_SITES: Mutex<Option<HashSet<(u32, u32)>>> = Mutex::new(None);

pub fn is_binding_site(file: u32, pos: u32) -> bool {
    BINDING_SITES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&(file, pos)))
}

static BODY_REFS: Mutex<Option<HashMap<DefId, BodyInfo>>> = Mutex::new(None);

pub fn reset() {
    *BINDING_SITES.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashSet::new());
    crate::lint_imports::reset();
    *BODY_REFS.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashMap::new());
}

// ================================================================ type and signature walker

/// References made by types, bounds and generics: rustc's `visit_path` on every path.
struct TypeWalker<'a> {
    prog: &'a Program,
    file: u32,
    ast: &'a Ast,
    module: u32,
    params: HashSet<String>,
    /// the enclosing impl (Self is its self type) or trait (Self is a parameter of it)
    self_def: Option<DefId>,
    out: Vec<Ref>,
}

impl<'a> TypeWalker<'a> {
    pub(crate) fn new(prog: &'a Program, d: DefId) -> TypeWalker<'a> {
        let def = prog.def(d);
        let mut w = TypeWalker { prog, file: def.file, ast: &prog.files[def.file as usize].ast, module: def.scope, params: HashSet::new(), self_def: None, out: Vec::new() };
        // generic parameters of the definition and the impl / trait around it
        let mut cur = d;
        let mut guard = 0;
        while cur != NO_DEF && guard < 64 {
            guard += 1;
            let c = prog.def(cur);
            if matches!(c.kind, DefKind::Mod) { break; }
            if !matches!(c.kind, DefKind::Closure | DefKind::AnonConst | DefKind::Variant) {
                let a = &prog.files[c.file as usize].ast;
                if let Some(g) = a.kind_generics(&a.item(c.item).kind) {
                    for p in a.list(g.params) {
                        if !matches!(p.kind, GenericParamKind::Lifetime(_)) { w.params.insert(prog.files[c.file as usize].text(p.name).to_string()); }
                    }
                }
                if matches!(c.kind, DefKind::Impl | DefKind::Trait) && w.self_def.is_none() { w.self_def = Some(cur); }
            }
            cur = c.parent;
        }
        w
    }

    fn use_def(&mut self, d: DefId) {
        // naming a variant uses its enum too
        let def = self.prog.def(d);
        if def.kind == DefKind::Variant { self.out.push(Ref::Use(def.parent)); }
        self.out.push(Ref::Use(d));
    }

    fn ty(&mut self, t: TyId) {
        let ast = self.ast;
        match *ast.ty(t) {
            Ty::Path(p) => self.path(&p, true),
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Paren(x) => self.ty(x),
            Ty::Array(x, len) => {
                self.ty(x);
                self.const_expr(len);
            }
            Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x) },
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                for &x in ast.list(f.params) { self.ty(x); }
                if let Some(r) = f.ret { self.ty(r); }
            }
            Ty::ImplTrait(b) | Ty::DynTrait(b, _) => self.bounds(b),
            Ty::ImplicitSelf => {
                if let Some(s) = self.self_def { self.use_def(s); }
            }
            Ty::Never | Ty::Infer | Ty::Mac(_) | Ty::CVarArgs => {}
        }
    }

    fn bounds(&mut self, b: List<Bound>) {
        let ast = self.ast;
        for bound in ast.list(b) {
            if let Bound::Trait { path, .. } = *bound { self.path(&path, true); }
        }
    }

    fn generic_args(&mut self, id: GenericArgsId) {
        let ast = self.ast;
        match ast.gargs(id) {
            Some(GenericArgs::Angle(l)) => {
                for a in ast.list(*l) {
                    match *a {
                        GenericArg::Type(t) => {
                            // an ambiguous argument (`L`) is a constant when no type has the name
                            let before = self.out.len();
                            self.ty(t);
                            if self.out.len() == before {
                                if let Ty::Path(p) = *ast.ty(t) { self.path(&p, false); }
                            }
                        }
                        GenericArg::Const(e) => self.const_expr(e),
                        GenericArg::Binding(_, ga, t) => {
                            self.generic_args(ga);
                            self.ty(t);
                        }
                        GenericArg::Constraint(_, _, b) => self.bounds(b),
                        GenericArg::Lifetime(_) => {}
                    }
                }
            }
            Some(GenericArgs::Paren(inputs, out)) => {
                for &t in ast.list(*inputs) { self.ty(t); }
                if let Some(t) = out { self.ty(*t); }
            }
            None => {}
        }
    }

    pub fn generics(&mut self, g: &Generics) {
        let ast = self.ast;
        for p in ast.list(g.params) {
            match p.kind {
                GenericParamKind::Type(b, def) => {
                    self.bounds(b);
                    if let Some(t) = def { self.ty(t); }
                }
                GenericParamKind::Const(t, def) => {
                    self.ty(t);
                    if let Some(e) = def { self.const_expr(e); }
                }
                GenericParamKind::Lifetime(_) => {}
            }
        }
        for w in ast.list(g.where_) {
            if let WherePred::Bound { ty, bounds, .. } = *w {
                self.ty(ty);
                self.bounds(bounds);
            }
        }
    }

    /// The generic arguments written on a path's segments (types inside are references too).
    pub fn path_args(&mut self, p: &Path) {
        let ast = self.ast;
        for s in ast.segs(p.segs) { self.generic_args(s.args()); }
        if let Some(q) = ast.qself(p) {
            self.ty(q.ty);
            if let Some(tp) = q.trait_path { self.path(&tp, true); }
        }
    }

    /// Type aliases named by a path (or its prefixes) that typeck resolved through: rustc's path
    /// resolution keeps the alias, so naming it uses it.
    pub fn alias_uses(&mut self, p: &Path) {
        let segs = crate::program::path_segs(self.prog, self.file, p);
        let n = segs.len();
        if n == 0 { return; }
        for k in 1..=n {
            let prefix = &segs[..k];
            if prefix[0].0 == SegKind::Ident && !p.global() && self.params.contains(self.prog.syms.str(prefix[0].1)) { return; }
            let d = if k == 1 && prefix[0].0 == SegKind::Ident && !p.global() {
                self.prog.lookup_in_scope(self.module, prefix[0].1, true)
            } else if k >= 2 {
                match self.prog.resolve_mod_path(self.module, p.global(), &prefix[..k - 1], false, false) {
                    Some(parent) if self.prog.def(parent).kind == DefKind::Mod => self.prog.lookup_in_container(parent, prefix[k - 1].1, true),
                    _ => None,
                }
            } else { None };
            if let Some(d) = d { if self.prog.def(d).kind == DefKind::TypeAlias { self.out.push(Ref::Use(d)); } }
        }
    }

    /// A path in the type namespace (a type, a trait in a bound or an impl header).
    fn path(&mut self, p: &Path, ty_ns: bool) {
        let ast = self.ast;
        self.path_args(p);
        let segs = crate::program::path_segs(self.prog, self.file, p);
        if segs.is_empty() { return; }
        // `<T as Trait>::Assoc`: the trait (above) and the associated item it names
        if let Some(q) = ast.qself(p) {
            if let Some(tp) = q.trait_path {
                if let Some(t) = self.resolve_path_def(&tp, true) {
                    self.use_def(t);
                    if let Some(last) = segs.last() {
                        if let Some(c) = self.prog.lookup_in_container(t, last.1, true).or_else(|| self.prog.lookup_in_container(t, last.1, false)) { self.use_def(c); }
                    }
                }
            }
            return;
        }
        let first = segs[0];
        match first.0 {
            SegKind::SelfType => {
                if let Some(s) = self.self_def { self.use_def(s); }
                return;
            }
            SegKind::Ident => {
                let name = self.prog.syms.str(first.1);
                if !p.global() && self.params.contains(name) { return; }
            }
            _ => {}
        }
        if segs.len() == 1 && first.0 == SegKind::Ident && !p.global() {
            if let Some(d) = self.prog.lookup_in_scope(self.module, first.1, ty_ns) { self.use_def(d); }
            return;
        }
        let (prefix, last) = segs.split_at(segs.len() - 1);
        let Some(parent) = self.prog.resolve_mod_path(self.module, p.global(), prefix, false, false) else { return };
        match self.prog.def(parent).kind {
            DefKind::Mod => {
                if let Some(d) = self.prog.lookup_in_container(parent, last[0].1, ty_ns) { self.use_def(d); }
            }
            DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Trait | DefKind::TypeAlias => self.use_def(parent),
            _ => {}
        }
    }

    pub(crate) fn resolve_path_def(&self, p: &Path, ty_ns: bool) -> Option<DefId> {
        let segs = crate::program::path_segs(self.prog, self.file, p);
        let first = *segs.first()?;
        if segs.len() == 1 && first.0 == SegKind::Ident && !p.global() {
            return self.prog.lookup_in_scope(self.module, first.1, ty_ns);
        }
        let (prefix, last) = segs.split_at(segs.len() - 1);
        let parent = self.prog.resolve_mod_path(self.module, p.global(), prefix, false, false)?;
        self.prog.lookup_in_container(parent, last[0].1, ty_ns)
    }

    /// An array length, a const argument, a discriminant: an expression naming constants.
    pub fn const_expr(&mut self, e: ExprId) {
        struct W<'w, 'a> { t: &'w mut TypeWalker<'a> }
        impl<'w, 'a> Visitor<'a> for W<'w, 'a> {
            fn ast(&self) -> &'a Ast { self.t.ast }
            fn expr(&mut self, e: ExprId) {
                let ast = self.t.ast;
                if let ExprKind::Path(p) = ast.expr(e).kind {
                    self.t.path_args(&p);
                    let segs = crate::program::path_segs(self.t.prog, self.t.file, &p);
                    if let Some(&first) = segs.first() {
                        let is_param = first.0 == SegKind::Ident && segs.len() == 1 && self.t.params.contains(self.t.prog.syms.str(first.1));
                        if !is_param {
                            if let Some(d) = self.t.resolve_path_def(&p, false) { self.t.use_def(d); }
                            else if segs.len() > 1 {
                                let (prefix, _) = segs.split_at(segs.len() - 1);
                                if let Some(par) = self.t.prog.resolve_mod_path(self.t.module, p.global(), prefix, false, false) { if self.t.prog.def(par).kind != DefKind::Mod { self.t.use_def(par); } }
                            }
                        }
                    }
                }
                ast_visit::walk_expr(self, e);
            }
        }
        W { t: self }.expr(e);
    }
}

/// The references an item's declaration makes (its signature, fields, header); the body of a fn,
/// const or static is collected while it is type-checked.
fn item_refs(prog: &Program, d: DefId) -> Vec<Ref> {
    item_refs_in(prog, d, false)
}

/// `pub_only`: only what the item exposes (the `pub` fields of a struct).
/// The trait definition a bound path of an item resolves to.
pub(crate) fn resolve_trait_path(prog: &Program, d: DefId, p: &Path) -> Option<DefId> {
    let w = TypeWalker::new(prog, d);
    w.resolve_path_def(p, true)
}

fn item_refs_in(prog: &Program, d: DefId, pub_only: bool) -> Vec<Ref> {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    if matches!(def.kind, DefKind::Mod | DefKind::Variant | DefKind::Closure | DefKind::AnonConst | DefKind::Field | DefKind::MacroRules | DefKind::Prim(_) | DefKind::OpaqueTy) { return Vec::new(); }
    let item = *ast.item(def.item);
    let mut w = TypeWalker::new(prog, d);
    if let Some(g) = ast.kind_generics(&item.kind) { w.generics(g); }
    match item.kind {
        ItemKind::Fn(sig, _, _) => {
            let sig = *ast.fn_sig(sig);
            for p in ast.list(sig.params) { w.ty(p.ty); }
            // `self` / `&self`: its type is `Self`
            match sig.self_param {
                Some(SelfParam::Typed(_, t)) => w.ty(t),
                Some(_) => { if let Some(s) = w.self_def { w.use_def(s); } }
                None => {}
            }
            if let Some(r) = sig.ret { w.ty(r); }
        }
        ItemKind::Struct(r) => {
            let s = *ast.row(r);
            if let VariantData::Struct(l) | VariantData::Tuple(l) = s.data { fields(&mut w, ast, l, pub_only); }
        }
        ItemKind::Union(r) => fields(&mut w, ast, ast.row(r).fields, pub_only),
        ItemKind::Enum(r) => {
            for v in ast.list(ast.row(r).variants) {
                if let VariantData::Struct(l) | VariantData::Tuple(l) = v.data { fields(&mut w, ast, l, false); }
                if let Some(e) = v.disc { w.const_expr(e); }
            }
        }
        ItemKind::TypeAlias(r) => {
            let a = *ast.row(r);
            w.bounds(a.bounds);
            if let Some(t) = a.ty { w.ty(t); }
        }
        ItemKind::Trait(r) => {
            let t = *ast.row(r);
            w.bounds(t.supers);
        }
        ItemKind::Impl(r) => {
            let i = *ast.row(r);
            if let Some(tp) = i.trait_ {
                w.path(&tp, true);
                // the associated types of a trait impl are part of what the impl exposes
                // (`Index::Output`, `IntoIterator::IntoIter`)
                for &it in ast.list(i.items) {
                    if let ItemKind::TypeAlias(r) = ast.item(it).kind {
                        if let Some(t) = ast.row(r).ty { w.ty(t); }
                    }
                }
            }
            w.ty(i.self_ty);
        }
        ItemKind::Const(t, e) => {
            if let Some(t) = t.get() { w.ty(t); }
            let _ = e;
        }
        ItemKind::Static(_, t, _) => w.ty(t),
        _ => {}
    }
    w.out
}

fn fields(w: &mut TypeWalker, ast: &Ast, l: List<FieldDef>, pub_only: bool) {
    for f in ast.list(l) {
        if pub_only && !matches!(f.vis, Vis::Pub) { continue; }
        w.ty(f.ty);
        if let Some(e) = f.default { w.const_expr(e); }
    }
}

// ================================================================ body references

pub fn collect_body(prog: &Program, tcx: &Tcx, body: &Body) {
    if !lint::enabled() { return; }
    let d = prog.def(body.def);
    let ast = &prog.files[body.file as usize].ast;
    let mut b = BodyRefs { prog, tcx, body, ast, tw: TypeWalker::new(prog, body.def), in_pat: false, ignore: Vec::new(), out: Vec::new(), stack: Vec::new(), notes: Vec::new() };
    b.tw.module = d.scope;
    match ast.item(d.item).kind {
        ItemKind::Fn(s, _, _) => {
            let Ok(Some(block)) = prog.fn_body(body.file, d.item) else { return };
            for p in ast.list(ast.fn_sig(s).params) {
                b.enter_attrs(p.attrs);
                b.pat(p.pat);
                b.leave_attrs(p.attrs);
            }
            b.block(block);
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            let Ok(Some(e)) = prog.const_body(body.file, d.item) else { return };
            b.expr(e);
        }
        _ => return,
    }
    let mut out = std::mem::take(&mut b.out);
    out.append(&mut b.tw.out);
    let mut g = BODY_REFS.lock().unwrap_or_else(|e| e.into_inner());
    let notes = std::mem::take(&mut b.notes);
    g.get_or_insert_with(HashMap::new).insert(body.def, BodyInfo { refs: out, notes });
}

struct BodyRefs<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    tw: TypeWalker<'a>,
    in_pat: bool,
    /// variants a pattern needs to match: constructing them in the arm is no use of them
    ignore: Vec<DefId>,
    out: Vec<Ref>,
    stack: Vec<Attr>,
    notes: Vec<(u32, String, Vec<Attr>)>,
}

impl<'a> BodyRefs<'a> {
    /// `check_for_self_assign`: `x = x`, `a.f = a.f`.
    fn self_assign(&mut self, e: ExprId, l: ExprId, r: ExprId) {
        fn same(b: &BodyRefs, l: ExprId, r: ExprId) -> bool {
            let ast = b.ast;
            match (ast.expr(l).kind, ast.expr(r).kind) {
                (ExprKind::Path(_), ExprKind::Path(_)) => matches!((b.body.res.get(&l.0), b.body.res.get(&r.0)), (Some(Res::Local(x)), Some(Res::Local(y))) if x == y),
                (ExprKind::Field(ll, nl), ExprKind::Field(rr, nr)) => {
                    let f = &b.prog.files[b.body.file as usize];
                    f.text(nl) == f.text(nr) && same(b, ll, rr)
                }
                (ExprKind::TupleField(ll, i), ExprKind::TupleField(rr, j)) => i == j && same(b, ll, rr),
                _ => false,
            }
        }
        let ex = self.ast.expr(e);
        if !same(self, l, r) || self.prog.files[self.body.file as usize].expansion_kind(ex.lo) != crate::parser::ExpnKind::Root { return; }
        let field = matches!(self.ast.expr(l).kind, ExprKind::Field(..) | ExprKind::TupleField(..));
        let li = self.body.expr_ix(l);
        let ty = if li < self.body.expr_ty.len() { crate::typeck::ty_to_string(false, self.prog, self.tcx, self.body.expr_ty[li]) } else { "?".to_string() };
        self.notes.push((ex.lo, format!("useless assignment of {} of type `{}` to itself", if field { "field" } else { "variable" }, ty), self.stack.clone()));
    }

    fn use_def(&mut self, d: DefId) { self.out.push(Ref::Use(d)); }

    /// `handle_res` for a resolved definition.
    fn handle_res(&mut self, d: DefId) {
        let def = self.prog.def(d);
        match def.kind {
            DefKind::Variant => {
                if self.in_pat { return; }
                self.use_def(def.parent);
                if !self.ignore.contains(&d) { self.use_def(d); }
            }
            _ => self.use_def(d),
        }
    }

    /// The definition behind a trait impl's item: its trait item.
    fn trait_item_of(&self, d: DefId) -> Option<DefId> {
        let def = self.prog.def(d);
        if def.parent == NO_DEF || self.prog.def(def.parent).kind != DefKind::Impl { return None; }
        let t = *self.tcx.impl_trait.get(&def.parent)?;
        self.prog.lookup_in_container(t, def.name, matches!(def.kind, DefKind::AssocTy)).or_else(|| self.prog.lookup_in_container(t, def.name, false))
    }

    fn adt_of_ty(&self, t: crate::types::TyId, derefs: u32) -> Option<DefId> {
        let mut t = t;
        for _ in 0..derefs {
            match *self.tcx.tys.kind(t) {
                TyKind::Ref(_, x, _) => t = x,
                TyKind::Adt(d, args, _) if lint::is_diagnostic_item(self.prog, d, "Box") => t = *self.tcx.tys.list(args).first()?,
                _ => return None,
            }
        }
        while let TyKind::Ref(_, x, _) = *self.tcx.tys.kind(t) { t = x; }
        match *self.tcx.tys.kind(t) {
            TyKind::Adt(d, ..) => Some(d),
            _ => None,
        }
    }

    fn field_ref(&mut self, e: ExprId, base: ExprId) {
        let Some(&(derefs, idx)) = self.body.field_idx.get(&e.0) else { return };
        let bi = self.body.expr_ix(base);
        if bi < self.body.expr_ty.len() {
            if let Some(adt) = self.adt_of_ty(self.body.expr_ty[bi], derefs) {
                self.out.push(Ref::Field((adt, 0, idx)));
                return;
            }
        }
        // through an overloaded dereference (`Pin<&mut S>`, a guard): the struct the body names
        // a field of with this index and name
        let name = match self.ast.expr(e).kind {
            ExprKind::Field(_, n) => Some(self.prog.files[self.body.file as usize].text(n).to_string()),
            _ => None,
        };
        for &(adt, fi) in self.body.field_uses.iter() {
            if fi != idx { continue; }
            let Some(a) = self.tcx.adts.get(&adt) else { continue };
            let Some(f) = a.variants.first().and_then(|v| v.fields.get(fi as usize)) else { continue };
            if name.as_deref().map_or(true, |n| self.prog.syms.str(f.name) == n) { self.out.push(Ref::Field((adt, 0, idx))); }
        }
    }

    /// An assignment target: fields written to are not reads (`handle_assign`).
    fn variant_of(&self, d: DefId) -> (DefId, u32) {
        let def = self.prog.def(d);
        if def.kind == DefKind::Variant { (def.parent, def.sub) } else { (d, 0) }
    }

    fn pat_fields(&mut self, p: PatId) {
        let ast = self.ast;
        let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() else { return };
        let (adt, vi) = self.variant_of(d);
        self.use_def(adt);
        match *ast.pat(p) {
            Pat::Struct(_, fs, _) => {
                let Some(a) = self.tcx.adts.get(&adt) else { return };
                let Some(var) = a.variants.get(vi as usize) else { return };
                for f in ast.list(fs) {
                    if matches!(ast.pat(f.pat), Pat::Wild) { continue; }
                    let name = self.prog.files[self.body.file as usize].text(f.name);
                    if let Some(i) = var.fields.iter().position(|ft| self.prog.syms.str(ft.name) == name) { self.out.push(Ref::Field((adt, vi, i as u32))); }
                }
            }
            Pat::TupleStruct(_, ps) => {
                let Some(a) = self.tcx.adts.get(&adt) else { return };
                let Some(var) = a.variants.get(vi as usize) else { return };
                let list = ast.list(ps);
                let dotdot = list.iter().position(|x| matches!(ast.pat(*x), Pat::Rest)).unwrap_or(list.len());
                let n = list.iter().filter(|x| !matches!(ast.pat(**x), Pat::Rest)).count();
                let missing = var.fields.len().saturating_sub(n);
                for (k, x) in list.iter().enumerate() {
                    if matches!(ast.pat(*x), Pat::Rest | Pat::Wild) { continue; }
                    let idx = if k < dotdot { k } else { k - 1 + missing };
                    self.out.push(Ref::Field((adt, vi, idx as u32)));
                }
            }
            _ => {}
        }
    }

    /// The variants a pattern needs (`Pat::necessary_variants`: not through or-patterns).
    fn necessary_variants(&self, p: PatId, out: &mut Vec<DefId>) {
        let ast = self.ast;
        match *ast.pat(p) {
            Pat::Or(_) => {}
            Pat::Struct(_, fs, _) => {
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0) { if self.prog.def(*d).kind == DefKind::Variant { out.push(*d); } }
                for f in ast.list(fs) { self.necessary_variants(f.pat, out); }
            }
            Pat::TupleStruct(_, ps) => {
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0) { if self.prog.def(*d).kind == DefKind::Variant { out.push(*d); } }
                for &x in ast.list(ps) { self.necessary_variants(x, out); }
            }
            Pat::Path(_) | Pat::Ident { .. } => {
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0) { if self.prog.def(*d).kind == DefKind::Variant { out.push(*d); } }
                if let Pat::Ident { sub: Some(s), .. } = *ast.pat(p) { self.necessary_variants(s, out); }
            }
            Pat::Tuple(ps) | Pat::Slice(ps) => for &x in ast.list(ps) { self.necessary_variants(x, out) },
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.necessary_variants(x, out),
            _ => {}
        }
    }

    fn path_expr(&mut self, e: ExprId, p: &Path) {
        self.tw.path_args(p);
        self.tw.alias_uses(p);
        match self.body.res.get(&e.0) {
            Some(Res::Def(d)) => {
                let d = *d;
                crate::lint_imports::note_method(self.prog, self.tcx, d);
                self.handle_res(d);
                // a trait impl's item stands for the trait's
                if let Some(t) = self.trait_item_of(d) { self.use_def(t); }
            }
            _ => {}
        }
        // the type a type-relative path starts from (`Foo::new`, `Self::CONST`)
        let segs = crate::program::path_segs(self.prog, self.body.file, p);
        if segs.len() >= 2 || self.ast.qself(p).is_some() {
            let pre = Path { segs: p.segs.prefix(segs.len() - 1), lo: p.lo };
            if self.ast.qself(p).is_none() {
                let first = segs[0];
                if first.0 == SegKind::SelfType {
                    if let Some(s) = self.tw.self_def { self.use_def(s); }
                } else if !(first.0 == SegKind::Ident && !p.global() && self.tw.params.contains(self.prog.syms.str(first.1))) {
                    let before = self.tw.out.len();
                    self.tw.path(&pre, true);
                    let _ = before;
                }
            }
        }
    }
}

impl<'a> Visitor<'a> for BodyRefs<'a> {
    fn ast(&self) -> &'a Ast { self.ast }

    fn enter_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    fn block(&mut self, b: BlockId) {
        let saved = self.tw.module;
        if let Some(&m) = self.prog.block_mods.get(&(self.body.file, b.0)) { self.tw.module = m; }
        ast_visit::walk_block(self, b);
        self.tw.module = saved;
    }

    fn stmt(&mut self, s: &Stmt, b: BlockId) {
        if let Stmt::Let(r) = *s {
            if let Some(t) = self.ast.row(r).ty { self.tw.ty(t); }
        }
        let _ = b;
        ast_visit::walk_stmt(self, s);
    }

    fn arm(&mut self, a: &Arm) {
        let len = self.ignore.len();
        let mut vs = Vec::new();
        self.necessary_variants(a.pat, &mut vs);
        self.ignore.extend(vs);
        ast_visit::walk_arm(self, a);
        self.ignore.truncate(len);
    }

    fn pat(&mut self, p: PatId) {
        let ast = self.ast;
        let was = std::mem::replace(&mut self.in_pat, true);
        match *ast.pat(p) {
            Pat::Struct(path, ..) | Pat::TupleStruct(path, _) => {
                self.tw.path_args(&path);
                self.tw.alias_uses(&path);
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() { self.handle_res(d); }
                self.pat_fields(p);
            }
            Pat::Path(path) => {
                self.tw.path_args(&path);
                self.tw.alias_uses(&path);
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() {
                    let pi = self.body.pat_ix(p);
                    if pi < self.body.pat_ty.len() {
                        if let TyKind::Adt(a, ..) = *self.tcx.tys.kind(self.body.pat_ty[pi]) { self.use_def(a); }
                    }
                    self.handle_res(d);
                }
            }
            Pat::Ident { name, .. } => {
                // (a binding a macro made has no place of its own: it maps to the call)
                if self.body.pat_local.get(&p.0).is_some() && self.body.pat_res.get(&p.0).is_none() && name.lo < self.prog.files[self.body.file as usize].src_len {
                    let site = self.prog.files[self.body.file as usize].diagnostic_pos(name.lo);
                    BINDING_SITES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert((self.body.file, site));
                }
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0).copied() {
                    let pi = self.body.pat_ix(p);
                    if pi < self.body.pat_ty.len() {
                        if let TyKind::Adt(a, ..) = *self.tcx.tys.kind(self.body.pat_ty[pi]) { self.use_def(a); }
                    }
                    self.handle_res(d);
                }
            }
            _ => {}
        }
        ast_visit::walk_pat(self, p);
        self.in_pat = was;
    }

    fn expr(&mut self, e: ExprId) {
        let ast = self.ast;
        let ex = *ast.expr(e);
        match ex.kind {
            ExprKind::Path(p) => self.path_expr(e, &p),
            ExprKind::MethodCall(_, row) => {
                let r = *ast.row(row);
                self.tw.generic_args(r.turbofish);
                if let Some(m) = self.body.methods.get(&e.0) {
                    let d = m.def;
                    crate::lint_imports::note_method(self.prog, self.tcx, d);
                    self.use_def(d);
                    if let Some(t) = self.trait_item_of(d) { self.use_def(t); }
                }
            }
            ExprKind::Field(base, _) | ExprKind::TupleField(base, _) => self.field_ref(e, base),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                self.tw.path_args(&s.path);
                self.tw.alias_uses(&s.path);
                if let Some(Res::Def(d)) = self.body.res.get(&e.0).copied() {
                    self.handle_res(d);
                    let (adt, vi) = self.variant_of(d);
                    // a union literal reads its fields
                    if let Some(a) = self.tcx.adts.get(&adt) {
                        if a.is_union && a.variants.first().is_some_and(|v| v.fields.len() > 1) {
                            if let Some(var) = a.variants.get(vi as usize) {
                                for f in ast.list(s.fields) {
                                    let name = self.prog.files[self.body.file as usize].text(f.name);
                                    if let Some(i) = var.fields.iter().position(|ft| self.prog.syms.str(ft.name) == name) { self.out.push(Ref::Field((adt, vi, i as u32))); }
                                }
                            }
                        }
                    }
                }
            }
            ExprKind::Cast(_, t) => self.tw.ty(t),
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                for p in ast.list(c.params) { if let Some(t) = p.ty { self.tw.ty(t); } }
                if let Some(t) = c.ret { self.tw.ty(t); }
            }
            ExprKind::Repeat(_, n) => self.tw.const_expr(n),
            ExprKind::Assign(l, r) => {
                self.self_assign(e, l, r);
                // rustc dead.rs handle_assign visits the target and then `walk_expr` visits it again
                // with its field accesses: a field written to is live
                self.expr(l);
                self.expr(r);
                return;
            }
            _ => {}
        }
        ast_visit::walk_expr(self, e);
    }
}

// ================================================================ effective visibility

/// The items reachable from outside the crate: `pub` along the module path, re-exported by a
/// `pub use`, and what such items expose (variants, `pub` fields, trait items, the `pub` items
/// of impls of reachable types).
pub struct Reach {
    own: Vec<bool>,
    pub defs: HashSet<DefId>,
    /// `defs` before the types of reachable interfaces were added: what a path can name
    pub named: HashSet<DefId>,
    pub fields: HashSet<FieldKey>,
}

pub fn module_items(prog: &Program, m: u32) -> (u32, Vec<ItemId>) {
    let md = &prog.mods[m as usize];
    if md.parent_mod.is_none() { return (md.file, prog.files[md.file as usize].ast.root_items.iter().copied().collect()); }
    let d = prog.def(md.def);
    let ast = &prog.files[d.file as usize].ast;
    match ast.item(d.item).kind {
        ItemKind::Mod(Some(l)) => (d.file, ast.list(l).to_vec()),
        _ => (md.file, prog.files[md.file as usize].ast.root_items.iter().copied().collect()),
    }
}

fn use_leaves(ast: &Ast, t: &UseTree, prefix: &mut Vec<PathSeg>, out: &mut Vec<(Vec<PathSeg>, Option<Ident>, bool)>) {
    match *t {
        UseTree::Name(seg, rename) => {
            let mut p = prefix.clone();
            p.push(seg);
            out.push((p, rename, false));
        }
        UseTree::Path(seg, child) => {
            prefix.push(seg);
            let c = *ast.use_tree(child);
            use_leaves(ast, &c, prefix, out);
            prefix.pop();
        }
        UseTree::Glob => out.push((prefix.clone(), None, true)),
        UseTree::Group(l) => for c in ast.list(l) { use_leaves(ast, c, prefix, out) },
    }
}

impl Reach {
    fn compute(prog: &Program, tcx: &Tcx, own: &[bool]) -> Reach {
        let mut r = Reach { own: own.to_vec(), defs: HashSet::new(), named: HashSet::new(), fields: HashSet::new() };
        let mut done: HashSet<u32> = HashSet::new();
        for (k, c) in prog.crates.iter().enumerate() {
            if own.get(k).copied().unwrap_or(false) { r.module(prog, c.root_mod, &mut done); }
        }
        r.impls(prog, tcx, own);
        r.named = r.defs.clone();
        // the types a reachable item mentions in its interface are reachable too
        let mut done2: HashSet<u32> = HashSet::new();
        loop {
            let mut found: Vec<DefId> = Vec::new();
            for &d in r.defs.iter() {
                let k = prog.def(d).kind;
                if !matches!(k, DefKind::Fn | DefKind::AssocFn | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Trait | DefKind::TypeAlias | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::AssocTy | DefKind::Impl | DefKind::ForeignFn | DefKind::ForeignStatic) { continue; }
                for u in item_refs_in(prog, d, true) {
                    if let Ref::Use(t) = u {
                        let tk = prog.def(t).kind;
                        if own.get(prog.def(t).krate as usize).copied().unwrap_or(false) && matches!(tk, DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Trait | DefKind::TypeAlias) && !r.defs.contains(&t) { found.push(t); }
                    }
                }
            }
            if found.is_empty() { break; }
            for t in found { let td = prog.def(t); r.mark_item(prog, td.file, td.item, t, &mut done2); }
            // the impls of the types just reached expose their items too
            r.impls(prog, tcx, own);
        }
        r
    }

    /// The `pub` items of impls of reachable types, and every item of a trait impl whose trait
    /// and type are reachable.
    fn impls(&mut self, prog: &Program, tcx: &Tcx, own: &[bool]) {
        let r = self;
        for i in 0..prog.defs.len() {
            let def = prog.defs[i];
            if def.kind != DefKind::Impl || !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
            let d = DefId(i as u32);
            let ast = &prog.files[def.file as usize].ast;
            let ItemKind::Impl(row) = ast.item(def.item).kind else { continue };
            let imp = *ast.row(row);
            let self_reach = match tcx.impl_self.get(&d).map(|t| *tcx.tys.kind(*t)) {
                Some(TyKind::Adt(a, ..)) => !own.get(prog.def(a).krate as usize).copied().unwrap_or(false) || r.defs.contains(&a),
                Some(_) => true,
                None => false,
            };
            if !self_reach { continue; }
            let trait_reach = match imp.trait_ {
                None => None,
                Some(_) => Some(tcx.impl_trait.get(&d).map_or(true, |t| !own.get(prog.def(*t).krate as usize).copied().unwrap_or(false) || r.defs.contains(t))),
            };
            if trait_reach == Some(false) { continue; }
            if trait_reach.is_some() { r.defs.insert(d); }
            for &it in ast.list(imp.items) {
                if let Some(&cd) = prog.item_defs.get(&(def.file, it.0)) {
                    if trait_reach.is_some() || matches!(ast.item(it).vis, Vis::Pub) { r.defs.insert(cd); }
                }
            }
        }
    }

    fn mark_item(&mut self, prog: &Program, file: u32, it: ItemId, d: DefId, done: &mut HashSet<u32>) {
        let ast = &prog.files[file as usize].ast;
        self.defs.insert(d);
        match ast.item(it).kind {
            ItemKind::Mod(_) => {
                let m = prog.def(d).sub;
                self.module(prog, m, done);
            }
            ItemKind::Enum(_) => {
                for &v in &prog.variants[prog.def(d).sub as usize] { self.defs.insert(v); }
            }
            ItemKind::Struct(r) => {
                if let VariantData::Struct(l) | VariantData::Tuple(l) = ast.row(r).data {
                    for (i, f) in ast.list(l).iter().enumerate() { if matches!(f.vis, Vis::Pub) { self.fields.insert((d, 0, i as u32)); } }
                }
            }
            ItemKind::Union(r) => {
                for (i, f) in ast.list(ast.row(r).fields).iter().enumerate() { if matches!(f.vis, Vis::Pub) { self.fields.insert((d, 0, i as u32)); } }
            }
            ItemKind::Trait(r) => {
                for &c in ast.list(ast.row(r).items) {
                    if let Some(&cd) = prog.item_defs.get(&(file, c.0)) { self.defs.insert(cd); }
                }
            }
            _ => {}
        }
        // enum variants' fields are as visible as the enum
        if let ItemKind::Enum(r) = ast.item(it).kind {
            for (vi, v) in ast.list(ast.row(r).variants).iter().enumerate() {
                if let VariantData::Struct(l) | VariantData::Tuple(l) = v.data {
                    for i in 0..ast.list(l).len() { self.fields.insert((d, vi as u32, i as u32)); }
                }
            }
        }
    }

    /// Reachable by name: a re-exported definition and what lies inside it.
    fn mark_def(&mut self, prog: &Program, d: DefId, done: &mut HashSet<u32>) {
        let def = prog.def(d);
        // another crate's items are not ours to judge
        if !self.own.get(def.krate as usize).copied().unwrap_or(false) { return; }
        let ast = &prog.files[def.file as usize].ast;
        if def.kind == DefKind::Variant {
            self.defs.insert(d);
            self.defs.insert(def.parent);
            return;
        }
        if matches!(def.kind, DefKind::Prim(_)) { return; }
        self.mark_item(prog, def.file, def.item, d, done);
        let _ = ast;
    }

    fn module(&mut self, prog: &Program, m: u32, done: &mut HashSet<u32>) {
        if !done.insert(m) { return; }
        self.defs.insert(prog.mods[m as usize].def);
        let (file, items) = module_items(prog, m);
        let ast = &prog.files[file as usize].ast;
        for it in items {
            let item = *ast.item(it);
            if !matches!(item.vis, Vis::Pub) { continue; }
            match item.kind {
                ItemKind::Use(r) => {
                    let u = *ast.row(r);
                    let mut leaves = Vec::new();
                    use_leaves(ast, &u.tree, &mut Vec::new(), &mut leaves);
                    for (segs, rename, glob) in leaves {
                        let syms: Vec<(SegKind, crate::program::Sym)> = segs.iter().map(|s| (s.kind(), prog.syms.intern_late(prog.files[file as usize].text(s.name)))).collect();
                        if glob {
                            if let Some(t) = prog.resolve_mod_path(m, u.global, &syms, false, true) {
                                if prog.def(t).kind == DefKind::Mod { self.module(prog, prog.def(t).sub, done); }
                                else if prog.def(t).kind == DefKind::Enum { self.mark_def(prog, t, done); }
                            }
                            continue;
                        }
                        let name = match rename {
                            Some(id) => prog.syms.intern_late(prog.files[file as usize].text(id)),
                            None => match syms.last() { Some(s) => s.1, None => continue },
                        };
                        for ns in [true, false] {
                            if let Some(t) = prog.lookup_in_scope(m, name, ns) { self.mark_def(prog, t, done); }
                        }
                    }
                }
                ItemKind::Impl(_) | ItemKind::ExternCrate(_) | ItemKind::MacroRules(_) | ItemKind::Mac(_) | ItemKind::ForeignMod(_) => {}
                _ => {
                    if let Some(&d) = prog.item_defs.get(&(file, it.0)) { self.mark_item(prog, file, it, d, done); }
                }
            }
        }
        // foreign items
        for it in module_items(prog, m).1 {
            if let ItemKind::ForeignMod(r) = ast.item(it).kind {
                for &c in ast.list(ast.row(r).items) {
                    if matches!(ast.item(c).vis, Vis::Pub) { if let Some(&cd) = prog.item_defs.get(&(file, c.0)) { self.defs.insert(cd); } }
                }
            }
        }
    }
}

// ================================================================ the analysis

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Work {
    id: DefId,
    propagated: bool,
    own: bool,
}

fn should_explore(k: DefKind) -> bool {
    !matches!(k, DefKind::Field | DefKind::Closure | DefKind::Prim(_))
}

struct Dead<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    own: &'a [bool],
    refs: HashMap<DefId, BodyInfo>,
    live: HashSet<DefId>,
    live_fields: HashSet<FieldKey>,
    worklist: Vec<Work>,
    scanned: HashSet<(DefId, bool)>,
    propagated: bool,
    /// ADTs with an ignored derived impl (Clone, Debug): the note
    ignored_derived: HashMap<DefId, Vec<String>>,
    children: HashMap<DefId, Vec<DefId>>,
}

impl<'a> Dead<'a> {
    fn local(&self, d: DefId) -> bool { self.own.get(self.prog.def(d).krate as usize).copied().unwrap_or(false) }

    /// `has_allow_dead_code_or_lang_attr`: Some(true) allowed, Some(false) kept alive, None.
    fn allow_or_lang(&self, d: DefId) -> Option<bool> {
        if lint::level(self.prog, d, &DEAD_CODE) == Level::Allow { return Some(true); }
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        if matches!(def.kind, DefKind::Prim(_) | DefKind::Variant | DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Field | DefKind::Mod) && def.kind != DefKind::Mod { return None; }
        if def.kind == DefKind::Mod && def.parent == NO_DEF { return None; }
        let attrs = ast.list(ast.item(def.item).attrs);
        let used_like = matches!(def.kind, DefKind::Fn | DefKind::Static | DefKind::AssocFn | DefKind::ForeignFn | DefKind::ForeignStatic)
            && attrs.iter().any(|a| {
                let n = lint::attr_path(self.prog, def.file, a);
                n == b"no_mangle" || n == b"export_name" || n == b"used"
            });
        if used_like || lint::has_attr(self.prog, def.file, attrs, "lang") { Some(false) } else { None }
    }

    fn own_allow(&self, d: DefId) -> bool {
        self.propagated && self.allow_or_lang(d) == Some(true)
    }

    fn check_def(&mut self, d: DefId) {
        if !self.local(d) { return; }
        let own = self.own_allow(d);
        if should_explore(self.prog.def(d).kind) { self.worklist.push(Work { id: d, propagated: self.propagated, own }); }
        if !own { self.live.insert(d); }
    }

    fn apply(&mut self, refs: &[Ref]) {
        for r in refs {
            match *r {
                Ref::Use(d) => self.check_def(d),
                Ref::Field(k) => {
                    // read in the scope of an allowed / expected item: a field that is allowed too
                    // is not made live by it
                    if self.propagated {
                        let (attrs, _) = field_ast(self.prog, k.0, k.1 as usize, k.2 as usize);
                        if lint::level_with(self.prog, k.0, &attrs, &DEAD_CODE) == Level::Allow { continue; }
                    }
                    self.live_fields.insert(k);
                }
            }
        }
    }

    fn explore(&mut self, w: Work) {
        let mut id = w.id;
        self.propagated = w.propagated;
        if !w.own { self.live.insert(id); }
        if !self.scanned.insert((id, w.propagated)) { return; }
        let def = *self.prog.def(id);
        let ast = &self.prog.files[def.file as usize].ast;
        // a derived impl of Clone / Debug does not read fields
        if self.should_ignore_impl_item(id) { return; }
        match def.kind {
            DefKind::Struct | DefKind::Union => {
                // repr(C) and transparent fields, and the fields visible from outside, are live
                let attrs = ast.list(ast.item(def.item).attrs);
                let repr_live = attrs.iter().any(|a| lint::attr_path(self.prog, def.file, a) == b"repr" && lint::attr_args(self.prog, def.file, a).iter().any(|w| w == "C" || w == "transparent"));
                let simd = attrs.iter().any(|a| lint::attr_path(self.prog, def.file, a) == b"repr" && lint::attr_args(self.prog, def.file, a).iter().any(|w| w == "simd"));
                if let Some(adt) = self.tcx.adts.get(&id) {
                    for i in 0..adt.variants[0].fields.len() {
                        let positional = adt.variants[0].shape == 1;
                        if repr_live || (positional && simd) { self.live_fields.insert((id, 0, i as u32)); }
                    }
                }
            }
            DefKind::Trait => {
                let kids = self.children.get(&id).cloned().unwrap_or_default();
                for c in kids { if self.prog.def(c).kind == DefKind::AssocTy { self.check_def(c); } }
            }
            DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy => {
                if def.parent != NO_DEF {
                    let p = self.prog.def(def.parent);
                    if p.kind == DefKind::Trait { self.check_def(def.parent); }
                    if p.kind == DefKind::Impl {
                        let pa = &self.prog.files[p.file as usize].ast;
                        if let ItemKind::Impl(r) = pa.item(p.item).kind {
                            if pa.row(r).trait_.is_none() {
                                // an inherent item keeps its type alive
                                if let Some(TyKind::Adt(a, ..)) = self.tcx.impl_self.get(&def.parent).map(|t| *self.tcx.tys.kind(*t)) { self.check_def(a); }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let _ = &mut id;
        let mut refs = item_refs(self.prog, id);
        if let Some(b) = self.refs.get(&id) {
            refs.extend_from_slice(&b.refs);
            for (pos, msg, attrs) in &b.notes {
                lint::emit_in(self.prog, id, attrs, &DEAD_CODE, def.file, *pos, msg);
            }
        }
        self.apply(&refs);
    }

    fn should_ignore_impl_item(&mut self, d: DefId) -> bool {
        let def = self.prog.def(d);
        if !matches!(def.kind, DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy) || def.parent == NO_DEF { return false; }
        let imp = self.prog.def(def.parent);
        if imp.kind != DefKind::Impl { return false; }
        if !lint::is_automatically_derived(self.prog, def.parent) { return false; }
        let Some(&t) = self.tcx.impl_trait.get(&def.parent) else { return false };
        let name = self.prog.name(t);
        if !(matches!(name, "Clone" | "Debug") && !self.local(t)) { return false; }
        if let Some(TyKind::Adt(a, ..)) = self.tcx.impl_self.get(&def.parent).map(|t| *self.tcx.tys.kind(*t)) {
            if self.local(a) {
                let e = self.ignored_derived.entry(a).or_default();
                if !e.iter().any(|x| x == name) { e.push(name.to_string()); }
            }
        }
        true
    }

    fn mark_live_symbols(&mut self) {
        while let Some(w) = self.worklist.pop() { self.explore(w); }
    }

    fn trait_item_of(&self, d: DefId) -> Option<DefId> {
        let def = self.prog.def(d);
        let t = *self.tcx.impl_trait.get(&def.parent)?;
        let tn = self.prog.def(t);
        self.children.get(&t)?.iter().copied().find(|&c| {
            let cd = self.prog.def(c);
            cd.name == def.name && std::mem::discriminant(&cd.kind) == std::mem::discriminant(&def.kind) && tn.krate == cd.krate
        })
    }

    /// `check_impl_or_impl_item_live`
    fn impl_live(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        let (impl_block, trait_def) = if def.kind == DefKind::Impl {
            (d, self.tcx.impl_trait.get(&d).copied())
        } else {
            (def.parent, self.trait_item_of(d))
        };
        if let Some(t) = trait_def {
            if self.local(t) && !self.live.contains(&t) { return false; }
        }
        if let Some(TyKind::Adt(a, ..)) = self.tcx.impl_self.get(&impl_block).map(|t| *self.tcx.tys.kind(*t)) {
            if self.local(a) && !self.live.contains(&a) { return false; }
        }
        true
    }

    fn mark_with_unsolved(&mut self, unsolved: &mut Vec<DefId>) {
        self.mark_live_symbols();
        let mut check: Vec<DefId> = Vec::new();
        let mut i = 0;
        while i < unsolved.len() {
            if self.impl_live(unsolved[i]) { check.push(unsolved.swap_remove(i)); } else { i += 1; }
        }
        while !check.is_empty() {
            for d in check.drain(..) { self.worklist.push(Work { id: d, propagated: false, own: false }); }
            self.mark_live_symbols();
            let mut i = 0;
            while i < unsolved.len() {
                if self.impl_live(unsolved[i]) { check.push(unsolved.swap_remove(i)); } else { i += 1; }
            }
        }
    }
}

/// The checked crates are done: run the analysis and report.
pub fn finish(prog: &Program, tcx: &Tcx, own: &[bool], root: u32) {
    if !lint::enabled() { return; }
    let refs = BODY_REFS.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let mut children: HashMap<DefId, Vec<DefId>> = HashMap::new();
    for i in 0..prog.defs.len() {
        let p = prog.defs[i].parent;
        if p != NO_DEF { children.entry(p).or_default().push(DefId(i as u32)); }
    }
    let reach = Reach::compute(prog, tcx, own);
    let mut s = Dead { prog, tcx, own, refs, live: HashSet::new(), live_fields: HashSet::new(), worklist: Vec::new(), scanned: HashSet::new(), propagated: false, ignored_derived: HashMap::new(), children };
    // seeds
    let executable = is_executable(prog, root);
    let mut deferred: Vec<Work> = Vec::new();
    let mut unsolved: Vec<DefId> = Vec::new();
    if executable {
        let rm = prog.crates[root as usize].root_mod;
        let main = prog.syms.intern_late("main");
        if let Some(m) = prog.lookup_in_container(prog.mods[rm as usize].def, main, false) {
            if prog.def(m).kind == DefKind::Fn { s.worklist.push(Work { id: m, propagated: false, own: false }); }
        }
    }
    for &d in reach.defs.iter() {
        if s.local(d) { deferred.push(Work { id: d, propagated: false, own: false }); }
    }
    let item_kinds = |k: DefKind| matches!(k, DefKind::Mod | DefKind::Fn | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Const | DefKind::Static | DefKind::Trait | DefKind::Impl
        | DefKind::TypeAlias | DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy | DefKind::ForeignFn | DefKind::ForeignStatic | DefKind::ForeignTy);
    let all: Vec<DefId> = (0..prog.defs.len()).map(|i| DefId(i as u32)).filter(|&d| s.local(d) && item_kinds(prog.def(d).kind)).collect();
    for &d in &all {
        let def = prog.def(d);
        if def.kind == DefKind::Mod && def.parent == NO_DEF { continue; }
        let allow = s.allow_or_lang(d);
        if let Some(a) = allow {
            let w = Work { id: d, propagated: a, own: a };
            if a { deferred.push(w) } else { s.worklist.push(w) }
        }
        match def.kind {
            DefKind::Enum => {
                if let Some(a) = allow {
                    for &v in &prog.variants[def.sub as usize] {
                        let w = Work { id: v, propagated: a, own: a };
                        if a { deferred.push(w) } else { s.worklist.push(w) }
                    }
                }
            }
            DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy => {
                if allow.is_none() && def.parent != NO_DEF && prog.def(def.parent).kind == DefKind::Impl && s.tcx.impl_trait.get(&def.parent).is_some() {
                    if let Some(t) = s.trait_item_of(d).filter(|&t| s.local(t)) {
                        if let Some(a) = s.allow_or_lang(t) {
                            let w = Work { id: d, propagated: a, own: a };
                            if a { deferred.push(w) } else { s.worklist.push(w) }
                        }
                    }
                    unsolved.push(d);
                }
            }
            DefKind::Impl => {
                if allow.is_none() && s.tcx.impl_trait.get(&d).is_some() {
                    if let Some(&t) = s.tcx.impl_trait.get(&d) {
                        if s.local(t) {
                            if let Some(a) = s.allow_or_lang(t) {
                                let w = Work { id: d, propagated: a, own: a };
                                if a { deferred.push(w) } else { s.worklist.push(w) }
                            }
                        }
                    }
                    unsolved.push(d);
                }
            }
            DefKind::Const => {
                if prog.name(d) == "_" { s.worklist.push(Work { id: d, propagated: false, own: false }); }
            }
            _ => {}
        }
    }
    s.mark_with_unsolved(&mut unsolved);
    let pre_live = s.live.clone();
    let pre_derived = s.ignored_derived.clone();
    s.worklist.extend(deferred);
    s.mark_with_unsolved(&mut unsolved);
    let _ = pre_derived;
    // report
    let rep = Reporter { s: &s, reach: &reach };
    if executable { rep.run(&DEAD_CODE_PUB_IN_BINARY, &pre_live, true, &all); }
    rep.run(&DEAD_CODE, &s.live, false, &all);
    crate::lint_late::missing_copy_implementations(prog, tcx, own, &reach);
    crate::lint_late::variant_size_differences(prog, tcx, own);
    crate::lint_late::async_fn_in_trait(prog, own, &reach);
    crate::lint_late::repr_transparent_fields(prog, tcx, own);
    unnameable_types(prog, own, &reach);
    crate::lint_imports::check(prog, tcx, own, &reach);
}

fn is_executable(prog: &Program, root: u32) -> bool {
    let rm = prog.crates[root as usize].root_mod;
    let main = prog.syms.intern_late("main");
    let has_main = prog.lookup_in_container(prog.mods[rm as usize].def, main, false).is_some_and(|m| prog.def(m).kind == DefKind::Fn);
    let ast = &prog.files[prog.mods[rm as usize].file as usize].ast;
    let lib = ast.root_attrs.iter().any(|a| {
        lint::attr_path(prog, prog.mods[rm as usize].file, a) == b"crate_type" && lint::attr_text(prog, prog.mods[rm as usize].file, a).contains("\"lib\"")
    });
    has_main && !lib
}

// ================================================================ reporting

struct Reporter<'a, 'b> {
    s: &'b Dead<'a>,
    reach: &'b Reach,
}

struct DeadItem {
    pos: u32,
    name: String,
    level: Level,
    /// the definition whose attributes (plus `inner`) decide the lint level
    scope: DefId,
    inner: Vec<Attr>,
    descr: &'static str,
}

impl<'a, 'b> Reporter<'a, 'b> {
    fn def_descr(&self, d: DefId) -> &'static str {
        let prog = self.s.prog;
        let def = prog.def(d);
        match def.kind {
            DefKind::Fn | DefKind::ForeignFn => "function",
            DefKind::AssocFn => {
                let ast = &prog.files[def.file as usize].ast;
                match ast.item(def.item).kind {
                    ItemKind::Fn(sig, _, _) if ast.fn_sig(sig).self_param.is_some() => "method",
                    _ => "associated function",
                }
            }
            DefKind::Struct => "struct",
            DefKind::Union => "union",
            DefKind::Enum => "enum",
            DefKind::Variant => "variant",
            DefKind::Const => "constant",
            DefKind::AssocConst => "associated constant",
            DefKind::Static | DefKind::ForeignStatic => "static",
            DefKind::TypeAlias => "type alias",
            DefKind::AssocTy => "associated type",
            DefKind::ForeignTy => "foreign type",
            DefKind::Trait => "trait",
            _ => "item",
        }
    }

    fn emit(&self, lint: &Lint, items: &[&DeadItem], participle: &str, parent: Option<DefId>, tuple_fields: bool) {
        let Some(first) = items.first() else { return };
        let prog = self.s.prog;
        let names: Vec<String> = items.iter().map(|i| format!("`{}`", i.name)).collect();
        let mut descr = first.descr;
        if items.iter().any(|i| i.descr != descr) { descr = "associated item"; }
        let n = items.len();
        let list = match n {
            1 => names[0].clone(),
            2 => format!("{} and {}", names[0], names[1]),
            _ => format!("{}, and {}", names[..n - 1].join(", "), names[n - 1]),
        };
        let multiple = n > 6;
        let msg = if multiple {
            format!("multiple {}s are never {}", descr, participle)
        } else if n == 1 {
            format!("{} {} is never {}", descr, list, participle)
        } else {
            format!("{}s {} are never {}", descr, list, participle)
        };
        let _ = (parent, tuple_fields);
        let def = prog.def(first.scope);
        if lint::external(prog, def.file, first.pos) { return; }
        lint::emit_in(prog, first.scope, &first.inner, lint, def.file, first.pos, &msg);
    }

    fn warn_multiple(&self, lint: &Lint, items: Vec<DeadItem>, participle: &str, parent: DefId, tuple_fields: bool) {
        let mut items: Vec<DeadItem> = items.into_iter().filter(|i| !i.name.starts_with('_')).collect();
        if items.is_empty() { return; }
        items.sort_by_key(|i| i.level);
        let mut k = 0;
        while k < items.len() {
            let mut e = k + 1;
            while e < items.len() && items[e].level == items[k].level { e += 1; }
            let group: Vec<&DeadItem> = items[k..e].iter().collect();
            self.emit(lint, &group, participle, Some(parent), tuple_fields);
            k = e;
        }
    }

    fn item(&self, lint: &Lint, d: DefId, name: String, inner: Vec<Attr>, scope: DefId, pos: u32) -> DeadItem {
        let level = lint::level_with(self.s.prog, scope, &inner, lint);
        DeadItem { pos, name, level, scope, inner, descr: self.def_descr(d) }
    }

    fn is_live_code(&self, live: &HashSet<DefId>, d: DefId) -> bool {
        let name = self.s.prog.name(d);
        if name.is_empty() { return true; }
        live.contains(&d) || name.starts_with('_')
    }

    fn is_zst(&self, t: crate::types::TyId, depth: u32) -> bool {
        if depth > 8 { return false; }
        let tcx = self.s.tcx;
        match *tcx.tys.kind(t) {
            TyKind::Tuple(l) => tcx.tys.list(l).iter().all(|&x| self.is_zst(x, depth + 1)),
            TyKind::Array(x, _) => self.is_zst(x, depth + 1),
            TyKind::Adt(d, args, _) => {
                if lint::is_diagnostic_item(self.s.prog, d, "PhantomData") { return true; }
                let Some(a) = tcx.adts.get(&d) else { return false };
                if a.is_enum || a.is_union || a.variants.len() != 1 { return false; }
                let args: Vec<_> = tcx.tys.list(args).to_vec();
                a.variants[0].fields.iter().all(|f| self.is_zst(tcx.tys.subst(f.ty, &args), depth + 1))
            }
            TyKind::FnDef(..) => true,
            // the layout of a generic type cannot be computed: rustc takes it for zero-sized
            TyKind::Param(_) | TyKind::Alias(..) | TyKind::Error | TyKind::Infer(_) => true,
            _ => false,
        }
    }

    fn should_warn_field(&self, adt: DefId, vi: u32, i: u32, live_fields: &HashSet<FieldKey>) -> bool {
        if live_fields.contains(&(adt, vi, i)) { return false; }
        let Some(a) = self.s.tcx.adts.get(&adt) else { return false };
        let f = &a.variants[vi as usize].fields[i as usize];
        // PhantomData fields; positional zero-sized fields
        if let TyKind::Adt(d, ..) = *self.s.tcx.tys.kind(f.ty) { if lint::is_diagnostic_item(self.s.prog, d, "PhantomData") { return false; } }
        let positional = a.variants[vi as usize].shape == 1;
        if positional && self.is_zst(f.ty, 0) { return false; }
        true
    }

    fn run(&self, lint: &Lint, live: &HashSet<DefId>, pub_in_binary: bool, all: &[DefId]) {
        let prog = self.s.prog;
        let tcx = self.s.tcx;
        // live fields are those found in the final solution (the pre-deferred one only for items)
        let live_fields = &self.s.live_fields;
        let candidates = |d: DefId| -> bool {
            if !pub_in_binary { return true; }
            self.reach.defs.contains(&d) && !live.contains(&d)
        };
        for &d in all {
            let def = prog.def(d);
            let ast = &prog.files[def.file as usize].ast;
            // free items only: not assoc items, not modules
            match def.kind {
                DefKind::Mod | DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy => continue,
                _ => {}
            }
            // the items of an inherent impl and of a live trait
            if (def.kind == DefKind::Impl && self.s.tcx.impl_trait.get(&d).is_none()) || (def.kind == DefKind::Trait && live.contains(&d)) {
                let mut dead = Vec::new();
                for &c in self.s.children.get(&d).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let cd = prog.def(c);
                    if !matches!(cd.kind, DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy) { continue; }
                    if self.is_live_code(live, c) || !candidates(c) { continue; }
                    let a = &prog.files[cd.file as usize].ast;
                    let it = a.item(cd.item);
                    dead.push(self.item(lint, c, prog.name(c).to_string(), a.list(it.attrs).to_vec(), c, it.name.lo));
                }
                if !dead.is_empty() {
                    // the impl's items are linted at their own level; the attributes are in the def chain
                    self.warn_multiple(lint, dead, "used", d, false);
                }
            }
            if def.kind == DefKind::Impl { continue; }
            if !live.contains(&d) {
                if !candidates(d) { continue; }
                // closures and anonymous constants are live with the item around them
                let mut parent = def.parent;
                while parent != NO_DEF && matches!(prog.def(parent).kind, DefKind::Closure | DefKind::AnonConst) { parent = prog.def(parent).parent; }
                if parent != NO_DEF && prog.def(parent).kind != DefKind::Mod && !live.contains(&parent) { continue; }
                if parent != NO_DEF && prog.def(parent).kind == DefKind::Mod && prog.block_owner.get(&prog.def(parent).sub).is_some() {
                    // an item inside a body: its owner decides
                    if let Some(&owner) = prog.block_owner.get(&prog.def(parent).sub) { if !live.contains(&owner) { continue; } }
                }
                if self.is_live_code(live, d) { continue; }
                let participle = match def.kind {
                    DefKind::Struct => "constructed",
                    DefKind::Fn | DefKind::Static | DefKind::Const | DefKind::TypeAlias | DefKind::Enum | DefKind::Union | DefKind::ForeignTy | DefKind::Trait | DefKind::ForeignFn | DefKind::ForeignStatic => "used",
                    _ => continue,
                };
                let it = ast.item(def.item);
                let item = self.item(lint, d, prog.name(d).to_string(), Vec::new(), d, it.name.lo);
                self.emit(lint, &[&item], participle, None, false);
                continue;
            }
            if pub_in_binary { continue; }
            if let DefKind::Struct | DefKind::Union | DefKind::Enum = def.kind {
                let Some(adt) = tcx.adts.get(&d) else { continue };
                let mut dead_variants = Vec::new();
                for (vi, var) in adt.variants.iter().enumerate() {
                    let vdef = var.def;
                    let (vattrs, vname_pos): (Vec<Attr>, u32) = if def.kind == DefKind::Enum {
                        match ast.item(def.item).kind {
                            ItemKind::Enum(r) => { let v = ast.list(ast.row(r).variants)[vi]; (ast.list(v.attrs).to_vec(), v.name.lo) }
                            _ => (Vec::new(), 0),
                        }
                    } else { (Vec::new(), 0) };
                    if def.kind == DefKind::Enum && !live.contains(&vdef) {
                        dead_variants.push(self.item(lint, vdef, prog.name(vdef).to_string(), vattrs.clone(), d, vname_pos));
                        continue;
                    }
                    let positional = var.shape == 1;
                    let mut dead_fields = Vec::new();
                    for (fi, f) in var.fields.iter().enumerate() {
                        if !self.should_warn_field(d, vi as u32, fi as u32, live_fields) { continue; }
                        // a field of a reachable type that is `pub` is live
                        if self.reach.fields.contains(&(d, vi as u32, fi as u32)) { continue; }
                        let (fattrs, fpos) = field_ast(prog, d, vi, fi);
                        let mut inner = vattrs.clone();
                        inner.extend(fattrs);
                        let fname = if var.shape == 1 { fi.to_string() } else { prog.syms.str(f.name).to_string() };
                        dead_fields.push(DeadItem { pos: fpos, name: fname, level: lint::level_with(prog, d, &inner, lint), scope: d, inner, descr: "field" });
                    }
                    self.warn_multiple(lint, dead_fields, "read", d, positional);
                }
                self.warn_multiple(lint, dead_variants, "constructed", d, false);
            }
        }
    }


}

fn field_ast(prog: &Program, adt: DefId, vi: usize, fi: usize) -> (Vec<Attr>, u32) {
    let def = prog.def(adt);
    let ast = &prog.files[def.file as usize].ast;
    let list = match ast.item(def.item).kind {
        ItemKind::Struct(r) => match ast.row(r).data { VariantData::Struct(l) | VariantData::Tuple(l) => Some(l), _ => None },
        ItemKind::Union(r) => Some(ast.row(r).fields),
        ItemKind::Enum(r) => match ast.list(ast.row(r).variants)[vi].data { VariantData::Struct(l) | VariantData::Tuple(l) => Some(l), _ => None },
        _ => None,
    };
    match list {
        Some(l) => {
            let f = ast.list(l)[fi];
            (ast.list(f.attrs).to_vec(), f.name.map_or(f.ty_span.0, |n| n.lo))
        }
        None => (Vec::new(), ast.item(def.item).name.lo),
    }
}

pub const UNNAMEABLE_TYPES: Lint = lint::lint("unnameable_types");

/// rustc_privacy `check_unnameable`: a type reachable through the interface of a public item but
/// not nameable by any public path.
fn unnameable_types(prog: &Program, own: &[bool], reach: &Reach) {
    for &d in reach.defs.iter() {
        let def = prog.def(d);
        if reach.named.contains(&d) || !own.get(def.krate as usize).copied().unwrap_or(false) || lint::generated(prog, d) { continue; }
        let kind = match def.kind {
            DefKind::Struct => "struct", DefKind::Enum => "enum", DefKind::Union => "union", DefKind::Trait => "trait", DefKind::TypeAlias => "type alias",
            _ => continue,
        };
        let pf = &prog.files[def.file as usize];
        let item = pf.ast.item(def.item);
        if lint::external(prog, def.file, item.lo) { continue; }
        let name = pf.text(item.name);
        lint::emit(prog, d, &UNNAMEABLE_TYPES, def.file, item.lo, &format!("{} `{}` is reachable but cannot be named", kind, name));
    }
}

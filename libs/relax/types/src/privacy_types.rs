//! rustc_privacy `TypePrivacyVisitor` (check_mod_privacy): the types of a checked crate's
//! expressions and patterns -- inferred ones included -- and of its item signatures
//! (rustc_ty_utils sig_types::walk_types) may not name a definition the module they are in
//! cannot see: "type `Priv` is private", "trait `Tr` is private". The definitions a type names
//! are found as rustc's `DefIdVisitorSkeleton` finds them: ADTs, fn items (with their
//! signature and, for an associated fn of an impl, the impl's self type), foreign types,
//! projections (the associated type and its trait), type aliases, the traits of `dyn` types and
//! the bounds of opaque types. Closures and coroutines are visible in the module they are
//! defined in and are not followed.
//!
//! Relax's own std/core/alloc are not rustc's: their private helper types are not checked.

use crate::ast::{ExprId, ExprKind, PatId};
use crate::next_solver::{ClauseKind, PredicateKind, Term, TraitRef};
use crate::consts::GenericArg;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{AliasTyKind, TyId, TyKind};

struct Skeleton<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    /// the module the checked code is in
    module: u32,
    file: u32,
    pos: u32,
    visited: crate::fxhash::HashSet<TyId>,
    errors: Vec<String>,
}

fn sysroot_crate(prog: &Program, krate: u32) -> bool {
    matches!(prog.syms.str(prog.crates[krate as usize].name), "core" | "alloc" | "std" | "proc_macro" | "test")
}

impl Skeleton<'_> {
    /// rustc_privacy `visibility` / `local_visibility`: an associated item of a trait or of a
    /// trait impl has the trait's visibility, a variant its enum's.
    fn visibility(&self, d: DefId) -> crate::program::BindVis {
        let def = self.prog.def(d);
        match def.kind {
            DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy => match self.prog.def(def.parent).kind {
                DefKind::Trait => return self.visibility(def.parent),
                DefKind::Impl => if let Some(&tr) = self.tcx.impl_trait.get(&def.parent) { return self.visibility(tr) },
                _ => {}
            },
            DefKind::Variant => return self.visibility(def.parent),
            _ => {}
        }
        self.prog.def_item_vis(d)
    }

    /// `check_def_id`: true (stop) when `d` is not accessible from the module.
    fn def_id(&mut self, d: DefId, kind: &str, descr: &dyn Fn(&Self) -> String) -> bool {
        if sysroot_crate(self.prog, self.prog.def(d).krate) || self.prog.vis_visible(self.visibility(d), self.module) {
            return false;
        }
        let msg = format!("{} `{}` is private", kind, descr(self));
        self.errors.push(self.prog.record_uncoded_error(self.file, self.pos, "privacy", msg));
        true
    }

    fn trait_ref(&mut self, def: DefId, args: &[GenericArg]) -> bool {
        if self.def_id(def, "trait", &|s: &Self| s.prog.def_path(def)) { return true; }
        self.args(args)
    }

    fn args(&mut self, args: &[GenericArg]) -> bool {
        args.iter().any(|a| matches!(a, GenericArg::Type(t) if self.ty(*t)))
    }

    fn tys(&mut self, l: &[TyId]) -> bool {
        l.iter().any(|t| self.ty(*t))
    }

    /// `visit_clause` over an opaque type's bounds.
    fn clauses(&mut self, preds: &[crate::next_solver::Predicate]) -> bool {
        for p in preds {
            let PredicateKind::Clause(c) = &p.kind else { continue };
            let stop = match c {
                ClauseKind::Trait(t) => { let TraitRef { def_id, args } = &t.trait_ref; self.trait_ref(*def_id, args) }
                ClauseKind::Projection(pc) => {
                    (match pc.term { Term::Ty(t) => self.ty(t), Term::Const(_) => false })
                        || match &pc.projection_term { crate::next_solver::AliasTerm::Ty(t) => self.ty(*t), _ => false }
                }
                ClauseKind::TypeOutlives(t, _) => self.ty(*t),
                ClauseKind::ConstArgHasType(_, t) => self.ty(*t),
                ClauseKind::WellFormed(Term::Ty(t)) => self.ty(*t),
                _ => false,
            };
            if stop { return true; }
        }
        false
    }

    /// `DefIdVisitorSkeleton::visit_ty`.
    fn ty(&mut self, t: TyId) -> bool {
        let (prog, tcx) = (self.prog, self.tcx);
        let shown = |s: &Self| crate::typeck::ty_to_string(false, s.prog, s.tcx, t);
        match tcx.tys.kind(t).clone() {
            TyKind::Adt(d, args, _) => {
                if self.def_id(d, "type", &shown) { return true; }
                self.tys(tcx.tys.list(args))
            }
            TyKind::Foreign(d) => self.def_id(d, "type", &shown),
            TyKind::FnDef(d, args, _) => {
                if self.def_id(d, "type", &shown) { return true; }
                // (a fn item's signature is part of its type: `fn() -> Priv {f}` is private)
                if let Some(sig) = tcx.fn_sig(prog, d).cloned() {
                    if self.tys(&sig.params) || self.ty(sig.ret) { return true; }
                }
                // (an inherent associated fn's type has no self type in its args)
                let def = prog.def(d);
                if def.kind == DefKind::AssocFn && prog.def(def.parent).kind == DefKind::Impl {
                    if let Some(&st) = tcx.impl_self.get(&def.parent) {
                        if self.ty(st) { return true; }
                    }
                }
                self.tys(tcx.tys.list(args))
            }
            TyKind::Alias(_, a) => match a.kind {
                AliasTyKind::Opaque { def_id } => {
                    if !self.visited.insert(t) { return false; }
                    let bounds = tcx.explicit_item_bounds(prog, def_id).to_vec();
                    self.clauses(&bounds)
                }
                AliasTyKind::Projection { def_id } | AliasTyKind::Inherent { def_id } | AliasTyKind::Free { def_id } => {
                    if !self.visited.insert(t) { return false; }
                    let kind = if matches!(a.kind, AliasTyKind::Free { .. }) { "type alias" } else { "associated type" };
                    if self.def_id(def_id, kind, &|s: &Self| s.prog.def_path(def_id)) { return true; }
                    // a projection's trait (visit_projection_term), then its arguments
                    if matches!(a.kind, AliasTyKind::Projection { .. }) {
                        let tr = prog.def(def_id).parent;
                        if prog.def(tr).kind == DefKind::Trait && self.def_id(tr, "trait", &|s: &Self| s.prog.def_path(tr)) { return true; }
                    }
                    self.tys(tcx.tys.list(a.args))
                }
            },
            TyKind::Dyn(principal, args, binds, _, _, _, autos) => {
                if self.def_id(principal, "trait", &|s: &Self| s.prog.def_path(principal)) { return true; }
                for &(assoc, _, _) in tcx.tys.list(binds) {
                    let tr = prog.def(assoc).parent;
                    if self.def_id(tr, "trait", &|s: &Self| s.prog.def_path(tr)) { return true; }
                }
                for &d in tcx.tys.list(autos) {
                    if self.def_id(d, "trait", &|s: &Self| s.prog.def_path(d)) { return true; }
                }
                if self.tys(tcx.tys.list(args)) { return true; }
                tcx.tys.list(binds).iter().any(|&(_, b, _)| self.ty(b))
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => self.ty(x),
            TyKind::Tuple(l) => self.tys(tcx.tys.list(l)),
            TyKind::FnPtr(ps, ret, _, _) => self.tys(tcx.tys.list(ps)) || self.ty(ret),
            _ => false,
        }
    }
}

fn skeleton<'a>(prog: &'a Program, tcx: &'a Tcx, d: DefId) -> Skeleton<'a> {
    let def = prog.def(d);
    Skeleton { prog, tcx, module: prog.mods[def.scope as usize].normal, file: def.file, pos: 0, visited: Default::default(), errors: Vec::new() }
}

/// The type privacy of a type-checked body (rustc `TypePrivacyVisitor::visit_nested_body`):
/// every expression's and pattern's type, a path's generic arguments, the targets of its
/// adjustments, and a method call's method type. A node with an error stops the walk below it.
pub fn check_body(prog: &Program, tcx: &Tcx, body: &crate::typeck::Body) -> Vec<String> {
    if !tcx.validates(prog, body.file) || sysroot_crate(prog, prog.def(body.def).krate) { return Vec::new(); }
    let d = prog.def(body.def);
    let ast = &prog.files[body.file as usize].ast;
    let mut v = BodyVisitor { s: skeleton(prog, tcx, body.def), body, ast };
    v.s.file = body.file;
    match ast.item(d.item).kind {
        crate::ast::ItemKind::Fn(sig, _, _) if d.kind != DefKind::AnonConst => {
            let Ok(Some(block)) = prog.fn_body(body.file, d.item) else { return Vec::new() };
            for p in ast.list(ast.fn_sig(sig).params) { crate::ast_visit::Visitor::pat(&mut v, p.pat); }
            crate::ast_visit::Visitor::block(&mut v, block);
        }
        crate::ast::ItemKind::Const(..) | crate::ast::ItemKind::Static(..) if d.kind != DefKind::AnonConst => {
            let Ok(Some(e)) = prog.const_body(body.file, d.item) else { return Vec::new() };
            crate::ast_visit::Visitor::expr(&mut v, e);
        }
        _ => {}
    }
    v.s.errors
}

struct BodyVisitor<'a> {
    s: Skeleton<'a>,
    body: &'a crate::typeck::Body,
    ast: &'a crate::ast::Ast,
}

impl BodyVisitor<'_> {
    /// `check_expr_pat_type` of an expression.
    fn expr_type(&mut self, e: ExprId) -> bool {
        if !self.body.has_expr(e) { return false; }
        let t = self.body.expr_ty[self.body.expr_ix(e)];
        self.s.pos = self.ast.expr(e).lo;
        if self.s.ty(t) { return true; }
        if let Some(args) = self.body.res_args.get(&e.0) {
            if self.s.tys(self.s.tcx.tys.list(*args)) { return true; }
        }
        if let Some(adj) = self.body.adjustments.get(&e.0) {
            for a in adj.operand.iter().chain(adj.value.iter()) {
                if self.s.ty(a.target) { return true; }
            }
        }
        false
    }
}

impl<'a> crate::ast_visit::Visitor<'a> for BodyVisitor<'a> {
    fn ast(&self) -> &'a crate::ast::Ast { self.ast }

    fn expr(&mut self, e: ExprId) {
        if self.expr_type(e) { return; }
        match self.ast.expr(e).kind {
            // (no duplicate errors for `x = y` and `match x { .. }`)
            ExprKind::Assign(_, rhs) | ExprKind::Match(rhs, _) => if self.expr_type(rhs) { return },
            ExprKind::MethodCall(..) => {
                if let Some(m) = self.body.methods.get(&e.0) {
                    let d = m.def;
                    // (`type_of(method)`: the fn item at its identity arguments)
                    let args = self.s.tcx.generic_indices_of(self.s.prog, d).identity_types_consts(&self.s.tcx.tys);
                    let t = self.s.tcx.tys.intern(TyKind::FnDef(d, self.s.tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY));
                    if self.s.ty(t) { return; }
                }
            }
            _ => {}
        }
        crate::ast_visit::walk_expr(self, e)
    }

    fn pat(&mut self, p: PatId) {
        let i = self.body.pat_ix(p);
        if i < self.body.pat_ty.len() && self.body.pat_typed.get(i) {
            self.s.pos = pat_pos(self.ast, p, self.s.pos);
            if self.s.ty(self.body.pat_ty[i]) { return; }
        }
        crate::ast_visit::walk_pat(self, p)
    }
}

/// The type privacy of an item's signature (rustc_ty_utils sig_types::walk_types under
/// check_mod_privacy): fn signatures, field types, const/static types, impl headers, the values
/// of associated types; the item's own module decides.
pub fn check_item(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    let def = prog.def(d);
    if !tcx.validates(prog, def.file) || sysroot_crate(prog, def.krate) || def.item.0 == u32::MAX { return Vec::new(); }
    if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { return Vec::new(); }
    let mut s = skeleton(prog, tcx, d);
    s.pos = prog.files[def.file as usize].ast.item(def.item).lo;
    match def.kind {
        DefKind::Fn | DefKind::AssocFn => {
            if let Some(sig) = tcx.fn_sig(prog, d).cloned() {
                let _ = s.tys(&sig.params) || s.ty(sig.ret);
            }
        }
        DefKind::Struct | DefKind::Union | DefKind::Enum => {
            if let Some(adt) = tcx.adts.get(&d).cloned() {
                // (a struct's field error is at the field's written type)
                let ast = &prog.files[def.file as usize].ast;
                let written: Vec<u32> = match ast.as_struct(&ast.item(def.item).kind).map(|st| st.data) {
                    Some(crate::ast::VariantData::Tuple(l)) | Some(crate::ast::VariantData::Struct(l)) => ast.list(l).iter().map(|f| f.ty_span.0).collect(),
                    _ => Vec::new(),
                };
                let item_lo = s.pos;
                'v: for v in &adt.variants {
                    for (i, f) in v.fields.iter().enumerate() {
                        s.pos = if adt.variants.len() == 1 { written.get(i).copied().filter(|p| *p != 0).unwrap_or(item_lo) } else { item_lo };
                        if s.ty(f.ty) { break 'v; }
                    }
                }
            }
        }
        DefKind::Const | DefKind::AssocConst | DefKind::Static => {
            if let Some(t) = tcx.const_ty(prog, d) { s.ty(t); }
        }
        DefKind::AssocTy if prog.def(def.parent).kind == DefKind::Impl => {
            if let Ok(t) = tcx.type_of(prog, d) { s.ty(t); }
        }
        DefKind::Impl => {
            if let Some(&st) = tcx.impl_self.get(&d) {
                if !s.ty(st) {
                    if let Some(&tr) = tcx.impl_trait.get(&d) {
                        // (the self type was visited above)
                        if !s.def_id(tr, "trait", &|s: &Skeleton| s.prog.def_path(tr)) {
                            if let Some(args) = tcx.impl_trait_args.get(&d).cloned() { s.tys(&args); }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    s.errors
}

/// Where a pattern starts (patterns carry no span of their own).
fn pat_pos(ast: &crate::ast::Ast, p: PatId, fallback: u32) -> u32 {
    use crate::ast::Pat;
    match *ast.pat(p) {
        Pat::Ident { name, .. } => name.lo,
        Pat::Lit(e) | Pat::Range(Some(e), _, _) | Pat::Range(None, Some(e), _) => ast.expr(e).lo,
        Pat::Path(ref path) | Pat::TupleStruct(ref path, _) | Pat::Struct(ref path, _, _) => path.lo,
        Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => pat_pos(ast, x, fallback),
        Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => ast.list(ps).first().map_or(fallback, |x| pat_pos(ast, *x, fallback)),
        _ => fallback,
    }
}

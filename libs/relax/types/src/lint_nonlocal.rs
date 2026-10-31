//! `non_local_definitions` (rustc_lint non_local_def.rs): an `impl` inside a body whose self type
//! and trait do not name an item declared at the impl's own level. Check-only.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;

pub const NON_LOCAL_DEFINITIONS: Lint = lint::lint("non_local_definitions");

/// The definition that encloses `d` as rustc's `tcx.parent` sees it: a body's block module is
/// its owner.
fn parent(prog: &Program, d: DefId) -> DefId {
    let p = prog.def(d).parent;
    if p != NO_DEF && prog.def(p).kind == DefKind::Mod {
        if let Some(&o) = prog.block_owner.get(&prog.def(p).sub) { return o; }
    }
    p
}

fn is_anon_const(prog: &Program, d: DefId) -> bool {
    prog.def(d).kind == DefKind::Const && prog.syms.str(prog.def(d).name) == "_"
}

/// `peel_parent_while` with "a module or an anonymous const": the first ancestor that is neither.
fn peel(prog: &Program, mut d: DefId) -> Option<DefId> {
    while prog.def(d).parent != NO_DEF && (prog.def(d).kind == DefKind::Mod || is_anon_const(prog, d)) {
        d = parent(prog, d);
        if d == NO_DEF { return None; }
    }
    Some(d)
}

fn is_body_owner(k: DefKind) -> bool {
    matches!(k, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static | DefKind::AnonConst | DefKind::Closure)
}

pub(crate) struct Paths<'a> {
    pub(crate) prog: &'a Program,
    pub(crate) tcx: &'a Tcx,
    pub(crate) file: u32,
    pub(crate) module: u32,
    pub(crate) ast: &'a Ast,
    /// the type parameter names
    pub(crate) params: Vec<String>,
    pub(crate) out: Vec<(DefId, u32)>,
}

impl<'a> Paths<'a> {
    pub(crate) fn path(&mut self, p: &Path) {
        let ast = self.ast;
        if let Some(q) = ast.qself(p) {
            let (ty, tp) = (q.ty, q.trait_path);
            self.ty(ty);
            if let Some(tp) = tp { self.path(&tp); }
            return;
        }
        let segs = ast.segs(p.segs);
        // generic arguments of every segment
        for s in segs {
            if let Some(ga) = ast.gargs(s.args()) {
                match *ga {
                    GenericArgs::Angle(l) => for a in ast.list(l) {
                        match *a {
                            GenericArg::Type(t) => self.ty(t),
                            GenericArg::Binding(_, _, t) => self.ty(t),
                            GenericArg::Constraint(_, _, bs) => self.bounds(bs),
                            _ => {}
                        }
                    },
                    GenericArgs::Paren(ins, out) => {
                        for &t in ast.list(ins) { self.ty(t); }
                        if let Some(t) = out { self.ty(t); }
                    }
                }
            }
        }
        // a lone type parameter is no item
        if segs.len() == 1 {
            let n = self.prog.files[self.file as usize].text(segs[0].name);
            if self.params.iter().any(|x| x == n) || n == "Self" { return; }
        }
        let d = self.tcx.resolve_type_path_def(self.prog, self.file, self.module, p).or_else(|| {
            if segs.len() > 1 {
                let sub = Path { segs: p.segs.prefix(p.segs.len() - 1), lo: p.lo };
                self.tcx.resolve_type_path_def(self.prog, self.file, self.module, &sub)
            } else { None }
        });
        if let Some(d) = d { self.out.push((d, p.lo)); }
    }

    pub(crate) fn bounds(&mut self, l: List<Bound>) {
        for b in self.ast.list(l) {
            if let Bound::Trait { path, .. } = *b { self.path(&path); }
        }
    }

    pub(crate) fn ty(&mut self, t: crate::ast::TyId) {
        let ast = self.ast;
        match *ast.ty(t) {
            Ty::Path(p) => self.path(&p),
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.ty(x),
            Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x); },
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                for &x in ast.list(f.params) { self.ty(x); }
                if let Some(r) = f.ret { self.ty(r); }
            }
            Ty::ImplTrait(bs) | Ty::DynTrait(bs, _) => self.bounds(bs),
            _ => {}
        }
    }
}

/// Runs the pass over the impls of the checked crates marked in `own`.
pub fn check(prog: &Program, tcx: &Tcx, own: &[bool]) {
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if d.kind != DefKind::Impl || !own.get(d.krate as usize).copied().unwrap_or(false) { continue; }
        let id = DefId(i as u32);
        // the bodies around the impl
        let mut depth = 0;
        let mut p = parent(prog, id);
        while p != NO_DEF {
            if is_body_owner(prog.def(p).kind) { depth += 1; }
            p = parent(prog, p);
        }
        if depth == 0 { continue; }
        let impl_parent = parent(prog, id);
        if impl_parent == NO_DEF { continue; }
        // `const _: () = { impl .. }` at the top level is ignored
        if depth == 1 && is_anon_const(prog, impl_parent) { continue; }
        let ast = &prog.files[d.file as usize].ast;
        let item = *ast.item(d.item);
        let Some(imp) = ast.as_impl(&item.kind) else { continue };
        let imp = *imp;
        let params: Vec<String> = ast.list(imp.generics.params).iter()
            .filter(|g| !matches!(g.kind, GenericParamKind::Lifetime(_)))
            .map(|g| prog.files[d.file as usize].text(g.name).to_string()).collect();
        let mut c = Paths { prog, tcx, file: d.file, module: d.scope, ast, params, out: Vec::new() };
        c.ty(imp.self_ty);
        if let Some(t) = imp.trait_ { c.path(&t); }
        let outermost = peel(prog, impl_parent);
        let has_local_parent = c.out.iter().any(|&(did, _)| {
            if prog.def(did).krate != d.krate { return false; }
            let pd = parent(prog, did);
            if pd == NO_DEF { return false; }
            match peel(prog, pd) {
                Some(pp) => pp == impl_parent || Some(pp) == outermost,
                None => false,
            }
        });
        if has_local_parent { continue; }
        lint::emit(prog, id, &NON_LOCAL_DEFINITIONS, d.file, item.lo, "non-local `impl` definition, `impl` blocks should be written at the same level as their item");
    }
}

impl<'a> Paths<'a> {
    pub(crate) fn generics(&mut self, g: Generics) {
        let ast = self.ast;
        for p in ast.list(g.params) {
            match p.kind {
                GenericParamKind::Type(bs, def) => { self.bounds(bs); if let Some(t) = def { self.ty(t); } }
                GenericParamKind::Const(t, _) => self.ty(t),
                GenericParamKind::Lifetime(_) => {}
            }
        }
        for w in ast.list(g.where_) {
            if let WherePred::Bound { ty, bounds, .. } = *w { self.ty(ty); self.bounds(bounds); }
        }
    }

    fn fields(&mut self, d: VariantData) {
        if let VariantData::Struct(l) | VariantData::Tuple(l) = d {
            for f in self.ast.list(l) { self.ty(f.ty); }
        }
    }

    /// Every type path written in the signature (and fields) of an item.
    pub(crate) fn item_signature(&mut self, item: &Item) {
        let ast = self.ast;
        match item.kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = *ast.fn_sig(sig);
                self.generics(sig.generics);
                for p in ast.list(sig.params) { self.ty(p.ty); }
                if let Some(r) = sig.ret { self.ty(r); }
            }
            ItemKind::Struct(r) => { let s = *ast.row(r); self.generics(s.generics); self.fields(s.data); }
            ItemKind::Union(r) => { let s = *ast.row(r); self.generics(s.generics); for f in ast.list(s.fields) { self.ty(f.ty); } }
            ItemKind::Enum(r) => {
                let e = *ast.row(r);
                self.generics(e.generics);
                for v in ast.list(e.variants) { self.fields(v.data); }
            }
            ItemKind::TypeAlias(r) => { let t = *ast.row(r); self.generics(t.generics); self.bounds(t.bounds); if let Some(x) = t.ty { self.ty(x); } }
            ItemKind::Trait(r) => { let t = *ast.row(r); self.generics(t.generics); self.bounds(t.supers); }
            ItemKind::Impl(r) => {
                let i = *ast.row(r);
                self.generics(i.generics);
                self.ty(i.self_ty);
                if let Some(t) = i.trait_ { self.path(&t); }
            }
            ItemKind::Const(t, _) => { if let Some(t) = t.get() { self.ty(t); } }
            ItemKind::Static(_, t, _) => self.ty(t),
            _ => {}
        }
    }
}

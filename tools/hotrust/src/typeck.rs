//! Type checking of one function instance. Generic functions are checked per
//! instance with concrete type arguments (rustc already proved the bounds; the
//! dialect is valid Rust), so every body HotRust lowers is fully monomorphic.
//! Inference is forward with local unification variables (dialect rule R4).

use crate::ast::*;
use crate::program::{DefId, DefKind, Prim, Program, Sym, NO_DEF};
use crate::tcx::{ty_key, FnSigT, GenEnv, Tcx, TyKey};
use crate::types::{TyId, TyKind};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Res {
    Local(u32),
    Def(DefId),
    Err,
}

#[derive(Clone)]
pub struct MethodRes {
    pub def: DefId,
    /// number of derefs applied to the receiver before the call
    pub derefs: u8,
    /// 0 none, 1 `&`, 2 `&mut`
    pub autoref: u8,
    /// full generic args (impl params then method params)
    pub args: Vec<TyId>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coerce {
    /// fn item -> fn pointer
    ReifyFn,
    /// non-capturing closure -> fn pointer
    ClosureFn,
    /// &[T; N] -> &[T]
    Unsize,
    /// &mut T -> &T, *mut T -> *const T
    MutToConst,
    /// `!` -> any
    Never,
}

pub struct Local {
    pub name: Sym,
    pub ty: TyId,
    pub mutable: bool,
    pub pat: u32,
}

/// Results of type checking one function instance.
pub struct Body {
    pub def: DefId,
    pub args: Vec<TyId>,
    pub file: u32,
    pub expr_lo: u32,
    pub expr_ty: Vec<TyId>,
    pub pat_lo: u32,
    pub pat_ty: Vec<TyId>,
    pub res: HashMap<u32, Res>,
    pub res_args: HashMap<u32, Vec<TyId>>,
    pub pat_res: HashMap<u32, Res>,
    pub pat_local: HashMap<u32, u32>,
    pub methods: HashMap<u32, MethodRes>,
    /// binary operators on non-primitive types: expr -> operator trait method call
    pub binops: HashMap<u32, MethodRes>,
    pub coerce: HashMap<u32, Coerce>,
    pub field_idx: HashMap<u32, (u8, u32)>,
    /// overloaded deref steps: (expr, step index) -> `Deref::deref` with [Self]
    pub ov_derefs: HashMap<(u32, u8), (DefId, Vec<TyId>)>,
    /// overloaded indexing: expr -> `Index::index` with [Self, Idx]
    pub ov_index: HashMap<u32, (DefId, Vec<TyId>)>,
    /// `for` over an Iterator: iterator expr -> `Iterator::next` with [Iter]
    pub for_next: HashMap<u32, (DefId, Vec<TyId>)>,
    /// `for` over an IntoIterator: iterator expr -> (`into_iter`, [Self], iterator type)
    pub for_into: HashMap<u32, (DefId, Vec<TyId>, TyId)>,
    pub index_derefs: HashMap<u32, u8>,
    pub locals: Vec<Local>,
    pub param_pats: Vec<PatId>,
    pub ret: TyId,
    pub body: Option<BlockId>,
    /// (macro expr, named arg) -> resolution of `{name}` in a format string
    pub fmt_named: HashMap<(u32, Sym), Res>,
    /// closures that capture nothing (can become plain functions)
    pub closures: Vec<u32>,
    pub closure_locals: HashMap<u32, Vec<u32>>,
    pub errors: Vec<String>,
}

impl Body {
    #[inline]
    pub fn ty(&self, e: ExprId) -> TyId {
        self.expr_ty[(e.0 - self.expr_lo) as usize]
    }
    #[inline]
    pub fn pty(&self, p: PatId) -> TyId {
        self.pat_ty[(p.0 - self.pat_lo) as usize]
    }
}

#[derive(Clone, Copy)]
enum VarSt {
    Free(u8), // 0 general, 1 int, 2 float
    Bound(TyId),
}

struct Loop {
    label: Option<Sym>,
    break_ty: Option<TyId>,
    is_loop: bool,
}

pub struct Fcx<'a> {
    prog: &'a Program,
    tcx: &'a mut Tcx,
    file: u32,
    module: u32,
    env: GenEnv,
    inst_args: Vec<TyId>,
    b: Body,
    scopes: Vec<(Sym, u32)>,
    vars: Vec<VarSt>,
    loops: Vec<Loop>,
    diverges: bool,
    ret_stack: Vec<TyId>,
    closure_depth: Vec<(u32, usize)>, // (closure expr, scope depth at entry)
    cur_binop: u32,
}

pub fn check_fn(prog: &Program, tcx: &mut Tcx, def: DefId, args: &[TyId]) -> Body {
    let d = prog.def(def);
    let file = d.file;
    let it = prog.files[file as usize].ast.item(d.item);
    let ranges = it.ranges;
    let env = tcx.item_env(prog, def);
    let sig = match tcx.sigs.get(&def) {
        Some(s) => s.clone(),
        None => FnSigT {
            params: Vec::new(),
            ret: tcx.tys.unit,
            self_kind: 0,
            n_generics: 0,
            n_parent_generics: 0,
            is_unsafe: false,
            c_abi: false,
            variadic: false,
        },
    };
    let n_expr = (ranges[1] - ranges[0]) as usize;
    let n_pat = (ranges[3] - ranges[2]) as usize;
    let error = tcx.tys.error;
    let mut fcx = Fcx {
        prog,
        tcx,
        file,
        module: d.scope,
        env,
        inst_args: args.to_vec(),
        b: Body {
            def,
            args: args.to_vec(),
            file,
            expr_lo: ranges[0],
            expr_ty: vec![error; n_expr],
            pat_lo: ranges[2],
            pat_ty: vec![error; n_pat],
            res: HashMap::new(),
            res_args: HashMap::new(),
            pat_res: HashMap::new(),
            pat_local: HashMap::new(),
            methods: HashMap::new(),
            binops: HashMap::new(),
            coerce: HashMap::new(),
            field_idx: HashMap::new(),
            ov_derefs: HashMap::new(),
            ov_index: HashMap::new(),
            for_next: HashMap::new(),
            for_into: HashMap::new(),
            index_derefs: HashMap::new(),
            locals: Vec::new(),
            param_pats: Vec::new(),
            ret: error,
            body: None,
            fmt_named: HashMap::new(),
            closures: Vec::new(),
            closure_locals: HashMap::new(),
            errors: Vec::new(),
        },
        scopes: Vec::new(),
        vars: Vec::new(),
        loops: Vec::new(),
        diverges: false,
        ret_stack: Vec::new(),
        closure_depth: Vec::new(),
        cur_binop: 0,
    };
    let mut params = Vec::new();
    for p in &sig.params {
        let x = fcx.tcx.tys.subst(*p, args);
        params.push(fcx.tcx.normalize(prog, x));
    }
    let ret = fcx.tcx.tys.subst(sig.ret, args);
    let ret = fcx.tcx.normalize(prog, ret);
    fcx.b.ret = ret;
    fcx.ret_stack.push(ret);
    let ast = &prog.files[file as usize].ast;
    let (fsig, body) = match &ast.item(d.item).kind {
        ItemKind::Fn(s, b, _) => (s, *b),
        ItemKind::Const(_, Some(init)) | ItemKind::Static(_, _, Some(init)) => {
            // const/static initializer: checked against the declared type
            let ct = fcx.tcx.const_tys.get(&def).copied().unwrap_or(error);
            let ct = fcx.tcx.tys.subst(ct, args);
            fcx.b.ret = ct;
            fcx.ret_stack.push(ct);
            let t = fcx.check_expr(*init, Some(ct));
            fcx.coerce_or_unify(t, ct, Some(*init), ast.expr(*init).lo);
            fcx.finish();
            return fcx.b;
        }
        _ => return fcx.b,
    };
    let mut pi = 0;
    if fsig.self_param.is_some() {
        let mutable = matches!(fsig.self_param, Some(SelfParam::Value(true)) | Some(SelfParam::Typed(true, _)));
        let s = prog.syms.get("self").unwrap_or(0);
        let li = fcx.b.locals.len() as u32;
        fcx.b.locals.push(Local { name: s, ty: params[0], mutable, pat: u32::MAX });
        fcx.scopes.push((s, li));
        pi = 1;
    }
    for p in &fsig.params {
        let t = if pi < params.len() { params[pi] } else { fcx.tcx.tys.error };
        fcx.check_pat(p.pat, t);
        fcx.b.param_pats.push(p.pat);
        pi += 1;
    }
    if let Some(bid) = body {
        fcx.b.body = Some(bid);
        let t = fcx.check_block(bid, Some(ret));
        if !fcx.diverges {
            fcx.coerce_or_unify(t, ret, None, ast.block(bid).hi);
        }
    }
    fcx.finish();
    fcx.b
}

impl<'a> Fcx<'a> {
    fn ast(&self) -> &'a Ast {
        &self.prog.files[self.file as usize].ast
    }

    fn err(&mut self, pos: u32, msg: String) {
        let (l, c) = crate::lexer::line_col(&self.prog.files[self.file as usize].src, pos);
        self.b.errors.push(format!("{}:{}:{}: {}", self.prog.file_paths[self.file as usize], l, c, msg));
    }

    fn text(&self, id: Ident) -> &'a str {
        let f = &self.prog.files[self.file as usize];
        let s = std::str::from_utf8(&f.src[id.lo as usize..id.hi as usize]).unwrap_or("?");
        match s.strip_prefix("r#") {
            Some(x) => x,
            None => s,
        }
    }

    fn sym(&self, id: Ident) -> Sym {
        self.prog.syms.get(self.text(id)).unwrap_or(u32::MAX)
    }

    // ------------------------------------------------------------ inference variables

    fn fresh(&mut self, kind: u8) -> TyId {
        let n = self.vars.len() as u32;
        self.vars.push(VarSt::Free(kind));
        self.tcx.tys.intern(TyKind::Infer(n))
    }

    fn shallow(&self, mut t: TyId) -> TyId {
        loop {
            match self.tcx.tys.kind(t) {
                TyKind::Infer(v) => match self.vars[*v as usize] {
                    VarSt::Bound(b) => t = b,
                    VarSt::Free(_) => return t,
                },
                _ => return t,
            }
        }
    }

    fn deep(&mut self, t: TyId) -> TyId {
        let t = self.shallow(t);
        let k = self.tcx.tys.kind(t).clone();
        match k {
            TyKind::Infer(v) => match self.vars[v as usize] {
                VarSt::Free(1) => self.tcx.tys.i32_,
                VarSt::Free(2) => self.tcx.tys.f64_,
                _ => t,
            },
            TyKind::Tuple(v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.deep(x));
                }
                self.tcx.tys.intern(TyKind::Tuple(n))
            }
            TyKind::Array(e, l) => {
                let e = self.deep(e);
                self.tcx.tys.intern(TyKind::Array(e, l))
            }
            TyKind::Slice(e) => {
                let e = self.deep(e);
                self.tcx.tys.intern(TyKind::Slice(e))
            }
            TyKind::Ref(m, e) => {
                let e = self.deep(e);
                self.tcx.tys.intern(TyKind::Ref(m, e))
            }
            TyKind::Ptr(m, e) => {
                let e = self.deep(e);
                self.tcx.tys.intern(TyKind::Ptr(m, e))
            }
            TyKind::Adt(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.deep(x));
                }
                self.tcx.tys.intern(TyKind::Adt(d, n))
            }
            TyKind::FnPtr(ps, r) => {
                let mut n = Vec::new();
                for x in ps {
                    n.push(self.deep(x));
                }
                let r = self.deep(r);
                self.tcx.tys.intern(TyKind::FnPtr(n, r))
            }
            TyKind::FnDef(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.deep(x));
                }
                self.tcx.tys.intern(TyKind::FnDef(d, n))
            }
            _ => t,
        }
    }

    fn unify(&mut self, a: TyId, b: TyId) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        if a == b {
            return true;
        }
        let ka = self.tcx.tys.kind(a).clone();
        let kb = self.tcx.tys.kind(b).clone();
        match (&ka, &kb) {
            (TyKind::Error, _) | (_, TyKind::Error) => true,
            (TyKind::Infer(va), _) => self.bind(*va, b),
            (_, TyKind::Infer(vb)) => self.bind(*vb, a),
            (TyKind::Tuple(x), TyKind::Tuple(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                for i in 0..x.len() {
                    if !self.unify(x[i], y[i]) {
                        return false;
                    }
                }
                true
            }
            (TyKind::Array(x, n), TyKind::Array(y, m)) => n == m && self.unify(*x, *y),
            (TyKind::Slice(x), TyKind::Slice(y)) => self.unify(*x, *y),
            (TyKind::Ref(m1, x), TyKind::Ref(m2, y)) => m1 == m2 && self.unify(*x, *y),
            (TyKind::Ptr(m1, x), TyKind::Ptr(m2, y)) => m1 == m2 && self.unify(*x, *y),
            (TyKind::Adt(d1, x), TyKind::Adt(d2, y)) => {
                if d1 != d2 || x.len() != y.len() {
                    return false;
                }
                for i in 0..x.len() {
                    if !self.unify(x[i], y[i]) {
                        return false;
                    }
                }
                true
            }
            (TyKind::FnPtr(p1, r1), TyKind::FnPtr(p2, r2)) => {
                if p1.len() != p2.len() {
                    return false;
                }
                for i in 0..p1.len() {
                    if !self.unify(p1[i], p2[i]) {
                        return false;
                    }
                }
                self.unify(*r1, *r2)
            }
            _ => false,
        }
    }

    fn bind(&mut self, v: u32, t: TyId) -> bool {
        let kind = match self.vars[v as usize] {
            VarSt::Free(k) => k,
            VarSt::Bound(b) => return self.unify(b, t),
        };
        let tk = self.tcx.tys.kind(t).clone();
        match tk {
            TyKind::Infer(w) => {
                let k2 = match self.vars[w as usize] {
                    VarSt::Free(k) => k,
                    _ => 0,
                };
                if kind != 0 && k2 != 0 && kind != k2 {
                    return false;
                }
                if kind != 0 && k2 == 0 {
                    // keep the more specific variable free
                    self.vars[w as usize] = VarSt::Bound(self.tcx.tys.intern(TyKind::Infer(v)));
                    return true;
                }
                self.vars[v as usize] = VarSt::Bound(t);
                true
            }
            TyKind::Int(_) if kind == 1 || kind == 0 => {
                self.vars[v as usize] = VarSt::Bound(t);
                true
            }
            TyKind::Float(_) if kind == 2 || kind == 0 => {
                self.vars[v as usize] = VarSt::Bound(t);
                true
            }
            _ if kind == 0 => {
                self.vars[v as usize] = VarSt::Bound(t);
                true
            }
            TyKind::Error => true,
            _ => false,
        }
    }

    fn ty_str(&mut self, t: TyId) -> String {
        let t = self.deep(t);
        ty_to_string(self.prog, self.tcx, t)
    }

    /// Coercion site: `actual` flows into a place of type `expected`.
    fn coerce_or_unify(&mut self, actual: TyId, expected: TyId, e: Option<ExprId>, pos: u32) -> bool {
        let a = self.shallow(actual);
        let x = self.shallow(expected);
        if a == x {
            return true;
        }
        let ka = self.tcx.tys.kind(a).clone();
        let kx = self.tcx.tys.kind(x).clone();
        if let TyKind::Never = ka {
            if let Some(e) = e {
                self.b.coerce.insert(e.0, Coerce::Never);
            }
            return true;
        }
        match (&ka, &kx) {
            (TyKind::FnDef(d, args), TyKind::FnPtr(ps, r)) => {
                if let Some(sig) = self.tcx.sigs.get(d).cloned() {
                    let args = args.clone();
                    let mut ok = sig.params.len() == ps.len();
                    if ok {
                        for i in 0..ps.len() {
                            let p = self.tcx.tys.subst(sig.params[i], &args);
                            ok = ok && self.unify(p, ps[i]);
                        }
                        let rr = self.tcx.tys.subst(sig.ret, &args);
                        ok = ok && self.unify(rr, *r);
                    }
                    if ok {
                        if let Some(e) = e {
                            self.b.coerce.insert(e.0, Coerce::ReifyFn);
                        }
                        return true;
                    }
                }
            }
            (TyKind::Closure(_, ce), TyKind::FnPtr(..)) => {
                // closure type was checked against the fn pointer's signature already
                if let Some(e) = e {
                    self.b.coerce.insert(e.0, Coerce::ClosureFn);
                }
                let _ = ce;
                return true;
            }
            (TyKind::Ref(m1, ia), TyKind::Ref(m2, ix)) => {
                let ia = self.shallow(*ia);
                let ix = self.shallow(*ix);
                if let (TyKind::Array(ea, _), TyKind::Slice(es)) = (self.tcx.tys.kind(ia).clone(), self.tcx.tys.kind(ix).clone()) {
                    if self.unify(ea, es) && (*m1 || !*m2) {
                        if let Some(e) = e {
                            self.b.coerce.insert(e.0, Coerce::Unsize);
                        }
                        return true;
                    }
                }
                if *m1 && !*m2 && self.unify(ia, ix) {
                    if let Some(e) = e {
                        self.b.coerce.insert(e.0, Coerce::MutToConst);
                    }
                    return true;
                }
            }
            (TyKind::Ptr(true, ia), TyKind::Ptr(false, ix)) => {
                let (ia, ix) = (*ia, *ix);
                if self.unify(ia, ix) {
                    if let Some(e) = e {
                        self.b.coerce.insert(e.0, Coerce::MutToConst);
                    }
                    return true;
                }
            }
            _ => {}
        }
        if self.unify(a, x) {
            return true;
        }
        let sa = self.ty_str(a);
        let sx = self.ty_str(x);
        self.err(pos, format!("mismatched types: expected `{}`, found `{}`", sx, sa));
        false
    }

    // ------------------------------------------------------------ writeback

    fn finish(&mut self) {
        for i in 0..self.b.expr_ty.len() {
            let t = self.b.expr_ty[i];
            self.b.expr_ty[i] = self.norm(t);
        }
        for i in 0..self.b.pat_ty.len() {
            let t = self.b.pat_ty[i];
            self.b.pat_ty[i] = self.norm(t);
        }
        for i in 0..self.b.locals.len() {
            let t = self.b.locals[i].ty;
            self.b.locals[i].ty = self.norm(t);
        }
        let r = self.b.ret;
        self.b.ret = self.norm(r);
        let keys: Vec<u32> = self.b.res_args.keys().copied().collect();
        for k in keys {
            let v = self.b.res_args[&k].clone();
            let mut n = Vec::new();
            for t in v {
                n.push(self.deep(t));
            }
            self.b.res_args.insert(k, n);
        }
        let keys: Vec<u32> = self.b.methods.keys().copied().collect();
        for k in keys {
            let v = self.b.methods[&k].args.clone();
            let mut n = Vec::new();
            for t in v {
                n.push(self.norm(t));
            }
            self.b.methods.get_mut(&k).unwrap().args = n;
        }
        let keys: Vec<u32> = self.b.binops.keys().copied().collect();
        for k in keys {
            let v = self.b.binops[&k].args.clone();
            let mut n = Vec::new();
            for t in v {
                n.push(self.norm(t));
            }
            self.b.binops.get_mut(&k).unwrap().args = n;
        }
    }

    // ------------------------------------------------------------ scopes

    fn lookup_local(&self, s: Sym) -> Option<u32> {
        let mut i = self.scopes.len();
        while i > 0 {
            i -= 1;
            if self.scopes[i].0 == s {
                return Some(self.scopes[i].1);
            }
        }
        None
    }

    fn set_ty(&mut self, e: ExprId, t: TyId) {
        let i = (e.0 - self.b.expr_lo) as usize;
        if i < self.b.expr_ty.len() {
            self.b.expr_ty[i] = t;
        }
    }

    // ------------------------------------------------------------ patterns

    pub fn check_pat(&mut self, p: PatId, expected: TyId) {
        let ast = self.ast();
        let pi = (p.0 - self.b.pat_lo) as usize;
        if pi < self.b.pat_ty.len() {
            self.b.pat_ty[pi] = expected;
        }
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { by_ref, mutbl, name, sub } => {
                let s = self.sym(*name);
                // a unit struct / unit variant / const in scope is a path pattern, not a binding
                if sub.is_none() && !*by_ref && !*mutbl {
                    if let Some(d) = self.prog.lookup_in_scope(self.module, s, false) {
                        let k = self.prog.def(d).kind;
                        if matches!(k, DefKind::Const | DefKind::Variant | DefKind::Struct | DefKind::AssocConst) {
                            self.b.pat_res.insert(p.0, Res::Def(d));
                            let t = self.value_def_ty(d, None, p.0, 0);
                            self.coerce_or_unify(t, expected, None, name.lo);
                            return;
                        }
                    }
                }
                let ty = if *by_ref {
                    self.tcx.tys.intern(TyKind::Ref(*mutbl, expected))
                } else {
                    expected
                };
                let li = self.b.locals.len() as u32;
                self.b.locals.push(Local { name: s, ty, mutable: *mutbl && !*by_ref, pat: p.0 });
                self.b.pat_local.insert(p.0, li);
                self.scopes.push((s, li));
                if let Some(sp) = sub {
                    self.check_pat(*sp, expected);
                }
            }
            Pat::Lit(e) => {
                let t = self.check_expr(*e, Some(expected));
                // string literal patterns against &str
                self.coerce_or_unify(t, expected, None, ast.expr(*e).lo);
            }
            Pat::Range(a, b, _) => {
                if let Some(a) = a {
                    let t = self.check_expr(*a, Some(expected));
                    self.unify(t, expected);
                }
                if let Some(b) = b {
                    let t = self.check_expr(*b, Some(expected));
                    self.unify(t, expected);
                }
            }
            Pat::Tuple(v) => {
                let et = self.shallow(expected);
                let elems = match self.tcx.tys.kind(et).clone() {
                    TyKind::Tuple(x) => x,
                    _ => {
                        let mut x = Vec::new();
                        for _ in 0..v.len() {
                            x.push(self.fresh(0));
                        }
                        let tt = self.tcx.tys.intern(TyKind::Tuple(x.clone()));
                        self.unify(tt, et);
                        x
                    }
                };
                self.check_seq_pats(v, &elems);
            }
            Pat::Paren(x) => self.check_pat(*x, expected),
            Pat::Ref(_, x) => {
                let et = self.shallow(expected);
                let inner = match self.tcx.tys.kind(et).clone() {
                    TyKind::Ref(_, i) => i,
                    _ => self.tcx.tys.error,
                };
                self.check_pat(*x, inner);
            }
            Pat::Or(v) => {
                for x in v {
                    self.check_pat(*x, expected);
                }
            }
            Pat::Slice(v) => {
                let et = self.shallow(expected);
                let elem = match self.tcx.tys.kind(et).clone() {
                    TyKind::Array(e, _) | TyKind::Slice(e) => e,
                    _ => self.tcx.tys.error,
                };
                for x in v {
                    if let Pat::Rest = ast.pat(*x) {
                        continue;
                    }
                    self.check_pat(*x, elem);
                }
            }
            Pat::Path(path) => {
                let (r, t) = self.resolve_value_path(path, p.0, true);
                self.b.pat_res.insert(p.0, r);
                self.coerce_or_unify(t, expected, None, path.lo);
            }
            Pat::TupleStruct(path, v) => {
                let (r, _) = self.resolve_value_path(path, p.0, true);
                self.b.pat_res.insert(p.0, r);
                let fields = self.ctor_fields(r, expected, path.lo);
                self.check_seq_pats(v, &fields);
            }
            Pat::Struct(path, fields, _) => {
                let d = self.resolve_struct_path(path);
                self.b.pat_res.insert(p.0, d);
                if let Res::Def(dd) = d {
                    let (adt_ty, vi) = self.adt_of_ctor(dd);
                    let adt_ty = self.instantiate_adt(adt_ty);
                    self.coerce_or_unify(adt_ty, expected, None, path.lo);
                    let et = self.shallow(expected);
                    for fp in fields {
                        let fname = self.sym(fp.name);
                        let ft = self.field_ty(et, vi, fname);
                        match ft {
                            Some((_, t)) => self.check_pat(fp.pat, t),
                            None => self.err(fp.name.lo, format!("no field `{}`", self.text(fp.name))),
                        }
                    }
                }
            }
            Pat::Box(x) => self.check_pat(*x, expected),
            Pat::Mac(_) => self.err(0, "macro in pattern position".to_string()),
        }
    }

    fn check_seq_pats(&mut self, v: &[PatId], elems: &[TyId]) {
        let ast = self.ast();
        let mut rest_at = None;
        for (i, x) in v.iter().enumerate() {
            if let Pat::Rest = ast.pat(*x) {
                rest_at = Some(i);
            }
        }
        match rest_at {
            None => {
                for i in 0..v.len() {
                    let t = if i < elems.len() { elems[i] } else { self.tcx.tys.error };
                    self.check_pat(v[i], t);
                }
            }
            Some(r) => {
                for i in 0..r {
                    let t = if i < elems.len() { elems[i] } else { self.tcx.tys.error };
                    self.check_pat(v[i], t);
                }
                let after = v.len() - r - 1;
                for j in 0..after {
                    let ei = elems.len() as isize - after as isize + j as isize;
                    let t = if ei >= 0 { elems[ei as usize] } else { self.tcx.tys.error };
                    self.check_pat(v[r + 1 + j], t);
                }
            }
        }
    }

    // ------------------------------------------------------------ ADT helpers

    /// For a struct/variant def: (adt type with Param args, variant index).
    fn adt_of_ctor(&mut self, d: DefId) -> (TyId, u32) {
        let def = self.prog.def(d);
        let (adt_def, vi) = match def.kind {
            DefKind::Variant => (def.parent, def.sub),
            _ => (d, 0),
        };
        let n = match self.tcx.adts.get(&adt_def) {
            Some(a) => a.n_generics,
            None => 0,
        };
        let mut args = Vec::new();
        for i in 0..n {
            args.push(self.tcx.tys.intern(TyKind::Param(i)));
        }
        (self.tcx.tys.intern(TyKind::Adt(adt_def, args)), vi)
    }

    /// Replace Param args of an ADT type by fresh variables.
    fn instantiate_adt(&mut self, t: TyId) -> TyId {
        if let TyKind::Adt(d, args) = self.tcx.tys.kind(t).clone() {
            if args.is_empty() {
                return t;
            }
            let mut fresh = Vec::new();
            for _ in 0..args.len() {
                fresh.push(self.fresh(0));
            }
            return self.tcx.tys.intern(TyKind::Adt(d, fresh));
        }
        t
    }

    fn field_ty(&mut self, adt_ty: TyId, vi: u32, name: Sym) -> Option<(u32, TyId)> {
        let t = self.shallow(adt_ty);
        if let TyKind::Adt(d, args) = self.tcx.tys.kind(t).clone() {
            let adt = self.tcx.adts.get(&d)?.clone();
            let v = adt.variants.get(vi as usize)?;
            for (i, f) in v.fields.iter().enumerate() {
                if f.name == name {
                    let ft = self.tcx.tys.subst(f.ty, &args);
                    return Some((i as u32, ft));
                }
            }
        }
        None
    }

    /// Field types of a tuple-struct / tuple-variant pattern or constructor.
    fn ctor_fields(&mut self, r: Res, expected: TyId, pos: u32) -> Vec<TyId> {
        if let Res::Def(d) = r {
            let (adt_ty, vi) = self.adt_of_ctor(d);
            let adt_ty = self.instantiate_adt(adt_ty);
            self.coerce_or_unify(adt_ty, expected, None, pos);
            let t = self.shallow(adt_ty);
            if let TyKind::Adt(ad, args) = self.tcx.tys.kind(t).clone() {
                if let Some(adt) = self.tcx.adts.get(&ad).cloned() {
                    if let Some(v) = adt.variants.get(vi as usize) {
                        let mut out = Vec::new();
                        for f in &v.fields {
                            out.push(self.tcx.tys.subst(f.ty, &args));
                        }
                        return out;
                    }
                }
            }
        }
        Vec::new()
    }

    // ------------------------------------------------------------ paths

    fn resolve_struct_path(&mut self, path: &Path) -> Res {
        let prog = self.prog;
        let segs = crate::tcx::path_segs(prog, self.file, path);
        if segs.len() == 1 && segs[0].0 == SegKind::SelfType {
            if let Some(st) = self.env.self_ty {
                let st = self.tcx.tys.subst(st, &self.inst_args.clone());
                if let TyKind::Adt(d, _) = self.tcx.tys.kind(st) {
                    return Res::Def(*d);
                }
            }
            return Res::Err;
        }
        if segs.len() == 1 {
            if let Some(d) = prog.lookup_in_scope(self.module, segs[0].1, true) {
                if matches!(prog.def(d).kind, DefKind::Struct | DefKind::Union) {
                    return Res::Def(d);
                }
                if prog.def(d).kind == DefKind::TypeAlias {
                    let t = self.tcx.alias_ty(prog, d);
                    if let TyKind::Adt(ad, _) = self.tcx.tys.kind(t) {
                        return Res::Def(*ad);
                    }
                }
            }
            if let Some(d) = prog.lookup_in_scope(self.module, segs[0].1, false) {
                if prog.def(d).kind == DefKind::Variant {
                    return Res::Def(d);
                }
            }
            self.err(path.lo, format!("unresolved struct `{}`", crate::tcx::path_str(prog, self.file, path)));
            return Res::Err;
        }
        // Enum::Variant / Self::Variant / module::Struct
        let (r, _) = self.resolve_value_path(path, u32::MAX, false);
        if let Res::Def(_) = r {
            return r;
        }
        if let Some(d) = self.tcx.resolve_type_path_def(prog, self.file, self.module, path) {
            return Res::Def(d);
        }
        Res::Err
    }

    /// Type of a value def used as an expression (fn items get fresh generic args).
    fn value_def_ty(&mut self, d: DefId, explicit: Option<Vec<TyId>>, key: u32, impl_args_from: u8) -> TyId {
        let _ = impl_args_from;
        let def = self.prog.def(d);
        match def.kind {
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                let n = self.tcx.sigs.get(&d).map_or(0, |s| s.n_generics);
                let mut args = Vec::new();
                match explicit {
                    Some(v) => {
                        for i in 0..n as usize {
                            if i < v.len() {
                                args.push(v[i]);
                            } else {
                                args.push(self.fresh(0));
                            }
                        }
                    }
                    None => {
                        for _ in 0..n {
                            args.push(self.fresh(0));
                        }
                    }
                }
                self.b.res_args.insert(key, args.clone());
                self.tcx.tys.intern(TyKind::FnDef(d, args))
            }
            DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic => {
                let t = self.tcx.const_tys.get(&d).copied().unwrap_or(self.tcx.tys.error);
                match explicit {
                    Some(v) => self.tcx.tys.subst(t, &v),
                    None => t,
                }
            }
            DefKind::Variant | DefKind::Struct => {
                let (adt_ty, vi) = self.adt_of_ctor(d);
                let adt_def = match self.tcx.tys.kind(adt_ty) {
                    TyKind::Adt(a, _) => *a,
                    _ => NO_DEF,
                };
                let adt = match self.tcx.adts.get(&adt_def) {
                    Some(a) => a.clone(),
                    None => return self.tcx.tys.error,
                };
                let mut args = Vec::new();
                match explicit {
                    Some(v) => args = v,
                    None => {
                        for _ in 0..adt.n_generics {
                            args.push(self.fresh(0));
                        }
                    }
                }
                self.b.res_args.insert(key, args.clone());
                let inst = self.tcx.tys.intern(TyKind::Adt(adt_def, args.clone()));
                let v = &adt.variants[vi as usize];
                if v.shape == 1 {
                    // tuple ctor: a function from fields to the ADT
                    let mut ps = Vec::new();
                    for f in &v.fields {
                        ps.push(self.tcx.tys.subst(f.ty, &args));
                    }
                    self.tcx.tys.intern(TyKind::FnPtr(ps, inst))
                } else {
                    inst
                }
            }
            _ => self.tcx.tys.error,
        }
    }

    /// Resolves a value path. `key`: expr or pat id for recording generic args.
    fn resolve_value_path(&mut self, path: &Path, key: u32, is_pat: bool) -> (Res, TyId) {
        let prog = self.prog;
        let segs = crate::tcx::path_segs(prog, self.file, path);
        let error = self.tcx.tys.error;
        if let Some(q) = &path.qself {
            // `<T as Trait>::item`
            let self_t = self.lower_ty(q.ty);
            let tp = match &q.trait_path {
                Some(tp) => tp,
                None => {
                    self.err(path.lo, "`<T>::item` without a trait is not supported".to_string());
                    return (Res::Err, error);
                }
            };
            let td = match self.tcx.resolve_type_path_def(prog, self.file, self.module, tp) {
                Some(d) => d,
                None => return (Res::Err, error),
            };
            let name = segs[segs.len() - 1].1;
            let mut item = None;
            for &it in &prog.traits[prog.def(td).sub as usize] {
                if prog.def(it).name == name {
                    item = Some(it);
                }
            }
            let item = match item {
                Some(i) => i,
                None => {
                    self.err(path.lo, "no such trait item".to_string());
                    return (Res::Err, error);
                }
            };
            let env = self.env.clone();
            let explicit = self.tcx.lower_generic_args(prog, self.file, self.module, &env, tp);
            let inst = self.inst_args.clone();
            let mut explicit2 = Vec::new();
            for e in explicit {
                explicit2.push(self.tcx.tys.subst(e, &inst));
            }
            let targs = self.tcx.fill_trait_defaults(prog, td, self_t, explicit2);
            let mut args = vec![self_t];
            args.extend(targs);
            let own = self.tcx.sigs.get(&item).map_or(0, |s| s.n_generics - s.n_parent_generics);
            for _ in 0..own {
                args.push(self.fresh(0));
            }
            let t = self.value_def_ty(item, Some(args), key, 1);
            return (Res::Def(item), t);
        }
        let last_args = self.explicit_args(path);
        if segs.len() == 1 && !path.global {
            let (k, s) = segs[0];
            if k == SegKind::SelfValue || k == SegKind::Ident {
                if !is_pat {
                    if let Some(li) = self.lookup_local(s) {
                        self.note_capture(li);
                        let t = self.b.locals[li as usize].ty;
                        return (Res::Local(li), t);
                    }
                }
            }
            if k == SegKind::SelfType {
                // `Self` as a unit/tuple struct ctor
                if let Some(st) = self.env.self_ty {
                    let st = self.tcx.tys.subst(st, &self.inst_args.clone());
                    if let TyKind::Adt(d, args) = self.tcx.tys.kind(st).clone() {
                        let t = self.value_def_ty(d, Some(args), key, 0);
                        return (Res::Def(d), t);
                    }
                }
            }
            if let Some(d) = prog.lookup_in_scope(self.module, s, false) {
                let t = self.value_def_ty(d, last_args, key, 0);
                return (Res::Def(d), t);
            }
            self.err(path.lo, format!("cannot find value `{}`", crate::tcx::path_str(prog, self.file, path)));
            return (Res::Err, error);
        }
        let n = segs.len();
        // container path (module / enum / trait)
        let parent = if segs[0].0 == SegKind::SelfType && n == 2 {
            None
        } else {
            prog.resolve_mod_path(self.module, path.global, &segs[..n - 1], false, false)
        };
        if let Some(pd) = parent {
            let pk = prog.def(pd).kind;
            if matches!(pk, DefKind::Mod | DefKind::Enum | DefKind::Trait) {
                if let Some(d) = prog.lookup_in_container(pd, segs[n - 1].1, false) {
                    // enum variant: generic args may sit on the enum segment
                    let explicit = if pk == DefKind::Enum { self.seg_args(path, n - 2).or(last_args) } else { last_args };
                    let t = self.value_def_ty(d, explicit, key, 0);
                    return (Res::Def(d), t);
                }
                if pk != DefKind::Enum && pk != DefKind::Trait {
                    self.err(path.lo, format!("cannot find value `{}`", crate::tcx::path_str(prog, self.file, path)));
                    return (Res::Err, error);
                }
            }
        }
        // type-relative: <Type>::item
        let self_ty = match self.type_of_prefix(path, n - 1) {
            Some(t) => t,
            None => {
                self.err(path.lo, format!("cannot resolve `{}`", crate::tcx::path_str(prog, self.file, path)));
                return (Res::Err, error);
            }
        };
        let name = segs[n - 1].1;
        // enum variant through a type alias / Self
        if let TyKind::Adt(ad, aargs) = self.tcx.tys.kind(self_ty).clone() {
            if prog.def(ad).kind == DefKind::Enum {
                if let Some(v) = prog.lookup_in_container(ad, name, false) {
                    let t = self.value_def_ty(v, Some(aargs), key, 0);
                    return (Res::Def(v), t);
                }
            }
        }
        match self.tcx.inherent_item(prog, self_ty, name) {
            Some((item, imp)) => {
                // impl generic args from matching the impl self type against self_ty
                let impl_args = self.match_impl_args(imp, self_ty);
                let own = self.tcx.sigs.get(&item).map_or(0, |s| s.n_generics - s.n_parent_generics);
                let mut args = impl_args;
                let la = last_args.unwrap_or_default();
                for i in 0..own as usize {
                    if i < la.len() {
                        args.push(la[i]);
                    } else {
                        args.push(self.fresh(0));
                    }
                }
                let t = self.value_def_ty(item, Some(args), key, 1);
                (Res::Def(item), t)
            }
            None => {
                // a trait method implemented for this type
                let cands = self.tcx.trait_methods_for(prog, self_ty, name);
                if let Some(&(item, imp)) = cands.first() {
                    let la = last_args.unwrap_or_default();
                    let args = self.trait_call_args(item, imp, self_ty, &la);
                    let t = self.value_def_ty(item, Some(args), key, 1);
                    return (Res::Def(item), t);
                }
                let ts = self.ty_str(self_ty);
                self.err(path.lo, format!("no associated item `{}` on `{}`", prog.syms.str(name), ts));
                (Res::Err, error)
            }
        }
    }

    /// Generic args [Self, trait args.., own..] to call trait method `item` (found through
    /// impl `imp`) on receiver type `t`.
    fn trait_call_args(&mut self, item: DefId, imp: DefId, t: TyId, explicit_own: &[TyId]) -> Vec<TyId> {
        let impl_args = self.match_impl_args(imp, t);
        let targs = self.tcx.impl_trait_args.get(&imp).cloned().unwrap_or_default();
        let mut args = vec![t];
        for ta in targs {
            let x = self.tcx.tys.subst(ta, &impl_args);
            args.push(x);
        }
        let own = self.tcx.sigs.get(&item).map_or(0, |s| s.n_generics - s.n_parent_generics);
        for i in 0..own as usize {
            if i < explicit_own.len() {
                args.push(explicit_own[i]);
            } else {
                args.push(self.fresh(0));
            }
        }
        args
    }

    /// One autoderef step from `t`: built-in for references and raw pointers, through
    /// `Deref` for other types. Records overloaded steps under (expr, step).
    fn deref_step(&mut self, e: u32, step: u8, t: TyId) -> Option<TyId> {
        let t = self.shallow(t);
        match self.tcx.tys.kind(t).clone() {
            TyKind::Ref(_, i) | TyKind::Ptr(_, i) => Some(self.shallow(i)),
            TyKind::Adt(..) => {
                let td = self.lang_item(&["ops", "Deref"])?;
                let dt = self.deep(t);
                self.tcx.find_impl(td, dt, &[])?;
                let target = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Target")?)?;
                let proj = self.tcx.tys.intern(TyKind::Assoc(target, vec![dt]));
                let r = self.tcx.normalize(self.prog, proj);
                let m = self.trait_fn(td, "deref")?;
                self.b.ov_derefs.insert((e, step), (m, vec![dt]));
                Some(r)
            }
            _ => None,
        }
    }

    fn trait_fn(&self, td: DefId, name: &str) -> Option<DefId> {
        let s = self.prog.syms.get(name)?;
        for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
            if self.prog.def(it).name == s {
                return Some(it);
            }
        }
        None
    }

    /// Normalizes associated-type projections whose Self is known.
    fn norm(&mut self, t: TyId) -> TyId {
        let d = self.deep(t);
        self.tcx.normalize(self.prog, d)
    }

    /// Generic args of impl `imp` such that its self type equals `t`.
    fn match_impl_args(&mut self, imp: DefId, t: TyId) -> Vec<TyId> {
        let n = self.tcx.impl_generics.get(&imp).map_or(0, |e| e.names.len());
        let mut args = Vec::new();
        for _ in 0..n {
            args.push(self.fresh(0));
        }
        if let Some(&st) = self.tcx.impl_self.get(&imp) {
            let inst = self.tcx.tys.subst(st, &args);
            self.unify(inst, t);
        }
        args
    }

    fn explicit_args(&mut self, path: &Path) -> Option<Vec<TyId>> {
        let n = path.segs.len();
        self.seg_args(path, n - 1)
    }

    fn seg_args(&mut self, path: &Path, i: usize) -> Option<Vec<TyId>> {
        let seg = path.segs.get(i)?;
        let a = seg.args.as_ref()?;
        let mut out = Vec::new();
        if let GenericArgs::Angle(v) = &**a {
            for g in v {
                if let GenericArg::Type(t) = g {
                    let lt = self.lower_ty(*t);
                    out.push(lt);
                }
            }
        }
        Some(out)
    }

    /// Lowers a syntactic type inside the body (generic params map to the instance args).
    pub fn lower_ty(&mut self, t: crate::ast::TyId) -> TyId {
        let prog = self.prog;
        let env = self.env.clone();
        if let Ty::Infer = prog.files[self.file as usize].ast.ty(t) {
            return self.fresh(0);
        }
        let lt = self.tcx.lower_ty(prog, self.file, self.module, &env, t);
        let args = self.inst_args.clone();
        let lt = self.tcx.tys.subst(lt, &args);
        self.replace_errors_with_vars(lt, t)
    }

    /// `Vec<_>`: the `_` lowered to Error becomes a fresh variable.
    fn replace_errors_with_vars(&mut self, t: TyId, _ast: crate::ast::TyId) -> TyId {
        let k = self.tcx.tys.kind(t).clone();
        match k {
            TyKind::Error => self.fresh(0),
            TyKind::Adt(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.replace_errors_with_vars(x, _ast));
                }
                self.tcx.tys.intern(TyKind::Adt(d, n))
            }
            TyKind::Ref(m, x) => {
                let x = self.replace_errors_with_vars(x, _ast);
                self.tcx.tys.intern(TyKind::Ref(m, x))
            }
            TyKind::Tuple(v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.replace_errors_with_vars(x, _ast));
                }
                self.tcx.tys.intern(TyKind::Tuple(n))
            }
            TyKind::Array(x, l) => {
                let x = self.replace_errors_with_vars(x, _ast);
                self.tcx.tys.intern(TyKind::Array(x, l))
            }
            TyKind::Slice(x) => {
                let x = self.replace_errors_with_vars(x, _ast);
                self.tcx.tys.intern(TyKind::Slice(x))
            }
            _ => t,
        }
    }

    /// Type named by the first `n` segments of a path (for `Type::item`).
    fn type_of_prefix(&mut self, path: &Path, n: usize) -> Option<TyId> {
        let prog = self.prog;
        if n == 1 && path.segs[0].kind == SegKind::SelfType {
            let st = self.env.self_ty?;
            let args = self.inst_args.clone();
            return Some(self.tcx.tys.subst(st, &args));
        }
        // generic param `T::item`
        if n == 1 {
            let s = self.sym(path.segs[0].name);
            for (i, nm) in self.env.names.iter().enumerate() {
                if *nm == s && i < self.inst_args.len() {
                    return Some(self.inst_args[i]);
                }
            }
        }
        let sub = Path { global: path.global, qself: None, segs: path.segs[..n].to_vec(), lo: path.lo };
        let d = self.tcx.resolve_type_path_def(prog, self.file, self.module, &sub)?;
        let args = self.seg_args(path, n - 1);
        match prog.def(d).kind {
            DefKind::Prim(p) => Some(self.tcx.tys.prim(p)),
            DefKind::Struct | DefKind::Enum | DefKind::Union => {
                let ng = self.tcx.adts.get(&d).map_or(0, |a| a.n_generics);
                let mut a = args.unwrap_or_default();
                while a.len() < ng as usize {
                    a.push(self.fresh(0));
                }
                Some(self.tcx.tys.intern(TyKind::Adt(d, a)))
            }
            DefKind::TypeAlias => {
                let t = self.tcx.alias_ty(prog, d);
                let a = args.unwrap_or_default();
                Some(self.tcx.tys.subst(t, &a))
            }
            _ => None,
        }
    }

    fn note_capture(&mut self, li: u32) {
        // a local declared outside the innermost closure is a capture
        if let Some(&(ce, depth)) = self.closure_depth.last() {
            let mut declared_inside = false;
            for i in depth..self.scopes.len() {
                if self.scopes[i].1 == li {
                    declared_inside = true;
                }
            }
            if !declared_inside {
                self.b.closure_locals.entry(ce).or_default().push(li);
            }
        }
    }

    // ------------------------------------------------------------ blocks

    fn check_block(&mut self, b: BlockId, expected: Option<TyId>) -> TyId {
        let ast = self.ast();
        let blk = ast.block(b);
        let saved_scope = self.scopes.len();
        let saved_module = self.module;
        if let Some(&m) = self.prog.block_mods.get(&(self.file, b.0)) {
            self.module = m;
        }
        let mut result = self.tcx.tys.unit;
        let n = blk.stmts.len();
        let mut diverged = false;
        for (i, s) in blk.stmts.iter().enumerate() {
            match s {
                Stmt::Let { attrs, pat, ty, init, else_ } => {
                    if !self.active(attrs) {
                        continue;
                    }
                    let decl = match ty {
                        Some(t) => Some(self.lower_ty(*t)),
                        None => None,
                    };
                    let t = match init {
                        Some(e) => {
                            let it = self.check_expr(*e, decl);
                            match decl {
                                Some(d) => {
                                    self.coerce_or_unify(it, d, Some(*e), ast.expr(*e).lo);
                                    d
                                }
                                None => it,
                            }
                        }
                        None => match decl {
                            Some(d) => d,
                            None => self.fresh(0),
                        },
                    };
                    if self.diverges {
                        diverged = true;
                    }
                    if let Some(eb) = else_ {
                        let save = self.diverges;
                        self.check_block(*eb, None);
                        self.diverges = save;
                    }
                    self.check_pat(*pat, t);
                }
                Stmt::Item(_) => {}
                Stmt::Expr(e, semi) => {
                    let is_tail = i == n - 1 && !*semi;
                    let t = self.check_expr(*e, if is_tail { expected } else { None });
                    if self.diverges {
                        diverged = true;
                    }
                    if is_tail {
                        result = t;
                    } else if !*semi {
                        // block-like statement without `;` must be unit (or diverge)
                        let st = self.shallow(t);
                        if st != self.tcx.tys.unit && st != self.tcx.tys.never {
                            let unit = self.tcx.tys.unit;
                            self.unify(st, unit);
                        }
                    }
                }
                Stmt::Empty => {}
            }
        }
        self.scopes.truncate(saved_scope);
        self.module = saved_module;
        if diverged {
            self.diverges = true;
            let tail_is_expr = n > 0 && matches!(blk.stmts[n - 1], Stmt::Expr(_, false));
            if !tail_is_expr {
                return self.tcx.tys.never;
            }
        }
        result
    }

    fn active(&self, attrs: &[Attr]) -> bool {
        if attrs.is_empty() {
            return true;
        }
        let krate = self.prog.file_crate[self.file as usize];
        crate::cfg::active(self.prog.src(self.file), attrs, &self.prog.crates[krate as usize].cfg)
    }

    // ------------------------------------------------------------ expressions

    pub fn check_expr(&mut self, e: ExprId, expected: Option<TyId>) -> TyId {
        let save_div = self.diverges;
        self.diverges = false;
        let t = self.check_expr_inner(e, expected);
        let ts = self.shallow(t);
        if ts == self.tcx.tys.never {
            self.diverges = true;
        }
        self.diverges = self.diverges || save_div;
        self.set_ty(e, t);
        t
    }

    fn check_expr_inner(&mut self, e: ExprId, expected: Option<TyId>) -> TyId {
        let ast = self.ast();
        let ex = ast.expr(e);
        let lo = ex.lo;
        let unit = self.tcx.tys.unit;
        let error = self.tcx.tys.error;
        match &ex.kind {
            ExprKind::Lit(k) => self.check_lit(e, *k, expected),
            ExprKind::Path(path) => {
                let (r, t) = self.resolve_value_path(path, e.0, false);
                self.b.res.insert(e.0, r);
                t
            }
            ExprKind::Paren(x) => self.check_expr(*x, expected),
            ExprKind::Unary(op, x) => match op {
                UnOp::Neg => {
                    let t = self.check_expr(*x, expected);
                    let st = self.shallow(t);
                    if !self.is_numeric(st) {
                        let s = self.ty_str(st);
                        self.err(lo, format!("cannot negate `{}`", s));
                    }
                    t
                }
                UnOp::Not => {
                    let t = self.check_expr(*x, expected);
                    t
                }
                UnOp::Deref => {
                    let t = self.check_expr(*x, None);
                    match self.deref_step(e.0, 0, t) {
                        Some(i) => i,
                        None => {
                            let s = self.ty_str(t);
                            self.err(lo, format!("cannot deref `{}`", s));
                            error
                        }
                    }
                }
            },
            ExprKind::Binary(op, a, b) => self.check_binary(e, *op, *a, *b, expected, lo),
            ExprKind::Assign(a, b) => {
                let ta = self.check_expr(*a, None);
                let tb = self.check_expr(*b, Some(ta));
                self.coerce_or_unify(tb, ta, Some(*b), lo);
                unit
            }
            ExprKind::AssignOp(op, a, b) => {
                let ta = self.check_expr(*a, None);
                let is_shift = matches!(op, BinOp::Shl | BinOp::Shr);
                let tb = self.check_expr(*b, if is_shift { None } else { Some(ta) });
                if !is_shift {
                    self.unify(ta, tb);
                }
                unit
            }
            ExprKind::Cast(x, t) => {
                let target = self.lower_ty(*t);
                let st = self.shallow(target);
                let _ = st;
                self.check_expr(*x, None);
                target
            }
            ExprKind::Block(b, label) => {
                if label.is_some() {
                    let l = label.map(|x| self.sym_lifetime(x));
                    self.loops.push(Loop { label: l, break_ty: expected, is_loop: true });
                    let t = self.check_block(*b, expected);
                    let lp = self.loops.pop().unwrap();
                    if let Some(bt) = lp.break_ty {
                        self.unify(bt, t);
                    }
                    self.diverges = false;
                    return t;
                }
                self.check_block(*b, expected)
            }
            ExprKind::Unsafe(b) | ExprKind::ConstBlock(b) => self.check_block(*b, expected),
            ExprKind::If(c, then, els) => {
                let bool_ = self.tcx.tys.bool_;
                let ct = self.check_expr(*c, Some(bool_));
                self.coerce_or_unify(ct, bool_, None, lo);
                let saved = self.scopes.len();
                let tt = self.check_block(*then, expected);
                let then_div = self.diverges;
                self.scopes.truncate(saved);
                self.diverges = false;
                match els {
                    None => {
                        self.diverges = false;
                        if !then_div {
                            let s = self.shallow(tt);
                            if s != unit && s != self.tcx.tys.never {
                                self.unify(s, unit);
                            }
                        }
                        unit
                    }
                    Some(x) => {
                        let et = self.check_expr(*x, expected.or(Some(tt)));
                        let else_div = self.diverges;
                        self.diverges = then_div && else_div;
                        let ts = self.shallow(tt);
                        let es = self.shallow(et);
                        if ts == self.tcx.tys.never {
                            return et;
                        }
                        if es == self.tcx.tys.never {
                            return tt;
                        }
                        self.coerce_or_unify(et, tt, Some(*x), lo);
                        tt
                    }
                }
            }
            ExprKind::Let(p, x) => {
                let t = self.check_expr(*x, None);
                self.check_pat(*p, t);
                self.tcx.tys.bool_
            }
            ExprKind::Match(x, arms) => {
                let st = self.check_expr(*x, None);
                let mut result: Option<TyId> = None;
                let mut all_div = !arms.is_empty();
                for arm in arms {
                    if !self.active(&arm.attrs) {
                        continue;
                    }
                    let saved = self.scopes.len();
                    self.check_pat(arm.pat, st);
                    if let Some(g) = arm.guard {
                        let bool_ = self.tcx.tys.bool_;
                        self.check_expr(g, Some(bool_));
                    }
                    self.diverges = false;
                    let bt = self.check_expr(arm.body, expected.or(result));
                    let div = self.diverges;
                    all_div = all_div && div;
                    let bs = self.shallow(bt);
                    if bs != self.tcx.tys.never {
                        match result {
                            None => result = Some(bt),
                            Some(r) => {
                                self.coerce_or_unify(bt, r, Some(arm.body), ast.expr(arm.body).lo);
                            }
                        }
                    }
                    self.scopes.truncate(saved);
                }
                self.diverges = all_div;
                match result {
                    Some(r) => r,
                    None => self.tcx.tys.never,
                }
            }
            ExprKind::While(c, b, label) => {
                let bool_ = self.tcx.tys.bool_;
                let saved = self.scopes.len();
                self.check_expr(*c, Some(bool_));
                let l = label.map(|x| self.sym_lifetime(x));
                self.loops.push(Loop { label: l, break_ty: None, is_loop: false });
                self.check_block(*b, Some(unit));
                self.loops.pop();
                self.scopes.truncate(saved);
                self.diverges = false;
                unit
            }
            ExprKind::Loop(b, label) => {
                let l = label.map(|x| self.sym_lifetime(x));
                self.loops.push(Loop { label: l, break_ty: None, is_loop: true });
                self.check_block(*b, Some(unit));
                let lp = self.loops.pop().unwrap();
                match lp.break_ty {
                    Some(t) => {
                        self.diverges = false;
                        t
                    }
                    None => {
                        // a loop without `break` never yields
                        if self.loop_has_break(*b, label.map(|x| self.sym_lifetime(x))) {
                            self.diverges = false;
                            unit
                        } else {
                            self.diverges = true;
                            self.tcx.tys.never
                        }
                    }
                }
            }
            ExprKind::For(p, it, b, label) => {
                let saved = self.scopes.len();
                let item = self.check_for_iter(*it, lo);
                self.check_pat(*p, item);
                let l = label.map(|x| self.sym_lifetime(x));
                self.loops.push(Loop { label: l, break_ty: None, is_loop: false });
                self.check_block(*b, Some(unit));
                self.loops.pop();
                self.scopes.truncate(saved);
                self.diverges = false;
                unit
            }
            ExprKind::Break(label, x) => {
                let l = label.map(|x| self.sym_lifetime(x));
                let t = match x {
                    Some(v) => {
                        let target = self.loop_index(l);
                        let exp = match target {
                            Some(i) => self.loops[i].break_ty,
                            None => None,
                        };
                        let vt = self.check_expr(*v, exp);
                        Some(vt)
                    }
                    None => None,
                };
                if let Some(i) = self.loop_index(l) {
                    let bt = match t {
                        Some(v) => v,
                        None => unit,
                    };
                    if self.loops[i].is_loop {
                        match self.loops[i].break_ty {
                            Some(prev) => {
                                self.unify(prev, bt);
                            }
                            None => self.loops[i].break_ty = Some(bt),
                        }
                    }
                } else {
                    self.err(lo, "`break` outside of a loop".to_string());
                }
                self.tcx.tys.never
            }
            ExprKind::Continue(_) => self.tcx.tys.never,
            ExprKind::Return(x) => {
                let rt = *self.ret_stack.last().unwrap();
                match x {
                    Some(v) => {
                        let t = self.check_expr(*v, Some(rt));
                        self.coerce_or_unify(t, rt, Some(*v), lo);
                    }
                    None => {
                        self.unify(rt, unit);
                    }
                }
                self.tcx.tys.never
            }
            ExprKind::Tuple(v) => {
                let exp_elems = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Tuple(x) if x.len() == v.len() => Some(x),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let mut ts = Vec::new();
                for (i, x) in v.iter().enumerate() {
                    let ex = match &exp_elems {
                        Some(es) => Some(es[i]),
                        None => None,
                    };
                    let t = self.check_expr(*x, ex);
                    if let Some(et) = ex {
                        self.coerce_or_unify(t, et, Some(*x), lo);
                        ts.push(et);
                    } else {
                        ts.push(t);
                    }
                }
                self.tcx.tys.intern(TyKind::Tuple(ts))
            }
            ExprKind::Array(v) => {
                let elem_exp = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Array(e, _) | TyKind::Slice(e) => Some(e),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let elem = match elem_exp {
                    Some(t) => t,
                    None => self.fresh(0),
                };
                for x in v {
                    let t = self.check_expr(*x, Some(elem));
                    self.coerce_or_unify(t, elem, Some(*x), lo);
                }
                self.tcx.tys.intern(TyKind::Array(elem, v.len() as u64))
            }
            ExprKind::Repeat(x, n) => {
                let elem_exp = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Array(e, _) => Some(e),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let t = self.check_expr(*x, elem_exp);
                let len = crate::consteval::eval_usize_expr(self.prog, self.tcx, self.file, self.module, *n).unwrap_or(0);
                let usize_ = self.tcx.tys.usize_;
                self.check_expr(*n, Some(usize_));
                self.tcx.tys.intern(TyKind::Array(t, len))
            }
            ExprKind::Field(x, name) => self.check_field(e, *x, *name, lo),
            ExprKind::TupleField(x, idx) => {
                let t = self.check_expr(*x, None);
                let mut cur = self.shallow(t);
                let mut derefs = 0u8;
                loop {
                    match self.tcx.tys.kind(cur).clone() {
                        TyKind::Tuple(v) => {
                            self.b.field_idx.insert(e.0, (derefs, *idx));
                            return v.get(*idx as usize).copied().unwrap_or(error);
                        }
                        TyKind::Adt(d, args) => {
                            let adt = self.tcx.adts.get(&d).cloned();
                            if let Some(adt) = adt {
                                if !adt.is_enum {
                                    if let Some(f) = adt.variants[0].fields.get(*idx as usize) {
                                        self.b.field_idx.insert(e.0, (derefs, *idx));
                                        return self.tcx.tys.subst(f.ty, &args);
                                    }
                                }
                            }
                            break;
                        }
                        _ => match self.deref_step(e.0, derefs, cur) {
                            Some(i) => {
                                cur = i;
                                derefs += 1;
                            }
                            None => break,
                        },
                    }
                }
                let s = self.ty_str(t);
                self.err(lo, format!("no field `{}` on `{}`", idx, s));
                error
            }
            ExprKind::Index(a, i) => {
                let ta = self.check_expr(*a, None);
                let ti = self.check_expr(*i, None);
                let mut cur = self.shallow(ta);
                let mut derefs = 0u8;
                let tis = self.shallow(ti);
                let idx_is_range = matches!(self.tcx.tys.kind(tis), TyKind::Adt(..));
                if !idx_is_range {
                    let usize_ = self.tcx.tys.usize_;
                    self.unify(tis, usize_);
                }
                loop {
                    match self.tcx.tys.kind(cur).clone() {
                        TyKind::Array(el, _) | TyKind::Slice(el) => {
                            self.b.index_derefs.insert(e.0, derefs);
                            if idx_is_range {
                                return self.tcx.tys.intern(TyKind::Slice(el));
                            }
                            return el;
                        }
                        TyKind::Str if idx_is_range => {
                            self.b.index_derefs.insert(e.0, derefs);
                            return self.tcx.tys.str_;
                        }
                        TyKind::Ref(_, x) | TyKind::Ptr(_, x) => {
                            cur = self.shallow(x);
                            derefs += 1;
                        }
                        TyKind::Adt(..) => {
                            // `Index<Idx>` impl on the type, else autoderef through Deref
                            if let Some(td) = self.lang_item(&["ops", "Index"]) {
                                let dc = self.deep(cur);
                                let ti2 = self.deep(ti);
                                if let Some((imp, iargs)) = self.tcx.find_impl(td, dc, &[ti2]) {
                                    self.b.index_derefs.insert(e.0, derefs);
                                    let m = self.trait_fn(td, "index").unwrap();
                                    self.b.ov_index.insert(e.0, (m, vec![dc, ti2]));
                                    let out = self.prog.syms.get("Output").unwrap_or(u32::MAX);
                                    let r = match self.tcx.impl_assoc_ty(self.prog, imp, out) {
                                        Some(at) => self.tcx.tys.subst(at, &iargs),
                                        None => error,
                                    };
                                    return self.tcx.normalize(self.prog, r);
                                }
                            }
                            match self.deref_step(e.0, derefs, cur) {
                                Some(i) => {
                                    cur = i;
                                    derefs += 1;
                                }
                                None => break,
                            }
                        }
                        _ => break,
                    }
                }
                let s = self.ty_str(ta);
                self.err(lo, format!("cannot index `{}`", s));
                error
            }
            ExprKind::AddrOf(raw, m, x) => {
                let exp_inner = match expected {
                    Some(t) => {
                        let s = self.shallow(t);
                        match self.tcx.tys.kind(s).clone() {
                            TyKind::Ref(_, i) => Some(i),
                            _ => None,
                        }
                    }
                    None => None,
                };
                // `&[a, b]` against `&[T]`: check the array with the slice's element
                let t = self.check_expr(*x, exp_inner);
                if *raw {
                    self.tcx.tys.intern(TyKind::Ptr(*m, t))
                } else {
                    self.tcx.tys.intern(TyKind::Ref(*m, t))
                }
            }
            ExprKind::Call(f, args) => self.check_call(e, *f, args, expected, lo),
            ExprKind::MethodCall { recv, name, turbofish, args } => self.check_method(e, *recv, *name, turbofish, args, lo),
            ExprKind::Struct(path, fields, base) => {
                let r = self.resolve_struct_path(path);
                self.b.res.insert(e.0, r);
                let d = match r {
                    Res::Def(d) => d,
                    _ => return error,
                };
                let (adt_ty, vi) = self.adt_of_ctor(d);
                let adt_ty = self.instantiate_adt(adt_ty);
                if let Some(x) = expected {
                    self.unify(adt_ty, x);
                }
                for fe in fields {
                    let fname = self.sym(fe.name);
                    let fname = if fe.name.hi > fe.name.lo && self.text(fe.name).as_bytes()[0].is_ascii_digit() {
                        self.text(fe.name).parse::<u32>().unwrap_or(0) | 0x8000_0000
                    } else {
                        fname
                    };
                    match self.field_ty(adt_ty, vi, fname) {
                        Some((_, ft)) => {
                            let t = self.check_expr(fe.expr, Some(ft));
                            self.coerce_or_unify(t, ft, Some(fe.expr), fe.name.lo);
                        }
                        None => {
                            self.err(fe.name.lo, format!("no field `{}`", self.text(fe.name)));
                        }
                    }
                }
                if let Some(bx) = base {
                    let t = self.check_expr(*bx, Some(adt_ty));
                    self.unify(t, adt_ty);
                }
                adt_ty
            }
            ExprKind::Range(a, b, incl) => {
                let elem = self.fresh(0);
                if let Some(a) = a {
                    let t = self.check_expr(*a, Some(elem));
                    self.unify(t, elem);
                }
                if let Some(b) = b {
                    let t = self.check_expr(*b, Some(elem));
                    self.unify(t, elem);
                }
                let name = match (a.is_some(), b.is_some(), *incl) {
                    (true, true, false) => "Range",
                    (true, true, true) => "RangeInclusive",
                    (true, false, _) => "RangeFrom",
                    (false, true, false) => "RangeTo",
                    (false, true, true) => "RangeToInclusive",
                    (false, false, _) => "RangeFull",
                };
                match self.lang_item(&["ops", name]) {
                    Some(d) => {
                        let n = self.tcx.adts.get(&d).map_or(0, |a| a.n_generics);
                        let args = if n == 0 { Vec::new() } else { vec![elem] };
                        self.tcx.tys.intern(TyKind::Adt(d, args))
                    }
                    None => {
                        self.err(lo, format!("core::ops::{} not found", name));
                        error
                    }
                }
            }
            ExprKind::Closure { params, ret, body, .. } => self.check_closure(e, params, *ret, *body, expected),
            ExprKind::Mac(m) => self.check_mac(e, m, expected, lo),
            ExprKind::Try(_) | ExprKind::Await(_) | ExprKind::Async(..) | ExprKind::Yield(_) | ExprKind::Box(_) => {
                self.err(lo, "unsupported expression (?, async, box)".to_string());
                error
            }
            ExprKind::Underscore => self.fresh(0),
        }
    }

    fn sym_lifetime(&self, id: Ident) -> Sym {
        // lifetimes are not interned as idents; use the text without the quote
        let t = self.text(id);
        let t = t.trim_start_matches('\'');
        self.prog.syms.get(t).unwrap_or(u32::MAX - 1)
    }

    fn loop_index(&self, label: Option<Sym>) -> Option<usize> {
        let mut i = self.loops.len();
        while i > 0 {
            i -= 1;
            match label {
                None => {
                    // unlabeled break targets the innermost real loop (not a labeled block)
                    return Some(i);
                }
                Some(l) => {
                    if self.loops[i].label == Some(l) {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    fn loop_has_break(&self, b: BlockId, label: Option<Sym>) -> bool {
        // conservative syntactic scan: any `break` that targets this loop
        let ast = self.ast();
        let blk = ast.block(b);
        let mut found = false;
        for s in &blk.stmts {
            match s {
                Stmt::Expr(e, _) => found = found || self.expr_has_break(*e, 0, label),
                Stmt::Let { init: Some(e), .. } => found = found || self.expr_has_break(*e, 0, label),
                _ => {}
            }
        }
        found
    }

    fn expr_has_break(&self, e: ExprId, depth: u32, label: Option<Sym>) -> bool {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::Break(l, x) => {
                let target_ok = match l {
                    None => depth == 0,
                    Some(li) => Some(self.sym_lifetime(*li)) == label,
                };
                target_ok || x.map_or(false, |v| self.expr_has_break(v, depth, label))
            }
            ExprKind::Loop(b, _) | ExprKind::While(_, b, _) | ExprKind::For(_, _, b, _) => self.block_has_break(*b, depth + 1, label),
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => self.block_has_break(*b, depth, label),
            ExprKind::If(c, t, el) => {
                self.expr_has_break(*c, depth, label) || self.block_has_break(*t, depth, label) || el.map_or(false, |x| self.expr_has_break(x, depth, label))
            }
            ExprKind::Match(x, arms) => {
                let mut r = self.expr_has_break(*x, depth, label);
                for a in arms {
                    r = r || self.expr_has_break(a.body, depth, label);
                }
                r
            }
            ExprKind::Paren(x) => self.expr_has_break(*x, depth, label),
            _ => false,
        }
    }

    fn block_has_break(&self, b: BlockId, depth: u32, label: Option<Sym>) -> bool {
        let ast = self.ast();
        for s in &ast.block(b).stmts {
            let r = match s {
                Stmt::Expr(e, _) => self.expr_has_break(*e, depth, label),
                Stmt::Let { init: Some(e), .. } => self.expr_has_break(*e, depth, label),
                _ => false,
            };
            if r {
                return true;
            }
        }
        false
    }

    pub fn lang_item(&self, path: &[&str]) -> Option<DefId> {
        // core::<path>
        let prog = self.prog;
        let core = prog.syms.get("core")?;
        let mut cur = None;
        for c in 0..prog.crates.len() {
            if prog.crates[c].name == core {
                cur = Some(prog.mods[prog.crates[c].root_mod as usize].def);
            }
        }
        let mut cur = cur?;
        for p in path {
            let s = prog.syms.get(p)?;
            cur = match prog.lookup_in_container(cur, s, true) {
                Some(d) => d,
                None => prog.lookup_in_container(cur, s, false)?,
            };
        }
        Some(cur)
    }

    fn is_numeric(&self, t: TyId) -> bool {
        match self.tcx.tys.kind(t) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Error => true,
            TyKind::Infer(v) => matches!(self.vars[*v as usize], VarSt::Free(1) | VarSt::Free(2)),
            _ => false,
        }
    }

    fn check_lit(&mut self, e: ExprId, k: LitKind, expected: Option<TyId>) -> TyId {
        let f = &self.prog.files[self.file as usize];
        let ex = f.ast.expr(e);
        let text = std::str::from_utf8(&f.src[ex.lo as usize..ex.hi as usize]).unwrap_or("");
        match k {
            LitKind::Int | LitKind::Float => {
                if let Some(suf) = crate::consteval::int_suffix(text) {
                    for (n, p) in crate::program::PRIMS {
                        if n == suf {
                            return self.tcx.tys.prim(p);
                        }
                    }
                }
                let want_float = k == LitKind::Float;
                if let Some(x) = expected {
                    let s = self.shallow(x);
                    let ok = match self.tcx.tys.kind(s) {
                        TyKind::Int(_) => !want_float,
                        TyKind::Float(_) => true,
                        TyKind::Infer(v) => match self.vars[*v as usize] {
                            VarSt::Free(1) => !want_float,
                            VarSt::Free(2) => true,
                            _ => false,
                        },
                        _ => false,
                    };
                    if ok {
                        return s;
                    }
                }
                self.fresh(if want_float { 2 } else { 1 })
            }
            LitKind::Bool(_) => self.tcx.tys.bool_,
            LitKind::Char => self.tcx.tys.char_,
            LitKind::Byte => self.tcx.tys.u8_,
            LitKind::Str | LitKind::RawStr => self.tcx.tys.str_ref,
            LitKind::ByteStr => {
                let n = crate::lower::unescape_bytes(text).len() as u64;
                let arr = self.tcx.tys.intern(TyKind::Array(self.tcx.tys.u8_, n));
                self.tcx.tys.intern(TyKind::Ref(false, arr))
            }
            LitKind::CStr => self.tcx.tys.error,
        }
    }

    fn check_binary(&mut self, e: ExprId, op: BinOp, a: ExprId, b: ExprId, expected: Option<TyId>, lo: u32) -> TyId {
        let bool_ = self.tcx.tys.bool_;
        match op {
            BinOp::And | BinOp::Or => {
                self.check_expr(a, Some(bool_));
                // `let` chains bind in the left operand; keep those scopes for the right
                self.check_expr(b, Some(bool_));
                self.diverges = false;
                bool_
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let ta = self.check_expr(a, None);
                let tb = self.check_expr(b, Some(ta));
                if !self.unify(ta, tb) {
                    let (sa, sb) = (self.ty_str(ta), self.ty_str(tb));
                    self.err(lo, format!("cannot compare `{}` with `{}`", sa, sb));
                }
                bool_
            }
            BinOp::Shl | BinOp::Shr => {
                let ta = self.check_expr(a, expected);
                self.check_expr(b, None);
                ta
            }
            _ => {
                let exp = match expected {
                    Some(x) => {
                        let s = self.shallow(x);
                        if self.is_numeric(s) {
                            Some(s)
                        } else {
                            None
                        }
                    }
                    None => None,
                };
                let ta = self.check_expr(a, exp);
                let sa0 = self.shallow(ta);
                if matches!(self.tcx.tys.kind(sa0), TyKind::Adt(..) | TyKind::Ref(..) | TyKind::Tuple(..) | TyKind::Array(..)) {
                    self.cur_binop = e.0;
                    return self.check_op_trait(op, sa0, b, lo);
                }
                let tb = self.check_expr(b, Some(ta));
                let sa = self.shallow(ta);
                let sb = self.shallow(tb);
                // primitive arithmetic: both sides the same type
                if !self.unify(sa, sb) {
                    let (x, y) = (self.ty_str(sa), self.ty_str(sb));
                    self.err(lo, format!("no binary operator for `{}` and `{}`", x, y));
                }
                ta
            }
        }
    }

    /// `Iterator::Item` of `t` if it implements Iterator (records `next` for the loop).
    fn iterator_item(&mut self, key: u32, t: TyId) -> Option<TyId> {
        let td = self.lang_item(&["iter", "Iterator"])?;
        let dt = self.deep(t);
        let (imp, iargs) = self.tcx.find_impl(td, dt, &[])?;
        let item = self.prog.syms.get("Item")?;
        let at = self.tcx.impl_assoc_ty(self.prog, imp, item)?;
        let r = self.tcx.tys.subst(at, &iargs);
        let r = self.tcx.normalize(self.prog, r);
        let next = self.trait_fn(td, "next")?;
        self.b.for_next.insert(key, (next, vec![dt]));
        Some(r)
    }

    fn check_for_iter(&mut self, it: ExprId, lo: u32) -> TyId {
        let t = self.check_expr(it, None);
        let s = self.shallow(t);
        let error = self.tcx.tys.error;
        match self.tcx.tys.kind(s).clone() {
            TyKind::Adt(d, args) => {
                let name = self.prog.name(d).to_string();
                if (name == "Range" || name == "RangeInclusive" || name == "RangeFrom") && args.len() == 1 {
                    let a = self.shallow(args[0]);
                    if self.is_numeric(a) {
                        return a;
                    }
                }
                if let Some(item) = self.iterator_item(it.0, s) {
                    return item;
                }
                // IntoIterator (e.g. Vec by value)
                if let Some(td) = self.lang_item(&["iter", "IntoIterator"]) {
                    let dt = self.deep(s);
                    if let Some((imp, iargs)) = self.tcx.find_impl(td, dt, &[]) {
                        let isym = self.prog.syms.get("IntoIter").unwrap_or(u32::MAX);
                        if let Some(at) = self.tcx.impl_assoc_ty(self.prog, imp, isym) {
                            let iter_t = self.tcx.tys.subst(at, &iargs);
                            let iter_t = self.tcx.normalize(self.prog, iter_t);
                            if let Some(m) = self.trait_fn(td, "into_iter") {
                                self.b.for_into.insert(it.0, (m, vec![dt], iter_t));
                                if let Some(item) = self.iterator_item(it.0, iter_t) {
                                    return item;
                                }
                            }
                        }
                    }
                }
                let ts = self.ty_str(s);
                self.err(lo, format!("`for` over `{}`: no Iterator / IntoIterator impl", ts));
                error
            }
            TyKind::Array(el, _) => el,
            TyKind::Ref(m, inner) => {
                let inner = self.shallow(inner);
                match self.tcx.tys.kind(inner).clone() {
                    TyKind::Array(el, _) | TyKind::Slice(el) => self.tcx.tys.intern(TyKind::Ref(m, el)),
                    TyKind::Adt(..) => {
                        // `for x in &v` with v: Vec<T> (Deref<Target = [T]>)
                        if let Some(target) = self.deref_step(it.0, 1, inner) {
                            let tg = self.shallow(target);
                            if let TyKind::Slice(el) = self.tcx.tys.kind(tg).clone() {
                                return self.tcx.tys.intern(TyKind::Ref(m, el));
                            }
                        }
                        // `for x in &mut iter`
                        if let Some(item) = self.iterator_item(it.0, s) {
                            return item;
                        }
                        let ts = self.ty_str(s);
                        self.err(lo, format!("`for` over `{}` is not supported yet", ts));
                        error
                    }
                    _ => {
                        let ts = self.ty_str(s);
                        self.err(lo, format!("`for` over `{}` is not supported yet", ts));
                        error
                    }
                }
            }
            _ => {
                let ts = self.ty_str(s);
                self.err(lo, format!("`for` over `{}` is not supported yet", ts));
                error
            }
        }
    }

    fn check_field(&mut self, e: ExprId, x: ExprId, name: Ident, lo: u32) -> TyId {
        let t = self.check_expr(x, None);
        let s = self.sym(name);
        let mut cur = self.shallow(t);
        let mut derefs = 0u8;
        loop {
            if let TyKind::Adt(d, args) = self.tcx.tys.kind(cur).clone() {
                if let Some(adt) = self.tcx.adts.get(&d).cloned() {
                    if !adt.is_enum {
                        for (i, f) in adt.variants[0].fields.iter().enumerate() {
                            if f.name == s {
                                self.b.field_idx.insert(e.0, (derefs, i as u32));
                                return self.tcx.tys.subst(f.ty, &args);
                            }
                        }
                    }
                }
            }
            // references, raw pointers, then Deref impls (Box<T>, wrappers)
            match self.deref_step(e.0, derefs, cur) {
                Some(i) => {
                    cur = i;
                    derefs += 1;
                }
                None => break,
            }
        }
        let ts = self.ty_str(t);
        self.err(lo, format!("no field `{}` on `{}`", self.text(name), ts));
        self.tcx.tys.error
    }

    fn check_call(&mut self, e: ExprId, f: ExprId, args: &[ExprId], expected: Option<TyId>, lo: u32) -> TyId {
        let ft = self.check_expr(f, None);
        let fs = self.shallow(ft);
        let error = self.tcx.tys.error;
        let (params, ret) = match self.tcx.tys.kind(fs).clone() {
            TyKind::FnDef(d, gargs) => match self.tcx.sigs.get(&d).cloned() {
                Some(sig) => {
                    let mut ps = Vec::new();
                    for p in &sig.params {
                        ps.push(self.tcx.tys.subst(*p, &gargs));
                    }
                    let r = self.tcx.tys.subst(sig.ret, &gargs);
                    if sig.variadic {
                        for (i, a) in args.iter().enumerate() {
                            if i < ps.len() {
                                let t = self.check_expr(*a, Some(ps[i]));
                                self.coerce_or_unify(t, ps[i], Some(*a), lo);
                            } else {
                                self.check_expr(*a, None);
                            }
                        }
                        return r;
                    }
                    (ps, r)
                }
                None => (Vec::new(), error),
            },
            TyKind::FnPtr(ps, r) => (ps, r),
            TyKind::Closure(_, ce) => {
                let (ps, r) = self.closure_sig(ce);
                (ps, r)
            }
            TyKind::Error => {
                for a in args {
                    self.check_expr(*a, None);
                }
                return error;
            }
            _ => {
                let s = self.ty_str(fs);
                self.err(lo, format!("`{}` is not callable", s));
                return error;
            }
        };
        if params.len() != args.len() {
            self.err(lo, format!("expected {} arguments, found {}", params.len(), args.len()));
        }
        // the expected result can guide generic args (e.g. `Some(x)` into Option<T>)
        if let Some(x) = expected {
            let rs = self.shallow(ret);
            if matches!(self.tcx.tys.kind(rs), TyKind::Adt(..)) {
                let xs = self.shallow(x);
                if matches!(self.tcx.tys.kind(xs), TyKind::Adt(..)) {
                    let save = self.vars.clone();
                    if !self.unify(rs, xs) {
                        self.vars = save;
                    }
                }
            }
        }
        for (i, a) in args.iter().enumerate() {
            let pt = if i < params.len() { self.norm(params[i]) } else { error };
            let t = self.check_expr(*a, Some(pt));
            self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
        }
        let _ = e;
        self.norm(ret)
    }

    fn closure_sig(&mut self, ce: u32) -> (Vec<TyId>, TyId) {
        let ast = self.ast();
        if let ExprKind::Closure { params, body, .. } = &ast.expr(ExprId(ce)).kind {
            let mut ps = Vec::new();
            for p in params {
                let pi = (p.pat.0 - self.b.pat_lo) as usize;
                ps.push(self.b.pat_ty[pi]);
            }
            let r = self.b.expr_ty[(body.0 - self.b.expr_lo) as usize];
            return (ps, r);
        }
        (Vec::new(), self.tcx.tys.error)
    }

    fn check_closure(&mut self, e: ExprId, params: &[ClosureParam], ret: Option<crate::ast::TyId>, body: ExprId, expected: Option<TyId>) -> TyId {
        let (exp_ps, exp_r) = match expected {
            Some(x) => {
                let s = self.shallow(x);
                match self.tcx.tys.kind(s).clone() {
                    TyKind::FnPtr(ps, r) => (Some(ps), Some(r)),
                    _ => (None, None),
                }
            }
            None => (None, None),
        };
        let saved = self.scopes.len();
        self.closure_depth.push((e.0, saved));
        for (i, p) in params.iter().enumerate() {
            let t = match p.ty {
                Some(t) => self.lower_ty(t),
                None => match &exp_ps {
                    Some(ps) if i < ps.len() => ps[i],
                    _ => self.fresh(0),
                },
            };
            self.check_pat(p.pat, t);
        }
        let rt = match ret {
            Some(t) => self.lower_ty(t),
            None => match exp_r {
                Some(r) => r,
                None => self.fresh(0),
            },
        };
        self.ret_stack.push(rt);
        let save_loops = std::mem::take(&mut self.loops);
        let bt = self.check_expr(body, Some(rt));
        self.loops = save_loops;
        self.coerce_or_unify(bt, rt, Some(body), self.ast().expr(body).lo);
        self.set_ty(body, rt);
        self.ret_stack.pop();
        self.closure_depth.pop();
        self.scopes.truncate(saved);
        self.diverges = false;
        self.b.closures.push(e.0);
        self.tcx.tys.intern(TyKind::Closure(self.file, e.0))
    }

    fn check_method(&mut self, e: ExprId, recv: ExprId, name: Ident, turbofish: &Option<Box<GenericArgs>>, args: &[ExprId], lo: u32) -> TyId {
        let rt = self.check_expr(recv, None);
        let s = self.sym(name);
        let error = self.tcx.tys.error;
        let mut cur = self.shallow(rt);
        let mut derefs = 0u8;
        let mut found = None;
        let mut trait_found: Option<(DefId, DefId, TyId)> = None;
        loop {
            if let Some((item, imp)) = self.tcx.inherent_item(self.prog, cur, s) {
                if self.prog.def(item).kind == DefKind::AssocFn {
                    found = Some((item, imp, cur));
                    break;
                }
            }
            let tm = self.tcx.trait_methods_for(self.prog, cur, s);
            if let Some(&(item, imp)) = tm.first() {
                trait_found = Some((item, imp, cur));
                break;
            }
            match self.tcx.tys.kind(cur).clone() {
                TyKind::Ref(_, i) => {
                    cur = self.shallow(i);
                    derefs += 1;
                }
                TyKind::Array(el, _) => {
                    // arrays get slice methods by unsizing
                    cur = self.tcx.tys.intern(TyKind::Slice(el));
                }
                _ => match self.deref_step(e.0, derefs, cur) {
                    Some(i) => {
                        cur = i;
                        derefs += 1;
                    }
                    None => break,
                },
            }
        }
        if let (None, Some((item, imp, self_ty))) = (found, trait_found) {
            return self.check_trait_method_call(e, item, imp, self_ty, derefs, turbofish, args, lo);
        }
        let (item, imp, self_ty) = match found {
            Some(x) => x,
            None => {
                let ts = self.ty_str(rt);
                self.err(lo, format!("no method `{}` on `{}`", self.text(name), ts));
                for a in args {
                    self.check_expr(*a, None);
                }
                return error;
            }
        };
        let sig = self.tcx.sigs.get(&item).cloned().unwrap();
        let mut gargs = self.match_impl_args(imp, self_ty);
        let own = sig.n_generics - sig.n_parent_generics;
        let mut tf = Vec::new();
        if let Some(a) = turbofish {
            if let GenericArgs::Angle(v) = &**a {
                for g in v {
                    if let GenericArg::Type(t) = g {
                        let lt = self.lower_ty(*t);
                        tf.push(lt);
                    }
                }
            }
        }
        for i in 0..own as usize {
            if i < tf.len() {
                gargs.push(tf[i]);
            } else {
                gargs.push(self.fresh(0));
            }
        }
        let autoref = match sig.self_kind {
            2 => 1,
            3 => 2,
            _ => 0,
        };
        self.b.methods.insert(e.0, MethodRes { def: item, derefs, autoref, args: gargs.clone() });
        if sig.self_kind == 0 {
            self.err(lo, format!("`{}` is an associated function, not a method", self.text(name)));
            return error;
        }
        let mut ps = Vec::new();
        for p in &sig.params {
            ps.push(self.tcx.tys.subst(*p, &gargs));
        }
        let ret = self.tcx.tys.subst(sig.ret, &gargs);
        if ps.len() != args.len() + 1 {
            self.err(lo, format!("expected {} arguments, found {}", ps.len() - 1, args.len()));
        }
        for (i, a) in args.iter().enumerate() {
            let pt = if i + 1 < ps.len() { ps[i + 1] } else { error };
            let t = self.check_expr(*a, Some(pt));
            self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
        }
        ret
    }

    /// `a op b` on a non-primitive left operand: core::ops trait call, result `Output`.
    fn check_op_trait(&mut self, op: BinOp, ta: TyId, b: ExprId, lo: u32) -> TyId {
        let (tr, m) = match op {
            BinOp::Add => ("Add", "add"),
            BinOp::Sub => ("Sub", "sub"),
            BinOp::Mul => ("Mul", "mul"),
            BinOp::Div => ("Div", "div"),
            BinOp::Rem => ("Rem", "rem"),
            BinOp::BitAnd => ("BitAnd", "bitand"),
            BinOp::BitOr => ("BitOr", "bitor"),
            BinOp::BitXor => ("BitXor", "bitxor"),
            BinOp::Shl => ("Shl", "shl"),
            BinOp::Shr => ("Shr", "shr"),
            _ => ("", ""),
        };
        let error = self.tcx.tys.error;
        let me = self.cur_binop;
        let td = match self.lang_item(&["ops", tr]) {
            Some(d) => d,
            None => {
                self.err(lo, format!("core::ops::{} not found", tr));
                return error;
            }
        };
        // a single impl for the left type fixes the right operand's expected type
        let key = crate::tcx::ty_key(&self.tcx.tys, ta);
        let mut cands = Vec::new();
        if let Some(list) = self.tcx.trait_impls.get(&(key, td)) {
            for &imp in list {
                if let Some(&pat) = self.tcx.impl_self.get(&imp) {
                    if self.tcx.impl_matches(pat, ta) {
                        cands.push(imp);
                    }
                }
            }
        }
        let mut exp_b = None;
        if cands.len() == 1 {
            let iargs = self.match_impl_args(cands[0], ta);
            if let Some(targs) = self.tcx.impl_trait_args.get(&cands[0]).cloned() {
                if let Some(t0) = targs.first() {
                    exp_b = Some(self.tcx.tys.subst(*t0, &iargs));
                }
            }
        }
        let tb = self.check_expr(b, exp_b);
        if let Some(x) = exp_b {
            self.coerce_or_unify(tb, x, Some(b), lo);
        }
        let tb = self.deep(tb);
        let ms = self.prog.syms.get(m).unwrap_or(u32::MAX);
        let mut item = None;
        for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
            if self.prog.def(it).name == ms {
                item = Some(it);
            }
        }
        let item = match item {
            Some(i) => i,
            None => return error,
        };
        let ta = self.deep(ta);
        if self.tcx.find_impl(td, ta, &[tb]).is_none() {
            let (x, y) = (self.ty_str(ta), self.ty_str(tb));
            self.err(lo, format!("no `{}` impl for `{}` and `{}`", tr, x, y));
            return error;
        }
        let args = vec![ta, tb];
        self.b.binops.insert(me, MethodRes { def: item, derefs: 0, autoref: 0, args: args.clone() });
        let sig = self.tcx.sigs.get(&item).cloned().unwrap();
        let r = self.tcx.tys.subst(sig.ret, &args);
        self.norm(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn check_trait_method_call(&mut self, e: ExprId, item: DefId, imp: DefId, self_ty: TyId, derefs: u8, turbofish: &Option<Box<GenericArgs>>, args: &[ExprId], lo: u32) -> TyId {
        let error = self.tcx.tys.error;
        let mut tf = Vec::new();
        if let Some(a) = turbofish {
            if let GenericArgs::Angle(v) = &**a {
                for g in v {
                    if let GenericArg::Type(t) = g {
                        let lt = self.lower_ty(*t);
                        tf.push(lt);
                    }
                }
            }
        }
        let gargs = self.trait_call_args(item, imp, self_ty, &tf);
        let sig = self.tcx.sigs.get(&item).cloned().unwrap();
        let autoref = match sig.self_kind {
            2 => 1,
            3 => 2,
            _ => 0,
        };
        self.b.methods.insert(e.0, MethodRes { def: item, derefs, autoref, args: gargs.clone() });
        let mut ps = Vec::new();
        for p in &sig.params {
            let x = self.tcx.tys.subst(*p, &gargs);
            ps.push(self.norm(x));
        }
        if ps.len() != args.len() + 1 {
            self.err(lo, format!("expected {} arguments, found {}", ps.len().saturating_sub(1), args.len()));
        }
        for (i, a) in args.iter().enumerate() {
            let pt = if i + 1 < ps.len() { ps[i + 1] } else { error };
            let t = self.check_expr(*a, Some(pt));
            self.coerce_or_unify(t, pt, Some(*a), self.ast().expr(*a).lo);
        }
        let r = self.tcx.tys.subst(sig.ret, &gargs);
        self.norm(r)
    }

    // ------------------------------------------------------------ macros

    fn check_mac(&mut self, e: ExprId, m: &MacCall, expected: Option<TyId>, lo: u32) -> TyId {
        let ast = self.ast();
        let name = match m.path.segs.last() {
            Some(s) => self.text(s.name),
            None => "",
        };
        let unit = self.tcx.tys.unit;
        let never = self.tcx.tys.never;
        let bool_ = self.tcx.tys.bool_;
        if m.args == NO_MAC_ARGS {
            self.err(lo, format!("macro `{}!` is not supported", name));
            return self.tcx.tys.error;
        }
        let margs = &ast.mac_args[m.args as usize];
        match name {
            "assert" | "debug_assert" => {
                if let Some(&c) = margs.exprs.first() {
                    let t = self.check_expr(c, Some(bool_));
                    self.coerce_or_unify(t, bool_, None, lo);
                }
                self.check_fmt_args(e, &margs.exprs[1.min(margs.exprs.len())..]);
                self.diverges = false;
                unit
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                if margs.exprs.len() >= 2 {
                    let ta = self.check_expr(margs.exprs[0], None);
                    let tb = self.check_expr(margs.exprs[1], Some(ta));
                    if !self.unify(ta, tb) {
                        let (sa, sb) = (self.ty_str(ta), self.ty_str(tb));
                        self.err(lo, format!("assert_eq of `{}` and `{}`", sa, sb));
                    }
                }
                self.check_fmt_args(e, &margs.exprs[2.min(margs.exprs.len())..]);
                self.diverges = false;
                unit
            }
            "print" | "println" | "eprint" | "eprintln" => {
                self.check_fmt_args(e, &margs.exprs);
                unit
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                self.check_fmt_args(e, &margs.exprs);
                never
            }
            "matches" => {
                if let (Some(&x), Some(p)) = (margs.exprs.first(), margs.pat) {
                    let t = self.check_expr(x, None);
                    let saved = self.scopes.len();
                    self.check_pat(p, t);
                    if let Some(g) = margs.guard {
                        self.check_expr(g, Some(bool_));
                    }
                    self.scopes.truncate(saved);
                }
                bool_
            }
            _ => {
                let _ = expected;
                self.err(lo, format!("macro `{}!` is not supported yet", name));
                for x in &margs.exprs {
                    self.check_expr(*x, None);
                }
                self.tcx.tys.error
            }
        }
    }

    /// Format string + args: checks args and resolves inline `{name}` arguments.
    fn check_fmt_args(&mut self, e: ExprId, exprs: &[ExprId]) {
        if exprs.is_empty() {
            return;
        }
        let ast = self.ast();
        let first = exprs[0];
        let f = &self.prog.files[self.file as usize];
        let fe = ast.expr(first);
        if let ExprKind::Lit(LitKind::Str) | ExprKind::Lit(LitKind::RawStr) = fe.kind {
            let text = std::str::from_utf8(&f.src[fe.lo as usize..fe.hi as usize]).unwrap_or("");
            let s = crate::lower::unescape_str(text);
            let pieces = crate::lower::parse_fmt(&s);
            for p in &pieces {
                if let crate::lower::FmtPiece::Arg { named: Some(n), .. } = p {
                    if let Some(sy) = self.prog.syms.get(n) {
                        match self.lookup_local(sy) {
                            Some(li) => {
                                self.b.fmt_named.insert((e.0, sy), Res::Local(li));
                            }
                            None => match self.prog.lookup_in_scope(self.module, sy, false) {
                                Some(d) => {
                                    self.b.fmt_named.insert((e.0, sy), Res::Def(d));
                                }
                                None => self.err(fe.lo, format!("cannot find `{}` for format string", n)),
                            },
                        }
                    } else {
                        self.err(fe.lo, format!("cannot find `{}` for format string", n));
                    }
                }
            }
            self.check_expr(first, None);
        } else {
            self.check_expr(first, None);
        }
        for x in &exprs[1..] {
            self.check_expr(*x, None);
        }
    }
}

pub fn ty_to_string(prog: &Program, tcx: &Tcx, t: TyId) -> String {
    match tcx.tys.kind(t) {
        TyKind::Bool => "bool".to_string(),
        TyKind::Char => "char".to_string(),
        TyKind::Str => "str".to_string(),
        TyKind::Int(p) | TyKind::Float(p) => crate::types::Types::prim_name(*p).to_string(),
        TyKind::Never => "!".to_string(),
        TyKind::Tuple(v) => {
            let mut s = "(".to_string();
            for (i, x) in v.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&ty_to_string(prog, tcx, *x));
            }
            if v.len() == 1 {
                s.push(',');
            }
            s.push(')');
            s
        }
        TyKind::Array(e, n) => format!("[{}; {}]", ty_to_string(prog, tcx, *e), n),
        TyKind::Slice(e) => format!("[{}]", ty_to_string(prog, tcx, *e)),
        TyKind::Ref(m, e) => format!("&{}{}", if *m { "mut " } else { "" }, ty_to_string(prog, tcx, *e)),
        TyKind::Ptr(m, e) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_to_string(prog, tcx, *e)),
        TyKind::Adt(d, args) => {
            let mut s = prog.name(*d).to_string();
            if !args.is_empty() {
                s.push('<');
                for (i, x) in args.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&ty_to_string(prog, tcx, *x));
                }
                s.push('>');
            }
            s
        }
        TyKind::FnPtr(ps, r) => {
            let mut s = "fn(".to_string();
            for (i, x) in ps.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&ty_to_string(prog, tcx, *x));
            }
            s.push_str(") -> ");
            s.push_str(&ty_to_string(prog, tcx, *r));
            s
        }
        TyKind::FnDef(d, _) => format!("fn item {}", prog.def_path(*d)),
        TyKind::Param(i) => format!("T{}", i),
        TyKind::Infer(v) => format!("?{}", v),
        TyKind::Closure(_, e) => format!("closure#{}", e),
        TyKind::Assoc(d, args) => {
            let st = match args.first() {
                Some(t) => ty_to_string(prog, tcx, *t),
                None => "?".to_string(),
            };
            format!("<{} as {}>::{}", st, prog.name(prog.def(*d).parent), prog.name(*d))
        }
        TyKind::Error => "{error}".to_string(),
    }
}

pub fn key_of(tcx: &Tcx, t: TyId) -> TyKey {
    ty_key(&tcx.tys, t)
}

pub fn prim_of(tcx: &Tcx, t: TyId) -> Option<Prim> {
    match tcx.tys.kind(t) {
        TyKind::Int(p) | TyKind::Float(p) => Some(*p),
        TyKind::Bool => Some(Prim::Bool),
        TyKind::Char => Some(Prim::Char),
        _ => None,
    }
}

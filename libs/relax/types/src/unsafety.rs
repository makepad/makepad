//! The unsafety check of `makepad check` (rustc_mir_build/src/check_unsafety.rs over Relax's
//! typed AST instead of THIR), and the item-level unsafety errors of impls
//! (rustc_hir_analysis/src/coherence/unsafety.rs, rustc_ast_passes ast_validation.rs:1181,
//! rustc_hir_analysis E0197).
//!
//! E0133 for unsafe operations outside an unsafe context; the `unsafe_op_in_unsafe_fn` lint
//! (allow before edition 2024, warn from it) for those in an `unsafe fn` body; `unused_unsafe`
//! for blocks no operation needed; and the `static_mut_refs` lint (rustc_lint static_mut_refs.rs).
//! Closures and inline consts are checked with their parent body and inherit its context; a
//! repeat count is an anonymous constant of its own (a fresh safe context). Unsafe blocks in
//! the output of built-in, derive and proc macros are compiler-generated (they neither need nor
//! count as used); lints are not reported inside macros from other files (rustc: external
//! macros). Check-only: `makepad run` never calls this.

use crate::ast::*;
use crate::lint::{self, Level};
use crate::parser::{ExpnKind, ParsedFile};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{Safety, TyKind};

#[derive(Clone)]
enum Ctx {
    Safe,
    BuiltinUnsafe,
    UnsafeFn,
    Block { pos: u32, used: bool, nested_used: Vec<u32> },
}

#[derive(Clone, Copy)]
enum Op {
    CallToUnsafeFunction(Option<DefId>),
    UseOfMutableStatic,
    UseOfExternStatic,
    DerefOfRawPointer,
    AccessToUnionField,
    CallToFunctionWith(DefId),
}

struct V<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    pf: &'a ParsedFile,
    ast: &'a Ast,
    ctx: Ctx,
    assignment: bool,
    in_union_destructure: bool,
    op_in_unsafe_fn_allowed: bool,
    /// the end of the call expression being checked (0: none): a call a macro of another file
    /// builds (`$func($args)`) starts at a call-site token but ends inside the expansion
    call_hi: u32,
    unused_unsafe_allowed: bool,
    body_features: Vec<&'static str>,
    errors: Vec<String>,
    /// (block position, enclosing used block position)
    unused: Vec<(u32, Option<u32>)>,
}

/// Checks one typed body owner; returns its E0133 errors (lints go to the lint sink).
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body) -> Vec<String> {
    if !lint::enabled() { return Vec::new(); }
    let d = prog.def(body.def);
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let sig = tcx.fn_sig(prog, body.def);
    let ctx = match sig {
        Some(s) if s.header.safety == Safety::Unsafe && !s.safe_target_features => Ctx::UnsafeFn,
        _ => Ctx::Safe,
    };
    let mut v = V {
        prog, tcx, body, pf, ast, ctx,
        assignment: false, in_union_destructure: false,
        op_in_unsafe_fn_allowed: lint::level(prog, body.def, &lint::UNSAFE_OP_IN_UNSAFE_FN) == Level::Allow,
        call_hi: 0,
        unused_unsafe_allowed: lint::level(prog, body.def, &lint::UNUSED_UNSAFE) == Level::Allow,
        body_features: sig.map_or(Vec::new(), |s| s.target_features.iter().map(|f| f.name).collect()),
        errors: Vec::new(), unused: Vec::new(),
    };
    match ast.item(d.item).kind {
        ItemKind::Fn(s, _, _) => {
            let Ok(Some(block)) = prog.fn_body(body.file, d.item) else { return Vec::new() };
            for p in ast.list(ast.fn_sig(s).params) { v.pat(p.pat, ast.item(d.item).lo); }
            v.block(block);
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            let Ok(Some(e)) = prog.const_body(body.file, d.item) else { return Vec::new() };
            v.expr(e);
        }
        _ => return Vec::new(),
    }
    v.unused.sort();
    for (pos, enclosing) in std::mem::take(&mut v.unused) {
        if v.external(pos) { continue; }
        let msg = match enclosing {
            Some(_) => "unnecessary `unsafe` block -- because it's nested under this `unsafe` block",
            None => "unnecessary `unsafe` block",
        };
        lint::emit(prog, body.def, &lint::UNUSED_UNSAFE, body.file, pos, msg);
    }
    v.errors
}

impl V<'_> {
    fn external(&self, pos: u32) -> bool {
        matches!(self.pf.expansion_kind(pos), ExpnKind::ExternalRules | ExpnKind::Builtin)
    }

    fn in_ctx(&mut self, ctx: Ctx, f: impl FnOnce(&mut Self)) {
        let prev = std::mem::replace(&mut self.ctx, ctx);
        f(self);
        let inner = std::mem::replace(&mut self.ctx, prev);
        if let Ctx::Block { pos, used, nested_used } = inner {
            if !used {
                self.unused.push((pos, None));
                if let Ctx::Block { nested_used: ref mut p, .. } = self.ctx { p.extend(nested_used); }
            } else {
                for b in nested_used { self.unused.push((b, Some(pos))); }
                if let Ctx::Block { nested_used: ref mut p, .. } = self.ctx { p.push(pos); }
            }
        }
    }

    /// rustc_mir_transform check_packed_ref: `&place` where the place is a field reached through a
    /// packed ADT (after the last dereference) whose alignment exceeds the packing (E0793).
    fn packed_ref(&mut self, pos: u32, place: ExprId) {
        let mut pack: Option<(u32, bool)> = None;
        let mut cur = place;
        loop {
            cur = self.peel(cur);
            let base = match self.ast.expr(cur).kind {
                ExprKind::Field(b, _) | ExprKind::TupleField(b, _) | ExprKind::Index(b, _) => b,
                _ => break,
            };
            let bt = self.body.ty(base);
            // an implicit or explicit dereference ends the place
            if matches!(self.tcx.tys.kind(bt), TyKind::Ref(..) | TyKind::Ptr(..)) { break; }
            if matches!(self.ast.expr(cur).kind, ExprKind::Index(..)) && !matches!(self.tcx.tys.kind(bt), TyKind::Array(..)) { break; }
            if let TyKind::Adt(d, _, _) = self.tcx.tys.kind(bt) {
                if let Some(a) = self.tcx.adts.get(d) {
                    if a.repr_packed > 0 && pack.map_or(true, |(p, _)| a.repr_packed < p) { pack = Some((a.repr_packed, a.is_union)); }
                }
            }
            cur = base;
        }
        let Some((pack, union)) = pack else { return };
        let ty = self.body.ty(place);
        if self.tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_ALIAS) { return; }
        let lay = crate::layout::Layouts::new();
        let l = lay.of(&crate::layout::LayoutCx::new(self.prog), self.tcx, ty);
        if l.align > pack {
            let msg = format!("E0793: reference to field of packed {} is unaligned", if union { "union" } else { "struct" });
            self.errors.push(self.prog.record_error(self.body.file, pos, "typeck", msg));
        }
    }

    fn fn_path(&self, d: DefId) -> String {
        let def = self.prog.def(d);
        let parent = self.prog.def(def.parent);
        if parent.kind == DefKind::Impl {
            if let Some(&t) = self.tcx.impl_self.get(&def.parent) {
                let s = crate::typeck::ty_to_string(false, self.prog, self.tcx, t);
                return format!("{}::{}", s, self.prog.syms.str(def.name));
            }
        }
        let p = self.prog.def_path(d);
        // rustc's def_path_str: the local crate's paths without its name
        if def.krate == self.prog.def(self.body.def).krate {
            if let Some((_, rest)) = p.split_once("::") { return rest.to_string(); }
        }
        p
    }

    fn requires_unsafe(&mut self, pos: u32, op: Op) {
        match self.ctx {
            Ctx::BuiltinUnsafe => {}
            Ctx::Block { ref mut used, .. } => *used = true,
            Ctx::UnsafeFn if self.op_in_unsafe_fn_allowed => {}
            Ctx::UnsafeFn => {
                if self.deprecated_safe(op) || self.external(pos) || (self.call_hi > 0 && self.external(self.call_hi - 1)) { return; }
                let msg = format!("E0133: {} is unsafe and requires unsafe block", self.what(op));
                lint::emit(self.prog, self.body.def, &lint::UNSAFE_OP_IN_UNSAFE_FN, self.body.file, pos, &msg);
            }
            Ctx::Safe => {
                if self.deprecated_safe(op) {
                    // emit_deprecated_safe_fn_call: the lint (allow by default, 2024 compatibility)
                    if let Op::CallToUnsafeFunction(Some(d)) = op {
                        let msg = format!("call to deprecated safe function `{}` is unsafe and requires unsafe block", self.prog.def_path(d));
                        lint::emit(self.prog, self.body.def, &lint::DEPRECATED_SAFE_2024, self.body.file, pos, &msg);
                    }
                    return;
                }
                let tail = if self.op_in_unsafe_fn_allowed && !matches!(op, Op::CallToFunctionWith(_)) { "function or block" } else { "block" };
                let msg = format!("E0133: {} is unsafe and requires unsafe {}", self.what(op), tail);
                self.errors.push(self.prog.record_error(self.body.file, pos, "typeck", msg));
            }
        }
    }

    fn what(&self, op: Op) -> String {
        match op {
            Op::CallToUnsafeFunction(Some(d)) => format!("call to unsafe function `{}`", self.fn_path(d)),
            Op::CallToUnsafeFunction(None) => "call to unsafe function".into(),
            Op::UseOfMutableStatic => "use of mutable static".into(),
            Op::UseOfExternStatic => "use of extern static".into(),
            Op::DerefOfRawPointer => "dereference of raw pointer".into(),
            Op::AccessToUnionField => "access to union field".into(),
            Op::CallToFunctionWith(d) => format!("call to function `{}` with `#[target_feature]`", self.fn_path(d)),
        }
    }

    /// check_unsafety.rs emit_deprecated_safe_fn_call: before edition 2024 a call to a
    /// `#[rustc_deprecated_safe_2024]` fn needs no unsafe block.
    fn deprecated_safe(&self, op: Op) -> bool {
        let Op::CallToUnsafeFunction(Some(d)) = op else { return false };
        if self.pf.edition >= 2024 { return false; }
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        crate::lang_items::has_attr(self.prog, def.file, ast.list(ast.item(def.item).attrs), "rustc_deprecated_safe_2024")
    }

    fn call_def(&mut self, pos: u32, d: DefId) {
        let Some(sig) = self.tcx.fn_sig(self.prog, d) else { return };
        if sig.header.safety == Safety::Unsafe && !sig.safe_target_features {
            self.requires_unsafe(pos, Op::CallToUnsafeFunction(Some(d)));
        } else if !sig.target_features.iter().all(|f| self.body_features.contains(&f.name)) {
            self.requires_unsafe(pos, Op::CallToFunctionWith(d));
        }
    }

    fn peel(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Paren(x) = self.ast.expr(e).kind { e = x; }
        e
    }

    /// The static a path expression names, if it is one: (def, mutable, safe foreign static).
    fn static_of(&self, e: ExprId) -> Option<(DefId, bool, bool)> {
        let ExprKind::Path(_) = self.ast.expr(e).kind else { return None };
        let Some(Res::Def(d)) = self.body.res.get(&e.0) else { return None };
        let def = self.prog.def(*d);
        if !matches!(def.kind, DefKind::Static | DefKind::ForeignStatic) { return None; }
        let ast = &self.prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        let ItemKind::Static(m, _, _) = item.kind else { return None };
        let safe = def.kind == DefKind::ForeignStatic && {
            let f = &self.prog.files[def.file as usize];
            let head = f.bytes(item.lo, item.name.lo);
            head.split(|c| !c.is_ascii_alphanumeric() && *c != b'_').any(|w| w == b"safe")
        };
        Some((*d, m, safe))
    }

    /// Whether field expression `e` (base `x`) projects a union's field.
    fn union_field(&self, e: ExprId, x: ExprId, name: Option<Ident>) -> bool {
        let Some(&(n, fi)) = self.body.field_idx.get(&e.0) else { return false };
        let mut t = self.body.ty(x);
        let mut known = true;
        for step in 0..n as usize {
            if self.body.ov_deref(e.0, step).is_some() {
                match &self.body.ov_deref(e.0, step + 1) {
                    Some((_, args)) if step + 1 < n as usize => t = self.tcx.tys.list(*args)[0],
                    _ => { known = false; break; }
                }
                continue;
            }
            t = match self.tcx.tys.kind(t) {
                TyKind::Ref(_, p, _) | TyKind::Ptr(_, p) => *p,
                TyKind::Adt(_, args, _) if !self.tcx.tys.list(*args).is_empty() => self.tcx.tys.list(*args)[0],
                _ => { known = false; break; }
            };
        }
        // the base type is known: a tuple or struct field is no union field
        if known {
            return match self.tcx.tys.kind(t) {
                TyKind::Adt(d, _, _) => self.tcx.adts.get(d).is_some_and(|a| a.is_union),
                _ => false,
            };
        }
        // through an overloaded deref: the ADT the body names this field of
        let name = name.map(|n| self.pf.text(n));
        let mut cands = self.body.field_uses.iter().filter(|(d, i)| *i == fi && self.tcx.adts.get(d).is_some_and(|a| {
            !a.is_enum && name.map_or(true, |n| a.variants[0].fields.get(*i as usize).is_some_and(|f| self.prog.syms.str(f.name) == n))
        }));
        cands.next().is_some_and(|(d, _)| self.tcx.adts.get(d).is_some_and(|a| a.is_union))
    }

    fn block(&mut self, b: BlockId) {
        for s in self.ast.stmts(b) {
            match *s {
                Stmt::Let(r) => {
                    let LetStmt { pat, init, else_, .. } = *self.ast.row(r);
                    if let Some(x) = init {
                        self.expr(x);
                        self.static_mut_ref_let(pat, x);
                    }
                    self.pat(pat, init.map_or(self.ast.block(b).lo, |x| self.ast.expr(x).lo));
                    if let Some(e) = else_ { self.block(e); }
                }
                Stmt::Expr(e, _) => self.expr(e),
                Stmt::Attrs(r) => self.expr(self.ast.row(r).expr),
                Stmt::Item(_) | Stmt::Empty => {}
            }
        }
    }

    fn pat_pos(&self, p: PatId, fallback: u32) -> u32 {
        match *self.ast.pat(p) {
            Pat::Ident { name, .. } => name.lo,
            Pat::Lit(e) | Pat::Range(Some(e), _, _) | Pat::Range(None, Some(e), _) => self.ast.expr(e).lo,
            Pat::Path(ref path) | Pat::TupleStruct(ref path, _) | Pat::Struct(ref path, _, _) => path.lo,
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_pos(x, fallback),
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => self.ast.list(ps).first().map_or(fallback, |x| self.pat_pos(*x, fallback)),
            _ => fallback,
        }
    }

    /// check_unsafety.rs visit_pat: binding or testing anything of a union's field.
    fn pat(&mut self, p: PatId, pos: u32) {
        let pat = *self.ast.pat(p);
        if self.in_union_destructure {
            match pat {
                Pat::Wild | Pat::Rest | Pat::Mac(_) => return,
                Pat::Or(ps) => { for x in self.ast.list(ps) { self.pat(*x, pos); } return; }
                Pat::Paren(x) => return self.pat(x, pos),
                _ => {
                    let at = self.pat_pos(p, pos);
                    self.requires_unsafe(at, Op::AccessToUnionField);
                    return;
                }
            }
        }
        match pat {
            Pat::Ident { sub: Some(s), .. } | Pat::Ref(_, s) | Pat::Paren(s) | Pat::Box(s) => self.pat(s, pos),
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) | Pat::TupleStruct(_, ps) => for x in self.ast.list(ps) { self.pat(*x, pos); },
            Pat::Struct(_, fs, _) => {
                let union = matches!(self.body.pat_res.get(&p.0), Some(Res::Def(d)) if self.prog.def(*d).kind == DefKind::Union);
                let old = std::mem::replace(&mut self.in_union_destructure, union);
                for f in self.ast.list(fs) {
                    // a shorthand field (`U { a }`) is a binding at the field's name
                    self.pat(f.pat, f.name.lo);
                }
                self.in_union_destructure = old;
            }
            _ => {}
        }
    }

    fn exprs(&mut self, xs: List<ExprId>) {
        for x in self.ast.list(xs) { self.expr(*x); }
    }

    fn expr(&mut self, e: ExprId) {
        let ex = *self.ast.expr(e);
        let lo = ex.lo;
        // could this be the left side of an assignment to a union field?
        if !matches!(ex.kind, ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Paren(_) | ExprKind::Cast(..)) {
            self.assignment = false;
        }
        match ex.kind {
            ExprKind::Call(f, args) => {
                if !self.body.call_traits.contains_key(&e.0) {
                    let mut t = self.body.ty(f);
                    while let TyKind::Ref(_, p, _) = self.tcx.tys.kind(t) { t = *p; }
                    self.call_hi = self.ast.expr(e).hi;
                    match *self.tcx.tys.kind(t) {
                        TyKind::FnDef(d, args, _) => {
                            self.call_def(lo, d);
                            // intrinsicck: both sides of a transmute have one size
                            if crate::transmute::is_transmute(self.prog, d) {
                                if let [from, to] = self.tcx.tys.list(args) {
                                    let env = crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(self.prog, self.body.def), param_env: self.body.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::PostAnalysis };
                                    if let Some(msg) = crate::transmute::check_transmute(self.prog, self.tcx, env, *from, *to) {
                                        self.errors.push(self.prog.record_error(self.body.file, lo, "typeck", msg));
                                    }
                                }
                            }
                        }
                        TyKind::FnPtr(_, _, h, _) if h.safety == Safety::Unsafe => self.requires_unsafe(lo, Op::CallToUnsafeFunction(None)),
                        _ => {}
                    }
                    self.call_hi = 0;
                }
                self.expr(f);
                self.exprs(args);
            }
            ExprKind::MethodCall(recv, row) => {
                if let Some(m) = self.body.methods.get(&e.0) { let d = m.def; self.call_def(lo, d); }
                self.static_mut_ref_method(e, recv);
                self.expr(recv);
                self.exprs(self.ast.row(row).args);
            }
            ExprKind::AddrOf(raw, m, x) => {
                let x = self.peel(x);
                if raw {
                    // a raw borrow of a dereference (or of a static) is safe: only the
                    // dereferenced pointer is checked
                    if let ExprKind::Unary(UnOp::Deref, y) = self.ast.expr(x).kind { return self.expr(y); }
                    if self.static_of(x).is_some() { return; }
                    // and of union fields
                    let mut peeled = x;
                    loop {
                        match self.ast.expr(peeled).kind {
                            ExprKind::Field(b, n) if self.union_field(peeled, b, Some(n)) => peeled = self.peel(b),
                            ExprKind::TupleField(b, _) if self.union_field(peeled, b, None) => peeled = self.peel(b),
                            _ => break,
                        }
                    }
                    return self.expr(peeled);
                }
                self.static_mut_ref(lo, x, m);
                self.packed_ref(lo, x);
                self.expr(x);
            }
            ExprKind::Unary(UnOp::Deref, x) => {
                if matches!(self.tcx.tys.kind(self.body.ty(x)), TyKind::Ptr(..)) {
                    self.requires_unsafe(lo, Op::DerefOfRawPointer);
                }
                self.expr(x);
            }
            ExprKind::Path(_) => {
                if let Some((_, m, safe)) = self.static_of(e) {
                    if m { self.requires_unsafe(lo, Op::UseOfMutableStatic); }
                    else if self.prog.def(match self.body.res.get(&e.0) { Some(Res::Def(d)) => *d, _ => return }).kind == DefKind::ForeignStatic && !safe {
                        self.requires_unsafe(lo, Op::UseOfExternStatic);
                    }
                }
            }
            ExprKind::Field(x, n) => {
                if self.union_field(e, x, Some(n)) && !self.assignment { self.requires_unsafe(lo, Op::AccessToUnionField); }
                self.expr(x);
            }
            ExprKind::TupleField(x, _) => {
                if self.union_field(e, x, None) && !self.assignment { self.requires_unsafe(lo, Op::AccessToUnionField); }
                self.expr(x);
            }
            ExprKind::Assign(l, r) => {
                self.assignment = true;
                self.expr(l);
                self.assignment = false;
                self.expr(r);
            }
            ExprKind::Unsafe(b) => {
                if self.pf.expansion_kind(lo) == ExpnKind::Builtin {
                    self.in_ctx(Ctx::BuiltinUnsafe, |v| v.block(b));
                } else {
                    let used = self.unused_unsafe_allowed;
                    self.in_ctx(Ctx::Block { pos: lo, used, nested_used: Vec::new() }, |v| v.block(b));
                }
            }
            ExprKind::Closure(row) => {
                let ClosureRow { params, body, .. } = *self.ast.row(row);
                for p in self.ast.list(params) { self.pat(p.pat, lo); }
                self.expr(body);
            }
            ExprKind::Repeat(x, count) => {
                self.expr(x);
                // the count is an anonymous constant: a body of its own
                let saved = std::mem::replace(&mut self.ctx, Ctx::Safe);
                self.expr(count);
                self.ctx = saved;
            }
            ExprKind::Block(b, _) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => self.block(b),
            ExprKind::If(c, then, els) => {
                self.expr(c);
                self.block(then);
                if let Some(x) = els.get() { self.expr(x); }
            }
            ExprKind::Let(p, x) => { self.expr(x); self.pat(p, lo); }
            ExprKind::Match(x, arms) => {
                self.expr(x);
                for a in self.ast.list(arms) {
                    self.pat(a.pat, self.ast.expr(a.body).lo);
                    if let Some(g) = a.guard { self.expr(g); }
                    self.expr(a.body);
                }
            }
            ExprKind::While(row) => {
                let WhileRow { cond, body, .. } = *self.ast.row(row);
                self.expr(cond);
                self.block(body);
            }
            ExprKind::For(row) => {
                let ForRow { pat, iter, body, .. } = *self.ast.row(row);
                self.expr(iter);
                self.pat(pat, lo);
                self.block(body);
            }
            ExprKind::Struct(row) => {
                let StructRow { fields, base, .. } = *self.ast.row(row);
                for f in self.ast.list(fields) { self.expr(f.expr); }
                if let Some(x) = base.get() { self.expr(x); }
            }
            ExprKind::Binary(_, a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) => { self.expr(a); self.expr(b); }
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Paren(x) | ExprKind::Box(x)
            | ExprKind::Try(x) | ExprKind::Await(x) => self.expr(x),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => self.exprs(xs),
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { self.expr(x); }
                if let Some(x) = b.get() { self.expr(x); }
            }
            ExprKind::Return(x) | ExprKind::Break(_, x) | ExprKind::Yield(x) => if let Some(x) = x.get() { self.expr(x); },
            ExprKind::Mac(m) => {
                let m = *self.ast.row(m);
                if m.args == NO_MAC_ARGS { return; }
                let args = *self.ast.mac_arg(m.args);
                self.exprs(args.exprs);
                if let Some(p) = args.pat { self.pat(p, lo); }
                if let Some(g) = args.guard { self.expr(g); }
                // format strings' inline `{NAME}` arguments naming a static
                if !self.body.fmt_named.is_empty() {
                    let named: Vec<DefId> = self.body.fmt_named.iter().filter(|((x, _), _)| *x == e.0)
                        .filter_map(|(_, r)| if let Res::Def(d) = r { Some(*d) } else { None }).collect();
                    for d in named {
                        let def = self.prog.def(d);
                        if !matches!(def.kind, DefKind::Static | DefKind::ForeignStatic) { continue; }
                        let ast = &self.prog.files[def.file as usize].ast;
                        if let ItemKind::Static(true, _, _) = ast.item(def.item).kind {
                            let at = self.ast.list(args.exprs).first().map_or(lo, |x| self.ast.expr(*x).lo);
                            self.requires_unsafe(at, Op::UseOfMutableStatic);
                        }
                    }
                }
            }
            ExprKind::Lit(_) | ExprKind::Continue(_) | ExprKind::Underscore => {}
        }
    }

    // ------------------------------------------------------------ static_mut_refs

    fn path_is_static_mut(&self, mut x: ExprId) -> bool {
        loop {
            x = self.peel(x);
            match self.ast.expr(x).kind {
                ExprKind::Field(b, _) | ExprKind::TupleField(b, _) => x = b,
                _ => break,
            }
        }
        // rustc: `Static { mutability: Mut }` (foreign `static mut` included)
        self.static_of(x).is_some_and(|(_, m, _)| m)
    }

    fn static_mut_ref_lint(&self, pos: u32, mutable: bool) {
        if self.external(pos) { return; }
        let msg = if mutable { "creating a mutable reference to mutable static" } else { "creating a shared reference to mutable static" };
        lint::emit(self.prog, self.body.def, &lint::STATIC_MUT_REFS, self.body.file, pos, msg);
    }

    fn static_mut_ref(&self, pos: u32, x: ExprId, mutable: bool) {
        if self.path_is_static_mut(x) { self.static_mut_ref_lint(pos, mutable); }
    }

    fn static_mut_ref_method(&self, e: ExprId, recv: ExprId) {
        if !self.path_is_static_mut(recv) { return; }
        let Some(m) = self.body.methods.get(&e.0) else { return };
        let Some(sig) = self.tcx.fn_sig(self.prog, m.def) else { return };
        if let Some(&p) = sig.params.first() {
            if let TyKind::Ref(mutable, _, _) = self.tcx.tys.kind(p) {
                self.static_mut_ref_lint(self.ast.expr(e).lo, *mutable);
            }
        }
    }

    fn static_mut_ref_let(&self, pat: PatId, init: ExprId) {
        if let Pat::Ident { by_ref: true, mutbl, .. } = *self.ast.pat(pat) {
            if self.path_is_static_mut(init) { self.static_mut_ref_lint(self.ast.expr(init).lo, mutbl); }
        }
    }
}

// ---------------------------------------------------------------- impls

/// rustc_hir_analysis coherence/unsafety.rs (E0199, E0200), E0197 (inherent impls) and
/// ast_validation's E0198 (negative impls) for impl `d`.
pub fn check_impl(prog: &Program, tcx: &Tcx, d: DefId) -> Vec<String> {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    let ItemKind::Impl(r) = item.kind else { return Vec::new() };
    let imp = *ast.row(r);
    let mut out = Vec::new();
    let mut err = |pos: u32, msg: String| out.push(prog.record_error(def.file, pos, "typeck", msg));
    let Some(path) = imp.trait_ else {
        if imp.unsafe_ { err(item.lo, "E0197: inherent impls cannot be unsafe".into()); }
        return out;
    };
    if imp.negative {
        if imp.unsafe_ { err(path.lo, "E0198: negative impls cannot be unsafe".into()); }
        return out;
    }
    let Some(&td) = tcx.impl_trait.get(&d) else { return out };
    let tdef = prog.def(td);
    let tast = &prog.files[tdef.file as usize].ast;
    let ItemKind::Trait(tr) = tast.item(tdef.item).kind else { return out };
    let trait_unsafe = tast.row(tr).unsafe_;
    let name = prog.syms.str(tdef.name);
    match (trait_unsafe, imp.unsafe_) {
        (false, true) => err(item.lo, format!("E0199: implementing the trait `{}` is not unsafe", name)),
        (true, false) => err(item.lo, format!("E0200: the trait `{}` requires an `unsafe impl` declaration", name)),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::program::{DefId, Program};
    use crate::tcx::Tcx;

    fn collect(source: &str) -> (Program, Tcx) {
        let sess = &std::sync::Arc::new(crate::Session::default());
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("relax-unsafety-{}-{n}.rs", std::process::id()));
        std::fs::write(&root, source).unwrap();
        let mut prog = Program::new(sess);
        prog.load_crate("custom", root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        std::fs::remove_file(root).unwrap();
        prog.resolve_imports();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        (prog, tcx)
    }

    fn named(prog: &Program, name: &str) -> DefId {
        (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == name).unwrap()
    }

    #[test]
    fn unsafety_e0133_and_impls() {
        crate::lint::enable();
        let (prog, tcx) = collect(r#"
            unsafe fn f() -> i32 { 1 }
            static mut S: i32 = 0;
            union U { a: u32, b: u32 }
            unsafe trait T {}
            trait Q {}
            struct X;
            impl T for X {}
            unsafe impl Q for X {}
            fn bad(p: *const i32) {
                f();
                let _ = *p;
                S = 1;
                let u = U { a: 1 };
                let _ = u.a;
                let U { b } = u;
                let _ = b;
            }
            fn good(p: *const i32) {
                let mut u = U { a: 1 };
                u.a = 2;
                let U { a: _ } = u;
                let _ = &raw const S;
                let _ = &raw const *p;
                unsafe { f(); }
            }
            unsafe fn in_unsafe_fn(p: *const i32) -> i32 { *p + f() }
        "#);
        let errs = |name: &str| super::check_body(&prog, &tcx, &tcx.typeck(&prog, named(&prog, name)));
        let bad = errs("bad");
        assert_eq!(bad.iter().filter(|e| e.contains("E0133")).count(), 5, "{bad:?}");
        assert!(errs("good").is_empty());
        assert!(errs("in_unsafe_fn").is_empty());
        let x = named(&prog, "X");
        let impls: Vec<String> = (0..prog.defs.len()).map(|i| DefId(i as u32))
            .filter(|&d| prog.def(d).kind == crate::program::DefKind::Impl && prog.def(d).krate == prog.def(x).krate)
            .flat_map(|d| super::check_impl(&prog, &tcx, d)).collect();
        assert!(impls.iter().any(|e| e.contains("E0200")) && impls.iter().any(|e| e.contains("E0199")), "{impls:?}");
    }
}

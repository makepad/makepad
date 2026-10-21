//! More late (type-aware) builtin lints of `makepad check`: `dangling_pointers_from_temporaries`
//! and `dangling_pointers_from_locals` (rustc_lint dangling.rs), `const_item_mutation`
//! (rustc_mir_transform check_const_item_mutation.rs), ... All run over one walk of a typed body
//! and are check-only (reached from `lint_late::check_body` behind `lint::enabled()`).

use crate::ast::*;
use crate::ast_visit::{self, Visitor};
use crate::lint::{self, Lint};
use crate::lang_items::LangItem;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{TyId, TyKind};
use std::collections::HashMap;

pub const DANGLING_POINTERS_FROM_TEMPORARIES: Lint = lint::lint("dangling_pointers_from_temporaries");
pub const DANGLING_POINTERS_FROM_LOCALS: Lint = lint::lint("dangling_pointers_from_locals");
pub const CONST_ITEM_MUTATION: Lint = lint::lint("const_item_mutation");

/// Runs the passes over one body owner (`block`: a fn body, `expr`: a const / static initialiser).
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body, block: Option<BlockId>, expr: Option<ExprId>) {
    if !lint::enabled() { return; }
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let mut w = L2 {
        prog, tcx, body, ast, stack: Vec::new(), fns: Vec::new(), owner: HashMap::new(), init: HashMap::new(), names: HashMap::new(),
        inside_call_args: false, reset_block: false, closure: u32::MAX, active: false, skip_if: Default::default(), stmt_like: Default::default(), module: u32::MAX, env: crate::pattern_analysis::body_cx(prog, tcx, body).typing_env(),
    };
    // pre-pass: which closure declares each local, the initialisers of plain bindings
    {
        let mut pre = Pre { w: &mut w, closure: u32::MAX };
        if let (Some(_), ItemKind::Fn(s, _, _)) = (block, ast.item(prog.def(body.def).item).kind) {
            for p in ast.list(ast.fn_sig(s).params) { pre.pat(p.pat); }
        }
        if let Some(b) = block { pre.block(b); }
        if let Some(e) = expr { pre.expr(e); }
    }
    let d = prog.def(body.def);
    if let Some(b) = block {
        // the fn itself
        let ret_ptr = match ast.item(d.item).kind {
            ItemKind::Fn(s, _, _) => ast.fn_sig(s).ret.filter(|&t| matches!(ast.ty(t), Ty::Ptr(..))).map(|t| (t, ast_ty_pos(ast, t))),
            _ => None,
        };
        let kind = if d.kind == DefKind::AssocFn { "method" } else { "function" };
        w.active = true;
        w.fns.push(w.fn_cx(body.ret, ret_ptr.map(|x| x.1), kind, u32::MAX));
        w.block(b);
        // the implicit return expression
        if let Some(t) = innermost_tail(ast, b) { w.lint_addr_of_local(t); }
        w.fns.pop();
    }
    if let Some(e) = expr { w.expr(e); }
}

fn ast_ty_pos(ast: &Ast, t: crate::ast::TyId) -> u32 {
    let _ = (ast, t);
    0
}

struct FnCx {
    /// the pointee type of a raw-pointer return type with a layout that is not a 1-ZST
    inner: Option<TyId>,
    ret_ty: TyId,
    kind: &'static str,
    /// the closure expression that owns the locals of this body (u32::MAX: the fn)
    owner: u32,
}

struct L2<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    /// the attributes of the enclosing statements, arms, ..., outermost first
    stack: Vec<Attr>,
    fns: Vec<Option<FnCx>>,
    /// local -> the closure expression declaring it (u32::MAX: the body owner)
    owner: HashMap<u32, u32>,
    /// local -> the initialiser of an immutable plain binding
    init: HashMap<u32, ExprId>,
    /// local -> its name
    names: HashMap<u32, Ident>,
    /// DanglingPointerSearcher::inside_call_args
    inside_call_args: bool,
    /// the next `block` call is the block of a block expression (statements reset the flag)
    reset_block: bool,
    env: crate::tcx::TypingEnv,
    /// `if` expressions already examined as part of an `if let` cascade
    skip_if: std::collections::HashSet<u32>,
    /// expressions whose parent is a statement, a block (as its tail) or a match arm
    stmt_like: std::collections::HashSet<u32>,
    /// the closure being walked
    closure: u32,
    /// inside a fn or closure body (rustc `check_fn`; const and static initialisers are not searched)
    active: bool,
    /// the block module the walk is in (u32::MAX: none, the body owner's module)
    module: u32,
}

/// The first pass.
struct Pre<'w, 'a> {
    w: &'w mut L2<'a>,
    closure: u32,
}

impl<'w, 'a> Visitor<'a> for Pre<'w, 'a> {
    fn ast(&self) -> &'a Ast { self.w.ast }
    fn expr(&mut self, e: ExprId) {
        let saved = self.closure;
        if matches!(self.w.ast.expr(e).kind, ExprKind::Closure(_) | ExprKind::Async(..)) { self.closure = e.0; }
        ast_visit::walk_expr(self, e);
        self.closure = saved;
    }
    fn pat(&mut self, p: PatId) {
        if let Pat::Ident { name, .. } = *self.w.ast.pat(p) {
            if let Some(&l) = self.w.body.pat_local.get(&p.0) {
                self.w.owner.insert(l, self.closure);
                self.w.names.insert(l, name);
            }
        }
        ast_visit::walk_pat(self, p);
    }
    fn arm(&mut self, a: &Arm) {
        let b = unparen(self.w.ast, a.body);
        self.w.stmt_like.insert(b.0);
        ast_visit::walk_arm(self, a);
    }
    fn stmt(&mut self, s: &Stmt, b: BlockId) {
        if let Stmt::Let(r) = *s {
            let l = *self.w.ast.row(r);
            if let (Pat::Ident { by_ref: false, mutbl: false, sub: None, .. }, Some(init)) = (*self.w.ast.pat(l.pat), l.init) {
                if let Some(&loc) = self.w.body.pat_local.get(&l.pat.0) { self.w.init.insert(loc, init); }
            }
        }
        match *s {
            Stmt::Expr(e, _) => { let e = unparen(self.w.ast, e); self.w.stmt_like.insert(e.0); }
            Stmt::Attrs(r) => { let e = unparen(self.w.ast, self.w.ast.row(r).expr); self.w.stmt_like.insert(e.0); }
            _ => {}
        }
        ast_visit::walk_stmt(self, s);
        let _ = b;
    }
}

fn unparen(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = ast.expr(e).kind { e = x; }
    e
}

/// The tail expression of a block (`Block::expr`).
fn block_tail(ast: &Ast, b: BlockId) -> Option<ExprId> {
    match *ast.stmts(b).last()? {
        Stmt::Expr(e, false) => Some(e),
        Stmt::Attrs(r) => { let a = *ast.row(r); if a.semi { None } else { Some(a.expr) } }
        _ => None,
    }
}

/// `Block::innermost_block().expr`.
fn innermost_tail(ast: &Ast, mut b: BlockId) -> Option<ExprId> {
    loop {
        let t = block_tail(ast, b)?;
        match ast.expr(unparen(ast, t)).kind {
            ExprKind::Block(b2, _) | ExprKind::Unsafe(b2) => b = b2,
            _ => return Some(t),
        }
    }
}

/// `Expr::peel_blocks`.
fn peel_blocks(ast: &Ast, mut e: ExprId) -> ExprId {
    loop {
        e = unparen(ast, e);
        match ast.expr(e).kind {
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) if ast.stmts(b).len() == 1 && block_tail(ast, b).is_some() => e = block_tail(ast, b).unwrap(),
            _ => return e,
        }
    }
}

/// `#[rustc_diagnostic_item = "name"]` of a definition.
fn diag_name(prog: &Program, d: DefId) -> Option<String> {
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::Fn | DefKind::AssocFn | DefKind::TypeAlias | DefKind::Const | DefKind::AssocConst) { return None; }
    let ast = &prog.files[def.file as usize].ast;
    let p = prog.src(def.file);
    for a in ast.list(ast.item(def.item).attrs) {
        if lint::attr_path(prog, def.file, a) != b"rustc_diagnostic_item" { continue; }
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        let mut i = lo;
        while i < hi && crate::cfg::tok_str(p, i) != b"=" { i += 1; }
        if i + 1 < hi {
            let t = crate::cfg::tok_str(p, i + 1);
            return Some(String::from_utf8_lossy(t).trim_matches('"').to_string());
        }
    }
    None
}

pub(crate) fn has_def_attr(prog: &Program, d: DefId, name: &str) -> bool {
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::Const | DefKind::AssocConst) { return false; }
    let ast = &prog.files[def.file as usize].ast;
    lint::has_attr(prog, def.file, ast.list(ast.item(def.item).attrs), name)
}

impl<'a> L2<'a> {
    fn kind(&self, t: TyId) -> TyKind { *self.tcx.tys.kind(t) }

    fn emit(&self, lint: &Lint, pos: u32, msg: &str) {
        if lint::external(self.prog, self.body.file, pos) { return; }
        lint::emit_in(self.prog, self.body.def, &self.stack, lint, self.body.file, pos, msg);
    }

    fn expr_ty(&self, e: ExprId) -> Option<TyId> {
        let i = self.body.expr_ix(e);
        if i < self.body.expr_ty.len() && self.body.expr_typed.get(i) { Some(self.body.expr_ty[i]) } else { None }
    }

    fn lang(&self, item: LangItem) -> Option<DefId> { self.tcx.lang_items.get(item) }

    fn fn_cx(&self, ret: TyId, ret_ptr: Option<u32>, kind: &'static str, owner: u32) -> Option<FnCx> {
        ret_ptr?;
        let TyKind::Ptr(_, inner) = self.kind(ret) else { return None };
        // `layout_of(inner).is_ok_and(|layout| !layout.is_1zst())`
        if self.layout_1zst(inner, 0) != Some(false) { return None; }
        Some(FnCx { inner: Some(inner), ret_ty: ret, kind, owner })
    }

    /// Whether the layout of `t` is a 1-ZST (size 0, alignment 1); None: the layout cannot be
    /// computed (generic parameters inline, projections).
    fn layout_1zst(&self, t: TyId, depth: u32) -> Option<bool> {
        if depth > 12 { return None; }
        match self.kind(t) {
            TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Str | TyKind::Ref(..) | TyKind::Ptr(..)
            | TyKind::FnPtr(..) | TyKind::Dyn(..) | TyKind::Foreign(_) => Some(false),
            TyKind::Never | TyKind::FnDef(..) => Some(true),
            TyKind::Slice(e) => { self.layout_1zst(e, depth + 1)?; Some(false) }
            TyKind::Tuple(l) => {
                let mut zst = true;
                for &x in self.tcx.tys.list(l) { zst &= self.layout_1zst(x, depth + 1)?; }
                Some(zst)
            }
            TyKind::Array(e, len) => {
                let ez = self.layout_1zst(e, depth + 1)?;
                match len.try_to_target_usize(&self.tcx.tys) {
                    Some(0) => Some(ez || matches!(self.kind(e), TyKind::Bool | TyKind::Int(crate::program::Prim::I8 | crate::program::Prim::U8))),
                    Some(_) => Some(ez),
                    None => None,
                }
            }
            TyKind::Adt(d, args, _) => {
                let a = self.tcx.adts.get(&d)?;
                if a.is_phantom_data() { return Some(true); }
                let mut all = true;
                for v in &a.variants {
                    for f in &v.fields {
                        let ft = self.tcx.tys.subst(f.ty, self.tcx.tys.list(args));
                        all &= self.layout_1zst(ft, depth + 1)?;
                    }
                }
                if a.variants.is_empty() { return Some(true); }
                if a.is_union || a.repr_align > 1 { return Some(false); }
                if a.is_enum {
                    return Some(a.variants.len() == 1 && all && a.repr_int.is_none());
                }
                Some(all)
            }
            _ => None,
        }
    }

    // ---------------------------------------------------------------- dangling pointers

    /// `cx.expr_or_init`.
    fn expr_or_init(&self, e: ExprId) -> ExprId {
        let mut e = peel_blocks(self.ast, e);
        while let ExprKind::Path(_) = self.ast.expr(e).kind {
            let Some(Res::Local(l)) = self.body.res.get(&e.0).copied() else { break };
            let Some(&init) = self.init.get(&l) else { break };
            e = peel_blocks(self.ast, init);
        }
        e
    }

    /// `utils::peel_casts`.
    fn peel_casts(&self, e: ExprId) -> ExprId { self.peel_casts_flag(e).0 }

    /// `utils::peel_casts`: the innermost expression and whether a cast went through
    /// `UnsafeCell::raw_get`.
    fn peel_casts_flag(&self, e: ExprId) -> (ExprId, bool) {
        let ast = self.ast;
        let mut e = e;
        let mut raw_get = false;
        loop {
            e = peel_blocks(ast, e);
            let next = match ast.expr(e).kind {
                ExprKind::Cast(x, _) => Some(x),
                ExprKind::MethodCall(x, row) if ast.list(ast.row(row).args).is_empty() => {
                    let name = self.body.methods.get(&e.0).and_then(|m| diag_name(self.prog, m.def));
                    if matches!(name.as_deref(), Some("ptr_cast" | "const_ptr_cast" | "ptr_cast_mut" | "ptr_cast_const")) { Some(x) } else { None }
                }
                ExprKind::Call(f, args) if ast.list(args).len() == 1 && matches!(ast.expr(unparen(ast, f)).kind, ExprKind::Path(_)) => {
                    let name = match self.body.res.get(&unparen(ast, f).0) { Some(Res::Def(d)) => diag_name(self.prog, *d), _ => None };
                    if matches!(name.as_deref(), Some("ptr_from_ref" | "unsafe_cell_raw_get" | "transmute")) {
                        if name.as_deref() == Some("unsafe_cell_raw_get") { raw_get = true; }
                        Some(ast.list(args)[0])
                    } else { None }
                }
                _ => None,
            };
            e = match next {
                Some(x) => x,
                None => {
                    let init = self.expr_or_init(e);
                    if init != e { init } else { return (e, raw_get) }
                }
            };
        }
    }

    /// `lint_addr_of_local`: `&<local of this body>` returned from a fn returning a raw pointer.
    fn lint_addr_of_local(&self, expr: ExprId) {
        let Some(Some(cx)) = self.fns.last() else { return };
        let inner = self.peel_casts(expr);
        let ExprKind::AddrOf(false, _, x) = self.ast.expr(inner).kind else { return };
        let x = peel_blocks(self.ast, x);
        if !matches!(self.ast.expr(x).kind, ExprKind::Path(_)) { return; }
        let Some(Res::Local(l)) = self.body.res.get(&x.0).copied() else { return };
        if self.owner.get(&l).copied() != Some(cx.owner) { return; }
        let name = self.names.get(&l).map(|n| self.prog.files[self.body.file as usize].text(*n).to_string()).unwrap_or_default();
        let _ = (cx.inner, cx.ret_ty);
        let pos = self.ast.expr(expr).lo;
        self.emit(&DANGLING_POINTERS_FROM_LOCALS, pos, &format!("{} returns a dangling pointer to dropped local variable `{}`", cx.kind, name));
    }

    /// `is_temporary_rvalue`.
    fn is_temporary_rvalue(&self, e: ExprId) -> bool {
        let e = unparen(self.ast, e);
        match self.ast.expr(e).kind {
            ExprKind::Call(..) | ExprKind::MethodCall(..) | ExprKind::Binary(..) | ExprKind::Mac(_) => true,
            ExprKind::If(..) | ExprKind::Loop(..) | ExprKind::Match(..) | ExprKind::Block(..) | ExprKind::Unsafe(_)
            | ExprKind::While(_) | ExprKind::For(_) | ExprKind::Try(_) | ExprKind::Await(_) => true,
            ExprKind::Struct(_) | ExprKind::Range(..) => true,
            _ => false,
        }
    }

    /// `owns_allocation`.
    fn owns_allocation(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Array(..) => true,
            TyKind::Adt(d, args, _) if Some(d) == self.lang(LangItem::OwnedBox) => {
                let Some(&inner) = self.tcx.tys.list(args).first() else { return false };
                match self.kind(inner) {
                    TyKind::Slice(_) | TyKind::Str => true,
                    TyKind::Adt(i, ..) if Some(i) == self.lang(LangItem::CStr) => true,
                    _ => self.owns_allocation(inner),
                }
            }
            TyKind::Adt(d, ..) => {
                if [LangItem::String, LangItem::MaybeUninit, LangItem::UnsafeCell].iter().any(|&l| Some(d) == self.lang(l)) { return true; }
                matches!(diag_name(self.prog, d).as_deref(), Some("cstring_type" | "Vec" | "Cell" | "SyncUnsafeCell"))
            }
            _ => false,
        }
    }

    /// `lint_expr` of dangling.rs.
    fn lint_temporary(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, row) = self.ast.expr(e).kind else { return };
        if !self.is_temporary_rvalue(recv) { return; }
        let Some(ty) = self.expr_ty(recv) else { return };
        if !self.owns_allocation(ty) { return; }
        let Some(m) = self.body.methods.get(&e.0) else { return };
        if !has_def_attr(self.prog, m.def, "rustc_as_ptr") { return; }
        let name = self.ast.row(row).name;
        let tys = crate::typeck::ty_to_string(false, self.prog, self.tcx, ty);
        self.emit(&DANGLING_POINTERS_FROM_TEMPORARIES, name.lo, &format!("this creates a dangling pointer because temporary `{}` is dropped at end of statement", tys));
    }
}

// ---------------------------------------------------------------- const_item_mutation

impl<'a> L2<'a> {
    /// The root of a place expression that is mutably used, when it is a `const` item (or a
    /// const parameter) reached by field / builtin index projections only: (the const's path
    /// expression, whether a projection is overloaded `IndexMut`). None: a dereference
    /// is involved (the mutation goes through a pointer), or the root is no constant.
    fn const_place_root(&self, e: ExprId) -> Option<(ExprId, bool)> {
        let e = unparen(self.ast, e);
        match self.ast.expr(e).kind {
            ExprKind::Path(_) => match self.body.res.get(&e.0).copied() {
                Some(Res::Def(d)) if matches!(self.prog.def(d).kind, DefKind::Const | DefKind::AssocConst) => Some((e, false)),
                Some(Res::ConstParam(_)) => Some((e, false)),
                _ => None,
            },
            ExprKind::Field(b, _) | ExprKind::TupleField(b, _) => {
                // a field of a reference or a smart pointer is reached through a dereference
                let t = self.expr_ty(b)?;
                if !matches!(self.kind(t), TyKind::Adt(..) | TyKind::Tuple(_)) { return None; }
                if self.body.ov_deref(b.0, 0).is_some() || self.body.ov_deref(e.0, 0).is_some() { return None; }
                self.const_place_root(b)
            }
            ExprKind::Index(b, _) => {
                if self.body.ov_index.get(&e.0).is_some() {
                    return self.const_place_root(b).map(|(r, _)| (r, true));
                }
                let t = self.expr_ty(b)?;
                if !matches!(self.kind(t), TyKind::Array(..)) { return None; }
                self.const_place_root(b)
            }
            _ => None,
        }
    }

    fn const_has_dtor(&self, root: ExprId) -> bool {
        let Some(Res::Def(d)) = self.body.res.get(&root.0).copied() else { return false };
        let Some(t) = self.tcx.const_ty(self.prog, d) else { return false };
        match self.kind(t) {
            TyKind::Adt(a, ..) => self.tcx.adt_destructor(a).is_some(),
            _ => false,
        }
    }

    fn lint_const_mutation(&self, e: ExprId) {
        const MODIFY: &str = "attempting to modify a `const` item";
        const BORROW: &str = "taking a mutable reference to a `const` item";
        let ast = self.ast;
        let pos = ast.expr(e).lo;
        match ast.expr(e).kind {
            ExprKind::Assign(l, _) | ExprKind::AssignOp(_, l, _) => {
                let overloaded_op = matches!(ast.expr(e).kind, ExprKind::AssignOp(..)) && self.body.binops.get(&e.0).is_some();
                let Some((root, over)) = self.const_place_root(l) else { return };
                if overloaded_op {
                    self.emit(&CONST_ITEM_MUTATION, pos, BORROW);
                } else if over {
                    self.emit(&CONST_ITEM_MUTATION, pos, BORROW);
                } else if unparen(ast, l) != root && !self.const_has_dtor(root) {
                    self.emit(&CONST_ITEM_MUTATION, pos, MODIFY);
                }
            }
            ExprKind::AddrOf(false, true, x) => {
                if self.const_place_root(x).is_some() { self.emit(&CONST_ITEM_MUTATION, pos, BORROW); }
            }
            ExprKind::MethodCall(r, _) => {
                let Some(m) = self.body.methods.get(&e.0) else { return };
                if m.autoref != 2 { return; }
                if m.derefs > 0 {
                    let overloaded = [e.0, r.0, unparen(ast, r).0].iter().any(|k| self.body.ov_deref(*k, 0).is_some());
                    if !overloaded { return; }
                }
                if self.const_place_root(r).is_some() { self.emit(&CONST_ITEM_MUTATION, pos, BORROW); }
            }
            _ => {}
        }
    }
}

impl<'a> Visitor<'a> for L2<'a> {
    fn ast(&self) -> &'a Ast { self.ast }
    fn enter_attrs(&mut self, attrs: List<Attr>) { self.stack.extend_from_slice(self.ast.list(attrs)); }
    fn leave_attrs(&mut self, attrs: List<Attr>) {
        let n = self.ast.list(attrs).len();
        self.stack.truncate(self.stack.len() - n);
    }

    fn block(&mut self, b: BlockId) {
        let ast = self.ast;
        let reset = std::mem::replace(&mut self.reset_block, false);
        let blk = *ast.block(b);
        let saved_module = self.module;
        if let Some(&m) = self.prog.block_mods.get(&(self.body.file, b.0)) { self.module = m; }
        self.enter_attrs(blk.attrs);
        let saved = self.inside_call_args;
        let stmts = ast.stmts(b);
        for (i, s) in stmts.iter().enumerate() {
            if reset {
                let tail = i + 1 == stmts.len() && matches!(*s, Stmt::Expr(_, false));
                self.inside_call_args = if tail { saved } else { false };
            }
            self.stmt(s, b);
        }
        self.inside_call_args = saved;
        self.leave_attrs(blk.attrs);
        self.module = saved_module;
    }

    fn expr(&mut self, e: ExprId) {
        let ast = self.ast;
        if self.active && !self.inside_call_args { self.lint_temporary(e); }
        self.lint_const_mutation(e);
        self.lint_prelude_dot_call(e);
        self.lint_shadowed_into_iter(e);
        self.lint_if_let_rescope(e);
        self.lint_comparisons(e);
        self.lint_ptr_nulls(e);
        self.lint_transmutes(e);
        self.lint_reference_casting(e);
        if matches!(ast.expr(e).kind, ExprKind::Path(_)) { self.lint_mutable_transmutes(e); self.lint_unused_qualifications(e); }
        self.lint_unsafe_block(e);
        self.lint_invalid_value(e);
        self.lint_interior_mutable_consts(e);
        self.lint_closure_returning_async_block(e);
        if matches!(ast.expr(e).kind, ExprKind::Path(_)) { self.lint_prelude_path(e); }
        match ast.expr(e).kind {
            ExprKind::Call(f, args) => {
                self.expr(f);
                let saved = std::mem::replace(&mut self.inside_call_args, true);
                for &a in ast.list(args) { self.expr(a); }
                self.inside_call_args = saved;
            }
            ExprKind::MethodCall(r, row) => {
                self.expr(r);
                let saved = std::mem::replace(&mut self.inside_call_args, true);
                for &a in ast.list(ast.row(row).args) { self.expr(a); }
                self.inside_call_args = saved;
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => {
                self.reset_block = true;
                self.block(b);
            }
            ExprKind::If(c, then, els) => {
                self.expr(c);
                self.reset_block = true;
                self.block(then);
                if let Some(x) = els.get() { self.expr(x); }
            }
            ExprKind::While(row) => {
                let w = *ast.row(row);
                self.expr(w.cond);
                self.reset_block = true;
                self.block(w.body);
            }
            ExprKind::For(row) => {
                let f = *ast.row(row);
                self.expr(f.iter);
                self.pat(f.pat);
                self.reset_block = true;
                self.block(f.body);
            }
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                let ret_ptr = c.ret.filter(|&t| matches!(ast.ty(t), Ty::Ptr(..)));
                let ret = self.body.closure_sigs.get(&e.0).and_then(|s| match self.kind(s.liberated) { TyKind::FnPtr(_, r, _, _) => Some(r), _ => None });
                let cx = match (ret_ptr, ret) { (Some(_), Some(r)) => self.fn_cx(r, Some(0), "closure", e.0), _ => None };
                let saved = (std::mem::replace(&mut self.inside_call_args, false), std::mem::replace(&mut self.closure, e.0), std::mem::replace(&mut self.active, true));
                self.fns.push(cx);
                ast_visit::walk_expr(self, e);
                if let Some(Some(_)) = self.fns.last() {
                    if let ExprKind::Block(b, l) = ast.expr(unparen(ast, c.body)).kind {
                        if l.get().is_none() { if let Some(t) = innermost_tail(ast, b) { self.lint_addr_of_local(t); } }
                    }
                }
                self.fns.pop();
                self.inside_call_args = saved.0;
                self.closure = saved.1;
                self.active = saved.2;
            }
            ExprKind::Async(..) => {
                let saved = std::mem::replace(&mut self.inside_call_args, false);
                let act = std::mem::replace(&mut self.active, true);
                self.fns.push(None);
                ast_visit::walk_expr(self, e);
                self.fns.pop();
                self.active = act;
                self.inside_call_args = saved;
            }
            ExprKind::ConstBlock(_) => {
                let act = std::mem::replace(&mut self.active, false);
                ast_visit::walk_expr(self, e);
                self.active = act;
            }
            ExprKind::Return(x) => {
                if let Some(x) = x.get() { self.lint_addr_of_local(x); }
                ast_visit::walk_expr(self, e);
            }
            _ => ast_visit::walk_expr(self, e),
        }
    }
}

// ---------------------------------------------------------------- prelude collisions

pub const RUST_2021_PRELUDE_COLLISIONS: Lint = lint::lint("rust_2021_prelude_collisions");
pub const RUST_2024_PRELUDE_COLLISIONS: Lint = lint::lint("rust_2024_prelude_collisions");
pub const ARRAY_INTO_ITER: Lint = lint::lint("array_into_iter");
pub const BOXED_SLICE_INTO_ITER: Lint = lint::lint("boxed_slice_into_iter");

impl<'a> L2<'a> {
    fn edition(&self) -> u16 { self.prog.files[self.body.file as usize].edition as u16 }

    /// Whether `d` comes from a standard library crate (`STDLIB_STABLE_CRATES`).
    fn in_std(&self, d: DefId) -> bool {
        matches!(self.prog.syms.str(self.prog.crates[self.prog.def(d).krate as usize].name), "core" | "std" | "alloc")
    }

    /// `lint_edition_dependent_dot_call` (rustc_hir_typeck method/prelude_edition_lints.rs).
    fn lint_prelude_dot_call(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, row) = self.ast.expr(e).kind else { return };
        let ed = self.edition();
        if ed >= 2024 { return; }
        let name = self.prog.files[self.body.file as usize].text(self.ast.row(row).name);
        if !matches!(name, "try_into" | "poll" | "into_future" | "into_iter") { return; }
        let Some(self_ty) = self.expr_ty(recv) else { return };
        let (lint, edition) = match name {
            "try_into" if ed < 2021 => (&RUST_2021_PRELUDE_COLLISIONS, "2021"),
            "poll" => {
                let TyKind::Adt(d, args, _) = self.kind(self_ty) else { return };
                if Some(d) != self.lang(LangItem::Pin) { return; }
                let Some(&a0) = self.tcx.tys.list(args).first() else { return };
                if !matches!(self.kind(a0), TyKind::Ref(true, ..)) { return; }
                (&RUST_2024_PRELUDE_COLLISIONS, "2024")
            }
            "into_future" => (&RUST_2024_PRELUDE_COLLISIONS, "2024"),
            "into_iter" => {
                if matches!(self.kind(self_ty), TyKind::Array(..)) && ed < 2021 {
                    (&ARRAY_INTO_ITER, "2021")
                } else if self.boxed_slice(self_ty) {
                    (&BOXED_SLICE_INTO_ITER, "2024")
                } else { return }
            }
            _ => return,
        };
        let Some(m) = self.body.methods.get(&e.0).copied() else { return };
        if self.in_std(m.def) { return; }
        let parent = self.prog.def(m.def).parent;
        // the receiver after the autoderefs is a trait object: an ObjectPick
        let mut t = self_ty;
        for _ in 0..m.derefs {
            t = match self.kind(t) {
                TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => i,
                TyKind::Adt(d, args, _) if Some(d) == self.lang(LangItem::OwnedBox) => match self.tcx.tys.list(args).first() { Some(&i) => i, None => break },
                _ => break,
            };
        }
        let object = matches!(self.kind(t), TyKind::Dyn(..));
        let inherent_like = object || (parent != crate::program::NO_DEF && self.prog.def(parent).kind == DefKind::Impl && self.tcx.impl_trait.get(&parent).is_none());
        if inherent_like {
            // avoid repeatedly adding unneeded `&*`s
            if m.derefs == 1 && m.autoref != 0 && matches!(self.kind(self_ty), TyKind::Ref(..)) { return; }
            // an inherent `self` method takes precedence over the trait's
            if m.derefs == 0 && m.autoref == 0 { return; }
        }
        let pos = self.ast.expr(recv).lo;
        // an inherent pick lints at the receiver, a trait pick at the whole call (whose span is not the macro-expanded receiver's)
        let check = if inherent_like { pos } else { self.ast.row(row).name.lo };
        if lint::external(self.prog, self.body.file, check) { return; }
        lint::emit_in(self.prog, self.body.def, &self.stack, lint, self.body.file, pos, &format!("trait method `{}` will become ambiguous in Rust {}", name, edition));
    }

    fn boxed_slice(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Adt(d, args, _) if Some(d) == self.lang(LangItem::OwnedBox) =>
                self.tcx.tys.list(args).first().is_some_and(|&i| matches!(self.kind(i), TyKind::Slice(_))),
            _ => false,
        }
    }

    /// `lint_fully_qualified_call_from_2018`.
    fn lint_prelude_path(&self, e: ExprId) {
        if self.edition() >= 2021 { return; }
        let Some(&item) = self.body.trait_item_paths.get(&e.0) else { return };
        let name = self.prog.syms.str(self.prog.def(item).name);
        if !matches!(name, "try_into" | "try_from" | "from_iter") { return; }
        if self.in_std(item) { return; }
        // `from_iter`: only when the type may implement `FromIterator`
        if name == "from_iter" {
            let fi = self.prog.defs.iter().position(|d| d.kind == DefKind::Trait && self.prog.syms.str(d.name) == "FromIterator").map(|i| DefId(i as u32)).filter(|&t| diag_name(self.prog, t).as_deref() == Some("FromIterator"));
            if let Some(tr) = fi {
                let self_ty = self.body.res_args.get(&e.0).and_then(|l| self.tcx.tys.list(*l).first().copied());
                if let Some(st) = self_ty {
                    let infcx = crate::infer::InferCtxt::new(self.tcx, self.env.mode, self.env.limit);
                    let var = infcx.next_ty_var();
                    let r = infcx.type_implements_trait(self.prog, self.env.param_env, crate::next_solver::TraitRef { def_id: tr, args: vec![crate::consts::GenericArg::Type(self.tcx.tys.erase_regions(st)), crate::consts::GenericArg::Type(var)] });
                    if !r.may_apply() { return; }
                }
            }
        }
        self.emit(&RUST_2021_PRELUDE_COLLISIONS, self.ast.expr(e).lo, &format!("trait-associated function `{}` will become ambiguous in Rust 2021", name));
    }
}

// ---------------------------------------------------------------- shadowed_into_iter

impl<'a> L2<'a> {
    /// `ShadowedIntoIter::check_expr` (rustc_lint shadowed_into_iter.rs).
    fn lint_shadowed_into_iter(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, row) = self.ast.expr(e).kind else { return };
        let Some(m) = self.body.methods.get(&e.0).copied() else { return };
        if Some(m.def) != self.lang(LangItem::IntoIterIntoIter) { return; }
        let Some(rt) = self.expr_ty(recv) else { return };
        // the receiver type and the targets of its adjustments
        let mut chain = vec![rt];
        let mut t = rt;
        for step in 0..m.derefs as usize {
            let ov = [e.0, recv.0, unparen(self.ast, recv).0].iter().any(|k| self.body.ov_deref(*k, step).is_some());
            t = match self.kind(t) {
                TyKind::Ref(_, i, _) if !ov => i,
                TyKind::Adt(d, args, _) if !ov && Some(d) == self.lang(LangItem::OwnedBox) => match self.tcx.tys.list(args).first() { Some(&i) => i, None => return },
                _ => match self.deref_target(t) { Some(i) => i, None => return },
            };
            chain.push(t);
        }
        if m.autoref != 0 {
            let r = self.tcx.tys.intern(TyKind::Ref(m.autoref == 2, t, crate::types::RegionId::ERASED));
            chain.push(r);
        }
        let is_ref_to = |ty: TyId, f: &dyn Fn(TyId) -> bool| matches!(self.kind(ty), TyKind::Ref(_, p, _) if f(p));
        let is_array = |ty: TyId| matches!(self.kind(ty), TyKind::Array(..));
        let last = *chain.last().unwrap();
        let (lint, target, edition, ufcs) = if is_ref_to(last, &is_array) {
            let Some(idx) = chain.iter().copied().take_while(|&ty| !is_ref_to(ty, &is_array)).position(|ty| is_array(ty)) else { return };
            (&ARRAY_INTO_ITER, "[T; N]", "2021", idx == 0)
        } else if is_ref_to(last, &|p| self.boxed_slice(p)) {
            let Some(idx) = chain.iter().copied().take_while(|&ty| !is_ref_to(ty, &|p| self.boxed_slice(p))).position(|ty| self.boxed_slice(ty)) else { return };
            (&BOXED_SLICE_INTO_ITER, "Box<[T]>", "2024", idx == 0)
        } else { return };
        let _ = ufcs;
        let pos = self.ast.row(row).name.lo;
        self.emit(lint, pos, &format!("this method call resolves to `<&{target} as IntoIterator>::into_iter` (due to backwards compatibility), but will resolve to `<{target} as IntoIterator>::into_iter` in Rust {edition}"));
    }
}

impl<'a> L2<'a> {
    /// `<t as Deref>::Target`, normalised.
    fn deref_target(&self, t: TyId) -> Option<TyId> {
        let td = self.lang(LangItem::Deref)?;
        let target = self.tcx.trait_assoc(self.prog, td, self.prog.syms.get("Target")?)?;
        let proj = self.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, crate::types::AliasTy {
            kind: crate::types::AliasTyKind::Projection { def_id: target },
            args: self.tcx.tys.mk_list(&[t]),
            regions: crate::types::RegionArgsId::EMPTY,
        }));
        self.tcx.try_normalize_erasing_regions(self.prog, self.env, proj).ok()
    }
}

// ---------------------------------------------------------------- if_let_rescope

pub const IF_LET_RESCOPE: Lint = lint::lint("if_let_rescope");

impl<'a> L2<'a> {
    fn norm(&self, t: TyId) -> TyId { self.tcx.try_normalize_erasing_regions(self.prog, self.env, t).unwrap_or(t) }

    /// `list_significant_drop_tys` (see significant_drop.rs).
    fn significant_drop_tys(&self, ty: TyId) -> Vec<TyId> {
        crate::significant_drop::SigDrop { prog: self.prog, tcx: self.tcx, env: self.env }.significant_drop_tys(ty)
    }

    /// `Expr::is_place_expr`.
    fn is_place_expr(&self, e: ExprId) -> bool {
        let e = unparen(self.ast, e);
        let allow_from = |b: ExprId| -> bool {
            // the base has a `Deref` adjustment: a reference or smart pointer
            self.expr_ty(b).is_some_and(|t| matches!(self.kind(t), TyKind::Ref(..)) || (matches!(self.kind(t), TyKind::Adt(d, ..) if Some(d) == self.lang(LangItem::OwnedBox)) && false))
                || self.body.ov_deref(b.0, 0).is_some() || self.body.ov_deref(e.0, 0).is_some()
        };
        match self.ast.expr(e).kind {
            ExprKind::Path(_) => match self.body.res.get(&e.0).copied() {
                Some(Res::Local(_)) | Some(Res::Err) => true,
                Some(Res::Def(d)) => self.prog.def(d).kind == DefKind::Static,
                None => true,
                _ => false,
            },
            ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Field(b, _) | ExprKind::TupleField(b, _) | ExprKind::Index(b, _) => allow_from(b) || self.is_place_expr(b),
            _ => false,
        }
    }

    /// `check_promoted_temp_with_drop`: whether `e` is an rvalue with a significant destructor.
    fn promoted_temp_with_drop(&self, e: ExprId) -> bool {
        if self.is_place_expr(e) { return false; }
        let Some(t) = self.expr_ty(e) else { return false };
        !self.significant_drop_tys(t).is_empty()
    }

    /// `FindSignificantDropper::visit_expr`: Break = true.
    fn find_significant_dropper(&self, e: ExprId) -> bool {
        let ast = self.ast;
        let e = unparen(ast, e);
        match ast.expr(e).kind {
            ExprKind::AddrOf(_, _, x) => return self.promoted_temp_with_drop(x) || self.find_significant_dropper(x),
            ExprKind::Index(x, i) => return self.promoted_temp_with_drop(x) || self.find_significant_dropper(x) || self.find_significant_dropper(i),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => return self.promoted_temp_with_drop(x) || self.find_significant_dropper(x),
            ExprKind::If(..) | ExprKind::Closure(_) | ExprKind::Async(..) | ExprKind::ConstBlock(_) | ExprKind::Lit(_) | ExprKind::Path(_)
            | ExprKind::Underscore | ExprKind::Continue(_) | ExprKind::While(_) => return false,
            ExprKind::Match(s, _) => return self.find_significant_dropper(s),
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Loop(b, _) => {
                return block_tail(ast, b).is_some_and(|t| self.find_significant_dropper(t));
            }
            ExprKind::MethodCall(r, row) => {
                // the receiver's autoref is a `Borrow` adjustment (a deref first ends the check)
                if let Some(m) = self.body.methods.get(&e.0) {
                    if m.derefs == 0 && m.autoref != 0 && self.promoted_temp_with_drop(r) { return true; }
                }
                if self.find_significant_dropper(r) { return true; }
                return ast.list(ast.row(row).args).iter().any(|&a| self.find_significant_dropper(a));
            }
            _ => {}
        }
        let mut found = false;
        let mut kids: Vec<ExprId> = Vec::new();
        match ast.expr(e).kind {
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Box(x) | ExprKind::Try(x) | ExprKind::Await(x) | ExprKind::Repeat(x, _) => kids.push(x),
            ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) => { kids.push(a); kids.push(b); }
            ExprKind::Call(f, args) => { kids.push(f); kids.extend_from_slice(ast.list(args)); }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => kids.extend_from_slice(ast.list(xs)),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                kids.extend(ast.list(s.fields).iter().map(|f| f.expr));
                if let Some(b) = s.base.get() { kids.push(b); }
            }
            ExprKind::Range(a, b, _) => { if let Some(x) = a.get() { kids.push(x); } if let Some(x) = b.get() { kids.push(x); } }
            ExprKind::For(row) => kids.push(ast.row(row).iter),
            ExprKind::Let(_, x) => kids.push(x),
            ExprKind::Break(_, x) | ExprKind::Return(x) | ExprKind::Yield(x) => { if let Some(x) = x.get() { kids.push(x); } }
            ExprKind::Mac(m) => {
                let m = *ast.row(m);
                if m.args != NO_MAC_ARGS { kids.extend_from_slice(ast.list(ast.mac_arg(m.args).exprs)); }
            }
            _ => {}
        }
        for k in kids { found |= self.find_significant_dropper(k); if found { break; } }
        found
    }

    /// `IfLetRescope::check_expr` / `probe_if_cascade`.
    fn lint_if_let_rescope(&mut self, e: ExprId) {
        if self.edition() >= 2024 { return; }
        let ast = self.ast;
        if !matches!(ast.expr(e).kind, ExprKind::If(..)) { return; }
        if self.skip_if.contains(&e.0) { return; }
        if self.stmt_like.contains(&e.0) && matches!(ast.expr(e).kind, ExprKind::If(_, _, els) if els.get().is_none()) { return; }
        let mut cur = e;
        let mut first: Option<u32> = None;
        loop {
            let ExprKind::If(cond, _, alt) = ast.expr(cur).kind else { break };
            self.skip_if.insert(cur.0);
            let cond = unparen(ast, cond);
            if let ExprKind::Let(_, init) = ast.expr(cond).kind {
                if first.is_none() && (self.promoted_temp_with_drop(init) || self.find_significant_dropper(init)) {
                    first = Some(ast.expr(cond).lo);
                }
            }
            match alt.get() {
                Some(a) => { let a = unparen(ast, a); if matches!(ast.expr(a).kind, ExprKind::If(..)) { cur = a; } else { break } }
                None => break,
            }
        }
        if let Some(pos) = first {
            self.emit(&IF_LET_RESCOPE, pos, "`if let` assigns a shorter lifetime since Edition 2024");
        }
    }
}

// ---------------------------------------------------------------- comparison lints (rustc_lint types.rs)

pub const UNUSED_COMPARISONS: Lint = lint::lint("unused_comparisons");
pub const INVALID_NAN_COMPARISONS: Lint = lint::lint("invalid_nan_comparisons");
pub const AMBIGUOUS_WIDE_POINTER_COMPARISONS: Lint = lint::lint("ambiguous_wide_pointer_comparisons");
pub const UNPREDICTABLE_FUNCTION_POINTER_COMPARISONS: Lint = lint::lint("unpredictable_function_pointer_comparisons");

#[derive(Clone, Copy, PartialEq)]
enum CmpOp { Bin(BinOp), Other }

fn is_comparison(op: BinOp) -> bool {
    matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
}

impl<'a> L2<'a> {
    /// `Expr::peel_borrows`.
    fn peel_borrows(&self, mut e: ExprId) -> ExprId {
        loop {
            e = unparen(self.ast, e);
            match self.ast.expr(e).kind {
                ExprKind::AddrOf(false, _, x) => e = x,
                _ => return e,
            }
        }
    }

    fn is_sized(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) | TyKind::Foreign(_) => false,
            TyKind::Adt(..) | TyKind::Param(_) | TyKind::Alias(..) => match self.lang(LangItem::Sized) {
                Some(s) => self.tcx.type_implements_trait(self.prog, self.env, crate::next_solver::TraitRef { def_id: s, args: vec![crate::consts::GenericArg::Type(t)] }).must_apply_modulo_regions(),
                None => true,
            },
            _ => true,
        }
    }

    /// `check_limits`: false when the comparison with a literal is decided by the type's range.
    fn check_limits(&self, op: BinOp, l: ExprId, r: ExprId) -> bool {
        let ast = self.ast;
        let (l, r) = (unparen(ast, l), unparen(ast, r));
        let (lit, expr, swap) = match (&ast.expr(l).kind, &ast.expr(r).kind) {
            (ExprKind::Lit(_), _) => (l, r, true),
            (_, ExprKind::Lit(_)) => (r, l, false),
            _ => return true,
        };
        // normalise so that the literal is on the right
        let norm = if swap { match op { BinOp::Lt => BinOp::Gt, BinOp::Le => BinOp::Ge, BinOp::Gt => BinOp::Lt, BinOp::Ge => BinOp::Le, o => o } } else { op };
        let ExprKind::Lit(LitKind::Int) = ast.expr(lit).kind else { return true };
        let text = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(ast.expr(lit).lo, ast.expr(lit).hi)).unwrap_or("");
        let Some(v) = crate::const_fold::parse_int_lit(text) else { return true };
        let Some(et) = self.expr_ty(expr) else { return true };
        let TyKind::Int(p) = self.kind(et) else { return true };
        let (bytes, signed) = crate::types::Types::int_info(p);
        if bytes == 0 { return true; }
        let is_valid = |v: i128, min: i128, max: i128| match norm {
            BinOp::Lt => v > min && v <= max,
            BinOp::Le => v >= min && v < max,
            BinOp::Gt => v >= min && v < max,
            BinOp::Ge => v > min && v <= max,
            _ => v >= min && v <= max,
        };
        if signed {
            // an unsigned suffix cannot type check against a signed operand
            let suffix = text.trim_start_matches(|c: char| c.is_ascii_hexdigit() || c == '_' || c == 'x' || c == 'o' || c == 'b');
            if suffix.starts_with('u') { return true; }
            let bits = bytes * 8;
            let (min, max) = if bits == 128 { (i128::MIN, i128::MAX) } else { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) };
            if v > i128::MAX as u128 { return true; }
            is_valid(v as i128, min, max)
        } else {
            let bits = bytes * 8;
            let max: u128 = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
            // u128 values compare through i128 only below 2^127
            match norm {
                BinOp::Lt => v > 0 && v <= max,
                BinOp::Le => v < max,
                BinOp::Gt => v < max,
                BinOp::Ge => v > 0 && v <= max,
                _ => v <= max,
            }
        }
    }

    fn cmp_op_of(&self, name: &str) -> Option<CmpOp> {
        Some(match name {
            "cmp_ord_max" | "cmp_ord_min" | "ord_cmp_method" | "cmp_partialord_cmp" => CmpOp::Other,
            "cmp_partialeq_eq" => CmpOp::Bin(BinOp::Eq),
            "cmp_partialeq_ne" => CmpOp::Bin(BinOp::Ne),
            "cmp_partialord_ge" => CmpOp::Bin(BinOp::Ge),
            "cmp_partialord_gt" => CmpOp::Bin(BinOp::Gt),
            "cmp_partialord_le" => CmpOp::Bin(BinOp::Le),
            "cmp_partialord_lt" => CmpOp::Bin(BinOp::Lt),
            _ => return None,
        })
    }

    /// `TypeLimits::check_expr` for comparisons and comparison calls.
    fn lint_comparisons(&self, e: ExprId) {
        let ast = self.ast;
        let pos = ast.expr(e).lo;
        match ast.expr(e).kind {
            ExprKind::Binary(op, l, r) if is_comparison(op) => {
                if !self.check_limits(op, l, r) {
                    self.emit(&UNUSED_COMPARISONS, pos, "comparison is useless due to type limits");
                } else {
                    self.lint_nan(e, op, l, r);
                    self.lint_wide_pointer(e, l, r);
                    self.lint_fn_pointer(e, CmpOp::Bin(op), l, r);
                }
            }
            ExprKind::Call(f, args) if ast.list(args).len() == 2 => {
                let f = unparen(ast, f);
                if !matches!(ast.expr(f).kind, ExprKind::Path(_)) { return; }
                let Some(Res::Def(d)) = self.body.res.get(&f.0).copied() else { return };
                let Some(op) = diag_name(self.prog, d).and_then(|n| self.cmp_op_of(&n)) else { return };
                let a = ast.list(args);
                self.lint_wide_pointer(e, a[0], a[1]);
                self.lint_fn_pointer(e, op, a[0], a[1]);
            }
            ExprKind::MethodCall(l, row) if ast.list(ast.row(row).args).len() == 1 => {
                let Some(m) = self.body.methods.get(&e.0) else { return };
                let Some(op) = diag_name(self.prog, m.def).and_then(|n| self.cmp_op_of(&n)) else { return };
                self.lint_wide_pointer(e, l, ast.list(ast.row(row).args)[0]);
                self.lint_fn_pointer(e, op, l, ast.list(ast.row(row).args)[0]);
            }
            _ => {}
        }
    }

    fn lint_nan(&self, e: ExprId, op: BinOp, l: ExprId, r: ExprId) {
        let is_nan = |x: ExprId| -> bool {
            let x = self.peel_borrows(peel_blocks(self.ast, x));
            if !matches!(self.ast.expr(x).kind, ExprKind::Path(_)) { return false; }
            let Some(Res::Def(d)) = self.body.res.get(&x.0).copied() else { return false };
            matches!(diag_name(self.prog, d).as_deref(), Some("f16_nan" | "f32_nan" | "f64_nan" | "f128_nan"))
        };
        let pos = self.ast.expr(e).lo;
        match op {
            BinOp::Eq | BinOp::Ne if is_nan(l) || is_nan(r) => self.emit(&INVALID_NAN_COMPARISONS, pos, "incorrect NaN comparison, NaN cannot be directly compared to itself"),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if is_nan(l) || is_nan(r) => self.emit(&INVALID_NAN_COMPARISONS, pos, "incorrect NaN comparison, NaN is not orderable"),
            _ => {}
        }
    }

    fn lint_wide_pointer(&self, e: ExprId, l: ExprId, r: ExprId) {
        let ptr_unsized = |mut ty: TyId| -> bool {
            while let TyKind::Ref(_, i, _) = self.kind(ty) { ty = i; }
            let inner = match self.kind(ty) {
                TyKind::Ptr(_, i) => i,
                TyKind::Adt(d, args, _) if diag_name(self.prog, d).as_deref() == Some("NonNull") => match self.tcx.tys.list(args).first() { Some(&i) => i, None => return false },
                _ => return false,
            };
            !self.is_sized(inner)
        };
        let (l, r) = (self.peel_borrows(l), self.peel_borrows(r));
        let (Some(lt), Some(rt)) = (self.expr_ty(l), self.expr_ty(r)) else { return };
        if !ptr_unsized(lt) || !ptr_unsized(rt) { return; }
        self.emit(&AMBIGUOUS_WIDE_POINTER_COMPARISONS, self.ast.expr(e).lo, "ambiguous wide pointer comparison, the comparison includes metadata which may not be expected");
    }

    fn lint_fn_pointer(&self, e: ExprId, _op: CmpOp, l: ExprId, r: ExprId) {
        let peel = |mut ty: TyId| -> TyId { while let TyKind::Ref(_, i, _) = self.kind(ty) { ty = i; } ty };
        let (l, r) = (self.peel_borrows(l), self.peel_borrows(r));
        let (Some(lt), Some(rt)) = (self.expr_ty(l), self.expr_ty(r)) else { return };
        let (lt, rt) = (peel(lt), peel(rt));
        let is_fn = |t: TyId| matches!(self.kind(t), TyKind::FnDef(..) | TyKind::FnPtr(..));
        if is_fn(lt) && is_fn(rt) {
        } else if let (TyKind::Adt(ld, la, _), TyKind::Adt(rd, ra, _)) = (self.kind(lt), self.kind(rt)) {
            let opt = self.lang(LangItem::Option);
            if Some(ld) != opt || Some(rd) != opt { return; }
            let (Some(&a0), Some(&b0)) = (self.tcx.tys.list(la).first(), self.tcx.tys.list(ra).first()) else { return };
            if !is_fn(a0) || !is_fn(b0) { return; }
        } else { return; }
        self.emit(&UNPREDICTABLE_FUNCTION_POINTER_COMPARISONS, self.ast.expr(e).lo, "function pointer comparisons do not produce meaningful results since their addresses are not guaranteed to be unique");
    }
}

// ---------------------------------------------------------------- closure_returning_async_block

pub const CLOSURE_RETURNING_ASYNC_BLOCK: Lint = lint::lint("closure_returning_async_block");

impl<'a> L2<'a> {
    /// `AsyncClosureUsage::check_expr` (rustc_lint async_closures.rs).
    fn lint_closure_returning_async_block(&self, e: ExprId) {
        let ExprKind::Closure(row) = self.ast.expr(e).kind else { return };
        let c = *self.ast.row(row);
        if c.is_async { return; }
        let mut body = c.body;
        // only blocks without statements are peeled
        loop {
            match self.ast.expr(body).kind {
                ExprKind::Paren(x) => body = x,
                ExprKind::Block(b, l) if l.get().is_none() && self.ast.stmts(b).len() == 1 && block_tail(self.ast, b).is_some() => body = block_tail(self.ast, b).unwrap(),
                _ => break,
            }
        }
        if matches!(self.ast.expr(body).kind, ExprKind::Async(..)) {
            self.emit(&CLOSURE_RETURNING_ASYNC_BLOCK, self.ast.expr(e).lo, "closure returning async block can be made into an async closure");
        }
    }
}

// ---------------------------------------------------------------- ptr_nulls (rustc_lint ptr_nulls.rs)

pub const USELESS_PTR_NULL_CHECKS: Lint = lint::lint("useless_ptr_null_checks");
pub const INVALID_NULL_ARGUMENTS: Lint = lint::lint("invalid_null_arguments");

impl<'a> L2<'a> {
    fn call_def(&self, f: ExprId) -> Option<DefId> {
        let f = unparen(self.ast, f);
        if !matches!(self.ast.expr(f).kind, ExprKind::Path(_)) { return None; }
        match self.body.res.get(&f.0).copied() { Some(Res::Def(d)) => Some(d), _ => None }
    }

    /// `useless_check`: the message when `e` is a pointer made from something never null.
    fn useless_null_check(&self, e: ExprId) -> Option<String> {
        let ast = self.ast;
        let mut e = e;
        let mut cast = false;
        loop {
            e = peel_blocks(ast, e);
            match ast.expr(e).kind {
                ExprKind::MethodCall(_, row) if ast.list(ast.row(row).args).is_empty() => {
                    if let Some(m) = self.body.methods.get(&e.0) {
                        if has_def_attr(self.prog, m.def, "rustc_never_returns_null_ptr") {
                            let n = self.prog.syms.str(self.prog.def(m.def).name);
                            return Some(format!("returned pointer of `{}` call is never null, so checking it for null will always return false", n));
                        }
                    }
                }
                ExprKind::Call(f, _) => {
                    if let Some(d) = self.call_def(f) {
                        if has_def_attr(self.prog, d, "rustc_never_returns_null_ptr") {
                            let n = self.prog.syms.str(self.prog.def(d).name);
                            return Some(format!("returned pointer of `{}` call is never null, so checking it for null will always return false", n));
                        }
                    }
                }
                _ => {}
            }
            let next = match ast.expr(e).kind {
                ExprKind::Cast(x, t) if matches!(ast.ty(t), Ty::Ptr(..)) => Some(x),
                ExprKind::MethodCall(x, row) if ast.list(ast.row(row).args).is_empty()
                    && self.body.methods.get(&e.0).and_then(|m| diag_name(self.prog, m.def)).is_some_and(|n| n == "ptr_cast" || n == "ptr_cast_mut" || n == "const_ptr_cast") => Some(x),
                _ => None,
            };
            match next {
                Some(x) => { cast = true; e = x; }
                None => {
                    if !cast { return None; }
                    let t = self.expr_ty(e)?;
                    return match self.kind(t) {
                        TyKind::FnDef(..) | TyKind::FnPtr(..) => Some("function pointers are not nullable, so checking them for null will always return false".into()),
                        TyKind::Ref(..) => Some("references are not nullable, so checking them for null will always return false".into()),
                        _ => None,
                    };
                }
            }
        }
    }

    /// `is_null_ptr`: a null pointer constant behind casts and bindings.
    fn is_null_ptr_arg(&self, e: ExprId) -> bool {
        let e = self.peel_casts(e);
        match self.ast.expr(e).kind {
            ExprKind::Call(f, args) if self.ast.list(args).is_empty() =>
                self.call_def(f).and_then(|d| diag_name(self.prog, d)).is_some_and(|n| n == "ptr_null" || n == "ptr_null_mut"),
            ExprKind::Lit(LitKind::Int) => {
                let x = self.ast.expr(e);
                let t = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(x.lo, x.hi)).unwrap_or("");
                crate::const_fold::parse_int_lit(t) == Some(0)
            }
            _ => false,
        }
    }

    fn lint_ptr_nulls(&self, e: ExprId) {
        let ast = self.ast;
        let pos = ast.expr(e).lo;
        let is_null_fn = |d: DefId| matches!(diag_name(self.prog, d).as_deref(), Some("ptr_const_is_null" | "ptr_is_null"));
        match ast.expr(e).kind {
            ExprKind::Call(f, args) => {
                let Some(d) = self.call_def(f) else { return };
                let a = ast.list(args);
                if a.len() == 1 && is_null_fn(d) {
                    if let Some(msg) = self.useless_null_check(a[0]) { self.emit(&USELESS_PTR_NULL_CHECKS, pos, &msg); return; }
                }
                let Some(name) = diag_name(self.prog, d) else { return };
                let (idx, zsts): (&[usize], bool) = match name.as_str() {
                    "ptr_read" | "ptr_read_unaligned" | "ptr_replace" | "ptr_write" | "ptr_write_bytes" | "ptr_write_unaligned" => (&[0], true),
                    "slice_from_raw_parts" | "slice_from_raw_parts_mut" => (&[0], false),
                    "ptr_copy" | "ptr_copy_nonoverlapping" | "ptr_swap" | "ptr_swap_nonoverlapping" => (&[0, 1], true),
                    _ => return,
                };
                for &i in idx {
                    let Some(&arg) = a.get(i) else { continue };
                    if !self.is_null_ptr_arg(arg) { continue; }
                    let Some(t) = self.expr_ty(arg) else { continue };
                    let TyKind::Ptr(_, pointee) = self.kind(t) else { continue };
                    if zsts && self.layout_1zst(pointee, 0) == Some(true) { break; }
                    self.emit(&INVALID_NULL_ARGUMENTS, pos, "calling this function with a null pointer is undefined behavior, even if the result of the function is unused");
                }
            }
            ExprKind::MethodCall(r, _) => {
                let Some(m) = self.body.methods.get(&e.0) else { return };
                if is_null_fn(m.def) {
                    if let Some(msg) = self.useless_null_check(r) { self.emit(&USELESS_PTR_NULL_CHECKS, pos, &msg); }
                }
            }
            ExprKind::Binary(BinOp::Eq, l, r) => {
                let (to_check, msg) = if let Some(m) = self.useless_null_check(l) { (r, m) } else if let Some(m) = self.useless_null_check(r) { (l, m) } else { return };
                let tc = peel_blocks(ast, to_check);
                match ast.expr(tc).kind {
                    ExprKind::Cast(x, _) => {
                        let x = unparen(ast, x);
                        if let ExprKind::Lit(LitKind::Int) = ast.expr(x).kind {
                            let xe = ast.expr(x);
                            let t = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(xe.lo, xe.hi)).unwrap_or("");
                            if crate::const_fold::parse_int_lit(t) == Some(0) { self.emit(&USELESS_PTR_NULL_CHECKS, pos, &msg); }
                        }
                    }
                    ExprKind::Call(f, args) if ast.list(args).is_empty() => {
                        if self.call_def(f).and_then(|d| diag_name(self.prog, d)).is_some_and(|n| n == "ptr_null" || n == "ptr_null_mut") {
                            self.emit(&USELESS_PTR_NULL_CHECKS, pos, &msg);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- unused_qualifications

pub const UNUSED_QUALIFICATIONS: Lint = lint::lint("unused_qualifications");

impl<'a> L2<'a> {
    /// rustc_resolve late.rs `lint_unused_qualifications` for a value path: the longest qualified
    /// prefix whose last segment names the same item when written alone in the current scope.
    fn lint_unused_qualifications(&self, e: ExprId) {
        let ast = self.ast;
        let ExprKind::Path(p) = ast.expr(e).kind else { return };
        if p.global() || p.has_qself() { return; }
        let segs = ast.segs(p.segs);
        if segs.len() < 2 { return; }
        let pf = &self.prog.files[self.body.file as usize];
        if lint::external(self.prog, self.body.file, p.lo) || p.lo >= pf.src_len { return; }
        let module = if self.module == u32::MAX { self.tcx.item_module(self.prog, self.body.def) } else { self.module };
        let Some(Res::Def(target)) = self.body.res.get(&e.0).copied() else { return };
        // segments after the first one with generic arguments are not considered
        let end = segs.iter().position(|s| ast.gargs(s.args()).is_some()).map_or(segs.len(), |i| i + 1);
        for i in (1..end).rev() {
            let last = i + 1 == segs.len();
            let seg = &segs[i];
            if seg.kind() != SegKind::Ident { continue; }
            let res = if last {
                target
            } else {
                let prefix: Vec<(SegKind, crate::program::Sym)> = segs[..=i].iter().map(|s| (s.kind(), self.prog.syms.intern_late(pf.text(s.name)))).collect();
                match self.prog.resolve_mod_path(module, false, &prefix, false, false) { Some(d) => d, None => continue }
            };
            let name = self.prog.syms.intern_late(pf.text(seg.name));
            if self.prog.lookup_in_scope(module, name, !last) == Some(res) {
                self.emit(&UNUSED_QUALIFICATIONS, p.lo, "unnecessary qualification");
                return;
            }
        }
    }
}

// ---------------------------------------------------------------- reference_casting.rs

pub const INVALID_REFERENCE_CASTING: Lint = lint::lint("invalid_reference_casting");
pub const MUTABLE_TRANSMUTES: Lint = lint::lint("mutable_transmutes");

#[derive(Clone, Copy, PartialEq, Eq)]
enum RcPat { Borrow(bool), Assign }

impl<'a> L2<'a> {
    /// rustc_lint builtin.rs MutableTransmutes: a use of `transmute` from `&T` to `&mut T`.
    fn lint_mutable_transmutes(&self, e: ExprId) {
        let Some(d) = self.call_def(e) else { return };
        if self.prog.def(d).kind != DefKind::Fn || diag_name(self.prog, d).as_deref() != Some("transmute") { return; }
        let targs = self.body.res_args.get(&e.0).map(|l| self.tcx.tys.list(*l).to_vec()).unwrap_or_default();
        let (Some(&from), Some(&to)) = (targs.first(), targs.get(1)) else { return };
        if let (TyKind::Ref(false, ..), TyKind::Ref(true, ..)) = (self.kind(from), self.kind(to)) {
            self.emit(&MUTABLE_TRANSMUTES, self.ast.expr(e).lo, "transmuting &T to &mut T is undefined behavior, even if the reference is unused, consider instead using an UnsafeCell");
        }
    }

    /// `borrow_or_assign`: `&(mut) *<e>`, `*<e> = ..`, `*<e> += ..`, `ptr::write(<e>, ..)`.
    fn borrow_or_assign(&self, e: ExprId) -> Option<(ExprId, RcPat)> {
        let ast = self.ast;
        let (inner, pat) = match ast.expr(e).kind {
            ExprKind::AddrOf(_, m, x) => (x, RcPat::Borrow(m)),
            ExprKind::Assign(x, _) | ExprKind::AssignOp(_, x, _) => (x, RcPat::Assign),
            ExprKind::Call(f, args) => {
                let a = ast.list(args);
                if a.len() != 2 { return None; }
                let d = self.call_def(f)?;
                if !matches!(diag_name(self.prog, d).as_deref(), Some("ptr_write" | "ptr_write_volatile" | "ptr_write_unaligned")) { return None; }
                return Some((a[0], RcPat::Assign));
            }
            _ => return None,
        };
        // the paren-free operand `*<e>`
        match ast.expr(inner).kind {
            ExprKind::Unary(UnOp::Deref, x) => Some((x, pat)),
            _ => None,
        }
    }

    /// reference_casting.rs `InvalidReferenceCasting::check_expr`.
    fn lint_reference_casting(&self, e: ExprId) {
        let Some((x, pat)) = self.borrow_or_assign(e) else { return };
        let init = self.expr_or_init(x);
        let pos = self.ast.expr(e).lo;
        let peeled = self.peel_casts_flag(init);
        if matches!(pat, RcPat::Borrow(true) | RcPat::Assign) && self.is_cast_from_ref_to_mut_ptr(init, peeled).is_some() {
            let msg = if pat == RcPat::Assign {
                "assigning to `&T` is undefined behavior, consider using an `UnsafeCell`"
            } else {
                "casting `&T` to `&mut T` is undefined behavior, even if the reference is unused, consider instead using an `UnsafeCell`"
            };
            self.emit(&INVALID_REFERENCE_CASTING, pos, msg);
        }
        if self.is_cast_to_bigger_memory_layout(init, peeled) {
            self.emit(&INVALID_REFERENCE_CASTING, pos, "casting references to a bigger memory layout than the backing allocation is undefined behavior, even if the reference is unused");
        }
    }

    fn rc_layoutable(&self, t: TyId) -> bool {
        let f = self.tcx.tys.flags(t);
        !f.intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_INFER | crate::types::TypeFlags::HAS_ALIAS | crate::types::TypeFlags::HAS_ERROR)
    }

    /// Some(the pointee has interior mutability) when `orig` is a `*mut` made from a `&T`.
    fn is_cast_from_ref_to_mut_ptr(&self, orig: ExprId, peeled: (ExprId, bool)) -> Option<bool> {
        let end_ty = self.expr_ty(orig)?;
        if !matches!(self.kind(end_ty), TyKind::Ptr(true, _)) { return None; }
        let (e, need_check_freeze) = peeled;
        let start_ty = self.expr_ty(e)?;
        let TyKind::Ref(false, inner, _) = self.kind(start_ty) else { return None };
        // `has_concrete_skeleton`: the outermost type is no parameter, inference variable or error
        let skeleton = !matches!(self.kind(inner), TyKind::Param(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error);
        let interior = skeleton && !self.tcx.is_freeze(self.prog, crate::tcx::TypingEnv::fully_monomorphized(self.prog), inner);
        (!need_check_freeze || !interior).then_some(interior)
    }

    fn is_cast_to_bigger_memory_layout(&self, orig: ExprId, peeled: (ExprId, bool)) -> bool {
        let ast = self.ast;
        let Some(end_ty) = self.expr_ty(orig) else { return false };
        let TyKind::Ptr(_, inner_end) = self.kind(end_ty) else { return false };
        let (e, _) = peeled;
        let Some(start_ty) = self.expr_ty(e) else { return false };
        let TyKind::Ref(_, inner_start, _) = self.kind(start_ty) else { return false };
        let mut e_alloc = self.expr_or_init(e);
        if let ExprKind::AddrOf(_, _, x) = ast.expr(e_alloc).kind { e_alloc = x; }
        if matches!(ast.expr(e_alloc).kind, ExprKind::Index(..) | ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Unary(UnOp::Deref, _)) { return false; }
        let Some(alloc_ty) = self.expr_ty(e_alloc) else { return false };
        if matches!(self.kind(alloc_ty), TyKind::Ptr(..) | TyKind::Ref(..)) { return false; }
        if !(self.rc_layoutable(alloc_ty) && self.rc_layoutable(inner_start) && self.rc_layoutable(inner_end)) { return false; }
        let lcx = crate::layout::LayoutCx::new(self.prog);
        let lay = crate::layout::Layouts::new();
        if lay.is_unsized(&lcx, self.tcx, inner_start) { return false; }
        let from = lay.of(&lcx, self.tcx, inner_start).size;
        let alloc = lay.of(&lcx, self.tcx, alloc_ty).size;
        // an unsized target has no size to compare
        if lay.is_unsized(&lcx, self.tcx, inner_end) { return false; }
        let to = lay.of(&lcx, self.tcx, inner_end).size;
        to > from && to > alloc
    }
}

// ---------------------------------------------------------------- transmute.rs

pub const PTR_TO_INTEGER_TRANSMUTE_IN_CONSTS: Lint = lint::lint("ptr_to_integer_transmute_in_consts");
pub const UNNECESSARY_TRANSMUTES: Lint = lint::lint("unnecessary_transmutes");
pub const INTEGER_TO_PTR_TRANSMUTES: Lint = lint::lint("integer_to_ptr_transmutes");

impl<'a> L2<'a> {
    /// `CheckTransmutes::check_expr` (rustc_lint transmute.rs).
    fn lint_transmutes(&self, e: ExprId) {
        let ast = self.ast;
        let ExprKind::Call(callee, args) = ast.expr(e).kind else { return };
        let a = ast.list(args);
        if a.len() != 1 { return; }
        let Some(d) = self.call_def(callee) else { return };
        if self.prog.def(d).kind != DefKind::Fn || diag_name(self.prog, d).as_deref() != Some("transmute") { return; }
        let callee = unparen(ast, callee);
        let targs = self.body.res_args.get(&callee.0).map(|l| self.tcx.tys.list(*l).to_vec()).unwrap_or_default();
        let (mut src, mut dst) = (targs.first().copied(), targs.get(1).copied());
        let bad = |t: Option<TyId>| t.is_none_or(|t| self.tcx.tys.has_infer(t));
        if bad(src) { src = self.expr_ty(a[0]); }
        if bad(dst) { dst = self.expr_ty(e); }
        let (Some(src), Some(dst)) = (src, dst) else { return };
        let pos = ast.expr(e).lo;
        let owner = self.prog.def(self.body.def);
        // the body is a const fn or an associated constant
        let const_fn = owner.kind == DefKind::AssocFn || owner.kind == DefKind::Fn;
        let is_const_fn = const_fn && matches!(ast.item(owner.item).kind, ItemKind::Fn(s, _, _) if ast.fn_sig(s).konst);
        let const_ctx = is_const_fn || matches!(owner.kind, DefKind::Const | DefKind::AssocConst | DefKind::Static);
        // ptr -> integer in const fns and associated constants
        if (is_const_fn || owner.kind == DefKind::AssocConst) && matches!(self.kind(src), TyKind::Ptr(..)) && matches!(self.kind(dst), TyKind::Int(_)) {
            self.emit(&PTR_TO_INTEGER_TRANSMUTE_IN_CONSTS, pos, "pointers cannot be transmuted to integers during const eval");
        }
        // unnecessary transmutes
        let u8_array = |t: TyId| matches!(self.kind(t), TyKind::Array(el, _) if matches!(self.kind(el), TyKind::Int(crate::program::Prim::U8)));
        let (sk, dk) = (self.kind(src), self.kind(dst));
        let is_int = |k: &TyKind| matches!(k, TyKind::Int(_));
        let prim = |k: &TyKind| matches!(k, TyKind::Int(_) | TyKind::Float(_));
        let unsigned = |k: &TyKind| matches!(k, TyKind::Int(p) if !crate::types::Types::int_info(*p).1);
        let signed = |k: &TyKind| matches!(k, TyKind::Int(p) if crate::types::Types::int_info(*p).1);
        let is_u32 = |k: &TyKind| matches!(k, TyKind::Int(crate::program::Prim::U32));
        let is_i32 = |k: &TyKind| matches!(k, TyKind::Int(crate::program::Prim::I32));
        let size = |k: &TyKind| matches!(k, TyKind::Int(crate::program::Prim::Usize | crate::program::Prim::Isize));
        let unnecessary = (u8_array(src) && prim(&dk))
            || (prim(&sk) && u8_array(dst))
            || (matches!(sk, TyKind::Char) && (is_u32(&dk) || is_i32(&dk)))
            || ((is_u32(&sk) || is_i32(&sk)) && matches!(dk, TyKind::Char))
            || (unsigned(&sk) && signed(&dk)) || (signed(&sk) && unsigned(&dk))
            || (matches!(sk, TyKind::Float(_)) && is_int(&dk))
            || (is_int(&sk) && matches!(dk, TyKind::Float(_)))
            || (matches!(sk, TyKind::Bool) && is_int(&dk));
        let _ = (size, const_ctx);
        if unnecessary {
            self.emit(&UNNECESSARY_TRANSMUTES, pos, "unnecessary transmute");
        }
        // integer -> pointer
        if is_int(&sk) {
            let inner = match dk { TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => i, _ => return };
            if let ExprKind::Lit(LitKind::Int) = ast.expr(unparen(ast, a[0])).kind {
                let x = ast.expr(unparen(ast, a[0]));
                let t = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(x.lo, x.hi)).unwrap_or("");
                if crate::const_fold::parse_int_lit(t) == Some(0) { return; }
            }
            if self.layout_1zst(inner, 0) != Some(false) { return; }
            self.emit(&INTEGER_TO_PTR_TRANSMUTES, pos, "transmuting an integer to a pointer creates a pointer without provenance");
        }
    }
}

// ---------------------------------------------------------------- unsafe_code (rustc_lint builtin.rs UnsafeCode)

/// The first attribute named `name` of an item: the position of its `#`.
fn attr_pos(prog: &Program, file: u32, attrs: &[Attr], name: &str) -> Option<u32> {
    let a = attrs.iter().find(|a| lint::attr_path(prog, file, a) == name.as_bytes())?;
    let p = prog.src(file);
    let at = a.toks.lo as usize;
    Some(p.tok(at.saturating_sub(if a.inner { 3 } else { 2 })).lo)
}

/// The item-level `unsafe_code` checks over the definitions of the checked crates marked in `own`.
pub fn check_items(prog: &Program, own: &[bool]) {
    const NOTE_MANGLE: &str = "";
    let _ = NOTE_MANGLE;
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if !own.get(d.krate as usize).copied().unwrap_or(false) { continue; }
        if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Static | DefKind::Trait | DefKind::Impl) { continue; }
        let id = DefId(i as u32);
        let ast = &prog.files[d.file as usize].ast;
        let item = *ast.item(d.item);
        let attrs = ast.list(item.attrs);
        let emit = |pos: u32, msg: &str| {
            if lint::external(prog, d.file, pos) { return; }
            lint::emit(prog, id, &lint::UNSAFE_CODE, d.file, pos, msg);
        };
        let in_impl = d.parent != crate::program::NO_DEF && prog.def(d.parent).kind == DefKind::Impl;
        match item.kind {
            ItemKind::Trait(r) => if ast.row(r).unsafe_ { emit(item.lo, "declaration of an `unsafe` trait"); },
            ItemKind::Impl(r) => {
                let imp = *ast.row(r);
                if imp.unsafe_ && imp.trait_.is_some() { emit(item.lo, "implementation of an `unsafe` trait"); }
            }
            ItemKind::Fn(sig, body, _) => {
                let sig = *ast.fn_sig(sig);
                if d.kind == DefKind::AssocFn && in_impl {
                    if let Some(p) = attr_pos(prog, d.file, attrs, "no_mangle") { emit(p, "declaration of a `no_mangle` method"); }
                    if let Some(p) = attr_pos(prog, d.file, attrs, "export_name") { emit(p, "declaration of a method with `export_name`"); }
                } else if d.kind == DefKind::Fn {
                    if let Some(p) = attr_pos(prog, d.file, attrs, "no_mangle") { emit(p, "declaration of a `no_mangle` function"); }
                    if let Some(p) = attr_pos(prog, d.file, attrs, "export_name") { emit(p, "declaration of a function with `export_name`"); }
                    if let Some(p) = attr_pos(prog, d.file, attrs, "link_section") { emit(p, "declaration of a function with `link_section`"); }
                }
                if sig.unsafe_ {
                    let has_body = body.is_some() || sig.lazy_body.is_some();
                    match d.kind {
                        DefKind::Fn => emit(item.lo, "declaration of an `unsafe` function"),
                        DefKind::AssocFn if !has_body => emit(item.lo, "declaration of an `unsafe` method"),
                        DefKind::AssocFn => emit(item.lo, "implementation of an `unsafe` method"),
                        _ => {}
                    }
                }
            }
            ItemKind::Static(..) => {
                if let Some(p) = attr_pos(prog, d.file, attrs, "no_mangle") { emit(p, "declaration of a `no_mangle` static"); }
                if let Some(p) = attr_pos(prog, d.file, attrs, "export_name") { emit(p, "declaration of a static with `export_name`"); }
                if let Some(p) = attr_pos(prog, d.file, attrs, "link_section") { emit(p, "declaration of a static with `link_section`"); }
            }
            _ => {}
        }
    }
}

impl<'a> L2<'a> {
    fn lint_unsafe_block(&self, e: ExprId) {
        if let ExprKind::Unsafe(_) = self.ast.expr(e).kind {
            self.emit(&lint::UNSAFE_CODE, self.ast.expr(e).lo, "usage of an `unsafe` block");
        }
    }
}

// ---------------------------------------------------------------- internal_eq_trait_method_impls

pub const INTERNAL_EQ_TRAIT_METHOD_IMPLS: Lint = lint::lint("internal_eq_trait_method_impls");

/// `InternalEqTraitMethodImpls::check_impl_item`: a hand-written `Eq::assert_receiver_is_total_eq`.
pub fn check_eq_impls(prog: &Program, tcx: &Tcx, own: &[bool]) {
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if d.kind != DefKind::AssocFn || !own.get(d.krate as usize).copied().unwrap_or(false) || d.parent == crate::program::NO_DEF { continue; }
        if prog.def(d.parent).kind != DefKind::Impl || prog.syms.str(d.name) != "assert_receiver_is_total_eq" { continue; }
        let Some(&tr) = tcx.impl_trait.get(&d.parent) else { continue };
        let item = prog.traits[prog.def(tr).sub as usize].iter().copied().find(|&it| prog.def(it).name == d.name);
        if !item.is_some_and(|it| diag_name(prog, it).as_deref() == Some("assert_receiver_is_total_eq")) { continue; }
        let ast = &prog.files[d.file as usize].ast;
        let pos = ast.item(d.item).lo;
        if lint::external(prog, d.file, pos) { continue; }
        lint::emit(prog, DefId(i as u32), &INTERNAL_EQ_TRAIT_METHOD_IMPLS, d.file, pos, "`Eq::assert_receiver_is_total_eq` should never be implemented by hand");
    }
}

// ---------------------------------------------------------------- deprecated: `use` declarations and signatures

use crate::lint_late::Dp;

fn use_leaves(ast: &Ast, prog: &Program, file: u32, t: &UseTree, prefix: &mut Vec<(crate::ast::SegKind, crate::program::Sym)>, out: &mut Vec<Vec<(crate::ast::SegKind, crate::program::Sym)>>) {
    let sym = |seg: &PathSeg| prog.syms.get(prog.files[file as usize].text(seg.name)).map(|s| (seg.kind(), s));
    match *t {
        UseTree::Name(seg, _) => {
            if let Some(s) = sym(&seg) { prefix.push(s); out.push(prefix.clone()); prefix.pop(); }
        }
        UseTree::Path(seg, sub) => {
            if let Some(s) = sym(&seg) {
                prefix.push(s);
                use_leaves(ast, prog, file, ast.use_tree(sub), prefix, out);
                prefix.pop();
            }
        }
        UseTree::Group(l) => for c in ast.list(l) { use_leaves(ast, prog, file, c, prefix, out); },
        UseTree::Glob => {}
    }
}

fn first_use_pos(ast: &Ast, t: &UseTree) -> Option<u32> {
    match *t {
        UseTree::Name(seg, _) | UseTree::Path(seg, _) => Some(seg.name.lo),
        UseTree::Group(l) => ast.list(l).first().and_then(|c| first_use_pos(ast, c)),
        UseTree::Glob => None,
    }
}

/// `use` declarations of deprecated items (rustc stability `visit_path` of the use paths).
pub fn check_use_deprecations(prog: &Program, tcx: &Tcx, own: &[bool]) {
    let dp = Dp { prog, tcx };
    for (m, md) in prog.mods.iter().enumerate() {
        if !own.get(md.krate as usize).copied().unwrap_or(false) || md.lexical_parent.is_some() { continue; }
        let (file, items) = crate::lint_dead::module_items(prog, m as u32);
        let ast = &prog.files[file as usize].ast;
        for &it in &items {
            let item = ast.item(it);
            let ItemKind::Use(row) = item.kind else { continue };
            let u = *ast.row(row);
            let Some(pos) = first_use_pos(ast, &u.tree) else { continue };
            if lint::external(prog, file, pos) { continue; }
            let mut leaves = Vec::new();
            use_leaves(ast, prog, file, &u.tree, &mut Vec::new(), &mut leaves);
            let attrs = ast.list(item.attrs).to_vec();
            for segs in leaves {
                if segs.len() < 2 { continue; }
                let Some(parent) = prog.resolve_mod_path(m as u32, u.global, &segs[..segs.len() - 1], false, true) else { continue };
                let last = segs[segs.len() - 1].1;
                for ns_type in [true, false] {
                    let Some(d) = prog.lookup_in_container(parent, last, ns_type) else { continue };
                    if let Some((future, msg)) = dp.message(d, md.def) {
                        lint::emit_in(prog, md.def, &attrs, if future { &crate::lint_late::DEPRECATED_IN_FUTURE } else { &crate::lint_late::DEPRECATED }, file, pos, &msg);
                    }
                }
            }
        }
    }
}

/// Deprecated items named in item signatures (rustc stability `visit_path` of the types and bounds).
pub fn check_signature_deprecations(prog: &Program, tcx: &Tcx, own: &[bool]) {
    let dp = Dp { prog, tcx };
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if !own.get(d.krate as usize).copied().unwrap_or(false) { continue; }
        if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::Trait | DefKind::Impl | DefKind::Const | DefKind::AssocConst | DefKind::Static) { continue; }
        let ast = &prog.files[d.file as usize].ast;
        let item = *ast.item(d.item);
        let mut c = crate::lint_nonlocal::Paths { prog, tcx, file: d.file, module: d.scope, ast, params: Vec::new(), out: Vec::new() };
        // the type parameters of the item and of the impl / trait around it
        let mut p = DefId(i as u32);
        while p != crate::program::NO_DEF {
            let pd = prog.def(p);
            if matches!(pd.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TypeAlias | DefKind::Trait | DefKind::Impl) {
                let pa = &prog.files[pd.file as usize].ast;
                if let Some(g) = pa.kind_generics(&pa.item(pd.item).kind) {
                    for gp in pa.list(g.params) {
                        if !matches!(gp.kind, GenericParamKind::Lifetime(_)) { c.params.push(prog.files[pd.file as usize].text(gp.name).to_string()); }
                    }
                }
            }
            p = pd.parent;
        }
        c.item_signature(&item);
        for (target, pos) in std::mem::take(&mut c.out) {
            if lint::external(prog, d.file, pos) { continue; }
            if let Some((future, msg)) = dp.message(target, DefId(i as u32)) {
                lint::emit(prog, DefId(i as u32), if future { &crate::lint_late::DEPRECATED_IN_FUTURE } else { &crate::lint_late::DEPRECATED }, d.file, pos, &msg);
            }
        }
    }
}

// ---------------------------------------------------------------- invalid_value (rustc_lint builtin.rs InvalidValue)

pub const INVALID_VALUE: Lint = lint::lint("invalid_value");

#[derive(Clone, Copy, PartialEq)]
enum InitKind { Zeroed, Uninit }

impl<'a> L2<'a> {
    fn is_zero_expr(&self, e: ExprId) -> bool {
        let e = unparen(self.ast, e);
        match self.ast.expr(e).kind {
            ExprKind::Lit(LitKind::Int) => {
                let x = self.ast.expr(e);
                let t = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(x.lo, x.hi)).unwrap_or("");
                crate::const_fold::parse_int_lit(t) == Some(0)
            }
            ExprKind::Tuple(xs) => self.ast.list(xs).iter().all(|&x| self.is_zero_expr(x)),
            _ => false,
        }
    }

    fn dangerous_init(&self, e: ExprId) -> Option<InitKind> {
        let ast = self.ast;
        match ast.expr(e).kind {
            ExprKind::Call(f, args) => {
                let d = self.call_def(f)?;
                match diag_name(self.prog, d).as_deref() {
                    Some("mem_zeroed") => Some(InitKind::Zeroed),
                    Some("mem_uninitialized") => Some(InitKind::Uninit),
                    Some("transmute") if ast.list(args).first().is_some_and(|&a| self.is_zero_expr(a)) => Some(InitKind::Zeroed),
                    _ => None,
                }
            }
            ExprKind::MethodCall(recv, _) => {
                let m = self.body.methods.get(&e.0)?;
                if diag_name(self.prog, m.def).as_deref() != Some("assume_init") { return None; }
                let ExprKind::Call(f, _) = ast.expr(unparen(ast, recv)).kind else { return None };
                let d = self.call_def(f)?;
                match diag_name(self.prog, d).as_deref() {
                    Some("maybe_uninit_zeroed") => Some(InitKind::Zeroed),
                    Some("maybe_uninit_uninit") => Some(InitKind::Uninit),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// `variant_find_init_error` for a struct / a variant; the message of the first problem.
    fn variant_init_error(&self, ty: TyId, d: DefId, vi: usize, args: crate::types::TyList, init: InitKind, depth: u32) -> Option<String> {
        let a = self.tcx.adts.get(&d)?;
        let var = a.variants.get(vi)?;
        let mut field_err = None;
        for f in &var.fields {
            let ft = self.tcx.tys.subst(f.ty, self.tcx.tys.list(args));
            if let Some(m) = self.ty_init_error(ft, init, depth + 1) { field_err = Some(m); break; }
        }
        // a constrained layout (NonNull, NonZero ...)
        if a.valid_start.is_some() || a.valid_end.is_some() {
            let non_null = a.valid_start.is_some_and(|s| s > 0);
            let msg = if non_null { "must be non-null" } else if init == InitKind::Uninit { "must be initialized inside its custom valid range" } else { return field_err };
            let _ = ty;
            return Some(format!("`{}` {}", crate::typeck::ty_to_string(false, self.prog, self.tcx, ty), msg));
        }
        field_err
    }

    /// `ty_find_init_error`: Some only when the type definitely does not allow the init.
    fn ty_init_error(&self, ty: TyId, init: InitKind, depth: u32) -> Option<String> {
        if depth > 16 { return None; }
        let ty = self.norm(ty);
        match self.kind(ty) {
            TyKind::Ref(..) => Some("references must be non-null".into()),
            TyKind::Adt(d, ..) if Some(d) == self.lang(LangItem::OwnedBox) => Some("`Box` must be non-null".into()),
            TyKind::FnPtr(..) => Some("function pointers must be non-null".into()),
            TyKind::Never => Some("the `!` type has no valid value".into()),
            TyKind::Ptr(_, i) if matches!(self.kind(i), TyKind::Dyn(..)) => Some("the vtable of a wide raw pointer must be non-null".into()),
            TyKind::Bool if init == InitKind::Uninit => Some("booleans must be either `true` or `false`".into()),
            TyKind::Char if init == InitKind::Uninit => Some("characters must be a valid Unicode codepoint".into()),
            TyKind::Int(_) if init == InitKind::Uninit => Some("integers must be initialized".into()),
            TyKind::Float(_) if init == InitKind::Uninit => Some("floats must be initialized".into()),
            TyKind::Ptr(..) if init == InitKind::Uninit => Some("raw pointers must be initialized".into()),
            TyKind::Adt(d, args, _) => {
                let a = self.tcx.adts.get(&d)?;
                if a.is_union { return None; }
                if !a.is_enum { return self.variant_init_error(ty, d, 0, args, init, depth); }
                // enums: skip the uninhabited variants
                let inhabited: Vec<(usize, bool)> = (0..a.variants.len()).filter_map(|vi| {
                    let tys: Vec<TyId> = a.variants[vi].fields.iter().map(|f| self.tcx.tys.subst(f.ty, self.tcx.tys.list(args))).collect();
                    if tys.iter().any(|&t| self.pcx_uninhabited(t)) { None } else { Some((vi, true)) }
                }).collect();
                match inhabited.len() {
                    0 => Some("enums with no inhabited variants have no valid value".into()),
                    1 => self.variant_init_error(ty, d, inhabited[0].0, args, init, depth),
                    n => if init == InitKind::Uninit && n > 1 { Some("enums with multiple inhabited variants have to be initialized to a variant".into()) } else { None },
                }
            }
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().find_map(|&f| self.ty_init_error(f, init, depth + 1)),
            TyKind::Array(e, n) => match n.try_to_target_usize(&self.tcx.tys) {
                Some(v) if v > 0 => self.ty_init_error(e, init, depth + 1),
                _ => None,
            },
            _ => None,
        }
    }

    fn pcx_uninhabited(&self, t: TyId) -> bool {
        crate::pattern_analysis::body_cx(self.prog, self.tcx, self.body).is_uninhabited(t)
    }

    fn lint_invalid_value(&self, e: ExprId) {
        let Some(init) = self.dangerous_init(e) else { return };
        let Some(ty) = self.expr_ty(e) else { return };
        if self.ty_init_error(ty, init, 0).is_some() {
            let ts = crate::typeck::ty_to_string(false, self.prog, self.tcx, ty);
            let msg = match init {
                InitKind::Zeroed => format!("the type `{}` does not permit zero-initialization", ts),
                InitKind::Uninit => format!("the type `{}` does not permit being left uninitialized", ts),
            };
            self.emit(&INVALID_VALUE, self.ast.expr(e).lo, &msg);
        }
    }
}

// ---------------------------------------------------------------- const_item_interior_mutations (rustc_lint interior_mutable_consts.rs)

pub const CONST_ITEM_INTERIOR_MUTATIONS: Lint = lint::lint("const_item_interior_mutations");

impl<'a> L2<'a> {
    fn lint_interior_mutable_consts(&self, e: ExprId) {
        let ast = self.ast;
        let (method, receiver) = match ast.expr(e).kind {
            ExprKind::MethodCall(r, _) => (self.body.methods.get(&e.0).map(|m| m.def), r),
            ExprKind::Call(f, args) => {
                let a = ast.list(args);
                let Some(&first) = a.first() else { return };
                let ExprKind::AddrOf(_, _, recv) = ast.expr(unparen(ast, first)).kind else { return };
                if !matches!(ast.expr(unparen(ast, f)).kind, ExprKind::Path(_)) { return; }
                (self.call_def(f), recv)
            }
            _ => return,
        };
        let Some(method) = method else { return };
        let recv = unparen(ast, receiver);
        if !matches!(ast.expr(recv).kind, ExprKind::Path(_)) { return; }
        let Some(Res::Def(c)) = self.body.res.get(&recv.0).copied() else { return };
        if !matches!(self.prog.def(c).kind, DefKind::Const | DefKind::AssocConst) { return; }
        // derefs among the receiver's adjustments are arbitrary code
        if let ExprKind::MethodCall(..) = ast.expr(e).kind {
            if self.body.methods.get(&e.0).is_some_and(|m| m.derefs > 0) { return; }
        }
        if !has_def_attr(self.prog, method, "rustc_should_not_be_called_on_const_items") { return; }
        let name = self.prog.syms.str(self.prog.def(method).name);
        self.emit(&CONST_ITEM_INTERIOR_MUTATIONS, ast.expr(e).lo, &format!("mutation of an interior mutable `const` item with call to `{}`", name));
    }
}

// ---------------------------------------------------------------- refining_impl_trait (compare_impl_item/refine.rs)

pub const REFINING_IMPL_TRAIT_REACHABLE: Lint = lint::lint("refining_impl_trait_reachable");
pub const REFINING_IMPL_TRAIT_INTERNAL: Lint = lint::lint("refining_impl_trait_internal");

/// The impl methods whose return-position `impl Trait` in the trait is implemented by a type that is
/// not an opaque of the impl method itself ("the hidden type is not an opaque": the first
/// refinement test of `check_refining_return_position_impl_trait_in_trait`). The bound-set and
/// capture comparisons are not ported.
pub fn check_refining_impls(prog: &Program, tcx: &Tcx, own: &[bool]) {
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if d.kind != DefKind::AssocFn || !own.get(d.krate as usize).copied().unwrap_or(false) || d.parent == crate::program::NO_DEF { continue; }
        if prog.def(d.parent).kind != DefKind::Impl || tcx.impl_trait.get(&d.parent).is_none() { continue; }
        let id = DefId(i as u32);
        let Some(&trait_m) = prog.trait_item_def_id.get(&id) else { continue };
        // the trait method has return-position impl trait: some RPITIT synthesized for it
        let has_rpitit = prog.impl_trait_in_trait_data.iter().any(|(_, v)| matches!(v, relax_front::assoc::ImplTraitInTraitData::Trait { fn_def_id, .. } if *fn_def_id == trait_m));
        if !has_rpitit { continue; }
        let Ok(hidden) = tcx.collect_return_position_impl_trait_in_trait_tys(prog, id) else { continue };
        if hidden.iter().any(|&(_, t)| tcx.tys.has_error(t)) { continue; }
        let mut refined = false;
        for &(_, t) in &hidden {
            match *tcx.tys.kind(t) {
                TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { def_id }, .. }) => {
                    // an opaque of some other item: a TAIT
                    let od = prog.def(def_id);
                    let mut p = od.parent;
                    let mut ok = false;
                    for _ in 0..8 { if p == id { ok = true; break; } if p == crate::program::NO_DEF { break; } p = prog.def(p).parent; }
                    if !ok { refined = true; }
                }
                _ => refined = true,
            }
        }
        if !refined { continue; }
        let ast = &prog.files[d.file as usize].ast;
        let item = *ast.item(d.item);
        let ItemKind::Fn(sig, _, _) = item.kind else { continue };
        let sig = *ast.fn_sig(sig);
        let pos = match sig.ret {
            Some(r) => match *ast.ty(r) {
                Ty::ImplTrait(bs) => ast.list(bs).iter().find_map(|b| if let Bound::Trait { path, .. } = *b { Some(path.lo) } else { None }).unwrap_or(item.name.lo),
                _ => crate::lint_late::ast_ty_pos(ast, r).unwrap_or(item.name.lo),
            },
            None => item.lo,
        };
        if lint::external(prog, d.file, pos) { continue; }
        // `is_internal`: the trait is not reachable
        let trait_def = prog.def(prog.def(trait_m).parent);
        let internal = !trait_def.is_pub;
        let lint_ = if internal { &REFINING_IMPL_TRAIT_INTERNAL } else { &REFINING_IMPL_TRAIT_REACHABLE };
        lint::emit(prog, id, lint_, d.file, pos, "impl trait in impl method signature does not match trait method signature");
    }
}

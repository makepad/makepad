//! Late (type-aware) builtin lints of `makepad check` that are not liveness, reachability or
//! dead-code analyses: ports of rustc_lint's LateLintPass passes (`unused_must_use`,
//! `path_statements`, `trivial_casts`, ...). Check-only.

use crate::ast::*;
use crate::ast_visit::{self, Visitor};
use crate::lang_items::LangItem;
use crate::lint::{self, Lint};
use crate::pattern_analysis::{self, Cx as PCx};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{AliasTyKind, TyId, TyKind};

pub const NON_SHORTHAND_FIELD_PATTERNS: Lint = lint::lint("non_shorthand_field_patterns");
pub const FUNCTION_CASTS_AS_INTEGER: Lint = lint::lint("function_casts_as_integer");
pub const MISSING_COPY_IMPLEMENTATIONS: Lint = lint::lint("missing_copy_implementations");
pub const UNUSED_MUST_USE: Lint = lint::lint("unused_must_use");
pub const UNUSED_RESULTS: Lint = lint::lint("unused_results");
pub const PATH_STATEMENTS: Lint = lint::lint("path_statements");
pub const DROPPING_REFERENCES: Lint = lint::lint("dropping_references");
pub const FORGETTING_REFERENCES: Lint = lint::lint("forgetting_references");
pub const DROPPING_COPY_TYPES: Lint = lint::lint("dropping_copy_types");
pub const FORGETTING_COPY_TYPES: Lint = lint::lint("forgetting_copy_types");
pub const UNDROPPED_MANUALLY_DROPS: Lint = lint::lint("undropped_manually_drops");
pub const INVALID_ATOMIC_ORDERING: Lint = lint::lint("invalid_atomic_ordering");
pub const LET_UNDERSCORE_DROP: Lint = lint::lint("let_underscore_drop");
pub const LET_UNDERSCORE_LOCK: Lint = lint::lint("let_underscore_lock");
pub const FOR_LOOPS_OVER_FALLIBLES: Lint = lint::lint("for_loops_over_fallibles");
pub const MAP_UNIT_FN: Lint = lint::lint("map_unit_fn");
pub const UNUSED_ALLOCATION: Lint = lint::lint("unused_allocation");
pub const DEPRECATED: Lint = lint::lint("deprecated");
pub const DEPRECATED_IN_FUTURE: Lint = lint::lint("deprecated_in_future");
pub const NOOP_METHOD_CALL: Lint = lint::lint("noop_method_call");
pub const SUSPICIOUS_DOUBLE_REF_OP: Lint = lint::lint("suspicious_double_ref_op");
pub const DEREF_NULLPTR: Lint = lint::lint("deref_nullptr");
pub const TRIVIAL_CASTS: Lint = lint::lint("trivial_casts");
pub const TRIVIAL_NUMERIC_CASTS: Lint = lint::lint("trivial_numeric_casts");

/// Body-level passes over one typed body owner.
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body) {
    if !lint::enabled() { return; }
    let d = prog.def(body.def);
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let mut w = Late { prog, tcx, body, ast, stack: Vec::new(), pcx: pattern_analysis::body_cx(prog, tcx, body), arm_body: None, raw_operand: None };
    match ast.item(d.item).kind {
        ItemKind::Fn(s, _, _) => {
            let Ok(Some(block)) = prog.fn_body(body.file, d.item) else { return };
            let params: Vec<PatId> = ast.list(ast.fn_sig(s).params).iter().map(|p| p.pat).collect();
            w.block(block);
            known_panics(prog, tcx, body, Some(block), None, &params);
            crate::lint_late2::check_body(prog, tcx, body, Some(block), None);
            crate::lint_recursion::check(prog, tcx, body, block);
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            let Ok(Some(e)) = prog.const_body(body.file, d.item) else { return };
            w.expr(e);
            known_panics(prog, tcx, body, None, Some(e), &[]);
            crate::lint_late2::check_body(prog, tcx, body, None, Some(e));
        }
        _ => {}
    }
}

struct Late<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    /// the attributes of the enclosing statements, arms, ..., outermost first
    stack: Vec<Attr>,
    pcx: PCx<'a>,
    /// the body of the match arm being visited (`is_single_call_in_arm`)
    arm_body: Option<ExprId>,
    /// the operand of the enclosing `&raw` borrow, when it is one (`&raw *NULL` is fine)
    raw_operand: Option<ExprId>,
}

// ---------------------------------------------------------------- must_use machinery

/// rustc_lint unused/must_use.rs `MustUsePath`: a path through a type to a `must_use` source.
enum MustUsePath {
    /// the def, its `#[must_use = ".."]` reason, and the position of the expression
    Def(u32, DefId),
    Boxed(Box<MustUsePath>),
    Pinned(Box<MustUsePath>),
    Opaque(Box<MustUsePath>),
    TraitObject(Box<MustUsePath>),
    TupleElement(Vec<(usize, MustUsePath)>),
    Result(Box<MustUsePath>),
    ControlFlow(Box<MustUsePath>),
    Array(Box<MustUsePath>, u64),
    Closure(u32),
    Coroutine(u32),
}

enum IsTyMustUse {
    Yes(MustUsePath),
    No,
    Trivial,
}

impl IsTyMustUse {
    fn map(self, f: impl FnOnce(MustUsePath) -> MustUsePath) -> Self {
        match self {
            Self::Yes(p) => Self::Yes(f(p)),
            o => o,
        }
    }
}

impl<'a> Late<'a> {
    fn kind(&self, t: TyId) -> TyKind { *self.tcx.tys.kind(t) }

    fn emit(&self, lint: &Lint, pos: u32, msg: &str) {
        if lint::external(self.prog, self.body.file, pos) { return; }
        lint::emit_in(self.prog, self.body.def, &self.stack, lint, self.body.file, pos, msg);
    }

    /// Lints declared `report_in_external_macro`.
    fn emit_ext(&self, lint: &Lint, pos: u32, msg: &str) {
        lint::emit_in(self.prog, self.body.def, &self.stack, lint, self.body.file, pos, msg);
    }

    fn expr_ty(&self, e: ExprId) -> Option<TyId> {
        let i = self.body.expr_ix(e);
        if i < self.body.expr_ty.len() && self.body.expr_typed.get(i) { Some(self.body.expr_ty[i]) } else { None }
    }

    fn lang(&self, item: LangItem) -> Option<DefId> { self.tcx.lang_items.get(item) }

    /// The enum a lang-item variant belongs to (`Result` of `ResultOk`).
    fn lang_parent(&self, variant: LangItem) -> Option<DefId> {
        self.lang(variant).map(|v| self.prog.def(v).parent)
    }

    /// `#[must_use]` on a definition: Some(()) when present.
    fn def_must_use(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        if !matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Variant) { return false; }
        let ast = &self.prog.files[def.file as usize].ast;
        lint::has_attr(self.prog, def.file, ast.list(ast.item(def.item).attrs), "must_use")
    }

    fn is_def_must_use(&self, d: DefId, pos: u32) -> Option<MustUsePath> {
        self.def_must_use(d).then_some(MustUsePath::Def(pos, d))
    }

    fn is_ty_must_use(&self, ty: TyId, expr: ExprId, pos: u32) -> IsTyMustUse {
        let kind = self.kind(ty);
        if matches!(kind, TyKind::Tuple(l) if self.tcx.tys.list(l).is_empty()) { return IsTyMustUse::Trivial; }
        if self.pcx.is_uninhabited(ty) { return IsTyMustUse::Trivial; }
        let uninh = |t: TyId| self.pcx.is_uninhabited(t);
        match kind {
            TyKind::Adt(d, args, _) if Some(d) == self.lang(LangItem::OwnedBox) => {
                let inner = self.tcx.tys.list(args)[0];
                self.is_ty_must_use(inner, expr, pos).map(|p| MustUsePath::Boxed(Box::new(p)))
            }
            TyKind::Adt(d, args, _) if Some(d) == self.lang(LangItem::Pin) => {
                let inner = self.tcx.tys.list(args)[0];
                self.is_ty_must_use(inner, expr, pos).map(|p| MustUsePath::Pinned(Box::new(p)))
            }
            TyKind::Adt(d, args, _) if Some(d) == self.lang_parent(LangItem::ResultOk) && uninh(self.tcx.tys.list(args)[1]) => {
                let ok = self.tcx.tys.list(args)[0];
                self.is_ty_must_use(ok, expr, pos).map(|p| MustUsePath::Result(Box::new(p)))
            }
            TyKind::Adt(d, args, _) if Some(d) == self.lang_parent(LangItem::ControlFlowBreak) && uninh(self.tcx.tys.list(args)[0]) => {
                let cont = self.tcx.tys.list(args)[1];
                self.is_ty_must_use(cont, expr, pos).map(|p| MustUsePath::ControlFlow(Box::new(p)))
            }
            TyKind::Adt(d, _, _) => self.is_def_must_use(d, pos).map_or(IsTyMustUse::No, IsTyMustUse::Yes),
            TyKind::Alias(_, a) if matches!(a.kind, AliasTyKind::Opaque { .. } | AliasTyKind::Projection { .. }) => {
                let def = a.kind.def_id();
                let found = self.tcx.item_bounds(self.prog, def).iter().find_map(|p| {
                    let crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(c)) = &p.kind else { return None };
                    // only the self bounds of the impl-trait
                    match c.trait_ref.args.first() {
                        Some(&crate::consts::GenericArg::Type(s)) if matches!(self.kind(s), TyKind::Alias(_, sa) if sa.kind.def_id() == def) => {}
                        _ => return None,
                    }
                    self.is_def_must_use(c.trait_ref.def_id, pos)
                });
                match found {
                    Some(inner) => IsTyMustUse::Yes(MustUsePath::Opaque(Box::new(inner))),
                    None => IsTyMustUse::No,
                }
            }
            TyKind::Dyn(principal, ..) => match self.is_def_must_use(principal, pos) {
                Some(inner) => IsTyMustUse::Yes(MustUsePath::TraitObject(Box::new(inner))),
                None => IsTyMustUse::No,
            },
            TyKind::Tuple(l) => {
                let tys = self.tcx.tys.list(l);
                let elems: Vec<ExprId> = match self.ast.expr(unparen(self.ast, expr)).kind {
                    ExprKind::Tuple(xs) => self.ast.list(xs).to_vec(),
                    _ => Vec::new(),
                };
                let mut all_trivial = true;
                let mut nested = Vec::new();
                for (i, &t) in tys.iter().enumerate() {
                    let (e, p) = match elems.get(i) { Some(&e) => (e, self.ast.expr(e).lo), None => (expr, pos) };
                    let r = self.is_ty_must_use(t, e, p);
                    all_trivial &= matches!(r, IsTyMustUse::Trivial);
                    if let IsTyMustUse::Yes(p) = r { nested.push((i, p)); }
                }
                if all_trivial { IsTyMustUse::Trivial }
                else if !nested.is_empty() { IsTyMustUse::Yes(MustUsePath::TupleElement(nested)) }
                else { IsTyMustUse::No }
            }
            TyKind::Array(elem, len) => match len.try_to_target_usize(&self.tcx.tys) {
                Some(0) | None => IsTyMustUse::No,
                Some(n) => self.is_ty_must_use(elem, expr, pos).map(|p| MustUsePath::Array(Box::new(p), n)),
            },
            TyKind::Closure(file, ce, _) => {
                if self.tcx.coroutines.contains(&(file, ce)) {
                    // an async block / async fn body: an implementer of `Future`
                    match self.lang(LangItem::Future).and_then(|f| self.is_def_must_use(f, pos)) {
                        Some(inner) => IsTyMustUse::Yes(MustUsePath::Opaque(Box::new(inner))),
                        None => IsTyMustUse::Yes(MustUsePath::Coroutine(pos)),
                    }
                } else {
                    IsTyMustUse::Yes(MustUsePath::Closure(pos))
                }
            }
            _ => IsTyMustUse::No,
        }
    }

    fn def_path_str(&self, d: DefId) -> String {
        let def = self.prog.def(d);
        let name = self.prog.name(d);
        if matches!(def.kind, DefKind::AssocFn) && def.parent != crate::program::NO_DEF {
            let p = self.prog.def(def.parent);
            match p.kind {
                DefKind::Trait => return format!("{}::{}", self.prog.name(def.parent), name),
                DefKind::Impl => {
                    let pa = &self.prog.files[p.file as usize].ast;
                    if let Some(i) = pa.as_impl(&pa.item(p.item).kind) {
                        if i.trait_.is_none() {
                            if let Ty::Path(path) = pa.ty(i.self_ty) {
                                return format!("{}::{}", crate::tcx::path_str(self.prog, p.file, path), name);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        name.to_string()
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_must_use(&self, path: &MustUsePath, pre: &str, post: &str, plural_len: usize) {
        let plural = if plural_len == 1 { "" } else { "s" };
        match path {
            MustUsePath::Boxed(p) => self.emit_must_use(p, &format!("{pre}boxed "), post, plural_len),
            MustUsePath::Pinned(p) => self.emit_must_use(p, &format!("{pre}pinned "), post, plural_len),
            MustUsePath::Opaque(p) => self.emit_must_use(p, &format!("{pre}implementer{plural} of "), post, plural_len),
            MustUsePath::TraitObject(p) => self.emit_must_use(p, pre, &format!(" trait object{plural}{post}"), plural_len),
            MustUsePath::TupleElement(es) => {
                for (i, p) in es { self.emit_must_use(p, pre, &format!(" in tuple element {i}"), plural_len); }
            }
            MustUsePath::Result(p) => self.emit_must_use(p, pre, &format!(" in a `Result` with an uninhabited error{post}"), plural_len),
            MustUsePath::ControlFlow(p) => self.emit_must_use(p, pre, &format!(" in a `ControlFlow` with an uninhabited break {post}"), plural_len),
            MustUsePath::Array(p, n) => self.emit_must_use(p, &format!("{pre}array{plural} of "), post, plural_len.saturating_add(*n as usize)),
            MustUsePath::Closure(pos) => {
                let plural = if plural_len == 1 { "closure" } else { "closures" };
                self.emit_ext(&UNUSED_MUST_USE, *pos, &format!("unused {pre}{plural}{post} that must be used"));
            }
            MustUsePath::Coroutine(pos) => {
                self.emit_ext(&UNUSED_MUST_USE, *pos, &format!("unused {pre}coroutine{post} that must be used"));
            }
            MustUsePath::Def(pos, d) => {
                self.emit_ext(&UNUSED_MUST_USE, *pos, &format!("unused {pre}`{}`{post} that must be used", self.def_path_str(*d)));
            }
        }
    }

    /// check_fn_must_use: a call of a `#[must_use]` fn or method.
    fn check_fn_must_use(&self, e: ExprId, pos: u32) -> bool {
        let def = match self.ast.expr(e).kind {
            ExprKind::Call(callee, _) => {
                if let ExprKind::Path(_) = self.ast.expr(unparen(self.ast, callee)).kind {
                    match self.body.res.get(&unparen(self.ast, callee).0) {
                        Some(Res::Def(d)) if matches!(self.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn) => Some(*d),
                        _ => None,
                    }
                } else { None }
            }
            ExprKind::MethodCall(..) => self.body.methods.get(&e.0).map(|m| m.def),
            _ => None,
        };
        let Some(d) = def else { return false };
        match self.is_def_must_use(d, pos) {
            Some(p) => { self.emit_must_use(&p, "return value of ", "", 1); true }
            None => false,
        }
    }

    fn check_unused_results(&self, s_lo: u32, mut expr: ExprId) {
        // blocks with a tail expression
        while let ExprKind::Block(b, _) = self.ast.expr(expr).kind {
            let stmts = self.ast.stmts(b);
            match stmts.last() {
                Some(Stmt::Expr(e, false)) => expr = *e,
                _ => break,
            }
        }
        if let ExprKind::Return(_) = self.ast.expr(expr).kind { return; }
        let pos = self.ast.expr(expr).lo;
        // `foo().await;` on the future of an `async fn` marked `#[must_use]`
        if let ExprKind::Await(f) = self.ast.expr(expr).kind {
            let callee = match self.ast.expr(unparen(self.ast, f)).kind {
                ExprKind::Call(c, _) => match self.body.res.get(&unparen(self.ast, c).0) { Some(Res::Def(d)) => Some(*d), _ => None },
                ExprKind::MethodCall(..) => self.body.methods.get(&unparen(self.ast, f).0).map(|m| m.def),
                _ => None,
            };
            if let Some(d) = callee {
                let def = self.prog.def(d);
                let fa = &self.prog.files[def.file as usize].ast;
                let is_async = matches!(def.kind, DefKind::Fn | DefKind::AssocFn)
                    && matches!(fa.item(def.item).kind, ItemKind::Fn(sig, _, _) if fa.fn_sig(sig).asyncness);
                if is_async {
                    if let Some(p) = self.is_def_must_use(d, pos) {
                        self.emit_must_use(&p, "output of future returned by ", "", 1);
                        return;
                    }
                }
            }
        }
        let Some(ty) = self.expr_ty(expr) else { return };
        let must_use_result = self.is_ty_must_use(ty, expr, pos);
        let type_lint_emitted_or_trivial = match must_use_result {
            IsTyMustUse::Yes(path) => { self.emit_must_use(&path, "", "", 1); true }
            IsTyMustUse::Trivial => true,
            IsTyMustUse::No => false,
        };
        let fn_warned = self.check_fn_must_use(expr, pos);
        if !fn_warned && type_lint_emitted_or_trivial { return; }
        let inner = unparen(self.ast, expr);
        let op = match self.ast.expr(inner).kind {
            ExprKind::Binary(op, ..) => Some(match op {
                BinOp::Eq | BinOp::Lt | BinOp::Le | BinOp::Ne | BinOp::Ge | BinOp::Gt => "comparison",
                BinOp::Add | BinOp::Sub | BinOp::Div | BinOp::Mul | BinOp::Rem => "arithmetic operation",
                BinOp::And | BinOp::Or => "logical operation",
                BinOp::BitXor | BinOp::BitAnd | BinOp::BitOr | BinOp::Shl | BinOp::Shr => "bitwise operation",
            }),
            ExprKind::AddrOf(..) => Some("borrow"),
            ExprKind::Unary(..) => Some("unary operation"),
            _ => None,
        };
        let op_warned = match op {
            Some(op) => {
                self.emit_ext(&UNUSED_MUST_USE, pos, &format!("unused {op} that must be used"));
                true
            }
            None => false,
        };
        if !(type_lint_emitted_or_trivial || fn_warned || op_warned) {
            let t = crate::typeck::ty_to_string(false, self.prog, self.tcx, ty);
            self.emit(&UNUSED_RESULTS, s_lo, &format!("unused result of type `{t}`"));
        }
    }

    fn check_path_statement(&self, e: ExprId) {
        let inner = unparen(self.ast, e);
        if !matches!(self.ast.expr(inner).kind, ExprKind::Path(_)) { return; }
        let Some(ty) = self.expr_ty(inner).or_else(|| self.expr_ty(e)) else { return };
        let lo = self.ast.expr(e).lo;
        if self.tcx.needs_drop(self.prog, self.pcx.typing_env(), ty) {
            self.emit(&PATH_STATEMENTS, lo, "path statement drops value");
        } else {
            self.emit(&PATH_STATEMENTS, lo, "path statement with no effect");
        }
    }
}

// ---------------------------------------------------------------- expression passes

impl<'a> Late<'a> {
    fn diag_item(&self, d: DefId) -> Option<String> { lint::diagnostic_item(self.prog, d) }

    /// The definition a path expression resolves to.
    fn path_def(&self, e: ExprId) -> Option<DefId> {
        let e = unparen(self.ast, e);
        if !matches!(self.ast.expr(e).kind, ExprKind::Path(_)) { return None; }
        match self.body.res.get(&e.0) { Some(Res::Def(d)) => Some(*d), _ => None }
    }

    /// `Expr::can_have_side_effects`.
    fn can_have_side_effects(&self, e: ExprId) -> bool {
        let ast = self.ast;
        match ast.expr(e).kind {
            ExprKind::Paren(x) => self.can_have_side_effects(x),
            ExprKind::Path(_) | ExprKind::Lit(_) | ExprKind::Underscore => false,
            ExprKind::Unary(_, b) | ExprKind::Field(b, _) | ExprKind::TupleField(b, _) | ExprKind::Index(b, _) | ExprKind::AddrOf(_, _, b) | ExprKind::Cast(b, _) => self.can_have_side_effects(b),
            ExprKind::Binary(_, l, r) => self.can_have_side_effects(l) || self.can_have_side_effects(r),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                ast.list(s.fields).iter().any(|f| self.can_have_side_effects(f.expr)) || s.base.get().is_some_and(|b| self.can_have_side_effects(b))
            }
            ExprKind::Array(xs) | ExprKind::Tuple(xs) => ast.list(xs).iter().any(|&x| self.can_have_side_effects(x)),
            ExprKind::Repeat(x, _) => self.can_have_side_effects(x),
            ExprKind::Call(f, args) => {
                // a tuple struct / variant constructor call
                match self.path_def(f) {
                    Some(d) if matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Variant) => ast.list(args).iter().any(|&x| self.can_have_side_effects(x)),
                    _ => true,
                }
            }
            _ => true,
        }
    }

    fn check_expr(&mut self, e: ExprId) {
        let ex = *self.ast.expr(e);
        match ex.kind {
            ExprKind::Call(f, args) => {
                self.check_drop_forget(e, f, args);
                self.check_memory_fence(f, args);
            }
            ExprKind::Path(_) => {
                if let Some(Res::Def(d)) = self.body.res.get(&e.0) { self.check_deprecated(*d, ex.lo); }
            }
            ExprKind::Struct(row) => {
                if let Some(Res::Def(d)) = self.body.res.get(&e.0) { let p = self.ast.row(row).path.lo; self.check_deprecated(*d, p); }
            }
            ExprKind::MethodCall(_, row) => {
                if let Some(m) = self.body.methods.get(&e.0) { let (d, pos) = (m.def, self.ast.row(row).name.lo); self.check_deprecated(d, pos); }
                self.check_map_unit_fn(e);
                self.check_unused_allocation(e);
                self.check_noop_method_call(e);
                self.check_atomic_load_store(e);
                self.check_atomic_compare_exchange(e);
            }
            ExprKind::Cast(x, _) => { self.check_trivial_cast(e, x); self.check_function_cast(e, x); }
            ExprKind::AddrOf(true, _, x) => self.raw_operand = Some(unparen(self.ast, x)),
            ExprKind::Unary(UnOp::Deref, x) => self.check_deref_nullptr(e, x),
            ExprKind::For(row) => self.check_for_loop_fallible(*self.ast.row(row)),
            ExprKind::Assign(l, r) => self.check_assign_desugar(l, r, ex.lo),
            _ => {}
        }
    }

    // ---- drop_forget_useless.rs
    fn check_drop_forget(&self, e: ExprId, f: ExprId, args: List<ExprId>) {
        let args = self.ast.list(args);
        if args.len() != 1 { return; }
        let Some(d) = self.path_def(f) else { return };
        let Some(name) = self.diag_item(d) else { return };
        let arg = args[0];
        let Some(arg_ty) = self.expr_ty(arg) else { return };
        let is_copy = self.tcx.is_copy_modulo_regions(self.prog, self.pcx.typing_env(), arg_ty);
        let is_ref = matches!(self.kind(arg_ty), TyKind::Ref(..));
        let single_call_in_arm = self.can_have_side_effects(arg) && self.arm_body == Some(e);
        let lo = self.ast.expr(e).lo;
        match name.as_str() {
            "mem_drop" if is_ref && !single_call_in_arm => self.emit(&DROPPING_REFERENCES, lo, "calls to `std::mem::drop` with a reference instead of an owned value does nothing"),
            "mem_forget" if is_ref => self.emit(&FORGETTING_REFERENCES, lo, "calls to `std::mem::forget` with a reference instead of an owned value does nothing"),
            "mem_drop" if is_copy && !single_call_in_arm => self.emit(&DROPPING_COPY_TYPES, lo, "calls to `std::mem::drop` with a value that implements `Copy` does nothing"),
            "mem_forget" if is_copy => self.emit(&FORGETTING_COPY_TYPES, lo, "calls to `std::mem::forget` with a value that implements `Copy` does nothing"),
            "mem_drop" => {
                if let TyKind::Adt(a, _, _) = self.kind(arg_ty) {
                    if self.diag_item(a).as_deref() == Some("ManuallyDrop") {
                        self.emit(&UNDROPPED_MANUALLY_DROPS, lo, "calls to `std::mem::drop` with `std::mem::ManuallyDrop` instead of the inner value does nothing");
                    }
                }
            }
            _ => {}
        }
    }

    // ---- types.rs InvalidAtomicOrdering
    /// An inherent method of the standard `Atomic` types: its name and arguments.
    fn inherent_atomic_method_call(&self, e: ExprId, names: &[&str]) -> Option<(String, Vec<ExprId>)> {
        let ExprKind::MethodCall(_, row) = self.ast.expr(e).kind else { return None };
        let m = *self.ast.row(row);
        let name = self.prog.files[self.body.file as usize].text(m.name).to_string();
        if !names.contains(&name.as_str()) { return None; }
        let md = self.body.methods.get(&e.0)?.def;
        let parent = self.prog.def(md).parent;
        if parent == crate::program::NO_DEF || self.prog.def(parent).kind != DefKind::Impl { return None; }
        // an inherent impl
        if self.tcx.impl_trait.contains_key(&parent) { return None; }
        let st = *self.tcx.impl_self.get(&parent)?;
        let TyKind::Adt(adt, ..) = self.kind(st) else { return None };
        if self.diag_item(adt).as_deref() != Some("Atomic") { return None; }
        Some((name, self.ast.list(m.args).to_vec()))
    }

    fn match_ordering(&self, arg: ExprId) -> Option<&'static str> {
        let d = self.path_def(arg)?;
        let name = self.prog.name(d);
        let parent = self.prog.def(d).parent;
        let is_ordering = |p: DefId| p != crate::program::NO_DEF && self.diag_item(p).as_deref() == Some("Ordering");
        if !(is_ordering(parent)) { return None; }
        ["Relaxed", "Release", "Acquire", "AcqRel", "SeqCst"].into_iter().find(|o| *o == name)
    }

    fn check_atomic_load_store(&self, e: ExprId) {
        let Some((method, args)) = self.inherent_atomic_method_call(e, &["load", "store"]) else { return };
        let (idx, invalid) = if method == "load" { (0, "Release") } else { (1, "Acquire") };
        let Some(&arg) = args.get(idx) else { return };
        let Some(ordering) = self.match_ordering(arg) else { return };
        if ordering == invalid || ordering == "AcqRel" {
            let lo = self.ast.expr(arg).lo;
            if method == "load" {
                self.emit(&INVALID_ATOMIC_ORDERING, lo, "atomic loads cannot have `Release` or `AcqRel` ordering");
            } else {
                self.emit(&INVALID_ATOMIC_ORDERING, lo, "atomic stores cannot have `Acquire` or `AcqRel` ordering");
            }
        }
    }

    fn check_memory_fence(&self, f: ExprId, args: List<ExprId>) {
        let Some(d) = self.path_def(f) else { return };
        if !matches!(self.diag_item(d).as_deref(), Some("fence" | "compiler_fence")) { return; }
        let Some(&arg) = self.ast.list(args).first() else { return };
        if self.match_ordering(arg) == Some("Relaxed") {
            self.emit(&INVALID_ATOMIC_ORDERING, self.ast.expr(arg).lo, "memory fences cannot have `Relaxed` ordering");
        }
    }

    fn check_atomic_compare_exchange(&self, e: ExprId) {
        let Some((method, args)) = self.inherent_atomic_method_call(e, &["update", "try_update", "fetch_update", "compare_exchange", "compare_exchange_weak"]) else { return };
        let idx = match method.as_str() { "update" | "try_update" | "fetch_update" => 1, _ => 3 };
        let Some(&arg) = args.get(idx) else { return };
        let Some(fail) = self.match_ordering(arg) else { return };
        if matches!(fail, "Release" | "AcqRel") {
            self.emit(&INVALID_ATOMIC_ORDERING, self.ast.expr(arg).lo, &format!("`{method}`'s failure ordering may not be `Release` or `AcqRel`, since a failed `{method}` does not result in a write"));
        }
    }

    // ---- let_underscore.rs
    /// `check_local`: the `_` patterns of a `let` (or of `_ = value;`) whose value is dropped
    /// at once. `local_lo`: the statement's position, `init`: its initializer.
    fn check_non_binding(&self, pat: Option<PatId>, init: Option<ExprId>, local_lo: u32) {
        let mut wilds: Vec<(Option<PatId>, bool)> = Vec::new();
        match pat {
            Some(p) => self.collect_wilds(p, true, &mut wilds),
            None => wilds.push((None, true)),
        }
        for (p, top) in wilds {
            let ty = match p {
                Some(p) => {
                    let i = self.body.pat_ix(p);
                    if i >= self.body.pat_ty.len() || !self.body.pat_typed.get(i) { continue; }
                    self.body.pat_ty[i]
                }
                None => match init.and_then(|x| self.expr_ty(x)) { Some(t) => t, None => continue },
            };
            if !self.tcx.needs_drop(self.prog, self.pcx.typing_env(), ty) { continue; }
            // `mutex.lock()` returns a `Result<MutexGuard, _>` as well
            let potential = match self.kind(ty) {
                TyKind::Adt(d, args, _) if Some(d) == self.lang_parent(LangItem::ResultOk) => self.tcx.tys.list(args)[0],
                _ => ty,
            };
            let is_sync_lock = match self.kind(potential) {
                TyKind::Adt(d, _, _) => matches!(self.diag_item(d).as_deref(), Some("MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard")),
                _ => false,
            };
            let can_use_init = top && init.is_some();
            if is_sync_lock {
                self.emit(&LET_UNDERSCORE_LOCK, local_lo, "non-binding let on a synchronization lock");
            } else if can_use_init {
                self.emit(&LET_UNDERSCORE_DROP, local_lo, "non-binding let on a type that has a destructor");
            }
        }
    }

    fn collect_wilds(&self, p: PatId, top: bool, out: &mut Vec<(Option<PatId>, bool)>) {
        let ast = self.ast;
        match *ast.pat(p) {
            Pat::Wild => out.push((Some(p), top)),
            Pat::Ident { sub: Some(s), .. } => self.collect_wilds(s, false, out),
            Pat::TupleStruct(_, ps) | Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => {
                for &q in ast.list(ps) { self.collect_wilds(q, false, out); }
            }
            Pat::Struct(_, fs, _) => { for f in ast.list(fs) { self.collect_wilds(f.pat, false, out); } }
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.collect_wilds(x, false, out),
            _ => {}
        }
    }

    /// A destructuring assignment `(_, a) = value;` / `_ = value;` is a `let` with the `_`
    /// patterns of the left side (LocalSource::AssignDesugar).
    fn check_assign_desugar(&self, l: ExprId, r: ExprId, lo: u32) {
        let mut wilds = Vec::new();
        self.assign_wilds(l, &mut wilds);
        for u in wilds {
            let top = unparen(self.ast, l) == u;
            let Some(ty) = (if top { self.expr_ty(r) } else { self.expr_ty(u) }) else { continue };
            if !self.tcx.needs_drop(self.prog, self.pcx.typing_env(), ty) { continue; }
            let potential = match self.kind(ty) {
                TyKind::Adt(d, args, _) if Some(d) == self.lang_parent(LangItem::ResultOk) => self.tcx.tys.list(args)[0],
                _ => ty,
            };
            let is_sync_lock = match self.kind(potential) {
                TyKind::Adt(d, _, _) => matches!(self.diag_item(d).as_deref(), Some("MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard")),
                _ => false,
            };
            if is_sync_lock {
                self.emit(&LET_UNDERSCORE_LOCK, self.ast.expr(u).lo, "non-binding let on a synchronization lock");
            } else if top {
                self.emit(&LET_UNDERSCORE_DROP, lo, "non-binding let on a type that has a destructor");
            }
        }
    }

    fn assign_wilds(&self, e: ExprId, out: &mut Vec<ExprId>) {
        let ast = self.ast;
        match ast.expr(e).kind {
            ExprKind::Underscore => out.push(e),
            ExprKind::Paren(x) => self.assign_wilds(x, out),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => { for &x in ast.list(xs) { self.assign_wilds(x, out); } }
            ExprKind::Call(f, xs) if self.path_def(f).is_some_and(|d| matches!(self.prog.def(d).kind, DefKind::Struct | DefKind::Variant)) => {
                for &x in ast.list(xs) { self.assign_wilds(x, out); }
            }
            ExprKind::Struct(row) => { for f in ast.list(ast.row(row).fields) { self.assign_wilds(f.expr, out); } }
            _ => {}
        }
    }

    // ---- unused.rs UnusedAllocation
    fn check_unused_allocation(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, _) = self.ast.expr(e).kind else { return };
        let recv = unparen(self.ast, recv);
        let ExprKind::Call(f, args) = self.ast.expr(recv).kind else { return };
        if self.ast.list(args).len() != 1 { return; }
        let Some(d) = self.path_def(f) else { return };
        if self.diag_item(d).as_deref() != Some("box_new") { return; }
        let Some(m) = self.body.methods.get(&e.0) else { return };
        // the receiver's adjustments: derefs, then a borrow; a borrow of the box itself is needed
        if m.autoref == 0 || m.derefs == 0 { return; }
        let msg = if m.autoref == 1 { "unnecessary allocation, use `&` instead" } else { "unnecessary allocation, use `&mut` instead" };
        self.emit(&UNUSED_ALLOCATION, self.ast.expr(recv).lo, msg);
    }

    // ---- middle/stability.rs: deprecation
    fn check_deprecated(&self, d: DefId, pos: u32) {
        if lint::external(self.prog, self.body.file, pos) { return; }
        let dp = Dp { prog: self.prog, tcx: self.tcx };
        let Some((future, msg)) = dp.message(d, self.body.def) else { return };
        self.emit(if future { &DEPRECATED_IN_FUTURE } else { &DEPRECATED }, pos, &msg);
    }

    // ---- noop_method_call.rs
    fn check_noop_method_call(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, row) = self.ast.expr(e).kind else { return };
        let m = *self.ast.row(row);
        if lint::external(self.prog, self.body.file, m.name.lo) { return; }
        let Some(mr) = self.body.methods.get(&e.0) else { return };
        let did = mr.def;
        let d = self.prog.def(did);
        if d.kind != DefKind::AssocFn || d.parent == crate::program::NO_DEF { return; }
        // the trait of the method (the trait itself, or the trait an impl method implements)
        let parent = self.prog.def(d.parent);
        let trait_id = match parent.kind {
            DefKind::Trait => d.parent,
            DefKind::Impl => match self.tcx.impl_trait.get(&d.parent) { Some(t) => *t, None => return },
            _ => return,
        };
        let trait_ = if Some(trait_id) == self.lang(LangItem::Clone) { "Clone" }
            else if Some(trait_id) == self.lang(LangItem::Deref) { "Deref" }
            else if self.diag_item(trait_id).as_deref() == Some("Borrow") { "Borrow" }
            else { return };
        // resolve the trait method to the impl's method
        let resolved = if parent.kind == DefKind::Trait {
            let args = self.tcx.tys.list(mr.args).to_vec();
            if args.iter().any(|&a| self.tcx.tys.has_infer(a) || self.tcx.tys.flags(a).intersects(crate::types::TypeFlags::HAS_PARAM)) { return; }
            let env = crate::tcx::TypingEnv::fully_monomorphized(self.prog);
            match self.tcx.resolve_instance_raw(self.prog, env, did, &args) {
                Ok(i) => match i.def { crate::instance::InstanceKind::Item(r) => r, _ => return },
                Err(_) => return,
            }
        } else { did };
        let Some(name) = self.diag_item(resolved) else { return };
        if !matches!(name.as_str(), "noop_method_borrow" | "noop_method_clone" | "noop_method_deref") { return; }
        let (Some(rt), Some(et)) = (self.expr_ty(recv), self.expr_ty(e)) else { return };
        // an overloaded deref among the receiver's adjustments: not linted
        if (0..mr.derefs as usize).any(|i| [e.0, recv.0, unparen(self.ast, recv).0].iter().any(|k| self.body.ov_deref(*k, i).is_some())) { return; }
        let pos = self.ast.expr(recv).hi;
        let (rt, et) = (self.tcx.tys.erase_all_regions(rt), self.tcx.tys.erase_all_regions(et));
        let method = self.prog.files[self.body.file as usize].text(m.name).to_string();
        if rt == et {
            let mut orig = et;
            while let TyKind::Ref(_, i, _) = self.kind(orig) { orig = i; }
            self.emit(&NOOP_METHOD_CALL, pos, &format!("call to `.{method}()` on a reference in this situation does nothing"));
            let _ = (trait_, orig);
        } else {
            let ts = crate::typeck::ty_to_string(false, self.prog, self.tcx, et);
            match name.as_str() {
                "noop_method_borrow" => {}
                "noop_method_clone" => self.emit(&SUSPICIOUS_DOUBLE_REF_OP, pos, &format!("using `.clone()` on a double reference, which returns `{ts}` instead of cloning the inner type")),
                "noop_method_deref" => self.emit(&SUSPICIOUS_DOUBLE_REF_OP, pos, &format!("using `.deref()` on a double reference, which returns `{ts}` instead of dereferencing the inner type")),
                _ => {}
            }
        }
    }

    // ---- for_loops_over_fallibles.rs
    fn check_for_loop_fallible(&self, f: ForRow) {
        let Some(ty) = self.expr_ty(f.iter) else { return };
        let (adt, by_ref) = match self.kind(ty) {
            TyKind::Adt(d, ..) => (d, false),
            TyKind::Ref(_, t, _) => match self.kind(t) { TyKind::Adt(d, ..) => (d, true), _ => return },
            _ => return,
        };
        let (article, name) = if Some(adt) == self.lang_parent(LangItem::OptionSome) {
            (if by_ref { "a" } else { "an" }, "Option")
        } else if Some(adt) == self.lang_parent(LangItem::ResultOk) {
            ("a", "Result")
        } else { return };
        let pos = self.ast.expr(f.iter).lo;
        self.emit(&FOR_LOOPS_OVER_FALLIBLES, pos, &format!("for loop over {article} `{name}`. This is more readably written as an `if let` statement"));
    }

    // ---- map_unit_fn.rs
    fn check_map_unit_fn(&self, e: ExprId) {
        let ExprKind::MethodCall(recv, row) = self.ast.expr(e).kind else { return };
        let m = *self.ast.row(row);
        let args = self.ast.list(m.args);
        if args.len() != 1 { return; }
        let pf = &self.prog.files[self.body.file as usize];
        if pf.text(m.name) != "map" { return; }
        let pos = m.name.lo;
        if lint::external(self.prog, self.body.file, pos) { return; }
        // the receiver's method is an inherent slice method (`.iter()`)
        let Some(rm) = self.body.methods.get(&unparen(self.ast, recv).0) else { return };
        let ip = self.prog.def(rm.def).parent;
        if ip == crate::program::NO_DEF || self.prog.def(ip).kind != DefKind::Impl { return; }
        if !self.tcx.impl_self.get(&ip).is_some_and(|&st| matches!(self.kind(st), TyKind::Slice(_))) { return; }
        let Some(mm) = self.body.methods.get(&e.0) else { return };
        if self.diag_item(mm.def).as_deref() != Some("IteratorMap") { return; }
        let Some(at) = self.expr_ty(args[0]) else { return };
        let ret = match self.kind(at) {
            TyKind::Closure(_, _, ca) => match self.kind(ca.closure_sig_as_fn_ptr_ty) { TyKind::FnPtr(_, r, _, _) => r, _ => return },
            TyKind::FnDef(d, ..) => match self.tcx.fn_item_sig(self.prog, d) { Some(sig) => sig.ret, None => return },
            _ => return,
        };
        let unit = matches!(self.kind(ret), TyKind::Tuple(l) if self.tcx.tys.list(l).is_empty());
        if !(unit || matches!(self.kind(ret), TyKind::Never)) { return; }
        self.emit(&MAP_UNIT_FN, pos, "`Iterator::map` call that discard the iterator's values");
    }

    // ---- builtin.rs DerefNullPtr
    fn is_null_ptr(&self, e: ExprId) -> bool {
        let e = unparen(self.ast, e);
        let Some(t) = self.expr_ty(e) else { return false };
        let TyKind::Ptr(_, pointee) = self.kind(t) else { return false };
        let f = self.tcx.tys.flags(pointee);
        if !f.intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_INFER | crate::types::TypeFlags::HAS_ALIAS) {
            let lcx = crate::layout::LayoutCx::new(self.prog);
            let lay = crate::layout::Layouts::new();
            if !lay.is_unsized(&lcx, self.tcx, pointee) && lay.of(&lcx, self.tcx, pointee).size == 0 { return false; }
        }
        match self.ast.expr(e).kind {
            ExprKind::Cast(x, ty) => {
                if matches!(self.ast.ty(ty), Ty::Ptr(..)) {
                    let x = unparen(self.ast, x);
                    let zero = matches!(self.ast.expr(x).kind, ExprKind::Lit(LitKind::Int))
                        && crate::const_fold::parse_int_lit(std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(self.ast.expr(x).lo, self.ast.expr(x).hi)).unwrap_or("")) == Some(0);
                    return zero || self.is_null_ptr(x);
                }
                false
            }
            ExprKind::Call(f, _) => match self.path_def(f) {
                Some(d) => matches!(self.diag_item(d).as_deref(), Some("ptr_null" | "ptr_null_mut")),
                None => false,
            },
            _ => false,
        }
    }

    fn check_deref_nullptr(&mut self, e: ExprId, x: ExprId) {
        if self.expr_ty(x).is_some_and(|t| matches!(self.kind(t), TyKind::Ptr(..))) && self.is_null_ptr(x) {
            // `&raw *NULL` is ok
            if self.raw_operand != Some(e) {
                self.emit(&DEREF_NULLPTR, self.ast.expr(e).lo, "dereferencing a null pointer");
            }
        }
    }

    /// rustc function_cast_as_integer.rs: `foo as usize` casts a function item to an integer.
    fn check_function_cast(&self, e: ExprId, x: ExprId) {
        let (Some(to), Some(from)) = (self.expr_ty(e), self.expr_ty(x)) else { return };
        if matches!(self.kind(to), TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Ptr(..)) { return; }
        if matches!(self.kind(from), TyKind::FnDef(..)) {
            self.emit(&FUNCTION_CASTS_AS_INTEGER, self.ast.expr(x).hi + 1, "direct cast of function item into an integer");
        }
    }

    // ---- rustc_hir_typeck cast.rs trivial_cast_lint
    fn check_trivial_cast(&self, e: ExprId, x: ExprId) {
        if self.body.coercion_casts.get(&x.0).is_none() { return; }
        let (Some(et), Some(ct)) = (self.expr_ty(x), self.expr_ty(e)) else { return };
        let numeric = |t: TyId| matches!(self.kind(t), TyKind::Int(_) | TyKind::Float(_) | TyKind::IntVar(_) | TyKind::FloatVar(_));
        let (is_num, lint) = if numeric(ct) && numeric(et) { (true, &TRIVIAL_NUMERIC_CASTS) } else { (false, &TRIVIAL_CASTS) };
        let (es, cs) = (crate::typeck::ty_to_string(false, self.prog, self.tcx, et), crate::typeck::ty_to_string(false, self.prog, self.tcx, ct));
        self.emit(lint, self.ast.expr(e).lo, &format!("trivial {}cast: `{}` as `{}`", if is_num { "numeric " } else { "" }, es, cs));
    }
}

fn unparen(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = ast.expr(e).kind { e = x; }
    e
}

impl<'a> Visitor<'a> for Late<'a> {
    fn ast(&self) -> &'a Ast { self.ast }
    fn enter_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    fn expr(&mut self, e: ExprId) {
        self.check_expr(e);
        ast_visit::walk_expr(self, e);
    }

    fn pat(&mut self, p: PatId) {
        match *self.ast.pat(p) {
            Pat::TupleStruct(path, _) | Pat::Struct(path, _, _) | Pat::Path(path) => {
                if let Some(Res::Def(d)) = self.body.pat_res.get(&p.0) { self.check_deprecated(*d, path.lo); }
            }
            _ => {}
        }
        // check_match: a by-value binding named like a unit variant of the matched enum (the lint level
        // is that of the enclosing arm, statement, .. attributes)
        if let Pat::Ident { name, by_ref: false, mutbl: false, sub: None } = *self.ast.pat(p) {
            if let Some(&l) = self.body.pat_local.get(&p.0) {
                let mut t = self.body.locals[l as usize].ty;
                while let TyKind::Ref(_, i, _) = self.kind(t) { t = i; }
                if let TyKind::Adt(ed, _, _) = self.kind(t) {
                    let text = self.prog.files[self.body.file as usize].text(name);
                    if self.tcx.adts.get(&ed).is_some_and(|a| a.is_enum && a.variants.iter().any(|v| v.shape == 0 && self.prog.syms.str(v.name) == text)) {
                        let en = crate::typeck::ty_to_string(false, self.prog, self.tcx, t);
                        self.emit(&lint::BINDINGS_WITH_VARIANT_NAME, name.lo, &format!("E0170: pattern binding `{}` is named the same as one of the variants of the type `{}`", text, en));
                    }
                }
            }
        }
        // rustc NonShorthandFieldPatterns: `field: field` (any binding mode) can be `field`
        if let Pat::Struct(_, fs, _) = *self.ast.pat(p) {
            let pf = &self.prog.files[self.body.file as usize];
            for f in self.ast.list(fs) {
                if f.shorthand { continue; }
                if let Pat::Ident { name, sub: None, .. } = *self.ast.pat(f.pat) {
                    if self.body.pat_local.contains_key(&f.pat.0) && pf.text(name) == pf.text(f.name) && pf.expansion_kind(name.lo) == pf.expansion_kind(f.name.lo) {
                        self.emit(&NON_SHORTHAND_FIELD_PATTERNS, f.name.lo, &format!("the `{}:` in this pattern is redundant", pf.text(f.name)));
                    }
                }
            }
        }
        ast_visit::walk_pat(self, p);
    }

    fn arm(&mut self, a: &Arm) {
        let saved = self.arm_body.replace(a.body);
        ast_visit::walk_arm(self, a);
        self.arm_body = saved;
    }

    fn stmt(&mut self, s: &Stmt, _b: BlockId) {
        match *s {
            Stmt::Expr(e, true) => {
                let attrs = self.ast.stmt_attrs(e);
                self.enter_attrs(attrs);
                self.check_path_statement(e);
                self.check_unused_results(self.ast.expr(e).lo, e);
                self.leave_attrs(attrs);
            }
            Stmt::Let(r) => {
                let l = *self.ast.row(r);
                self.enter_attrs(l.attrs);
                if let Some(x) = l.init { self.check_non_binding(Some(l.pat), Some(x), self.ast.expr(x).lo); }
                self.leave_attrs(l.attrs);
            }
            Stmt::Attrs(r) => {
                let a = *self.ast.row(r);
                if a.semi {
                    self.enter_attrs(a.attrs);
                    self.check_path_statement(a.expr);
                    self.check_unused_results(self.ast.expr(a.expr).lo, a.expr);
                    self.leave_attrs(a.attrs);
                }
            }
            _ => {}
        }
        ast_visit::walk_stmt(self, s);
    }
}

// ---------------------------------------------------------------- known_panics_lint
//
// rustc_mir_transform known_panics_lint.rs: constant propagation over the body's MIR with
// lint checks at the arithmetic operations, divisions and index operations. The port follows the
// propagation over the typed syntax tree in MIR evaluation order: operands are known when they
// are literals, constants, the operations on known operands that do not fail, and locals that
// are assigned exactly once and never borrowed (rustc `FullConstProp`); a conditional whose
// condition is known visits only the taken branch, and code after a diverging expression is not
// visited. Locals with several assignments (`OnlyInsideOwnBlock`) are not propagated.

pub const ARITHMETIC_OVERFLOW: Lint = lint::lint("arithmetic_overflow");
pub const UNCONDITIONAL_PANIC: Lint = lint::lint("unconditional_panic");

#[derive(Clone, Copy, PartialEq, Debug)]
enum V {
    Unknown,
    /// the bits of an integer (sign-extended to 128 bits for a signed value)
    Int(u128),
    Bool(bool),
    Float(f64),
    /// an array or tuple whose element values are in `Kp::aggs`
    Agg(u32),
}

#[derive(Clone, Copy, Default)]
struct LocalInfo {
    assigns: u32,
    borrowed: bool,
}

/// rustc `CanConstProp`: how often each local is assigned and whether its address is taken.
struct Scan<'a> {
    body: &'a Body,
    ast: &'a Ast,
    info: Vec<LocalInfo>,
    in_closure: u32,
}

impl<'a> Scan<'a> {
    fn root_local(&self, e: ExprId) -> Option<u32> {
        let mut e = e;
        loop {
            match self.ast.expr(e).kind {
                ExprKind::Paren(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Index(x, _) => e = x,
                ExprKind::Path(_) => return match self.body.res.get(&e.0) { Some(Res::Local(l)) => Some(*l), _ => None },
                _ => return None,
            }
        }
    }
    fn borrow(&mut self, e: ExprId) {
        if let Some(l) = self.root_local(e) { if let Some(i) = self.info.get_mut(l as usize) { i.borrowed = true; } }
    }
    fn assign(&mut self, e: ExprId) {
        if let Some(l) = self.root_local(e) { if let Some(i) = self.info.get_mut(l as usize) { i.assigns += 1; } }
    }
    fn pat_by_ref(&self, p: PatId) -> bool {
        struct R<'b> { ast: &'b Ast, body: &'b Body, found: bool }
        impl<'b> Visitor<'b> for R<'b> {
            fn ast(&self) -> &'b Ast { self.ast }
            fn pat(&mut self, p: PatId) {
                if let Pat::Ident { by_ref, .. } = *self.ast.pat(p) {
                    if by_ref || self.body.pat_bind_ref.get(&p.0).is_some() { self.found = true; }
                }
                ast_visit::walk_pat(self, p);
            }
        }
        let mut r = R { ast: self.ast, body: self.body, found: false };
        r.pat(p);
        r.found
    }
}

impl<'a> Visitor<'a> for Scan<'a> {
    fn ast(&self) -> &'a Ast { self.ast }
    fn pat(&mut self, p: PatId) {
        if let Pat::Ident { .. } = *self.ast.pat(p) {
            if let Some(&l) = self.body.pat_local.get(&p.0) {
                if let Some(i) = self.info.get_mut(l as usize) { i.assigns += 1; }
            }
        }
        ast_visit::walk_pat(self, p);
    }
    fn stmt(&mut self, s: &Stmt, b: BlockId) {
        if let Stmt::Let(r) = *s {
            let l = *self.ast.row(r);
            if let Some(x) = l.init { if self.pat_by_ref(l.pat) { self.borrow(x); } }
        }
        let _ = b;
        ast_visit::walk_stmt(self, s);
    }
    fn expr(&mut self, e: ExprId) {
        match self.ast.expr(e).kind {
            ExprKind::AddrOf(_, _, x) => self.borrow(x),
            ExprKind::Assign(l, _) | ExprKind::AssignOp(_, l, _) => self.assign(l),
            ExprKind::MethodCall(recv, _) => {
                if self.body.methods.get(&e.0).is_some_and(|m| m.autoref != 0) { self.borrow(recv); }
            }
            ExprKind::Mac(m) => {
                let m = *self.ast.row(m);
                if m.args != NO_MAC_ARGS {
                    let args = *self.ast.mac_arg(m.args);
                    for &x in self.ast.list(args.exprs) { self.borrow(x); }
                }
            }
            ExprKind::Match(x, arms) => {
                if self.ast.list(arms).iter().any(|a| self.pat_by_ref(a.pat)) { self.borrow(x); }
            }
            ExprKind::Let(p, x) => { if self.pat_by_ref(p) { self.borrow(x); } }
            ExprKind::Path(_) => {
                if self.in_closure > 0 { self.borrow(e); }
            }
            ExprKind::Closure(_) => {
                self.in_closure += 1;
                ast_visit::walk_expr(self, e);
                self.in_closure -= 1;
                return;
            }
            _ => {}
        }
        ast_visit::walk_expr(self, e);
    }
}

struct Kp<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    stack: Vec<Attr>,
    info: Vec<LocalInfo>,
    env: std::collections::HashMap<u32, V>,
    aggs: Vec<Vec<V>>,
    /// the code being visited is unreachable (after a diverging expression)
    dead: bool,
}

fn mask(bytes: u32) -> u128 { if bytes >= 16 { u128::MAX } else { (1u128 << (bytes * 8)) - 1 } }

/// The sign-extended value of `bits` of an integer type of `bytes` bytes.
fn signed_of(bits: u128, bytes: u32) -> i128 {
    let shift = 128 - bytes * 8;
    if shift == 0 { bits as i128 } else { ((bits << shift) as i128) >> shift }
}

impl<'a> Kp<'a> {
    fn emit(&self, lint: &Lint, pos: u32, msg: &str) {
        if lint::external(self.prog, self.body.file, pos) { return; }
        lint::emit_in(self.prog, self.body.def, &self.stack, lint, self.body.file, pos, msg);
    }
    fn overflow(&self, pos: u32) { self.emit(&ARITHMETIC_OVERFLOW, pos, "this arithmetic operation will overflow"); }
    fn panics(&self, pos: u32) { self.emit(&UNCONDITIONAL_PANIC, pos, "this operation will panic at runtime"); }

    fn ty_of(&self, e: ExprId) -> Option<TyKind> {
        let i = self.body.expr_ix(e);
        if i < self.body.expr_ty.len() && self.body.expr_typed.get(i) { Some(*self.tcx.tys.kind(self.body.expr_ty[i])) } else { None }
    }
    /// (bytes, signed) of an integer-typed expression.
    fn int_of(&self, e: ExprId) -> Option<(u32, bool)> {
        match self.ty_of(e)? { TyKind::Int(p) => Some(crate::types::Types::int_info(p)), _ => None }
    }
    fn is_never(&self, e: ExprId) -> bool { matches!(self.ty_of(e), Some(TyKind::Never)) }

    fn enter(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    fn block(&mut self, b: BlockId) -> V {
        let blk = *self.ast.block(b);
        self.enter(blk.attrs);
        let mut last = V::Unknown;
        let stmts = self.ast.stmts(b);
        for s in stmts {
            if self.dead { break; }
            last = V::Unknown;
            match *s {
                Stmt::Let(r) => {
                    let l = *self.ast.row(r);
                    self.enter(l.attrs);
                    if let Some(x) = l.init {
                        let v = self.eval(x);
                        if let Some(eb) = l.else_ {
                            let d = self.dead;
                            self.block(eb);
                            self.dead = d;
                        }
                        if let Pat::Ident { by_ref: false, sub: None, .. } = *self.ast.pat(l.pat) {
                            if let Some(&loc) = self.body.pat_local.get(&l.pat.0) {
                                if let Some(i) = self.info.get(loc as usize) {
                                    if i.assigns == 1 && !i.borrowed && v != V::Unknown { self.env.insert(loc, v); }
                                }
                            }
                        }
                    }
                    self.leave(l.attrs);
                }
                Stmt::Expr(e, semi) => {
                    let attrs = self.ast.stmt_attrs(e);
                    self.enter(attrs);
                    let v = self.eval(e);
                    self.leave(attrs);
                    if !semi { last = v; }
                }
                Stmt::Attrs(r) => {
                    let a = *self.ast.row(r);
                    self.enter(a.attrs);
                    self.eval(a.expr);
                    self.leave(a.attrs);
                }
                Stmt::Item(_) | Stmt::Empty => {}
            }
        }
        self.leave(blk.attrs);
        last
    }

    /// Visits `f` as an alternative path: the dead state starts clean and its end is returned.
    fn branch(&mut self, f: impl FnOnce(&mut Self)) -> bool {
        let saved = self.dead;
        self.dead = false;
        f(self);
        let d = self.dead;
        self.dead = saved;
        d
    }

    fn eval(&mut self, e: ExprId) -> V {
        if self.dead { return V::Unknown; }
        let ex = *self.ast.expr(e);
        let v = self.eval_kind(e, ex);
        if self.is_never(e) { self.dead = true; }
        v
    }

    fn eval_kind(&mut self, e: ExprId, ex: Expr) -> V {
        let ast = self.ast;
        match ex.kind {
            ExprKind::Lit(LitKind::Bool(b)) => V::Bool(b),
            ExprKind::Lit(LitKind::Int) => {
                let Some(TyKind::Int(p)) = self.ty_of(e) else { return V::Unknown };
                let (bytes, signed) = crate::types::Types::int_info(p);
                let text = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(ex.lo, ex.hi)).unwrap_or("");
                match crate::const_fold::parse_int_lit(text) {
                    Some(v) => { let v = v & mask(bytes); V::Int(if signed { signed_of(v, bytes) as u128 } else { v }) }
                    None => V::Unknown,
                }
            }
            ExprKind::Lit(LitKind::Float) => {
                let text = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(ex.lo, ex.hi)).unwrap_or("");
                let digits: String = text.chars().filter(|c| *c != '_').collect();
                let digits = digits.trim_end_matches("f32").trim_end_matches("f64");
                digits.parse::<f64>().map_or(V::Unknown, V::Float)
            }
            ExprKind::Lit(_) | ExprKind::Underscore | ExprKind::Continue(_) => V::Unknown,
            ExprKind::Paren(x) => self.eval(x),
            ExprKind::Path(_) => self.eval_path(e),
            ExprKind::Unary(UnOp::Neg, x) if matches!(self.ast.expr(unparen(self.ast, x)).kind, ExprKind::Lit(LitKind::Int)) && self.int_of(x).is_some() => {
                // a negated literal is one constant (THIR: the literal is negated, `-128i8` is fine)
                let (bytes, signed) = self.int_of(x).unwrap();
                if !signed { return V::Unknown; }
                match self.eval_lit_wide(unparen(self.ast, x)) {
                    Some(v) => V::Int(signed_of((v as i128).wrapping_neg() as u128 & mask(bytes), bytes) as u128),
                    None => V::Unknown,
                }
            }
            ExprKind::Unary(op, x) => {
                let v = self.eval(x);
                if self.body.binops.get(&e.0).is_some() { return V::Unknown; }
                match (op, v) {
                    (UnOp::Neg, V::Int(b)) => {
                        let Some((bytes, true)) = self.int_of(x) else { return V::Unknown };
                        let s = signed_of(b, bytes);
                        let min = if bytes == 16 { i128::MIN } else { -(1i128 << (bytes * 8 - 1)) };
                        if s == min { self.overflow(ex.lo); V::Unknown } else { V::Int((-s) as u128) }
                    }
                    (UnOp::Neg, V::Float(f)) => V::Float(-f),
                    (UnOp::Not, V::Bool(b)) => V::Bool(!b),
                    (UnOp::Not, V::Int(b)) => match self.int_of(x) {
                        Some((bytes, signed)) => { let r = !b & mask(bytes); V::Int(if signed { signed_of(r, bytes) as u128 } else { r }) }
                        None => V::Unknown,
                    },
                    _ => V::Unknown,
                }
            }
            ExprKind::Binary(op, a, b) => self.eval_binary(e, ex.lo, op, a, b),
            ExprKind::Assign(l, r) => {
                self.eval(r);
                self.eval(l);
                V::Unknown
            }
            ExprKind::AssignOp(op, l, r) => {
                let rv = self.eval(r);
                let lv = self.eval(l);
                if self.body.binops.get(&e.0).is_none() {
                    self.int_binop(ex.lo, op, lv, rv, self.int_of(l), self.int_of(r));
                }
                V::Unknown
            }
            ExprKind::Cast(x, _) => {
                let v = self.eval(x);
                self.eval_cast(e, x, v)
            }
            ExprKind::Index(base, idx) => {
                let bv = self.eval(base);
                let iv = self.eval(idx);
                if self.body.ov_index.get(&e.0).is_some() { return V::Unknown; }
                let mut bt = self.body.expr_ty.get(self.body.expr_ix(base)).copied();
                while let Some(t) = bt {
                    if let TyKind::Ref(_, inner, _) = *self.tcx.tys.kind(t) { bt = Some(inner); } else { break; }
                }
                if let (Some(t), V::Int(i), Some((8, false))) = (bt, iv, self.int_of(idx)) {
                    if let TyKind::Array(_, len) = *self.tcx.tys.kind(t) {
                        if let Some(n) = len.try_to_target_usize(&self.tcx.tys) {
                            if i >= n as u128 { self.panics(ex.lo); return V::Unknown; }
                        }
                    }
                }
                match (bv, iv) {
                    (V::Agg(a), V::Int(i)) => self.aggs[a as usize].get(i as usize).copied().unwrap_or(V::Unknown),
                    _ => V::Unknown,
                }
            }
            ExprKind::If(c, then, els) => {
                let cv = self.eval(c);
                if self.dead { return V::Unknown; }
                let take_then = !matches!(cv, V::Bool(false));
                let take_else = !matches!(cv, V::Bool(true));
                let d1 = if take_then { self.branch(|s| { s.block(then); }) } else { true };
                let d2;
                match els.get() {
                    Some(x) => { if take_else { d2 = self.branch(|s| { s.eval(x); }); } else { d2 = true; } }
                    None => { d2 = !take_else; }
                }
                self.dead = d1 && d2;
                V::Unknown
            }
            ExprKind::Let(_, x) => { self.eval(x); V::Unknown }
            ExprKind::Match(x, arms) => {
                let sv = self.eval(x);
                if self.dead { return V::Unknown; }
                let arms: Vec<Arm> = ast.list(arms).to_vec();
                let mut all_dead = true;
                if matches!(sv, V::Int(_) | V::Bool(_)) {
                    // a known scrutinee: only the arm it selects is visited; when the selection cannot
                    // be decided no arm is visited (a missed lint rather than a false one)
                    for a in &arms {
                        match self.pat_matches(a.pat, sv) {
                            Some(false) => continue,
                            Some(true) => {
                                if let Some(g) = a.guard {
                                    match self.eval(g) {
                                        V::Bool(false) => continue,
                                        V::Bool(true) => {}
                                        _ => { all_dead = false; break; }
                                    }
                                }
                                self.enter(a.attrs);
                                all_dead = self.branch(|s| { s.eval(a.body); });
                                self.leave(a.attrs);
                                break;
                            }
                            None => { all_dead = false; break; }
                        }
                    }
                } else {
                    for a in &arms {
                        self.enter(a.attrs);
                        let d = self.branch(|s| {
                            if let Some(g) = a.guard { s.eval(g); }
                            s.eval(a.body);
                        });
                        self.leave(a.attrs);
                        all_dead &= d;
                    }
                    if arms.is_empty() { all_dead = true; }
                }
                self.dead = all_dead;
                V::Unknown
            }
            ExprKind::While(row) => {
                let w = *ast.row(row);
                let cv = self.eval(w.cond);
                if !matches!(cv, V::Bool(false)) && !self.dead { self.branch(|s| { s.block(w.body); }); }
                self.dead = false;
                V::Unknown
            }
            ExprKind::Loop(b, _) => { self.branch(|s| { s.block(b); }); self.dead = false; V::Unknown }
            ExprKind::For(row) => {
                let f = *ast.row(row);
                self.eval(f.iter);
                if !self.dead { self.branch(|s| { s.block(f.body); }); }
                self.dead = false;
                V::Unknown
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => self.block(b),
            ExprKind::Async(..) | ExprKind::ConstBlock(_) => V::Unknown,
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                let env = std::mem::take(&mut self.env);
                let d = self.dead;
                self.dead = false;
                self.eval(c.body);
                self.dead = d;
                self.env = env;
                V::Unknown
            }
            ExprKind::Call(f, args) => {
                self.eval(f);
                for &a in ast.list(args) { self.eval(a); }
                V::Unknown
            }
            ExprKind::MethodCall(recv, row) => {
                self.eval(recv);
                for &a in ast.list(ast.row(row).args) { self.eval(a); }
                V::Unknown
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                let vals: Vec<V> = ast.list(xs).iter().map(|&x| self.eval(x)).collect();
                self.aggs.push(vals);
                V::Agg(self.aggs.len() as u32 - 1)
            }
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                for f in ast.list(s.fields) { self.eval(f.expr); }
                if let Some(b) = s.base.get() { self.eval(b); }
                V::Unknown
            }
            ExprKind::Repeat(x, _) => { self.eval(x); V::Unknown }
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { self.eval(x); }
                if let Some(x) = b.get() { self.eval(x); }
                V::Unknown
            }
            ExprKind::TupleField(x, i) => {
                match self.eval(x) {
                    V::Agg(a) => self.aggs[a as usize].get(i as usize).copied().unwrap_or(V::Unknown),
                    _ => V::Unknown,
                }
            }
            ExprKind::Field(x, _) | ExprKind::AddrOf(_, _, x) | ExprKind::Try(x)
            | ExprKind::Await(x) | ExprKind::Box(x) => { self.eval(x); V::Unknown }
            ExprKind::Break(_, x) | ExprKind::Return(x) | ExprKind::Yield(x) => {
                if let Some(x) = x.get() { self.eval(x); }
                V::Unknown
            }
            ExprKind::Mac(m) => {
                let m = *ast.row(m);
                if m.args != NO_MAC_ARGS {
                    let args = *ast.mac_arg(m.args);
                    for &x in ast.list(args.exprs) { self.eval(x); }
                }
                V::Unknown
            }
        }
    }

    /// The unsigned value of an integer literal (before the type's range is applied).
    fn eval_lit_wide(&self, e: ExprId) -> Option<u128> {
        let ex = self.ast.expr(e);
        let text = std::str::from_utf8(self.prog.files[self.body.file as usize].bytes(ex.lo, ex.hi)).unwrap_or("");
        crate::const_fold::parse_int_lit(text)
    }

    fn eval_path(&mut self, e: ExprId) -> V {
        match self.body.res.get(&e.0) {
            Some(Res::Local(l)) => self.env.get(l).copied().unwrap_or(V::Unknown),
            Some(Res::Def(d)) => {
                let d = *d;
                match self.prog.def(d).kind {
                    DefKind::Const | DefKind::AssocConst => self.eval_const(e, d),
                    DefKind::Variant => self.variant_discr(d),
                    _ => V::Unknown,
                }
            }
            _ => V::Unknown,
        }
    }

    /// A unit variant's discriminant (the operand of an `as` cast).
    fn variant_discr(&self, d: DefId) -> V {
        let parent = self.prog.def(d).parent;
        let Some(a) = self.tcx.adts.get(&parent) else { return V::Unknown };
        let Some(idx) = a.variants.iter().position(|v| v.def == d) else { return V::Unknown };
        if a.variants[idx].shape != 0 { return V::Unknown; }
        match a.discriminant_for_variant(self.tcx, self.prog, idx as u32) {
            Some(disc) => V::Int(disc.signed_value() as u128),
            None => V::Unknown,
        }
    }

    /// Whether evaluating constant `d` cannot loop: its initializer is arithmetic over literals
    /// and other such constants (no cycles, no calls).
    fn const_terminates(&self, d: DefId, visited: &mut Vec<DefId>) -> bool {
        if visited.contains(&d) || visited.len() > 24 { return false; }
        visited.push(d);
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        let Ok(Some(init)) = self.prog.const_body(def.file, def.item) else { return false };
        let body = self.tcx.typeck(self.prog, d);
        if !body.errors.is_empty() { return false; }
        fn simple(k: &Kp, ast: &Ast, body: &Body, e: ExprId, visited: &mut Vec<DefId>) -> bool {
            match ast.expr(e).kind {
                ExprKind::Lit(_) => true,
                ExprKind::Paren(x) | ExprKind::Unary(_, x) | ExprKind::Cast(x, _) => simple(k, ast, body, x, visited),
                ExprKind::Binary(_, a, b) => simple(k, ast, body, a, visited) && simple(k, ast, body, b, visited),
                ExprKind::Path(_) => match body.res.get(&e.0) {
                    Some(crate::typeck::Res::Def(c)) if matches!(k.prog.def(*c).kind, DefKind::Const | DefKind::AssocConst | DefKind::Variant) => {
                        k.prog.def(*c).kind == DefKind::Variant || k.const_terminates(*c, visited)
                    }
                    _ => false,
                },
                _ => false,
            }
        }
        let r = simple(self, ast, &body, init, visited);
        visited.pop();
        r
    }

    fn eval_const(&self, e: ExprId, d: DefId) -> V {
        let Some((bytes, signed)) = self.int_of(e) else { return V::Unknown };
        let args: Vec<crate::consts::GenericArg> = match self.body.res_args.get(&e.0) {
            Some(l) => {
                let l = self.tcx.tys.list(*l);
                for &x in l {
                    let f = self.tcx.tys.flags(x);
                    if f.intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_INFER) || self.tcx.tys.has_infer(x) { return V::Unknown; }
                }
                l.iter().map(|x| match self.tcx.tys.kind(*x) {
                    TyKind::Const(c) => crate::consts::GenericArg::Const(*c),
                    _ => crate::consts::GenericArg::Type(*x),
                }).collect()
            }
            None => Vec::new(),
        };
        // a constant whose value depends on itself (or on code) is left to the evaluator proper
        if !self.const_terminates(d, &mut Vec::new()) { return V::Unknown; }
        let env = crate::tcx::TypingEnv::fully_monomorphized(self.prog);
        match self.tcx.const_eval_resolve(self.prog, env, d, &args) {
            Ok(v) => match v.try_to_leaf() {
                Some(s) if s.size() as u32 == bytes => { let b = s.to_bits(bytes as u8); V::Int(if signed { signed_of(b, bytes) as u128 } else { b }) }
                _ => V::Unknown,
            },
            Err(_) => V::Unknown,
        }
    }

    fn eval_binary(&mut self, e: ExprId, pos: u32, op: BinOp, a: ExprId, b: ExprId) -> V {
        match op {
            BinOp::And | BinOp::Or => {
                let lv = self.eval(a);
                let short = if op == BinOp::And { V::Bool(false) } else { V::Bool(true) };
                if lv == short { return lv; }
                let d = self.dead;
                let rv = self.eval(b);
                // the right operand is a path of its own: a diverging right operand does not make the join dead
                self.dead = d;
                match (lv, rv) {
                    (V::Bool(_), r) => r,
                    _ => V::Unknown,
                }
            }
            _ => {
                let lv = self.eval(a);
                let rv = self.eval(b);
                if self.dead || self.body.binops.get(&e.0).is_some() { return V::Unknown; }
                match self.ty_of(a) {
                    Some(TyKind::Int(_)) => self.int_binop(pos, op, lv, rv, self.int_of(a), self.int_of(b)),
                    Some(TyKind::Bool) => match (op, lv, rv) {
                        (BinOp::Eq, V::Bool(x), V::Bool(y)) => V::Bool(x == y),
                        (BinOp::Ne, V::Bool(x), V::Bool(y)) => V::Bool(x != y),
                        (BinOp::BitAnd, V::Bool(x), V::Bool(y)) => V::Bool(x & y),
                        (BinOp::BitOr, V::Bool(x), V::Bool(y)) => V::Bool(x | y),
                        (BinOp::BitXor, V::Bool(x), V::Bool(y)) => V::Bool(x ^ y),
                        _ => V::Unknown,
                    },
                    _ => V::Unknown,
                }
            }
        }
    }

    /// check_binary_op / the division assertions, then the operation's value.
    fn int_binop(&mut self, pos: u32, op: BinOp, l: V, r: V, lt: Option<(u32, bool)>, rt: Option<(u32, bool)>) -> V {
        let Some((lb, ls)) = lt else { return V::Unknown };
        let li = if let V::Int(x) = l { Some(x) } else { None };
        let ri = if let V::Int(x) = r { Some(x) } else { None };
        match op {
            BinOp::Shl | BinOp::Shr => {
                let Some((rb, _)) = rt else { return V::Unknown };
                if let Some(rx) = ri {
                    // the shift amount as an unsigned number of its own width
                    if (rx & mask(rb)) >= (lb * 8) as u128 { self.overflow(pos); return V::Unknown; }
                    if let Some(lx) = li {
                        let amt = (rx & mask(rb)) as u32;
                        let res = if op == BinOp::Shl { (lx << amt) & mask(lb) } else if ls { (signed_of(lx, lb) >> amt) as u128 & mask(lb) } else { (lx & mask(lb)) >> amt };
                        return V::Int(if ls { signed_of(res, lb) as u128 } else { res });
                    }
                }
                V::Unknown
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let (Some(lx), Some(rx)) = (li, ri) else { return V::Unknown };
                let (res, over) = if ls {
                    let (a, b) = (lx as i128, rx as i128);
                    let (v, o) = match op { BinOp::Add => a.overflowing_add(b), BinOp::Sub => a.overflowing_sub(b), _ => a.overflowing_mul(b) };
                    let fits = lb == 16 || { let min = -(1i128 << (lb * 8 - 1)); let max = (1i128 << (lb * 8 - 1)) - 1; v >= min && v <= max };
                    (v as u128, o || !fits)
                } else {
                    let (a, b) = (lx, rx);
                    let (v, o) = match op { BinOp::Add => a.overflowing_add(b), BinOp::Sub => a.overflowing_sub(b), _ => a.overflowing_mul(b) };
                    (v, o || (lb < 16 && v > mask(lb)))
                };
                if over { self.overflow(pos); return V::Unknown; }
                V::Int(res)
            }
            BinOp::Div | BinOp::Rem => {
                if ri == Some(0) { self.panics(pos); return V::Unknown; }
                if ls {
                    if let (Some(lx), Some(rx)) = (li, ri) {
                        let min = if lb == 16 { i128::MIN } else { -(1i128 << (lb * 8 - 1)) };
                        if rx as i128 == -1 && lx as i128 == min { self.panics(pos); return V::Unknown; }
                    }
                }
                let (Some(lx), Some(rx)) = (li, ri) else { return V::Unknown };
                if ls {
                    let (a, b) = (lx as i128, rx as i128);
                    V::Int(if op == BinOp::Div { a.wrapping_div(b) } else { a.wrapping_rem(b) } as u128)
                } else {
                    V::Int(if op == BinOp::Div { lx / rx } else { lx % rx })
                }
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => match (li, ri) {
                (Some(a), Some(b)) => {
                    let v = match op { BinOp::BitAnd => a & b, BinOp::BitOr => a | b, _ => a ^ b };
                    V::Int(if ls { signed_of(v & mask(lb), lb) as u128 } else { v & mask(lb) })
                }
                _ => V::Unknown,
            },
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match (li, ri) {
                (Some(a), Some(b)) => V::Bool(if ls {
                    let (a, b) = (a as i128, b as i128);
                    match op { BinOp::Eq => a == b, BinOp::Ne => a != b, BinOp::Lt => a < b, BinOp::Le => a <= b, BinOp::Gt => a > b, _ => a >= b }
                } else {
                    match op { BinOp::Eq => a == b, BinOp::Ne => a != b, BinOp::Lt => a < b, BinOp::Le => a <= b, BinOp::Gt => a > b, _ => a >= b }
                }),
                _ => V::Unknown,
            },
            BinOp::And | BinOp::Or => V::Unknown,
        }
    }

    fn eval_cast(&mut self, e: ExprId, x: ExprId, v: V) -> V {
        let Some(dest) = self.ty_of(e) else { return V::Unknown };
        let src = self.ty_of(x);
        match (dest, v) {
            (TyKind::Int(p), V::Int(b)) => {
                let (db, ds) = crate::types::Types::int_info(p);
                // an integer or a (sign-extended) enum discriminant: the value is already extended
                match src {
                    Some(TyKind::Int(_)) | Some(TyKind::Adt(..)) => { let t = b & mask(db); V::Int(if ds { signed_of(t, db) as u128 } else { t }) }
                    _ => V::Unknown,
                }
            }
            (TyKind::Int(p), V::Bool(b)) => { let _ = p; V::Int(b as u128) }
            (TyKind::Int(p), V::Float(f)) => {
                let (db, ds) = crate::types::Types::int_info(p);
                if f.is_nan() { return V::Int(0); }
                let r = if ds {
                    let min = if db == 16 { i128::MIN } else { -(1i128 << (db * 8 - 1)) };
                    let max = if db == 16 { i128::MAX } else { (1i128 << (db * 8 - 1)) - 1 };
                    (f as i128).clamp(min, max) as u128
                } else {
                    let max = mask(db);
                    if f <= 0.0 { 0 } else { (f as u128).min(max) }
                };
                V::Int(r)
            }
            (TyKind::Float(_), V::Int(b)) => match src {
                Some(TyKind::Int(p)) => { let (sb, ss) = crate::types::Types::int_info(p); V::Float(if ss { signed_of(b, sb) as f64 } else { (b & mask(sb)) as f64 }) }
                _ => V::Unknown,
            },
            (TyKind::Float(_), V::Float(f)) => V::Float(f),
            _ => V::Unknown,
        }
    }

    /// Whether the pattern matches the known scrutinee value (None: cannot tell).
    fn pat_matches(&mut self, p: PatId, v: V) -> Option<bool> {
        match *self.ast.pat(p) {
            Pat::Wild => Some(true),
            Pat::Paren(x) => self.pat_matches(x, v),
            Pat::Ident { by_ref: false, sub: None, .. } => if self.body.pat_local.get(&p.0).is_some() { Some(true) } else { None },
            Pat::Lit(e) => {
                let lv = self.eval(e);
                match (lv, v) { (V::Int(a), V::Int(b)) => Some(a == b), (V::Bool(a), V::Bool(b)) => Some(a == b), _ => None }
            }
            Pat::Or(ps) => {
                let ps: Vec<PatId> = self.ast.list(ps).to_vec();
                let mut undecided = false;
                for q in ps {
                    match self.pat_matches(q, v) { Some(true) => return Some(true), Some(false) => {} None => undecided = true }
                }
                if undecided { None } else { Some(false) }
            }
            _ => None,
        }
    }
}

/// Runs the known-panics lint over a function or associated constant body.
fn known_panics(prog: &Program, tcx: &Tcx, body: &Body, block: Option<BlockId>, expr: Option<ExprId>, params: &[PatId]) {
    let d = prog.def(body.def);
    // only functions, methods, closures and associated constants (a `const` item or a static is
    // evaluated by the interpreter anyway)
    if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::AssocConst) { return; }
    let ast = &prog.files[body.file as usize].ast;
    let mut sc = Scan { body, ast, info: vec![LocalInfo::default(); body.locals.len()], in_closure: 0 };
    for &p in params { sc.pat(p); }
    if let Some(b) = block { sc.block(b); }
    if let Some(e) = expr { sc.expr(e); }
    // arguments are assigned on entry
    let mut kp = Kp { prog, tcx, body, ast, stack: Vec::new(), info: sc.info, env: Default::default(), aggs: Vec::new(), dead: false };
    if let Some(b) = block { kp.block(b); }
    if let Some(e) = expr { kp.eval(e); }
}

// ---------------------------------------------------------------- item-level passes

pub const TEXT_DIRECTION_CODEPOINT_IN_COMMENT: Lint = lint::lint("text_direction_codepoint_in_comment");
pub const TEXT_DIRECTION_CODEPOINT_IN_LITERAL: Lint = lint::lint("text_direction_codepoint_in_literal");

/// Item-level passes over the definitions of the checked crates marked in `own`.
pub fn check_items(prog: &Program, tcx: &Tcx, own: &[bool]) {
    if !lint::enabled() { return; }
    text_direction(prog, own);
    improper_ctypes(prog, tcx, own);
    crate::lint_nonlocal::check(prog, tcx, own);
    crate::lint_late2::check_items(prog, own);
    crate::lint_late2::check_eq_impls(prog, tcx, own);
    crate::lint_late2::check_use_deprecations(prog, tcx, own);
    crate::lint_late2::check_signature_deprecations(prog, tcx, own);
    crate::lint_late2::check_refining_impls(prog, tcx, own);
}

/// rustc_ast::util::unicode TEXT_FLOW_CONTROL_CHARS.
fn is_text_flow_control(c: char) -> bool {
    matches!(c, '\u{202A}' | '\u{202B}' | '\u{202D}' | '\u{202E}' | '\u{2066}' | '\u{2067}' | '\u{2068}' | '\u{202C}' | '\u{2069}')
}

/// rustc_parse lexer `lint_unicode_text_flow` / `lint_literal_unicode_text_flow`: comments,
/// doc comments and string / char literals containing characters that change the visible
/// direction of text. Reported while lexing, at the crate root's lint level.
fn text_direction(prog: &Program, own: &[bool]) {
    use crate::lexer::{LexOptions, Mode, T};
    let mut seen = std::collections::HashSet::new();
    for m in &prog.mods {
        if !own.get(m.krate as usize).copied().unwrap_or(false) { continue; }
        let def = prog.def(m.def);
        // block modules have no file of their own
        if prog.block_owner.get(&def.sub).is_some() { continue; }
        if !seen.insert(m.file) { continue; }
        let pf = &prog.files[m.file as usize];
        let len = pf.src_len;
        if len == 0 { continue; }
        let src = pf.bytes(0, len);
        if !std::str::from_utf8(src).map_or(false, |s| s.chars().any(is_text_flow_control)) { continue; }
        let root = prog.mods[prog.crates[m.krate as usize].root_mod as usize].def;
        let mut toks = Vec::new();
        let opts = LexOptions { edition: pf.edition, mode: Mode::Rust, trivia: true };
        if crate::lexer::lex_with(src, &opts, &mut toks).is_err() { continue; }
        for t in &toks {
            let flow = |s: &[u8]| std::str::from_utf8(s).map_or(false, |s| s.chars().any(is_text_flow_control));
            let text = &src[t.lo as usize..t.hi as usize];
            match t.kind {
                T::Comment => {
                    if flow(text.get(2..).unwrap_or(&[])) {
                        lint::emit(prog, root, &TEXT_DIRECTION_CODEPOINT_IN_COMMENT, m.file, t.lo,
                            "unicode codepoint changing visible direction of text present in comment");
                    }
                }
                T::DocComment => {
                    if flow(text) {
                        lint::emit(prog, root, &TEXT_DIRECTION_CODEPOINT_IN_LITERAL, m.file, t.lo,
                            "unicode codepoint changing visible direction of text present in doc comment");
                    }
                }
                T::Str | T::RawStr | T::Char | T::CStr => {
                    if flow(text) {
                        lint::emit(prog, root, &TEXT_DIRECTION_CODEPOINT_IN_LITERAL, m.file, t.lo,
                            "unicode codepoint changing visible direction of text present in literal");
                    }
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------- improper_ctypes
//
// rustc_lint types/improper_ctypes.rs: the FFI-safety of the types in `extern` blocks
// (`improper_ctypes`) and of `extern "C" fn` definitions (`improper_ctypes_definitions`), and of
// the extern fn pointer types anywhere else.

pub const IMPROPER_CTYPES: Lint = lint::lint("improper_ctypes");
pub const IMPROPER_CTYPES_DEFINITIONS: Lint = lint::lint("improper_ctypes_definitions");

#[derive(Clone, Copy, PartialEq, Eq)]
enum CItemKind { Declaration, Definition }

#[derive(Clone, Copy, PartialEq, Eq)]
enum FnPos { Arg, Ret }

enum Ffi {
    Safe,
    Phantom(TyId),
    Unsafe { ty: TyId, reason: &'static str },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outer { None, NoneThroughFnPtr, Other }

const R_STATIC: u8 = 1;
const R_FUNC: u8 = 2;
const R_FN_RETURN: u8 = 4;
const R_DEFINED: u8 = 8;
const R_THEORETICAL: u8 = 16;

#[derive(Clone, Copy)]
struct VState { flags: u8, outer: Outer }

impl VState {
    fn next(self, ty_kind: &TyKind) -> Self {
        VState { flags: self.flags, outer: if matches!(ty_kind, TyKind::FnPtr(..)) { Outer::NoneThroughFnPtr } else { Outer::Other } }
    }
    fn next_in_fnptr(self, pos: FnPos) -> Self {
        VState { flags: match pos { FnPos::Ret => R_FUNC | R_THEORETICAL | R_FN_RETURN, FnPos::Arg => R_FUNC | R_THEORETICAL }, outer: Outer::NoneThroughFnPtr }
    }
    fn fn_entry(mode: CItemKind, pos: FnPos) -> Self {
        let flags = match (mode, pos) {
            (CItemKind::Definition, FnPos::Ret) => R_FUNC | R_FN_RETURN | R_DEFINED,
            (CItemKind::Declaration, FnPos::Ret) => R_FUNC | R_FN_RETURN,
            (CItemKind::Definition, FnPos::Arg) => R_FUNC | R_DEFINED,
            (CItemKind::Declaration, FnPos::Arg) => R_FUNC,
        };
        VState { flags, outer: Outer::None }
    }
    fn static_entry() -> Self { VState { flags: R_STATIC, outer: Outer::None } }
    fn in_function(self) -> bool { self.flags & R_FUNC != 0 }
    fn in_function_return(self) -> bool { self.flags & R_FN_RETURN != 0 }
    fn in_defined_function(self) -> bool { self.flags & R_DEFINED != 0 && self.in_function() }
    fn in_fnptr(self) -> bool { self.flags & R_THEORETICAL != 0 && self.in_function() }
    fn can_expect_ty_params(self) -> bool { self.flags & R_THEORETICAL != 0 || self.in_defined_function() }
}

fn is_rustic_abi(a: crate::types::Abi) -> bool {
    matches!(a, crate::types::Abi::Rust | crate::types::Abi::RustCall | crate::types::Abi::RelaxIntrinsic)
}

struct Ffic<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    env: crate::tcx::TypingEnv,
    krate: u32,
    cache: std::collections::HashSet<TyId>,
    base_ty: TyId,
    base_mode: CItemKind,
    /// the environment of the ADT whose fields are being judged
    adt_env: std::cell::Cell<Option<crate::tcx::TypingEnv>>,
}

impl<'a> Ffic<'a> {
    fn kind(&self, t: TyId) -> TyKind { *self.tcx.tys.kind(t) }
    fn norm(&self, t: TyId) -> TyId { self.tcx.try_normalize_erasing_regions(self.prog, self.env, t).unwrap_or(t) }

    fn lang(&self, i: LangItem) -> Option<DefId> { self.tcx.lang_items.get(i) }

    /// A field type normalized in its ADT's environment (`layout_of` of the field type there).
    fn norm_in_adt(&self, t: TyId) -> TyId {
        match self.adt_env.get() {
            Some(env) => self.tcx.try_normalize_erasing_regions(self.prog, env, t).unwrap_or(t),
            None => t,
        }
    }

    fn diag_item(&self, d: DefId) -> Option<String> {
        let def = self.prog.def(d);
        if !matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) { return None; }
        let ast = &self.prog.files[def.file as usize].ast;
        let p = self.prog.src(def.file);
        for a in ast.list(ast.item(def.item).attrs) {
            if lint::attr_path(self.prog, def.file, a) != b"rustc_diagnostic_item" { continue; }
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            let mut i = lo;
            while i < hi && crate::cfg::tok_str(p, i) != b"=" { i += 1; }
            if i + 1 < hi { return Some(String::from_utf8_lossy(crate::cfg::tok_str(p, i + 1)).trim_matches('"').to_string()); }
        }
        None
    }

    fn repr_transparent(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        ast.list(ast.item(def.item).attrs).iter().any(|a| lint::attr_path(self.prog, def.file, a) == b"repr" && lint::attr_args(self.prog, def.file, a).iter().any(|w| w == "transparent"))
    }

    fn is_sized(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) | TyKind::Foreign(_) => false,
            _ => match self.lang(LangItem::Sized) {
                Some(s) => self.tcx.type_implements_trait(self.prog, self.env, crate::next_solver::TraitRef { def_id: s, args: vec![crate::consts::GenericArg::Type(t)] }).must_apply_modulo_regions(),
                None => true,
            },
        }
    }

    /// A 1-ZST (size 0, alignment 1), judged structurally (layouts of generic types do not exist).
    fn is_1zst(&self, t: TyId, depth: u32) -> bool {
        if depth > 16 { return false; }
        let t = self.norm_in_adt(t);
        match self.kind(t) {
            TyKind::Tuple(l) => self.tcx.tys.list(l).iter().all(|&x| self.is_1zst(x, depth + 1)),
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                if a.is_phantom_data() { return true; }
                if a.is_union || a.repr_align > 1 { return false; }
                if a.is_enum {
                    return a.variants.is_empty() || (a.variants.len() == 1 && a.variants[0].fields.iter().all(|f| self.is_1zst(self.tcx.tys.subst(f.ty, self.tcx.tys.list(args)), depth + 1)) && a.repr_int.is_none());
                }
                a.variants[0].fields.iter().all(|f| self.is_1zst(self.tcx.tys.subst(f.ty, self.tcx.tys.list(args)), depth + 1))
            }
            _ => false,
        }
    }

    /// `transparent_newtype_field`: the variant's field that is not a 1-ZST.
    fn transparent_field(&self, d: DefId, vi: usize, identity_args: &[TyId]) -> Option<usize> {
        let a = self.tcx.adts.get(&d)?;
        let _ = identity_args;
        let saved = self.adt_env.replace(Some(crate::tcx::TypingEnv { limit: self.env.limit, param_env: self.tcx.param_env_id(self.prog, d), mode: crate::tcx::TypingMode::non_body_analysis() }));
        let r = a.variants[vi].fields.iter().position(|f| !self.is_1zst(f.ty, 0));
        self.adt_env.set(saved);
        r
    }

    fn field_ty(&self, d: DefId, vi: usize, fi: usize, args: crate::types::TyList) -> TyId {
        let a = &self.tcx.adts[&d];
        self.norm(self.tcx.tys.subst(a.variants[vi].fields[fi].ty, self.tcx.tys.list(args)))
    }

    fn ty_str(&self, t: TyId) -> String { crate::typeck::ty_to_string(false, self.prog, self.tcx, t) }

    // ---- types.rs: ty_is_known_nonnull / get_nullable_type / repr_nullable_ptr
    fn known_nonnull(&self, ty: TyId) -> bool {
        let ty = self.norm(ty);
        match self.kind(ty) {
            TyKind::FnPtr(..) | TyKind::Ref(..) => true,
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                if a.is_box() { return true; }
                if self.repr_transparent(d) && !a.is_union {
                    // `#[rustc_nonnull_optimization_guaranteed]`
                    let def = self.prog.def(d);
                    let ast = &self.prog.files[def.file as usize].ast;
                    if lint::has_attr(self.prog, def.file, ast.list(ast.item(def.item).attrs), "rustc_nonnull_optimization_guaranteed") { return true; }
                    if a.is_unsafe_cell() || a.is_unsafe_pinned() { return false; }
                    return (0..a.variants.len()).filter_map(|vi| self.transparent_field(d, vi, &[]).map(|fi| (vi, fi)))
                        .any(|(vi, fi)| self.known_nonnull(self.field_ty(d, vi, fi, args)));
                }
                false
            }
            _ => false,
        }
    }

    fn nullable_type(&self, ty: TyId) -> Option<TyId> {
        let ty = self.norm(ty);
        match self.kind(ty) {
            TyKind::Adt(d, args, _) => {
                let a = self.tcx.adts.get(&d)?;
                let (vi, fi) = (0..a.variants.len()).rev().find_map(|vi| self.transparent_field(d, vi, &[]).map(|fi| (vi, fi)))?;
                self.nullable_type(self.field_ty(d, vi, fi, args))
            }
            TyKind::Int(_) | TyKind::Ptr(..) | TyKind::FnPtr(..) => Some(ty),
            TyKind::Ref(_, inner, _) => Some(self.tcx.tys.intern(TyKind::Ptr(false, inner))).map(|p| {
                if let TyKind::Ref(m, i, _) = self.kind(ty) { self.tcx.tys.intern(TyKind::Ptr(m, i)) } else { p }
            }),
            _ => None,
        }
    }

    fn niche_candidate(&self, ty: TyId) -> bool {
        match self.kind(ty) {
            TyKind::Adt(d, _, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                if !self.is_1zst(ty, 0) { return false; }
                let empty = (a.is_struct() && a.variants[0].fields.is_empty()) || (a.is_enum() && a.variants.is_empty());
                !a.is_variant_list_non_exhaustive() && empty
            }
            TyKind::Tuple(l) => self.tcx.tys.list(l).is_empty(),
            _ => false,
        }
    }

    /// Whether the known-nonnull type is a single scalar (a thin pointer); a wide one has no
    /// scalar layout.
    fn is_scalar(&self, ty: TyId) -> bool {
        match self.kind(self.norm(ty)) {
            TyKind::FnPtr(..) | TyKind::Int(_) => true,
            TyKind::Ref(_, p, _) | TyKind::Ptr(_, p) => self.is_sized(p),
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return false };
                if a.is_box() { return args_first_sized(self, args); }
                (0..a.variants.len()).filter_map(|vi| self.transparent_field(d, vi, &[]).map(|fi| (vi, fi))).last()
                    .is_some_and(|(vi, fi)| self.is_scalar(self.field_ty(d, vi, fi, args)))
            }
            _ => false,
        }
    }

    fn repr_nullable_ptr(&self, ty: TyId) -> Option<TyId> {
        let TyKind::Adt(d, args, _) = self.kind(ty) else { return None };
        let a = self.tcx.adts.get(&d)?;
        if a.variants.len() != 2 { return None; }
        let (v1, v2) = (&a.variants[0], &a.variants[1]);
        let sub = |f: &crate::tcx::FieldT| self.norm(self.tcx.tys.subst(f.ty, self.tcx.tys.list(args)));
        let field_ty = match (v1.fields.len(), v2.fields.len()) {
            (0, 1) => sub(&v2.fields[0]),
            (1, 0) => sub(&v1.fields[0]),
            (1, 1) => {
                let (t1, t2) = (sub(&v1.fields[0]), sub(&v2.fields[0]));
                if self.niche_candidate(t1) { t2 } else if self.niche_candidate(t2) { t1 } else { return None }
            }
            _ => return None,
        };
        if !self.known_nonnull(field_ty) { return None; }
        if !self.is_scalar(field_ty) { return None; }
        self.nullable_type(field_ty)
    }

    // ---- the visitor
    fn visit_indirection(&mut self, state: VState, ty: TyId, inner: TyId, boxed: bool) -> Ffi {
        if boxed {
            if state.in_defined_function() || (state.in_fnptr() && self.base_mode == CItemKind::Definition) {
                if self.is_sized(inner) { return Ffi::Safe; }
                return Ffi::Unsafe { ty, reason: "box cannot be represented as a single pointer" };
            }
            if let TyKind::Adt(d, args, _) = self.kind(ty) { return self.visit_struct_or_union(state, ty, d, args); }
            return Ffi::Safe;
        }
        if (state.in_defined_function() || state.in_fnptr()) && self.is_sized(inner) {
            Ffi::Safe
        } else {
            let k = self.kind(ty);
            self.visit_type(state.next(&k), inner)
        }
    }

    fn visit_variant_fields(&mut self, state: VState, ty: TyId, d: DefId, vi: usize, args: crate::types::TyList) -> Ffi {
        let a = self.tcx.adts[&d].clone();
        let tk = self.kind(ty);
        let mut transparent_with_all_zst = false;
        if self.repr_transparent(d) {
            if let Some(fi) = self.transparent_field(d, vi, &[]) {
                let fty = self.field_ty(d, vi, fi, args);
                match self.visit_type(state.next(&tk), fty) {
                    Ffi::Unsafe { ty, .. } if self.is_unit(ty) => {}
                    r => return r,
                }
            } else {
                transparent_with_all_zst = true;
            }
        }
        let n = a.variants[vi].fields.len();
        let mut all_phantom = n > 0;
        for fi in 0..n {
            let fty = self.field_ty(d, vi, fi, args);
            all_phantom &= match self.visit_type(state.next(&tk), fty) {
                Ffi::Safe => false,
                Ffi::Unsafe { ty, .. } if self.is_unit(ty) => false,
                Ffi::Phantom(_) => true,
                r @ Ffi::Unsafe { .. } => return r,
            };
        }
        if all_phantom { Ffi::Phantom(ty) }
        else if transparent_with_all_zst { Ffi::Unsafe { ty, reason: "this struct contains only zero-sized fields" } }
        else { Ffi::Safe }
    }

    fn is_unit(&self, t: TyId) -> bool { matches!(self.kind(t), TyKind::Tuple(l) if self.tcx.tys.list(l).is_empty()) }

    fn visit_struct_or_union(&mut self, state: VState, ty: TyId, d: DefId, args: crate::types::TyList) -> Ffi {
        let a = self.tcx.adts[&d].clone();
        let transparent = self.repr_transparent(d);
        if !a.repr_c && !transparent {
            return if a.is_struct() {
                Ffi::Unsafe { ty, reason: "this struct has unspecified layout" }
            } else {
                Ffi::Unsafe { ty, reason: "this union has unspecified layout" }
            };
        }
        if a.variants.first().is_none_or(|v| v.fields.is_empty()) {
            return if a.is_struct() {
                Ffi::Unsafe { ty, reason: "this struct has no fields" }
            } else {
                Ffi::Unsafe { ty, reason: "this union has no fields" }
            };
        }
        self.visit_variant_fields(state, ty, d, 0, args)
    }

    fn visit_enum(&mut self, state: VState, ty: TyId, d: DefId, args: crate::types::TyList) -> Ffi {
        let a = self.tcx.adts[&d].clone();
        if a.variants.is_empty() { return Ffi::Safe; }
        let transparent = self.repr_transparent(d);
        if !a.repr_c && !transparent && a.repr_int.is_none() {
            // `Option<extern fn()>`, `Result<extern fn(), ()>`
            if let Some(inner) = self.repr_nullable_ptr(ty) {
                let k = self.kind(ty);
                return self.visit_type(state.next(&k), inner);
            }
            return Ffi::Unsafe { ty, reason: "enum has no representation hint" };
        }
        let non_exhaustive = a.is_variant_list_non_exhaustive() && self.prog.def(d).krate != self.krate;
        for vi in 0..a.variants.len() {
            // check_non_exhaustive_variant
            if non_exhaustive && a.variants[vi].shape != 0 {
                return Ffi::Unsafe { ty, reason: "this enum is non-exhaustive" };
            }
            match self.visit_variant_fields(state, ty, d, vi, args) {
                Ffi::Safe => {}
                r => return r,
            }
        }
        Ffi::Safe
    }

    fn visit_type(&mut self, state: VState, ty: TyId) -> Ffi {
        if !self.cache.insert(ty) { return Ffi::Safe; }
        let k = self.kind(ty);
        match k {
            TyKind::Adt(d, args, _) => {
                let Some(a) = self.tcx.adts.get(&d) else { return Ffi::Safe };
                if a.is_box() {
                    let inner = self.tcx.tys.list(args)[0];
                    return self.visit_indirection(state, ty, inner, true);
                }
                if a.is_phantom_data() { return Ffi::Phantom(ty); }
                if a.is_enum() { return self.visit_enum(state, ty, d, args); }
                if matches!(self.diag_item(d).as_deref(), Some("cstring_type" | "cstr_type")) && !matches!(self.kind(self.base_ty), TyKind::Ptr(true, _)) {
                    return Ffi::Unsafe { ty, reason: "`CStr`/`CString` do not have a guaranteed layout" };
                }
                self.visit_struct_or_union(state, ty, d, args)
            }
            TyKind::Int(p) if crate::types::Types::int_info(p).0 != 0 => Ffi::Safe,
            TyKind::Float(_) | TyKind::Bool => Ffi::Safe,
            TyKind::Char => Ffi::Unsafe { ty, reason: "the `char` type has no C equivalent" },
            TyKind::Slice(_) => Ffi::Unsafe { ty, reason: "slices have no C equivalent" },
            TyKind::Dyn(..) => Ffi::Unsafe { ty, reason: "trait objects have no C equivalent" },
            TyKind::Str => Ffi::Unsafe { ty, reason: "string slices have no C equivalent" },
            TyKind::Tuple(l) => {
                if self.tcx.tys.list(l).is_empty() && state.in_function_return() && matches!(state.outer, Outer::None | Outer::NoneThroughFnPtr) {
                    Ffi::Safe
                } else {
                    Ffi::Unsafe { ty, reason: "tuples have unspecified layout" }
                }
            }
            TyKind::Ptr(_, inner) if self.is_unit(inner) => Ffi::Safe,
            TyKind::Ptr(_, inner) => self.visit_indirection(state, ty, inner, false),
            TyKind::Ref(_, inner, _) => self.visit_indirection(state, ty, inner, false),
            TyKind::Array(inner, _) => {
                if state.in_function() && state.outer == Outer::None {
                    Ffi::Unsafe { ty, reason: "passing raw arrays by value is not FFI-safe" }
                } else {
                    self.visit_type(state.next(&k), inner)
                }
            }
            TyKind::FnPtr(params, ret, header, _) => {
                if is_rustic_abi(header.abi) {
                    return Ffi::Unsafe { ty, reason: "this function pointer has Rust-specific calling convention" };
                }
                for &arg in self.tcx.tys.list(params) {
                    match self.visit_type(state.next_in_fnptr(FnPos::Arg), arg) {
                        Ffi::Safe => {}
                        r => return r,
                    }
                }
                self.visit_type(state.next_in_fnptr(FnPos::Ret), ret)
            }
            TyKind::Foreign(_) | TyKind::Never => Ffi::Safe,
            TyKind::Alias(_, a) if matches!(a.kind, AliasTyKind::Opaque { .. }) => Ffi::Unsafe { ty, reason: "opaque types have no C equivalent" },
            TyKind::Param(_) | TyKind::Alias(..) if state.can_expect_ty_params() => Ffi::Safe,
            _ => Ffi::Safe,
        }
    }

    /// `visit_for_opaque_ty`: an opaque type anywhere inside the type.
    fn find_opaque(&self, ty: TyId, depth: u32) -> Option<TyId> {
        if depth > 32 || !self.tcx.tys.has_opaque(ty) { return None; }
        if let TyKind::Alias(_, a) = self.kind(ty) { if matches!(a.kind, AliasTyKind::Opaque { .. }) { return Some(ty); } }
        ty_children(self.tcx, ty).into_iter().find_map(|c| self.find_opaque(c, depth + 1))
    }

    fn check_type(&mut self, state: VState, ty: TyId) -> Ffi {
        let ty = self.norm(ty);
        if let Some(o) = self.find_opaque(ty, 0) {
            return Ffi::Unsafe { ty: o, reason: "opaque types have no C equivalent" };
        }
        self.visit_type(state, ty)
    }
}

fn args_first_sized(f: &Ffic, args: crate::types::TyList) -> bool {
    f.tcx.tys.list(args).first().is_some_and(|&t| f.is_sized(t))
}

/// The types inside a type, in the order of rustc's `super_visit_with`.
fn ty_children(tcx: &Tcx, ty: TyId) -> Vec<TyId> {
    match *tcx.tys.kind(ty) {
        TyKind::Tuple(l) => tcx.tys.list(l).to_vec(),
        TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => vec![e],
        TyKind::Adt(_, args, _) => tcx.tys.list(args).to_vec(),
        TyKind::FnPtr(params, ret, _, _) => { let mut v = tcx.tys.list(params).to_vec(); v.push(ret); v }
        TyKind::Alias(_, a) => tcx.tys.list(a.args).to_vec(),
        TyKind::Dyn(_, args, bindings, ..) => { let mut v = tcx.tys.list(args).to_vec(); v.extend(tcx.tys.list(bindings).iter().map(|b| b.1)); v }
        _ => Vec::new(),
    }
}

/// Position of a syntax type (the first path or nested element), when it has one.
pub(crate) fn ast_ty_pos(ast: &Ast, t: crate::ast::TyId) -> Option<u32> {
    match *ast.ty(t) {
        Ty::Path(p) => Some(p.lo),
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => ast_ty_pos(ast, x),
        Ty::Tuple(l) => ast.list(l).iter().find_map(|&x| ast_ty_pos(ast, x)),
        Ty::Fn(f) => { let f = *ast.fn_ptr(f); ast.list(f.params).iter().find_map(|&x| ast_ty_pos(ast, x)).or_else(|| f.ret.and_then(|r| ast_ty_pos(ast, r))) }
        _ => None,
    }
}

/// The extern fn-pointer types written in a syntax type, pre-order (the HIR walk): their positions.
fn ast_extern_fnptrs(pf: &crate::parser::ParsedFile, t: crate::ast::TyId, out: &mut Vec<Option<u32>>) {
    let ast = &pf.ast;
    match *ast.ty(t) {
        Ty::Fn(f) => {
            let fp = *ast.fn_ptr(f);
            // `extern fn` (the C ABI) or `extern "abi" fn`; `extern "Rust" fn` is the Rust ABI
            let external = match fp.abi { None => false, Some(None) => true, Some(Some(a)) => pf.text(a).trim_matches('"') != "Rust" };
            if external { out.push(ast_ty_pos(ast, t)); }
            for &p in ast.list(fp.params) { ast_extern_fnptrs(pf, p, out); }
            if let Some(r) = fp.ret { ast_extern_fnptrs(pf, r, out); }
        }
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => ast_extern_fnptrs(pf, x, out),
        Ty::Tuple(l) => { for &x in ast.list(l) { ast_extern_fnptrs(pf, x, out); } }
        Ty::Path(p) => {
            for seg in ast.segs(p.segs) {
                if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                    for a in ast.list(*l) { if let GenericArg::Type(x) = *a { ast_extern_fnptrs(pf, x, out); } }
                }
            }
        }
        _ => {}
    }
}

/// The extern fn-pointer types inside a semantic type, pre-order.
fn sem_extern_fnptrs(tcx: &Tcx, ty: TyId, out: &mut Vec<TyId>) {
    if let TyKind::FnPtr(_, _, h, _) = *tcx.tys.kind(ty) { if !is_rustic_abi(h.abi) { out.push(ty); } }
    for c in ty_children(tcx, ty) { sem_extern_fnptrs(tcx, c, out); }
}

struct Improper<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
}

impl<'a> Improper<'a> {
    fn env(&self, d: DefId) -> crate::tcx::TypingEnv {
        crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(self.prog, d), param_env: self.tcx.param_env_id(self.prog, d), mode: crate::tcx::TypingMode::non_body_analysis() }
    }

    fn emit(&self, d: DefId, ffic: &Ffic, pos: u32, res: Ffi, mode: CItemKind) {
        let (ty, note) = match res {
            Ffi::Safe => return,
            Ffi::Phantom(ty) => (ty, "composed only of `PhantomData`"),
            Ffi::Unsafe { ty, reason, .. } => (ty, reason),
        };
        let (lint, desc) = match mode {
            CItemKind::Declaration => (&IMPROPER_CTYPES, "block"),
            CItemKind::Definition => (&IMPROPER_CTYPES_DEFINITIONS, "fn"),
        };
        if lint::external(self.prog, self.prog.def(d).file, pos) { return; }
        lint::emit(self.prog, d, lint, self.prog.def(d).file, pos,
            &format!("`extern` {desc} uses type `{}`, which is not FFI-safe -- {note}", ffic.ty_str(ty)));
    }

    /// `check_foreign_fn`: the parameter and return types of an extern fn declaration or definition.
    fn check_foreign_fn(&self, d: DefId, mode: CItemKind) {
        let Some(sig) = self.tcx.fn_item_sig(self.prog, d) else { return };
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        let ItemKind::Fn(fsig, _, _) = item.kind else { return };
        let fsig = *ast.fn_sig(fsig);
        let params = ast.list(fsig.params);
        for (i, &pt) in sig.params.iter().enumerate() {
            let pos = params.get(i).and_then(|p| ast_ty_pos(ast, p.ty).or_else(|| match *ast.pat(p.pat) { Pat::Ident { name, .. } => Some(name.lo), _ => None })).unwrap_or(item.name.lo);
            let mut f = Ffic { prog: self.prog, tcx: self.tcx, env: self.env(d), krate: def.krate, cache: Default::default(), base_ty: pt, base_mode: mode, adt_env: Default::default() };
            f.base_ty = f.norm(pt);
            let res = f.check_type(VState::fn_entry(mode, FnPos::Arg), pt);
            self.emit(d, &f, pos, res, mode);
        }
        if let Some(rt) = fsig.ret {
            let pos = ast_ty_pos(ast, rt).unwrap_or(item.name.lo);
            let mut f = Ffic { prog: self.prog, tcx: self.tcx, env: self.env(d), krate: def.krate, cache: Default::default(), base_ty: sig.ret, base_mode: mode, adt_env: Default::default() };
            f.base_ty = f.norm(sig.ret);
            let res = f.check_type(VState::fn_entry(mode, FnPos::Ret), sig.ret);
            self.emit(d, &f, pos, res, mode);
        }
    }

    /// `check_type_for_external_abi_fnptr` over the extern fn pointers of one declared type.
    fn check_fnptrs(&self, d: DefId, ast_ty: crate::ast::TyId, ty: TyId, state: VState, mode: CItemKind) {
        let def = self.prog.def(d);
        let pf = &self.prog.files[def.file as usize];
        let mut sem = Vec::new();
        sem_extern_fnptrs(self.tcx, ty, &mut sem);
        let mut syn = Vec::new();
        ast_extern_fnptrs(pf, ast_ty, &mut syn);
        for (fty, pos) in sem.into_iter().zip(syn) {
            let mut f = Ffic { prog: self.prog, tcx: self.tcx, env: self.env(d), krate: def.krate, cache: Default::default(), base_ty: fty, base_mode: mode, adt_env: Default::default() };
            f.base_ty = f.norm(fty);
            let res = f.check_type(state, fty);
            self.emit(d, &f, pos.unwrap_or(pf.ast.item(def.item).name.lo), res, mode);
        }
    }

    /// `check_fn_for_external_abi_fnptr`: the extern fn pointers in a Rust-ABI fn's signature.
    fn check_fn_fnptrs(&self, d: DefId, mode: CItemKind) {
        let Some(sig) = self.tcx.fn_item_sig(self.prog, d) else { return };
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        let ItemKind::Fn(fsig, _, _) = ast.item(def.item).kind else { return };
        let fsig = *ast.fn_sig(fsig);
        for (i, p) in ast.list(fsig.params).iter().enumerate() {
            if let Some(&pt) = sig.params.get(i) { self.check_fnptrs(d, p.ty, pt, VState::fn_entry(mode, FnPos::Arg), mode); }
        }
        if let Some(rt) = fsig.ret { self.check_fnptrs(d, rt, sig.ret, VState::fn_entry(mode, FnPos::Ret), mode); }
    }
}

/// The improper_ctypes passes over the definitions of the checked crates.
fn improper_ctypes(prog: &Program, tcx: &Tcx, own: &[bool]) {
    let ip = Improper { prog, tcx };
    // the ABI of each foreign block's items
    let mut foreign_abi: std::collections::HashMap<(u32, u32), Option<crate::ast::Ident>> = std::collections::HashMap::new();
    for (fi, pf) in prog.files.iter().enumerate() {
        if !prog.file_crate.get(fi).is_some_and(|&k| own.get(k as usize).copied().unwrap_or(false)) { continue; }
        for (ii, it) in pf.ast.items.iter().enumerate() {
            if let ItemKind::ForeignMod(r) = it.kind {
                let fm = *pf.ast.row(r);
                for &c in pf.ast.list(fm.items) { foreign_abi.insert((fi as u32, c.0), fm.abi); }
            }
            let _ = ii;
        }
    }
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        let d = DefId(i as u32);
        if lint::generated(prog, d) { continue; }
        let file = &prog.files[def.file as usize];
        let ast = &file.ast;
        match def.kind {
            DefKind::ForeignFn => {
                let abi = foreign_abi.get(&(def.file, def.item.0)).copied().flatten();
                let rust = abi.is_some_and(|a| file.text(a).trim_matches('"') == "Rust");
                if !rust { ip.check_foreign_fn(d, CItemKind::Declaration); } else { ip.check_fn_fnptrs(d, CItemKind::Declaration); }
            }
            DefKind::ForeignStatic => {
                let abi = foreign_abi.get(&(def.file, def.item.0)).copied().flatten();
                let rust = abi.is_some_and(|a| file.text(a).trim_matches('"') == "Rust");
                if rust { continue; }
                if let ItemKind::Static(_, ty, _) = ast.item(def.item).kind {
                    if let Some(t) = tcx.const_ty(prog, d) {
                        let mut f = Ffic { prog, tcx, env: ip.env(d), krate: def.krate, cache: Default::default(), base_ty: t, base_mode: CItemKind::Declaration, adt_env: Default::default() };
                        f.base_ty = f.norm(t);
                        let res = f.check_type(VState::static_entry(), t);
                        ip.emit(d, &f, ast_ty_pos(ast, ty).unwrap_or(ast.item(def.item).name.lo), res, CItemKind::Declaration);
                    }
                }
            }
            DefKind::Fn | DefKind::AssocFn => {
                let ItemKind::Fn(fsig, body, _) = ast.item(def.item).kind else { continue };
                let fsig = *ast.fn_sig(fsig);
                // required trait methods have no body and are not visited (`check_fn`)
                if body.is_none() && fsig.lazy_body.is_none() { continue; }
                let rust = match fsig.abi { None => true, Some(None) => false, Some(Some(a)) => file.text(a).trim_matches('"') == "Rust" };
                if !rust { ip.check_foreign_fn(d, CItemKind::Definition); } else { ip.check_fn_fnptrs(d, CItemKind::Definition); }
            }
            DefKind::Struct | DefKind::Union | DefKind::Enum => {
                // `check_field_def`: extern fn pointers in the fields
                let Some(a) = tcx.adts.get(&d) else { continue };
                let groups: Vec<List<FieldDef>> = match ast.item(def.item).kind {
                    ItemKind::Struct(r) => match ast.row(r).data { VariantData::Struct(l) | VariantData::Tuple(l) => vec![l], VariantData::Unit => vec![] },
                    ItemKind::Union(r) => vec![ast.row(r).fields],
                    ItemKind::Enum(r) => ast.list(ast.row(r).variants).iter().filter_map(|v| match v.data { VariantData::Struct(l) | VariantData::Tuple(l) => Some(l), VariantData::Unit => None }).collect(),
                    _ => vec![],
                };
                let mut vi = 0;
                let mut vars = a.variants.iter().filter(|v| !v.fields.is_empty());
                let _ = &mut vi;
                for l in groups {
                    let Some(v) = vars.next() else { break };
                    for (fd, ft) in ast.list(l).iter().zip(v.fields.iter()) {
                        ip.check_fnptrs(d, fd.ty, ft.ty, VState::static_entry(), CItemKind::Definition);
                    }
                }
            }
            DefKind::Static | DefKind::Const | DefKind::TypeAlias => {
                let ty_id = match ast.item(def.item).kind {
                    ItemKind::Static(_, ty, _) => Some(ty),
                    ItemKind::Const(ty, _) => ty.get(),
                    ItemKind::TypeAlias(r) => ast.row(r).ty,
                    _ => None,
                };
                if let (Some(ty_id), Some(t)) = (ty_id, if def.kind == DefKind::TypeAlias { tcx.type_of(prog, d).ok() } else { tcx.const_ty(prog, d) }) { ip.check_fnptrs(d, ty_id, t, VState::static_entry(), CItemKind::Definition); }
            }
            _ => {}
        }
    }
}

/// Deprecation lookups (rustc_middle middle/stability.rs) for the bodies and the signatures.
pub struct Dp<'a> {
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
}

impl<'a> Dp<'a> {
    /// The definition enclosing `d` for the deprecation inheritance (a block module's owner).
    pub fn depr_parent(&self, d: DefId) -> Option<DefId> {
        let def = self.prog.def(d);
        if def.parent == crate::program::NO_DEF { return None; }
        let p = self.prog.def(def.parent);
        if p.kind == DefKind::Mod {
            if let Some(&owner) = self.prog.block_owner.get(&p.sub) { return Some(owner); }
        }
        Some(def.parent)
    }

    /// `#[deprecated]` among the attributes of the definition itself: (since, note).
    pub fn own_deprecation(&self, d: DefId) -> Option<(Option<String>, Option<String>)> {
        let def = self.prog.def(d);
        let ast = &self.prog.files[def.file as usize].ast;
        let attrs: &[Attr] = match def.kind {
            DefKind::Mod => {
                if def.parent == crate::program::NO_DEF || self.prog.block_owner.get(&def.sub).is_some() { return None; }
                ast.list(ast.item(def.item).attrs)
            }
            DefKind::Variant => match ast.item(def.item).kind {
                ItemKind::Enum(r) => ast.list(ast.list(ast.row(r).variants).get(def.sub as usize)?.attrs),
                _ => return None,
            },
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait
            | DefKind::TypeAlias | DefKind::Const | DefKind::AssocConst | DefKind::AssocTy | DefKind::Static | DefKind::ForeignStatic | DefKind::Impl => ast.list(ast.item(def.item).attrs),
            _ => return None,
        };
        let a = attrs.iter().find(|a| lint::attr_path(self.prog, def.file, a) == b"deprecated")?;
        // `#[deprecated]`, `#[deprecated = "note"]`, `#[deprecated(since = "..", note = "..")]`
        let p = self.prog.src(def.file);
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        let mut since = None;
        let mut note = None;
        let unq = |t: &[u8]| String::from_utf8_lossy(t).trim_matches('"').to_string();
        // a value that is no string literal is a malformed attribute (an error): it deprecates nothing
        let is_str = |t: &[u8]| t.first() == Some(&b'"') || t.first() == Some(&b'r');
        let mut i = lo + 1;
        if i < hi && crate::cfg::tok_str(p, i) == b"=" && i + 1 < hi {
            if !is_str(crate::cfg::tok_str(p, i + 1)) { return None; }
            note = Some(unq(crate::cfg::tok_str(p, i + 1)));
        } else {
            while i + 2 < hi {
                let name = crate::cfg::tok_str(p, i);
                if crate::cfg::tok_str(p, i + 1) == b"=" {
                    if !is_str(crate::cfg::tok_str(p, i + 2)) { return None; }
                    let v = unq(crate::cfg::tok_str(p, i + 2));
                    if name == b"since" { since = Some(v); } else if name == b"note" { note = Some(v); }
                    i += 3;
                } else { i += 1; }
            }
        }
        Some((since, note))
    }

    /// `lookup_deprecation_entry`: the nearest `#[deprecated]` at or above the definition:
    /// (the definition carrying it, since, note).
    pub fn deprecation_entry(&self, d: DefId) -> Option<(DefId, Option<String>, Option<String>)> {
        let mut cur = d;
        for _ in 0..64 {
            if let Some((since, note)) = self.own_deprecation(cur) { return Some((cur, since, note)); }
            cur = self.depr_parent(cur)?;
        }
        None
    }

    pub fn depr_descr(&self, d: DefId) -> &'static str {
        let def = self.prog.def(d);
        let shape = |adt: DefId, vi: usize| self.tcx.adts.get(&adt).and_then(|a| a.variants.get(vi)).map(|v| v.shape);
        match def.kind {
            DefKind::Struct => match shape(d, 0) { Some(0) => "unit struct", Some(1) => "tuple struct", _ => "struct" },
            DefKind::Variant => match shape(def.parent, def.sub as usize) { Some(0) => "unit variant", Some(1) => "tuple variant", _ => "variant" },
            DefKind::Mod => "module",
            DefKind::Static | DefKind::ForeignStatic => "static",
            DefKind::MacroRules => "macro",
            _ => self.tcx.def_descr(self.prog, d),
        }
    }

    /// The lint message for a use of `d` inside the definition `scope`: (deprecated_in_future, message).
    pub fn message(&self, d: DefId, scope: DefId) -> Option<(bool, String)> {
        let (origin, since, note) = self.deprecation_entry(d)?;
        // a use inside an item deprecated at the same origin is silent
        if self.deprecation_entry(scope).is_some_and(|(o, _, _)| o == origin) { return None; }
        // `since` a version later than this compiler's (or "TBD"): deprecated_in_future
        let future = since.as_deref().is_some_and(|s| {
            if s == "TBD" { return true; }
            let v: Vec<u32> = s.split('.').map(|x| x.parse().unwrap_or(0)).collect();
            v.len() == 3 && (v[0], v[1], v[2]) > (1, 97, 1)
        });
        let path = {
            let full = self.prog.def_path(d);
            match full.split_once("::") { Some((_, rest)) if self.prog.def(d).krate == self.prog.def(scope).krate => rest.to_string(), _ => full }
        };
        let kind = self.depr_descr(d);
        let msg = match (&note, future) {
            (Some(n), false) => format!("use of deprecated {kind} `{path}`: {n}"),
            (None, false) => format!("use of deprecated {kind} `{path}`"),
            (Some(n), true) => format!("use of {kind} `{path}` that will be deprecated in a future Rust version: {n}"),
            (None, true) => format!("use of {kind} `{path}` that will be deprecated in a future Rust version"),
        };
        Some((future, msg))
    }
}

pub const VARIANT_SIZE_DIFFERENCES: Lint = lint::lint("variant_size_differences");

/// rustc_lint types.rs `VariantSizeDifferences`: a tagged enum whose largest variant is more than
/// three times larger than the second largest (sizes without the tag).
pub fn variant_size_differences(prog: &Program, tcx: &Tcx, own: &[bool]) {
    if !lint::enabled() { return; }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if def.kind != DefKind::Enum || !own.get(def.krate as usize).copied().unwrap_or(false) || lint::generated(prog, d) { continue; }
        let Some(adt) = tcx.adts.get(&d) else { continue };
        if adt.n_generics != 0 || adt.variants.len() < 2 { continue; }
        let bad = crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_ALIAS | crate::types::TypeFlags::HAS_ERROR | crate::types::TypeFlags::HAS_INFER;
        if adt.variants.iter().any(|v| v.fields.iter().any(|f| tcx.tys.flags(f.ty).intersects(bad))) { continue; }
        let ty = tcx.tys.intern(TyKind::Adt(d, tcx.tys.mk_list(&[]), adt.region_params));
        let lcx = crate::layout::LayoutCx::new(prog);
        let lay = crate::layout::Layouts::new();
        let l = lay.of(&lcx, tcx, ty);
        let (Some((_, mem)), crate::layout::TagEnc::Direct) = (l.tag, l.enc) else { continue };
        let tag_size = match mem { relax_backend::rir::Mem::Int(b, _) => b as u32, relax_backend::rir::Mem::F32 => 4, relax_backend::rir::Mem::F64 => 8 };
        if l.variant_fields.len() != adt.variants.len() { continue; }
        let (mut largest, mut slargest, mut li) = (0u32, 0u32, 0usize);
        for (vi, v) in adt.variants.iter().enumerate() {
            let (mut end, mut align) = (tag_size, tag_size.max(1));
            for (fi, f) in v.fields.iter().enumerate() {
                let fl = lay.of(&lcx, tcx, f.ty);
                let off = l.variant_fields[vi].get(fi).copied().unwrap_or(0);
                if off == u32::MAX { continue; }
                end = end.max(off + fl.size);
                align = align.max(fl.align);
            }
            let size = crate::layout::round_up(end, align);
            let bytes = size.saturating_sub(tag_size);
            if bytes > largest { slargest = largest; largest = bytes; li = vi; }
            else if bytes > slargest { slargest = bytes; }
        }
        if largest > slargest * 3 && slargest > 0 {
            let ast = &prog.files[def.file as usize].ast;
            let ItemKind::Enum(r) = ast.item(def.item).kind else { continue };
            let Some(v) = ast.list(ast.row(r).variants).get(li) else { continue };
            if lint::external(prog, def.file, v.name.lo) { continue; }
            lint::emit(prog, d, &VARIANT_SIZE_DIFFERENCES, def.file, v.name.lo, &format!("enum variant is more than three times larger ({} bytes) than the next largest", largest));
        }
    }
}

pub const REPR_TRANSPARENT_NON_ZST_FIELDS: Lint = lint::lint("repr_transparent_non_zst_fields");

/// rustc_hir_analysis check.rs `check_transparent`, the lint half: a zero-sized field of a
/// `repr(transparent)` struct that contains a `repr(C)` type, next to a non-zero-sized field.
pub fn repr_transparent_fields(prog: &Program, tcx: &Tcx, own: &[bool]) {
    if !lint::enabled() { return; }
    fn contains_repr_c(prog: &Program, tcx: &Tcx, t: TyId, depth: u32) -> bool {
        if depth > 16 { return false; }
        match *tcx.tys.kind(t) {
            TyKind::Tuple(l) => tcx.tys.list(l).iter().any(|&x| contains_repr_c(prog, tcx, x, depth + 1)),
            TyKind::Array(e, _) => contains_repr_c(prog, tcx, e, depth + 1),
            TyKind::Adt(d, args, _) => {
                let Some(adt) = tcx.adts.get(&d) else { return false };
                if adt.repr_c { return true; }
                adt.variants.iter().any(|v| v.fields.iter().any(|f| contains_repr_c(prog, tcx, tcx.tys.subst(f.ty, tcx.tys.list(args)), depth + 1)))
            }
            _ => false,
        }
    }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if def.kind != DefKind::Struct || !own.get(def.krate as usize).copied().unwrap_or(false) || lint::generated(prog, d) { continue; }
        let pf = &prog.files[def.file as usize];
        let ast = &pf.ast;
        let item = ast.item(def.item);
        let transparent = ast.list(item.attrs).iter().any(|a| lint::attr_path(prog, def.file, a) == b"repr" && lint::attr_text(prog, def.file, a).contains("transparent"));
        if !transparent { continue; }
        let Some(adt) = tcx.adts.get(&d) else { continue };
        let bad = crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_ALIAS | crate::types::TypeFlags::HAS_ERROR | crate::types::TypeFlags::HAS_INFER;
        let Some(v) = adt.variants.first() else { continue };
        if v.fields.iter().any(|f| tcx.tys.flags(f.ty).intersects(bad)) { continue; }
        let ItemKind::Struct(r) = item.kind else { continue };
        let fields: Vec<FieldDef> = match ast.row(r).data { VariantData::Unit => Vec::new(), VariantData::Tuple(l) | VariantData::Struct(l) => ast.list(l).to_vec() };
        if fields.len() != v.fields.len() { continue; }
        let lcx = crate::layout::LayoutCx::new(prog);
        let lay = crate::layout::Layouts::new();
        let trivial: Vec<bool> = v.fields.iter().map(|f| {
            let l = lay.of(&lcx, tcx, f.ty);
            !lay.is_unsized(&lcx, tcx, f.ty) && l.size == 0 && l.align == 1
        }).collect();
        let non_trivial = trivial.iter().filter(|t| !**t).count();
        let mut prev = false;
        for (k, f) in v.fields.iter().enumerate() {
            if !trivial[k] || !contains_repr_c(prog, tcx, f.ty, 0) { continue; }
            if non_trivial > 0 || prev {
                let pos = fields[k].name.map_or(fields[k].ty_span.0, |n| n.lo);
                if lint::external(prog, def.file, pos) { continue; }
                lint::emit(prog, d, &REPR_TRANSPARENT_NON_ZST_FIELDS, def.file, pos, "zero-sized fields in `repr(transparent)` cannot contain `repr(C)` types");
            } else {
                prev = true;
            }
        }
    }
}

pub const ASYNC_FN_IN_TRAIT: Lint = lint::lint("async_fn_in_trait");

/// rustc_lint async_fn_in_trait.rs: an `async fn` in a publicly reachable trait.
pub fn async_fn_in_trait(prog: &Program, own: &[bool], reach: &crate::lint_dead::Reach) {
    if !lint::enabled() { return; }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if def.kind != DefKind::AssocFn || !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        if prog.def(def.parent).kind != DefKind::Trait || !reach.defs.contains(&def.parent) || lint::generated(prog, d) { continue; }
        let pf = &prog.files[def.file as usize];
        let ast = &pf.ast;
        let item = ast.item(def.item);
        let ItemKind::Fn(sig, _, _) = item.kind else { continue };
        if !ast.fn_sig(sig).asyncness { continue; }
        // the `async` keyword before the name
        let head = pf.bytes(item.lo, item.name.lo);
        let Some(k) = head.windows(5).position(|w| w == b"async") else { continue };
        let pos = item.lo + k as u32;
        if lint::external(prog, def.file, pos) { continue; }
        lint::emit(prog, d, &ASYNC_FN_IN_TRAIT, def.file, pos, "use of `async fn` in public traits is discouraged as auto trait bounds cannot be specified");
    }
}

/// rustc builtin.rs `MissingCopyImplementations`: a reachable, non-generic ADT with no
/// destructor, no raw pointer field, no `Iterator` impl, at most 256 bytes and only `Copy`
/// fields could implement `Copy`.
pub fn missing_copy_implementations(prog: &Program, tcx: &Tcx, own: &[bool], reach: &crate::lint_dead::Reach) {
    if !lint::enabled() { return; }
    let Some(copy) = tcx.copy_trait else { return };
    let iter = tcx.lang_items.get(LangItem::Iterator);
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) { continue; }
        if !own.get(def.krate as usize).copied().unwrap_or(false) || !reach.defs.contains(&d) { continue; }
        if lint::generated(prog, d) { continue; }
        let Some(adt) = tcx.adts.get(&d) else { continue };
        if adt.n_generics != 0 || adt.flags.contains(crate::tcx::AdtFlags::IS_VARIANT_LIST_NON_EXHAUSTIVE) { continue; }
        if tcx.adt_destructor(d).is_some() { continue; }
        if adt.variants.iter().any(|v| v.fields.iter().any(|f| matches!(tcx.tys.kind(f.ty), TyKind::Ptr(..)))) { continue; }
        let bad = crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_ALIAS | crate::types::TypeFlags::HAS_ERROR;
        if adt.variants.iter().any(|v| v.fields.iter().any(|f| tcx.tys.flags(f.ty).intersects(bad))) { continue; }
        let ty = tcx.tys.intern(TyKind::Adt(d, tcx.tys.mk_list(&[]), adt.region_params));
        let env = tcx.param_env_id(prog, d);
        let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), d);
        let implements = |tr: DefId, t: TyId| {
            let predicate = crate::next_solver::collect::trait_predicate(tr, vec![crate::consts::GenericArg::Type(t)]);
            infcx.evaluate_obligation(prog, crate::next_solver::Goal { param_env: env, predicate }).must_apply_modulo_regions()
        };
        if implements(copy, ty) { continue; }
        if iter.is_some_and(|it| implements(it, ty)) { continue; }
        if tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_PARAM | crate::types::TypeFlags::HAS_ALIAS | crate::types::TypeFlags::HAS_ERROR) { continue; }
        let lay = crate::layout::Layouts::new();
        if lay.of(&crate::layout::LayoutCx::new(prog), tcx, ty).size > 256 { continue; }
        // every field must be Copy
        let all_copy = adt.variants.iter().all(|v| v.fields.iter().all(|f| implements(copy, f.ty)));
        if !all_copy { continue; }
        let ast = &prog.files[def.file as usize].ast;
        let item = ast.item(def.item);
        if lint::has_attr(prog, def.file, ast.list(item.attrs), "non_exhaustive") { continue; }
        if let ItemKind::Enum(r) = item.kind {
            if ast.list(ast.row(r).variants).iter().any(|v| lint::has_attr(prog, def.file, ast.list(v.attrs), "non_exhaustive")) { continue; }
        }
        if lint::external(prog, def.file, item.lo) { continue; }
        lint::emit(prog, d, &MISSING_COPY_IMPLEMENTATIONS, def.file, ast.item(def.item).lo, "type could implement `Copy`; consider adding `impl Copy`");
    }
}

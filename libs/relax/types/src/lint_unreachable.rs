//! `unreachable_code` (rustc_hir_typeck `Diverges` + `FnCtxt::warn_if_unreachable`): the
//! divergence state of a body evaluated in rustc's order over the typed syntax tree. The state is
//! `Maybe < Always < WarnedAlways`; every expression, statement, match arm and `if` condition
//! reports when it is checked while the state is `Always`, then the state becomes `WarnedAlways`
//! (one warning per diverging point). Expressions of type `!` set it; blocks, `if`, `match`,
//! loops and the lazy boolean operators combine it as rustc's check functions do.
//!
//! Check-only (reports through the lint sink); `makepad run` never calls this.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::program::Program;
use crate::tcx::Tcx;
use crate::typeck::Body;
use crate::types::TyKind;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Div {
    Maybe,
    Always,
    Warned,
}

struct Breakable {
    label: Option<Ident>,
    /// a `loop`/`while`/`for` takes unlabeled breaks; a labeled block only `break 'label`
    is_loop: bool,
    may_break: bool,
}

struct Cx<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    div: Div,
    breakables: Vec<Breakable>,
    stack: Vec<Attr>,
    /// the current divergence comes from a call of an uninhabited type (`uninhabited_call`):
    /// rustc's MIR lint names the next code "expression" and does not look at macros
    unin: bool,
}

pub fn check(prog: &Program, tcx: &Tcx, body: &Body, block: Option<BlockId>, expr: Option<ExprId>) {
    if !lint::enabled() { return; }
    let ast = &prog.files[body.file as usize].ast;
    let mut cx = Cx { prog, tcx, body, ast, div: Div::Maybe, breakables: Vec::new(), stack: Vec::new(), unin: false };
    if let Some(b) = block { cx.block(b); }
    if let Some(e) = expr { cx.expr(e); }
}

impl<'a> Cx<'a> {
    fn warn(&mut self, pos: u32, kind: &str) {
        if self.div != Div::Always { return; }
        self.div = Div::Warned;
        let unin = std::mem::take(&mut self.unin);
        // a statement that is a macro call reports at the call (rustc: the statement's span is the call's)
        if kind != "statement" && !unin && lint::external(self.prog, self.body.file, pos) { return; }
        let kind = if unin && kind == "statement" { "expression" } else { kind };
        lint::emit_in(self.prog, self.body.def, &self.stack, &lint::UNREACHABLE_CODE, self.body.file, pos, &format!("unreachable {}", kind));
    }

    fn enter(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    /// rustc_mir_build builder/mod.rs (`UnreachableDueToUninhabited`): a call whose result type has
    /// no values (not `!`: typeck handles that), in a fn whose own return type has some, makes
    /// what follows unreachable.
    fn uninhabited_call(&mut self, e: ExprId) -> bool {
        let b = self.body;
        let i = b.expr_ix(e);
        if i >= b.expr_ty.len() || !b.expr_typed.get(i) { return false; }
        let ty = b.expr_ty[i];
        if !matches!(self.tcx.tys.kind(ty), TyKind::Adt(..)) { return false; }
        // a tuple-struct or variant constructor is an aggregate, not a call
        if let ExprKind::Call(f, _) = self.ast.expr(e).kind {
            if matches!(self.body.res.get(&f.0), Some(crate::typeck::Res::Def(d)) if matches!(self.prog.def(*d).kind, crate::program::DefKind::Struct | crate::program::DefKind::Variant)) { return false; }
        }
        if !matches!(self.prog.def(b.def).kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) { return false; }
        let Some(sig) = self.tcx.fn_sig(self.prog, b.def) else { return false };
        let ret = sig.ret;
        let cx = crate::pattern_analysis::body_cx(self.prog, self.tcx, b);
        cx.is_uninhabited(ty) && !cx.is_uninhabited(ret)
    }

    fn is_never(&self, e: ExprId) -> bool {
        let i = self.body.expr_ix(e);
        i < self.body.expr_ty.len() && self.body.expr_typed.get(i) && matches!(self.tcx.tys.kind(self.body.expr_ty[i]), TyKind::Never)
    }

    /// A closure or async block body: a function context of its own.
    fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        let saved = (self.div, std::mem::take(&mut self.breakables));
        self.div = Div::Maybe;
        f(self);
        self.div = saved.0;
        self.breakables = saved.1;
    }

    // ------------------------------------------------------------ blocks and statements

    fn block(&mut self, b: BlockId) {
        let blk = *self.ast.block(b);
        self.enter(blk.attrs);
        let prev = self.div;
        self.breakables.push(Breakable { label: None, is_loop: false, may_break: false });
        self.block_inner(b, None);
        let ctx = self.breakables.pop().unwrap();
        if ctx.may_break { self.div = prev; }
        self.leave(blk.attrs);
    }

    /// The statements of a block and its tail expression.
    fn block_inner(&mut self, b: BlockId, _label: Option<Ident>) {
        let stmts = self.ast.stmts(b);
        // the last expression statement without `;` is the tail expression
        let tail = match stmts.last() {
            Some(Stmt::Expr(e, false)) => Some(*e),
            _ => None,
        };
        let n = stmts.len() - usize::from(tail.is_some());
        for s in &stmts[..n] { self.stmt(s, b); }
        if let Some(t) = tail {
            let attrs = self.ast.stmt_attrs(t);
            self.enter(attrs);
            self.expr(t);
            self.leave(attrs);
        }
    }

    fn stmt(&mut self, s: &Stmt, b: BlockId) {
        let ast = self.ast;
        match *s {
            Stmt::Item(_) | Stmt::Empty => {}
            Stmt::Let(r) => {
                let l = *ast.row(r);
                self.enter(l.attrs);
                let pos = self.stmt_pos_let(&l, b);
                self.warn(pos, "statement");
                let old = std::mem::replace(&mut self.div, Div::Maybe);
                if let Some(x) = l.init {
                    self.expr(x);
                }
                if let Some(e) = l.else_ {
                    let prev = self.div;
                    self.block(e);
                    self.div = prev;
                }
                self.div = self.div.max(old);
                self.leave(l.attrs);
            }
            Stmt::Expr(e, _) => {
                let attrs = ast.stmt_attrs(e);
                self.enter(attrs);
                self.expr_stmt(e);
                self.leave(attrs);
            }
            Stmt::Attrs(r) => {
                let a = *ast.row(r);
                self.enter(a.attrs);
                self.expr_stmt(a.expr);
                self.leave(a.attrs);
            }
        }
    }

    fn stmt_pos_let(&self, l: &LetStmt, b: BlockId) -> u32 {
        let _ = b;
        // the pattern starts at or after `let`; its line is the statement's in practice
        self.pat_pos(l.pat)
    }

    fn pat_pos(&self, p: PatId) -> u32 {
        match *self.ast.pat(p) {
            Pat::Ident { name, .. } => name.lo,
            Pat::Lit(e) | Pat::Range(Some(e), _, _) | Pat::Range(None, Some(e), _) => self.ast.expr(e).lo,
            Pat::Path(ref path) | Pat::TupleStruct(ref path, _) | Pat::Struct(ref path, _, _) => path.lo,
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_pos(x),
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => self.ast.list(ps).first().map_or(0, |x| self.pat_pos(*x)),
            _ => 0,
        }
    }

    fn expr_stmt(&mut self, e: ExprId) {
        let pos = self.ast.expr(e).lo;
        self.warn(pos, "statement");
        let old = std::mem::replace(&mut self.div, Div::Maybe);
        self.expr(e);
        self.div = self.div.max(old);
    }

    // ------------------------------------------------------------ expressions

    fn exprs(&mut self, l: List<ExprId>) {
        for &x in self.ast.list(l) { self.expr(x); }
    }

    /// rustc `check_expr_with_expectation_and_args`.
    fn expr(&mut self, e: ExprId) {
        let ex = *self.ast.expr(e);
        if let ExprKind::Paren(x) = ex.kind {
            return self.expr(x);
        }
        // Warn for expressions after diverging siblings.
        self.warn(ex.lo, "expression");
        let old = std::mem::replace(&mut self.div, Div::Maybe);
        self.kind(e, ex);
        // Warn for non-block expressions with diverging children.
        match ex.kind {
            ExprKind::Block(..) | ExprKind::Unsafe(..) | ExprKind::If(..) | ExprKind::Let(..) | ExprKind::Loop(..) | ExprKind::Match(..)
            | ExprKind::While(..) | ExprKind::For(..) | ExprKind::Cast(..) => {}
            // a destructuring assignment is lowered to a block (`{ let pat = rhs; .. }`)
            ExprKind::Assign(l, _) if self.is_destructuring(l) => {}
            ExprKind::Call(callee, _) => {
                let pos = self.ast.expr(callee).lo;
                self.warn(pos, "call");
            }
            ExprKind::MethodCall(_, row) => {
                let pos = self.ast.row(row).name.lo;
                self.warn(pos, "call");
            }
            _ => self.warn(ex.lo, "expression"),
        }
        if self.is_never(e) && !self.not_a_read(e) {
            self.div = self.div.max(Div::Always);
        } else if matches!(ex.kind, ExprKind::Call(..) | ExprKind::MethodCall(..)) && self.uninhabited_call(e) {
            if self.div != Div::Always { self.unin = true; }
            self.div = self.div.max(Div::Always);
        }
        self.div = self.div.max(old);
    }

    fn is_destructuring(&self, mut l: ExprId) -> bool {
        while let ExprKind::Paren(x) = self.ast.expr(l).kind { l = x; }
        matches!(self.ast.expr(l).kind, ExprKind::Underscore | ExprKind::Tuple(_) | ExprKind::Array(_) | ExprKind::Struct(_))
            || matches!(self.ast.expr(l).kind, ExprKind::Call(f, _) if matches!(self.ast.expr(f).kind, ExprKind::Path(_)))
    }

    /// A place expression of type `!` that is not read diverges nothing
    /// (`tcx.expr_guaranteed_to_constitute_read_for_never`, the expression itself).
    fn not_a_read(&self, _e: ExprId) -> bool { false }

    fn kind(&mut self, e: ExprId, ex: Expr) {
        let ast = self.ast;
        match ex.kind {
            ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Underscore | ExprKind::Paren(_) => {}
            ExprKind::Continue(_) => {}
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Box(x) | ExprKind::Try(x) | ExprKind::Await(x)
            | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::AddrOf(_, _, x) => self.expr(x),
            ExprKind::Binary(op, a, b) => {
                self.expr(a);
                if matches!(op, BinOp::And | BinOp::Or) {
                    let lhs = self.div;
                    self.expr(b);
                    // depending on the left value the right side never runs
                    self.div = lhs;
                } else {
                    self.expr(b);
                }
            }
            ExprKind::Assign(a, b) if self.is_destructuring(a) => {
                // `{ let pat = rhs; place = ..; }`: the places are patterns here
                self.expr(b);
            }
            ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) | ExprKind::Repeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(f, args) => {
                self.expr(f);
                self.args(args);
            }
            ExprKind::MethodCall(recv, row) => {
                self.expr(recv);
                self.args(ast.row(row).args);
            }
            ExprKind::Block(b, label) => {
                if let Some(l) = label.get() {
                    self.labeled_block(b, l);
                } else {
                    self.block(b);
                }
            }
            ExprKind::Unsafe(b) => self.block(b),
            ExprKind::Async(_, b) => self.nested(|c| c.block(b)),
            ExprKind::ConstBlock(b) => self.nested(|c| c.block(b)),
            ExprKind::If(c, then, els) => self.if_expr(c, then, els),
            ExprKind::Let(p, x) => {
                let pos = ast.expr(x).lo;
                self.warn(pos, "block in `let` expression");
                self.expr(x);
                let _ = p;
            }
            ExprKind::Match(x, arms) => self.match_expr(e, x, arms),
            ExprKind::While(row) => {
                let w = *ast.row(row);
                // loop { if cond { body } else { break } }
                self.breakables.push(Breakable { label: w.label, is_loop: true, may_break: true });
                self.if_expr_block(w.cond, w.body);
                let ctx = self.breakables.pop().unwrap();
                debug_assert!(ctx.may_break);
                self.div = Div::Maybe;
            }
            ExprKind::For(row) => {
                let f = *ast.row(row);
                self.expr(f.iter);
                let after_iter = self.div;
                self.breakables.push(Breakable { label: f.label, is_loop: true, may_break: true });
                let saved = self.div;
                self.div = Div::Maybe;
                self.block(f.body);
                self.breakables.pop();
                let _ = saved;
                self.div = after_iter;
            }
            ExprKind::Loop(b, label) => {
                self.breakables.push(Breakable { label: label.get(), is_loop: true, may_break: false });
                self.block(b);
                let ctx = self.breakables.pop().unwrap();
                if ctx.may_break {
                    self.div = Div::Maybe;
                } else {
                    self.div = self.div.max(Div::Always);
                }
            }
            ExprKind::Break(label, val) => {
                if let Some(v) = val.get() { self.expr(v); }
                let target = self.break_target(label.get());
                // the break exits the loop unless its own value already diverged
                let reachable = self.div != Div::Always && self.div != Div::Warned;
                if let Some(t) = target {
                    if reachable { self.breakables[t].may_break = true; }
                }
            }
            ExprKind::Return(val) | ExprKind::Yield(val) => {
                if let Some(v) = val.get() { self.expr(v); }
            }
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                self.nested(|cx| cx.expr(c.body));
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => self.exprs(xs),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                for f in ast.list(s.fields) {
                    self.enter(f.attrs);
                    self.expr(f.expr);
                    self.leave(f.attrs);
                }
                if let Some(x) = s.base.get() { self.expr(x); }
            }
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { self.expr(x); }
                if let Some(x) = b.get() { self.expr(x); }
            }
            ExprKind::Mac(m) => {
                let m = *ast.row(m);
                if m.args == NO_MAC_ARGS { return; }
                let args = *ast.mac_arg(m.args);
                self.exprs(args.exprs);
                if let Some(g) = args.guard { self.expr(g); }
            }
        }
    }

    /// Call arguments: each warns when checked (closures are checked last but warn in order).
    fn args(&mut self, args: List<ExprId>) {
        let ast = self.ast;
        let mut closures = Vec::new();
        for &a in ast.list(args) {
            let pos = ast.expr(a).lo;
            self.warn(pos, "expression");
            if matches!(ast.expr(a).kind, ExprKind::Closure(_)) {
                closures.push(a);
            } else {
                self.expr(a);
            }
        }
        for a in closures { self.expr(a); }
    }

    fn break_target(&self, label: Option<Ident>) -> Option<usize> {
        match label {
            Some(l) => {
                let t = self.prog.files[self.body.file as usize].text(l);
                self.breakables.iter().rposition(|b| b.label.is_some_and(|x| self.prog.files[self.body.file as usize].text(x) == t))
            }
            None => self.breakables.iter().rposition(|b| b.is_loop),
        }
    }

    fn labeled_block(&mut self, b: BlockId, l: Ident) {
        let blk = *self.ast.block(b);
        self.enter(blk.attrs);
        let prev = self.div;
        self.breakables.push(Breakable { label: Some(l), is_loop: false, may_break: false });
        self.block_inner(b, Some(l));
        let ctx = self.breakables.pop().unwrap();
        if ctx.may_break { self.div = prev; }
        self.leave(blk.attrs);
    }

    /// rustc `check_expr_if`.
    fn if_expr(&mut self, c: ExprId, then: BlockId, els: OptExprId) {
        self.expr(c);
        let pos = self.ast.block(then).lo;
        self.warn(pos, "block in `if` or `while` expression");
        let cond = self.div;
        self.div = Div::Maybe;
        self.block_expr(then);
        let then_div = self.div;
        self.div = Div::Maybe;
        if let Some(x) = els.get() {
            self.expr(x);
            let else_div = self.div;
            self.div = cond.max(then_div.min(else_div));
        } else {
            self.div = cond;
        }
    }

    /// The condition and body of a `while` (an `if` whose else breaks).
    fn if_expr_block(&mut self, c: ExprId, body: BlockId) {
        self.expr(c);
        let pos = self.ast.block(body).lo;
        self.warn(pos, "block in `if` or `while` expression");
        let cond = self.div;
        self.div = Div::Maybe;
        self.block_expr(body);
        self.div = cond;
    }

    /// A block checked as an expression (the entry warning is the `if` check's own).
    fn block_expr(&mut self, b: BlockId) {
        let old = std::mem::replace(&mut self.div, Div::Maybe);
        self.block(b);
        self.div = self.div.max(old);
    }

    /// rustc `check_expr_match`.
    fn match_expr(&mut self, e: ExprId, x: ExprId, arms: List<Arm>) {
        let ast = self.ast;
        self.expr(x);
        let arms = ast.list(arms);
        if arms.is_empty() {
            self.div = self.div.max(Div::Always);
            return;
        }
        for a in arms {
            let pos = ast.expr(a.body).lo;
            self.warn(pos, "arm");
        }
        let scrut = std::mem::replace(&mut self.div, Div::Maybe);
        let mut all = Div::Warned;
        for a in arms {
            self.enter(a.attrs);
            self.div = Div::Maybe;
            if let Some(g) = a.guard { self.expr(g); }
            self.expr(a.body);
            all = all.min(self.div);
            self.leave(a.attrs);
        }
        let _ = e;
        self.div = scrut.max(all);
    }
}

#[allow(dead_code)]
fn _lint(_: &Lint) {}

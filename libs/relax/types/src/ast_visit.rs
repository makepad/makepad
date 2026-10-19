//! A visitor over a fn body's syntax tree (rustc_ast::visit / rustc_hir::intravisit at the
//! level the lint passes need): blocks, statements, expressions, patterns, match arms. It
//! announces the attributes of the nodes that carry them (`let` statements, attributed
//! statements, arms, struct-literal and pattern fields, closure parameters, blocks) through
//! `enter_attrs` / `leave_attrs`, so a pass can follow lint levels (`#[allow(..)]` on a
//! statement) as rustc's lint level builder does.
//!
//! Nested items (`fn` inside a body) are not entered: they are definitions of their own.
//! Built-in macro calls are walked through their parsed arguments.

use crate::ast::*;

pub trait Visitor<'a>: Sized {
    fn ast(&self) -> &'a Ast;

    /// The attributes of a node are in effect for everything visited until the matching
    /// `leave_attrs`.
    fn enter_attrs(&mut self, _attrs: List<Attr>) {}
    fn leave_attrs(&mut self, _attrs: List<Attr>) {}

    fn block(&mut self, b: BlockId) { walk_block(self, b) }
    fn stmt(&mut self, s: &Stmt, _b: BlockId) { walk_stmt(self, s) }
    fn expr(&mut self, e: ExprId) { walk_expr(self, e) }
    fn pat(&mut self, p: PatId) { walk_pat(self, p) }
    fn arm(&mut self, a: &Arm) { walk_arm(self, a) }
}

pub fn walk_block<'a, V: Visitor<'a>>(v: &mut V, b: BlockId) {
    let ast = v.ast();
    let blk = *ast.block(b);
    v.enter_attrs(blk.attrs);
    for s in ast.stmts(b) { v.stmt(s, b); }
    v.leave_attrs(blk.attrs);
}

pub fn walk_stmt<'a, V: Visitor<'a>>(v: &mut V, s: &Stmt) {
    let ast = v.ast();
    match *s {
        Stmt::Let(r) => {
            let l = *ast.row(r);
            v.enter_attrs(l.attrs);
            if let Some(x) = l.init { v.expr(x); }
            v.pat(l.pat);
            if let Some(e) = l.else_ { v.block(e); }
            v.leave_attrs(l.attrs);
        }
        Stmt::Expr(e, _) => {
            let attrs = ast.stmt_attrs(e);
            v.enter_attrs(attrs);
            v.expr(e);
            v.leave_attrs(attrs);
        }
        Stmt::Attrs(r) => {
            let a = *ast.row(r);
            v.enter_attrs(a.attrs);
            v.expr(a.expr);
            v.leave_attrs(a.attrs);
        }
        Stmt::Item(_) | Stmt::Empty => {}
    }
}

pub fn walk_arm<'a, V: Visitor<'a>>(v: &mut V, a: &Arm) {
    v.enter_attrs(a.attrs);
    v.pat(a.pat);
    if let Some(g) = a.guard { v.expr(g); }
    v.expr(a.body);
    v.leave_attrs(a.attrs);
}

pub fn walk_exprs<'a, V: Visitor<'a>>(v: &mut V, l: List<ExprId>) {
    let ast = v.ast();
    for &x in ast.list(l) { v.expr(x); }
}

pub fn walk_expr<'a, V: Visitor<'a>>(v: &mut V, e: ExprId) {
    let ast = v.ast();
    match ast.expr(e).kind {
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Continue(_) | ExprKind::Underscore => {}
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Paren(x) | ExprKind::Box(x)
        | ExprKind::Try(x) | ExprKind::Await(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _)
        | ExprKind::AddrOf(_, _, x) => v.expr(x),
        ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) | ExprKind::Repeat(a, b) => {
            v.expr(a);
            v.expr(b);
        }
        ExprKind::Call(f, args) => {
            v.expr(f);
            walk_exprs(v, args);
        }
        ExprKind::MethodCall(recv, row) => {
            v.expr(recv);
            walk_exprs(v, ast.row(row).args);
        }
        ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => v.block(b),
        ExprKind::If(c, then, els) => {
            v.expr(c);
            v.block(then);
            if let Some(x) = els.get() { v.expr(x); }
        }
        ExprKind::Let(p, x) => {
            v.expr(x);
            v.pat(p);
        }
        ExprKind::Match(x, arms) => {
            v.expr(x);
            for a in ast.list(arms) { v.arm(a); }
        }
        ExprKind::While(row) => {
            let w = *ast.row(row);
            v.expr(w.cond);
            v.block(w.body);
        }
        ExprKind::For(row) => {
            let f = *ast.row(row);
            v.expr(f.iter);
            v.pat(f.pat);
            v.block(f.body);
        }
        ExprKind::Break(_, x) | ExprKind::Return(x) | ExprKind::Yield(x) => if let Some(x) = x.get() { v.expr(x); },
        ExprKind::Closure(row) => {
            let c = *ast.row(row);
            for p in ast.list(c.params) {
                v.enter_attrs(p.attrs);
                v.pat(p.pat);
                v.leave_attrs(p.attrs);
            }
            v.expr(c.body);
        }
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => walk_exprs(v, xs),
        ExprKind::Struct(row) => {
            let s = *ast.row(row);
            for f in ast.list(s.fields) {
                v.enter_attrs(f.attrs);
                v.expr(f.expr);
                v.leave_attrs(f.attrs);
            }
            if let Some(x) = s.base.get() { v.expr(x); }
        }
        ExprKind::Range(a, b, _) => {
            if let Some(x) = a.get() { v.expr(x); }
            if let Some(x) = b.get() { v.expr(x); }
        }
        ExprKind::Mac(m) => {
            let m = *ast.row(m);
            if m.args == NO_MAC_ARGS { return; }
            let args = *ast.mac_arg(m.args);
            walk_exprs(v, args.exprs);
            if let Some(p) = args.pat { v.pat(p); }
            if let Some(g) = args.guard { v.expr(g); }
        }
    }
}

pub fn walk_pat<'a, V: Visitor<'a>>(v: &mut V, p: PatId) {
    let ast = v.ast();
    match *ast.pat(p) {
        Pat::Wild | Pat::Rest | Pat::Path(_) | Pat::Mac(_) => {}
        Pat::Ident { sub, .. } => if let Some(s) = sub { v.pat(s) },
        Pat::Lit(e) => v.expr(e),
        Pat::Range(a, b, _) => {
            if let Some(x) = a { v.expr(x); }
            if let Some(x) = b { v.expr(x); }
        }
        Pat::TupleStruct(_, ps) | Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => for &x in ast.list(ps) { v.pat(x); },
        Pat::Struct(_, fs, _) => {
            for f in ast.list(fs) {
                v.enter_attrs(f.attrs);
                v.pat(f.pat);
                v.leave_attrs(f.attrs);
            }
        }
        Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => v.pat(x),
    }
}

/// The lint attributes of the nodes around the one being visited, outermost first. Every pass
/// that reports lints keeps one and hands it to `lint::emit_in`.
pub fn push_attrs(stack: &mut Vec<Attr>, ast: &Ast, attrs: List<Attr>) { stack.extend_from_slice(ast.list(attrs)); }
pub fn pop_attrs(stack: &mut Vec<Attr>, ast: &Ast, attrs: List<Attr>) {
    let n = ast.list(attrs).len();
    stack.truncate(stack.len() - n);
}

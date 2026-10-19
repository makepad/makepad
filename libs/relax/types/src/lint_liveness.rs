//! `unused_variables` and `unused_assignments` (rustc_mir_transform/src/liveness.rs): a backward
//! liveness analysis of the user's variables over the control flow of a body, reporting the
//! variables that are never read, the variables that are only assigned, and the assignments
//! whose value is never read.
//!
//! rustc runs it on the MIR of each body owner and closure; here the flow graph is built from
//! the typed syntax tree in MIR's evaluation order. A graph node is one event: a read of a
//! variable (`use`), a write that kills it (`def`), or a recorded assignment that neither reads
//! nor kills it (a field assignment, a compound assignment on a primitive). Code after a
//! diverging expression gets no node (MIR drops unreachable blocks before the lint runs).
//! A closure is its own analysis; the captures it never reads ("dead captures") are not reads
//! of the enclosing function's variables at the closure expression.
//!
//! Check-only (reports through the lint sink); `makepad run` never calls this.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::program::{DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::TyKind;
use std::collections::{HashMap, HashSet};

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AccessKind {
    Param,
    Assign,
    Capture,
}

struct Var {
    local: u32,
    name: String,
    /// positions of the binding identifiers (an or-pattern or macro may bind it more than once)
    intro: Vec<u32>,
    own: bool,
    /// a captured variable the closure holds by reference
    by_ref: bool,
}

#[derive(Default)]
struct Node {
    uses: Vec<u32>,
    def: u32,
    succ: Vec<u32>,
    has_pred: bool,
}

/// An assignment (or parameter / capture entry) of a variable: where rustc records it.
struct Access {
    var: u32,
    kind: AccessKind,
    key: (u32, u32),
    node: u32,
    /// liveness is read on entry to the body (captures), else after the node
    at_entry: bool,
    direct: bool,
    scope: usize,
    /// the value a binding is initialised with (removed from the assignments of a variable that
    /// is never used at all: it is the unused variable)
    init: bool,
}

struct Report {
    lint: &'static Lint,
    pos: u32,
    msg: String,
    scope: usize,
}

struct LoopCx {
    label: Option<String>,
    break_to: u32,
    continue_to: u32,
    is_loop: bool,
}

pub struct Cx<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    /// expression -> the locals its format string names (`{x}`)
    fmt_named: HashMap<u32, Vec<u32>>,
    /// local -> locals' names for the token-scan fallback of macros without parsed arguments
    locals_by_name: HashMap<String, Vec<u32>>,
    scopes: Vec<Vec<Attr>>,
    reports: Vec<Report>,
    closure_memo: HashMap<u32, Vec<(u32, bool)>>,
    /// the pattern analysis context (inhabitedness of call results), made on first use
    pcx: Option<crate::pattern_analysis::Cx<'a>>,
}

struct Unit<'c, 'a> {
    cx: &'c mut Cx<'a>,
    vars: Vec<Var>,
    var_of: HashMap<u32, u32>,
    declared: HashSet<u32>,
    nodes: Vec<Node>,
    accesses: Vec<Access>,
    cur: Option<u32>,
    entry: u32,
    exit: u32,
    loops: Vec<LoopCx>,
    scope_stack: Vec<usize>,
    cur_scope: usize,
    /// the closure kind: FnMut closures run again after returning
    closure: Option<ClosureInfo>,
    decl_scopes: HashMap<u32, usize>,
    pos0: u32,
}

struct ClosureInfo {
    by_ref: HashMap<u32, bool>,
    fn_mut: bool,
}

/// Runs the analysis on one body owner.
pub fn check(prog: &Program, tcx: &Tcx, body: &Body, params: &[(PatId, List<Attr>)], block: Option<BlockId>, expr: Option<ExprId>) {
    if !lint::enabled() { return; }
    // not on the output of `#[derive]`
    let def = prog.def(body.def);
    let mut owner = def.parent;
    while owner != crate::program::NO_DEF && matches!(prog.def(owner).kind, DefKind::Closure | DefKind::AnonConst) { owner = prog.def(owner).parent; }
    if owner != crate::program::NO_DEF && prog.def(owner).kind == DefKind::Impl {
        if lint::is_automatically_derived(prog, owner) { return; }
    }
    let ast = &prog.files[body.file as usize].ast;
    let mut fmt_named: HashMap<u32, Vec<u32>> = HashMap::new();
    for ((e, _), r) in body.fmt_named.iter() {
        if let Res::Local(l) = r { fmt_named.entry(*e).or_default().push(*l); }
    }
    let mut locals_by_name: HashMap<String, Vec<u32>> = HashMap::new();
    for (i, l) in body.locals.iter().enumerate() {
        locals_by_name.entry(prog.syms.str(l.name).to_string()).or_default().push(i as u32);
    }
    let mut cx = Cx { prog, tcx, body, ast, fmt_named, locals_by_name, scopes: vec![Vec::new()], reports: Vec::new(), closure_memo: HashMap::new(), pcx: None };
    {
        let mut u = Unit::new(&mut cx, None);
        for &(p, attrs) in params {
            u.enter(attrs);
            u.param(p);
            u.leave();
        }
        if let Some(b) = block { u.block(b); }
        if let Some(e) = expr { u.expr(e); }
        u.finish();
    }
    let mut reports = std::mem::take(&mut cx.reports);
    reports.sort_by_key(|r| r.pos);
    for r in reports {
        lint::emit_in(prog, body.def, &cx.scopes[r.scope], r.lint, body.file, r.pos, &r.msg);
    }
}

impl<'c, 'a> Unit<'c, 'a> {
    fn new(cx: &'c mut Cx<'a>, closure: Option<ClosureInfo>) -> Self {
        let mut u = Unit {
            cx, vars: Vec::new(), var_of: HashMap::new(), declared: HashSet::new(), nodes: Vec::new(), accesses: Vec::new(),
            cur: None, entry: 0, exit: 0, loops: Vec::new(), scope_stack: vec![0], cur_scope: 0, closure, decl_scopes: HashMap::new(), pos0: 0,
        };
        let entry = u.new_node();
        u.entry = entry;
        u.nodes[entry as usize].has_pred = true;
        u.exit = u.new_node();
        u.cur = Some(entry);
        u
    }

    // ------------------------------------------------------------ graph building

    fn new_node(&mut self) -> u32 {
        self.nodes.push(Node { def: NONE, ..Default::default() });
        self.nodes.len() as u32 - 1
    }

    fn link(&mut self, a: u32, b: u32) {
        self.nodes[a as usize].succ.push(b);
        self.nodes[b as usize].has_pred = true;
    }

    /// A node after the current one (none in unreachable code).
    fn step(&mut self, uses: Vec<u32>, def: u32) -> Option<u32> {
        let c = self.cur?;
        let n = self.new_node();
        self.nodes[n as usize].uses = uses;
        self.nodes[n as usize].def = def;
        self.link(c, n);
        self.cur = Some(n);
        Some(n)
    }

    /// Flow continues into the join node `n` (reachable once something has flowed into it).
    fn merge(&mut self, n: u32) {
        if let Some(c) = self.cur { self.link(c, n); }
        self.cur = if self.nodes[n as usize].has_pred { Some(n) } else { None };
    }

    fn goto(&mut self, n: u32) {
        if let Some(c) = self.cur { self.link(c, n); }
        self.cur = None;
    }

    fn var(&mut self, local: u32) -> u32 {
        if let Some(&v) = self.var_of.get(&local) { return v; }
        let l = &self.cx.body.locals[local as usize];
        let name = self.cx.prog.syms.str(l.name).to_string();
        let own = self.declared.contains(&local);
        let by_ref = self.closure.as_ref().map_or(false, |c| c.by_ref.get(&local).copied().unwrap_or(true));
        self.vars.push(Var { local, name, intro: Vec::new(), own, by_ref });
        let v = self.vars.len() as u32 - 1;
        self.var_of.insert(local, v);
        v
    }

    fn use_local(&mut self, local: u32) {
        let v = self.var(local);
        self.step(vec![v], NONE);
    }

    fn use_all_named(&mut self, name: &str) {
        let Some(ls) = self.cx.locals_by_name.get(name).cloned() else { return };
        for l in ls { self.use_local(l); }
    }

    fn enter(&mut self, attrs: List<Attr>) {
        let a = self.cx.ast.list(attrs);
        if a.is_empty() { self.scope_stack.push(self.cur_scope); return; }
        let mut s = self.cx.scopes[self.cur_scope].clone();
        s.extend_from_slice(a);
        self.cx.scopes.push(s);
        self.cur_scope = self.cx.scopes.len() - 1;
        self.scope_stack.push(self.cur_scope);
    }

    fn leave(&mut self) {
        self.scope_stack.pop();
        self.cur_scope = *self.scope_stack.last().unwrap();
    }

    // ------------------------------------------------------------ patterns and parameters

    fn ignored(name: &str) -> bool {
        name.is_empty() || name == "self" || name.starts_with('_')
    }

    /// The bindings of pattern `p`: each is declared (and written when `define`).
    fn bind_pat(&mut self, p: PatId, define: bool) {
        let ast = self.cx.ast;
        match *ast.pat(p) {
            Pat::Ident { name, sub, .. } => {
                if self.cx.body.pat_res.get(&p.0).is_none() {
                    if let Some(&l) = self.cx.body.pat_local.get(&p.0) {
                        self.declared.insert(l);
                        let v = self.var(l);
                        self.vars[v as usize].own = true;
                        self.vars[v as usize].intro.push(name.lo);
                        let sc = self.cur_scope;
                        self.decl_scopes.entry(v).or_insert(sc);
                        if define {
                            if let Some(n) = self.step(Vec::new(), v) {
                                self.accesses.push(Access { var: v, kind: AccessKind::Assign, key: (name.lo, name.hi), node: n, at_entry: false, direct: true, scope: self.cur_scope, init: true });
                            }
                        }
                    }
                }
                if let Some(s) = sub { self.bind_pat(s, define); }
            }
            Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) | Pat::Wild | Pat::Rest | Pat::Mac(_) => {}
            Pat::Or(ps) => {
                // each alternative is a path of its own to the join
                let join = self.new_node();
                let start = self.cur;
                for &x in ast.list(ps) {
                    self.cur = start;
                    self.bind_pat(x, define);
                    if let Some(c) = self.cur { self.link(c, join); }
                }
                self.cur = if self.nodes[join as usize].has_pred { Some(join) } else { None };
            }
            Pat::TupleStruct(_, ps) | Pat::Tuple(ps) | Pat::Slice(ps) => {
                for &x in ast.list(ps) { self.bind_pat(x, define); }
            }
            Pat::Struct(_, fs, _) => {
                for f in ast.list(fs) {
                    self.enter(f.attrs);
                    self.bind_pat(f.pat, define);
                    self.leave();
                }
            }
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.bind_pat(x, define),
        }
    }

    /// A parameter pattern: a plain name is the argument itself (its value is the entry access).
    fn param(&mut self, p: PatId) {
        let ast = self.cx.ast;
        let mut q = p;
        while let Pat::Paren(x) = *ast.pat(q) { q = x; }
        if let Pat::Ident { name, sub: None, by_ref: false, .. } = *ast.pat(q) {
            if self.cx.body.pat_res.get(&q.0).is_none() {
                if let Some(&l) = self.cx.body.pat_local.get(&q.0) {
                    self.declared.insert(l);
                    let v = self.var(l);
                    self.vars[v as usize].own = true;
                    self.vars[v as usize].intro.push(name.lo);
                    let sc = self.cur_scope;
                    self.decl_scopes.entry(v).or_insert(sc);
                    if let Some(n) = self.step(Vec::new(), v) {
                        self.accesses.push(Access { var: v, kind: AccessKind::Param, key: (name.lo, name.hi), node: n, at_entry: false, direct: true, scope: self.cur_scope, init: true });
                    }
                    return;
                }
            }
        }
        self.bind_pat(p, true);
    }

    // ------------------------------------------------------------ blocks and statements

    fn block(&mut self, b: BlockId) {
        let ast = self.cx.ast;
        let blk = *ast.block(b);
        self.enter(blk.attrs);
        for s in ast.stmts(b) { self.stmt(s); }
        self.leave();
    }

    fn stmt(&mut self, s: &Stmt) {
        let ast = self.cx.ast;
        match *s {
            Stmt::Item(_) | Stmt::Empty => {}
            Stmt::Expr(e, _) => {
                let attrs = ast.stmt_attrs(e);
                self.enter(attrs);
                self.expr(e);
                self.leave();
            }
            Stmt::Attrs(r) => {
                let a = *ast.row(r);
                self.enter(a.attrs);
                self.expr(a.expr);
                self.leave();
            }
            Stmt::Let(r) => {
                let l = *ast.row(r);
                self.enter(l.attrs);
                match l.init {
                    Some(init) => {
                        self.expr(init);
                        if let Some(e) = l.else_ {
                            // refutable: the else block diverges
                            let start = self.cur;
                            self.block(e);
                            self.cur = start;
                            self.bind_pat(l.pat, true);
                        } else {
                            self.bind_pat(l.pat, true);
                        }
                    }
                    None => self.bind_pat(l.pat, false),
                }
                self.leave();
            }
        }
    }

    // ------------------------------------------------------------ expressions

    fn exprs(&mut self, l: List<ExprId>) {
        let ast = self.cx.ast;
        for &x in ast.list(l) { self.expr(x); }
    }

    fn is_never(&self, e: ExprId) -> bool {
        let b = self.cx.body;
        let i = b.expr_ix(e);
        i < b.expr_ty.len() && b.expr_typed.get(i) && matches!(self.cx.tcx.tys.kind(b.expr_ty[i]), TyKind::Never)
    }

    fn ty_kind_of(&self, e: ExprId) -> Option<TyKind> {
        let b = self.cx.body;
        let i = b.expr_ix(e);
        (i < b.expr_ty.len() && b.expr_typed.get(i)).then(|| *self.cx.tcx.tys.kind(b.expr_ty[i]))
    }

    fn expr(&mut self, e: ExprId) {
        let ast = self.cx.ast;
        let ex = *ast.expr(e);
        match ex.kind {
            ExprKind::Lit(_) | ExprKind::Continue(_) | ExprKind::Underscore => {
                if let ExprKind::Continue(l) = ex.kind {
                    if let Some(t) = self.find_loop(l.get()).map(|i| self.loops[i].continue_to) { self.goto(t); }
                }
            }
            ExprKind::Path(_) => {
                if let Some(Res::Local(l)) = self.cx.body.res.get(&e.0) { self.use_local(*l); }
            }
            ExprKind::Paren(x) | ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Box(x) | ExprKind::Await(x)
            | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::AddrOf(_, _, x) => self.expr(x),
            ExprKind::Try(x) => {
                self.expr(x);
                // the residual path returns
                if let Some(c) = self.cur {
                    let ex = self.exit;
                    self.link(c, ex);
                }
            }
            ExprKind::Binary(op, a, b) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    self.expr(a);
                    let join = self.new_node();
                    if let Some(c) = self.cur { self.link(c, join); }
                    self.expr(b);
                    self.merge(join);
                } else {
                    self.expr(a);
                    self.expr(b);
                }
            }
            ExprKind::Assign(l, r) => self.assign(e, ex, l, r),
            ExprKind::AssignOp(_, l, r) => self.assign_op(e, ex, l, r),
            ExprKind::Index(a, b) | ExprKind::Repeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(f, args) => {
                self.expr(f);
                self.exprs(args);
                // a tuple-struct or variant constructor is an aggregate, not a call (THIR `Adt`)
                let ctor = match ast.expr(f).kind {
                    ExprKind::Path(_) => matches!(self.cx.body.res.get(&f.0), Some(Res::Def(d)) if matches!(self.cx.prog.def(*d).kind, DefKind::Struct | DefKind::Variant)),
                    _ => false,
                };
                if !ctor { self.after_call(e); }
            }
            ExprKind::MethodCall(recv, row) => {
                self.expr(recv);
                self.exprs(ast.row(row).args);
                self.after_call(e);
            }
            ExprKind::Block(b, label) => {
                if let Some(l) = label.get() {
                    let join = self.new_node();
                    let text = self.cx.prog.files[self.cx.body.file as usize].text(l).to_string();
                    self.loops.push(LoopCx { label: Some(text), break_to: join, continue_to: join, is_loop: false });
                    self.block(b);
                    self.loops.pop();
                    self.merge(join);
                } else {
                    self.block(b);
                }
            }
            ExprKind::Unsafe(b) => self.block(b),
            ExprKind::ConstBlock(_) => {}
            ExprKind::Async(_, b) => self.closure_like(e, None, Some(b)),
            ExprKind::If(c, then, els) => {
                let then_n = self.new_node();
                let else_n = self.new_node();
                let join = self.new_node();
                self.cond(c, then_n, else_n);
                self.merge(then_n);
                self.block(then);
                self.merge(join);
                self.cur = None;
                self.merge(else_n);
                if let Some(x) = els.get() { self.expr(x); }
                self.merge(join);
            }
            ExprKind::Let(p, x) => {
                self.expr(x);
                self.bind_pat(p, true);
            }
            ExprKind::Match(x, arms) => self.match_expr(x, arms),
            ExprKind::While(row) => {
                let w = *ast.row(row);
                let head = self.new_node();
                let body_n = self.new_node();
                let exit = self.new_node();
                self.merge(head);
                let label = w.label.map(|l| self.cx.prog.files[self.cx.body.file as usize].text(l).to_string());
                self.loops.push(LoopCx { label, break_to: exit, continue_to: head, is_loop: true });
                self.cond(w.cond, body_n, exit);
                self.merge(body_n);
                self.block(w.body);
                self.goto(head);
                self.loops.pop();
                self.merge(exit);
            }
            ExprKind::For(row) => {
                let f = *ast.row(row);
                self.expr(f.iter);
                let head = self.new_node();
                let exit = self.new_node();
                self.merge(head);
                let label = f.label.map(|l| self.cx.prog.files[self.cx.body.file as usize].text(l).to_string());
                self.loops.push(LoopCx { label, break_to: exit, continue_to: head, is_loop: true });
                if let Some(c) = self.cur { self.link(c, exit); }
                self.bind_pat(f.pat, true);
                self.block(f.body);
                self.goto(head);
                self.loops.pop();
                self.merge(exit);
            }
            ExprKind::Loop(b, label) => {
                let head = self.new_node();
                let exit = self.new_node();
                self.merge(head);
                let label = label.get().map(|l| self.cx.prog.files[self.cx.body.file as usize].text(l).to_string());
                self.loops.push(LoopCx { label, break_to: exit, continue_to: head, is_loop: true });
                self.block(b);
                self.goto(head);
                self.loops.pop();
                self.merge(exit);
            }
            ExprKind::Break(label, val) => {
                if let Some(v) = val.get() { self.expr(v); }
                let l = label.get().map(|l| self.cx.prog.files[self.cx.body.file as usize].text(l).to_string());
                if let Some(t) = self.find_loop_named(l.as_deref(), true).map(|i| self.loops[i].break_to) { self.goto(t); } else { self.cur = None; }
            }
            ExprKind::Return(val) => {
                if let Some(v) = val.get() { self.expr(v); }
                let ex = self.exit;
                self.goto(ex);
            }
            ExprKind::Yield(val) => {
                if let Some(v) = val.get() { self.expr(v); }
            }
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                self.closure_like(e, Some(c), None);
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => self.exprs(xs),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                for f in ast.list(s.fields) {
                    self.enter(f.attrs);
                    self.expr(f.expr);
                    self.leave();
                }
                if let Some(x) = s.base.get() { self.expr(x); }
            }
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { self.expr(x); }
                if let Some(x) = b.get() { self.expr(x); }
            }
            ExprKind::Mac(m) => self.mac(e, m),
        }
    }

    /// A call that never returns (its result is `!`, or a type without values) ends the flow.
    fn after_call(&mut self, e: ExprId) {
        if self.is_never(e) || self.uninhabited(e) { self.cur = None; }
    }

    fn uninhabited(&mut self, e: ExprId) -> bool {
        let b = self.cx.body;
        let i = b.expr_ix(e);
        if i >= b.expr_ty.len() || !b.expr_typed.get(i) { return false; }
        let ty = b.expr_ty[i];
        if !matches!(self.cx.tcx.tys.kind(ty), TyKind::Adt(..)) { return false; }
        let cx = self.cx.pcx.get_or_insert_with(|| crate::pattern_analysis::body_cx(self.cx.prog, self.cx.tcx, self.cx.body));
        cx.is_uninhabited(ty)
    }

    fn find_loop(&self, label: Option<Ident>) -> Option<usize> {
        let l = label.map(|l| self.cx.prog.files[self.cx.body.file as usize].text(l).to_string());
        self.find_loop_named(l.as_deref(), false)
    }

    fn find_loop_named(&self, label: Option<&str>, is_break: bool) -> Option<usize> {
        match label {
            Some(l) => self.loops.iter().rposition(|c| c.label.as_deref() == Some(l)),
            None => {
                let _ = is_break;
                self.loops.iter().rposition(|c| c.is_loop)
            }
        }
    }

    /// Evaluates a condition, flowing to `t` when it holds and `f` when it does not (`&&`, `||`,
    /// `!` and `let` conditions branch as in MIR).
    fn cond(&mut self, c: ExprId, t: u32, f: u32) {
        let ast = self.cx.ast;
        match ast.expr(c).kind {
            ExprKind::Paren(x) => self.cond(x, t, f),
            ExprKind::Unary(UnOp::Not, x) if matches!(self.ty_kind_of(c), Some(TyKind::Bool)) => self.cond(x, f, t),
            ExprKind::Binary(BinOp::And, a, b) => {
                let mid = self.new_node();
                self.cond(a, mid, f);
                self.merge(mid);
                self.cond(b, t, f);
            }
            ExprKind::Binary(BinOp::Or, a, b) => {
                let mid = self.new_node();
                self.cond(a, t, mid);
                self.merge(mid);
                self.cond(b, t, f);
            }
            ExprKind::Let(p, x) => {
                self.expr(x);
                // fails: the bindings are not assigned
                if let Some(cn) = self.cur { self.link(cn, f); }
                self.bind_pat(p, true);
                if let Some(cn) = self.cur { self.link(cn, t); }
                self.cur = None;
            }
            _ => {
                self.expr(c);
                if let Some(cn) = self.cur {
                    self.link(cn, t);
                    self.link(cn, f);
                }
                self.cur = None;
            }
        }
    }

    fn match_expr(&mut self, x: ExprId, arms: List<Arm>) {
        let ast = self.cx.ast;
        self.expr(x);
        let join = self.new_node();
        let start = self.cur;
        let arms = ast.list(arms);
        let starts: Vec<u32> = arms.iter().map(|_| self.new_node()).collect();
        for (i, a) in arms.iter().enumerate() {
            // every arm is entered from the scrutinee (and from the previous arm's failed guard)
            if let Some(s) = start { self.link(s, starts[i]); }
            self.cur = if self.nodes[starts[i] as usize].has_pred { Some(starts[i]) } else { None };
            self.enter(a.attrs);
            self.bind_pat(a.pat, true);
            if let Some(g) = a.guard {
                let ok = self.new_node();
                let next = starts.get(i + 1).copied().unwrap_or(join);
                // a failed guard falls through to the next arm (or nowhere: exhaustive)
                let fail = if i + 1 < starts.len() { next } else { self.new_node() };
                self.cond(g, ok, fail);
                self.merge(ok);
            }
            self.expr(a.body);
            self.leave();
            self.merge(join);
            self.cur = None;
        }
        self.cur = if self.nodes[join as usize].has_pred { Some(join) } else { None };
    }

    // ------------------------------------------------------------ assignments

    fn record(&mut self, var: u32, key: (u32, u32), node: u32, direct: bool) {
        self.accesses.push(Access { var, kind: AccessKind::Assign, key, node, at_entry: false, direct, scope: self.cur_scope, init: false });
    }

    fn peel(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Paren(x) = self.cx.ast.expr(e).kind { e = x; }
        e
    }

    fn assign(&mut self, e: ExprId, ex: Expr, l: ExprId, r: ExprId) {
        // MIR evaluates the value, then the place
        self.expr(r);
        self.assign_to(l, (ex.lo, ex.hi));
        let _ = e;
    }

    /// A write to place `l` (a destructuring assignment writes each leaf).
    fn assign_to(&mut self, l: ExprId, key: (u32, u32)) {
        let ast = self.cx.ast;
        let l = self.peel(l);
        match ast.expr(l).kind {
            ExprKind::Underscore => {}
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                for &x in ast.list(xs) {
                    let ex = ast.expr(x);
                    self.assign_to(x, (ex.lo, ex.hi));
                }
            }
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                for f in ast.list(s.fields) {
                    let ex = ast.expr(f.expr);
                    self.assign_to(f.expr, (ex.lo, ex.hi));
                }
            }
            ExprKind::Call(f, args) if matches!(ast.expr(f).kind, ExprKind::Path(_)) => {
                for &x in ast.list(args) {
                    let ex = ast.expr(x);
                    self.assign_to(x, (ex.lo, ex.hi));
                }
            }
            _ => {
                let (root, direct, indirect) = self.place(l);
                match root {
                    Some(local) if !indirect => {
                        let v = self.var(local);
                        if direct {
                            if let Some(n) = self.step(Vec::new(), v) { self.record(v, key, n, true); }
                        } else if let Some(n) = self.step(Vec::new(), NONE) {
                            self.record(v, key, n, false);
                        }
                    }
                    Some(local) => self.use_local(local),
                    None => {}
                }
            }
        }
    }

    /// The place's root local, whether it is the local itself, and whether it goes through a
    /// dereference. Operands of the place (an index) are evaluated.
    fn place(&mut self, e: ExprId) -> (Option<u32>, bool, bool) {
        let ast = self.cx.ast;
        let e = self.peel(e);
        match ast.expr(e).kind {
            ExprKind::Path(_) => match self.cx.body.res.get(&e.0) {
                Some(Res::Local(l)) => (Some(*l), true, false),
                _ => (None, false, false),
            },
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (r, _, ind) = self.place(x);
                let deref = matches!(self.ty_kind_of(x), Some(TyKind::Ref(..) | TyKind::Ptr(..))) || self.is_box(x);
                (r, false, ind || deref)
            }
            ExprKind::Index(a, i) => {
                let (r, _, ind) = self.place(a);
                self.expr(i);
                let overloaded = self.cx.body.ov_index.get(&e.0).is_some();
                let deref = matches!(self.ty_kind_of(a), Some(TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::Slice(_))) || overloaded;
                (r, false, ind || deref)
            }
            ExprKind::Unary(UnOp::Deref, x) => {
                self.expr(x);
                (None, false, true)
            }
            _ => {
                self.expr(e);
                (None, false, true)
            }
        }
    }

    fn is_box(&self, e: ExprId) -> bool {
        match self.ty_kind_of(e) {
            Some(TyKind::Adt(d, ..)) => self.cx.prog.def(d).kind == DefKind::Struct && crate::lint::is_diagnostic_item(self.cx.prog, d, "Box"),
            _ => false,
        }
    }

    fn assign_op(&mut self, e: ExprId, ex: Expr, l: ExprId, r: ExprId) {
        self.expr(r);
        let overloaded = self.cx.body.binops.get(&e.0).is_some();
        if overloaded {
            // `AddAssign::add_assign(&mut place, rhs)`: a borrow of the place
            self.expr(l);
            return;
        }
        // a primitive compound assignment reads and writes the place without a new value: the
        // variable is neither read nor killed, the write is recorded
        let (root, direct, indirect) = self.place(l);
        match root {
            Some(local) if !indirect => {
                let v = self.var(local);
                if let Some(n) = self.step(Vec::new(), NONE) { self.record(v, (ex.lo, ex.hi), n, direct); }
            }
            Some(local) => self.use_local(local),
            None => {}
        }
    }

    // ------------------------------------------------------------ macros

    fn mac(&mut self, e: ExprId, m: Row<MacCall>) {
        let ast = self.cx.ast;
        let m = *ast.row(m);
        if m.args == NO_MAC_ARGS {
            // arguments that were not parsed: every identifier that names a variable is a read
            let pf = &self.cx.prog.files[self.cx.body.file as usize];
            let names: Vec<String> = (m.toks.lo..m.toks.hi).map(|i| pf.tok_text(i).to_string()).filter(|t| t.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')).collect();
            for n in names { self.use_all_named(&n); }
            self.after_call(e);
            return;
        }
        let args = *ast.mac_arg(m.args);
        let exprs = ast.list(args.exprs);
        let name = {
            let segs = ast.segs(m.path.segs);
            segs.last().map(|s| self.cx.prog.files[self.cx.body.file as usize].text(s.name).to_string()).unwrap_or_default()
        };
        // assert!(cond, msg..): the message is only evaluated when the assertion fails
        let checked = match name.as_str() {
            "assert" | "debug_assert" => 1,
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => 2,
            _ => usize::MAX,
        };
        if checked != usize::MAX {
            for &x in exprs.iter().take(checked) { self.expr(x); }
            let join = self.new_node();
            if let Some(c) = self.cur { self.link(c, join); }
            for &x in exprs.iter().skip(checked) { self.expr(x); }
            self.named_args(e);
            self.cur = None;
            self.merge(join);
            return;
        }
        for &x in exprs { self.expr(x); }
        if let Some(p) = args.pat {
            // matches!(e, pat if guard)
            self.bind_pat(p, true);
            if let Some(g) = args.guard { self.expr(g); }
        } else if let Some(g) = args.guard {
            self.expr(g);
        }
        if let Some(asm) = args.asm {
            for op in ast.list(asm.ops) {
                if let Some(i) = op.input { self.expr(i); }
                if let Some(o) = op.output { self.expr(o); }
            }
        }
        self.named_args(e);
        self.after_call(e);
    }

    /// `{x}` captured by a format string.
    fn named_args(&mut self, e: ExprId) {
        if let Some(ls) = self.cx.fmt_named.get(&e.0).cloned() {
            for l in ls { self.use_local(l); }
        }
    }

    // ------------------------------------------------------------ closures

    /// A closure or async block: analysed on its own; at the expression the variables it reads
    /// are read.
    fn closure_like(&mut self, e: ExprId, c: Option<ClosureRow>, block: Option<BlockId>) {
        let captured = self.analyse_closure(e, c, block);
        for (l, dead) in captured {
            if !dead { self.use_local(l); }
        }
    }

    fn analyse_closure(&mut self, e: ExprId, c: Option<ClosureRow>, block: Option<BlockId>) -> Vec<(u32, bool)> {
        if let Some(m) = self.cx.closure_memo.get(&e.0) { return m.clone(); }
        let info = match self.cx.body.min_captures.get(&e.0) {
            Some(mc) => {
                let mut by_ref: HashMap<u32, bool> = HashMap::new();
                for cap in &mc.captures {
                    let r = matches!(cap.info.kind, crate::upvar::CaptureKind::ByRef(_));
                    let ent = by_ref.entry(cap.place.root).or_insert(true);
                    *ent = *ent && r;
                }
                ClosureInfo { by_ref, fn_mut: mc.kind == crate::upvar::ClosureKind::FnMut }
            }
            None => ClosureInfo { by_ref: HashMap::new(), fn_mut: false },
        };
        let scope = self.cur_scope;
        let pos0 = self.cx.ast.expr(e).lo;
        let res = {
            let mut u = Unit::new(&mut *self.cx, Some(info));
            u.scope_stack = vec![scope];
            u.cur_scope = scope;
            u.pos0 = pos0;
            if let Some(c) = c {
                let ast = u.cx.ast;
                for p in ast.list(c.params) {
                    u.enter(p.attrs);
                    u.param(p.pat);
                    u.leave();
                }
                u.expr(c.body);
            }
            if let Some(b) = block { u.block(b); }
            u.finish()
        };
        self.cx.closure_memo.insert(e.0, res.clone());
        res
    }

    // ------------------------------------------------------------ analysis and reports

    /// Solves the liveness, reports, and returns the captured variables with whether each is dead
    /// (never read in the closure).
    fn finish(mut self) -> Vec<(u32, bool)> {
        // the end of the body
        let exit = self.exit;
        if let Some(c) = self.cur { self.link(c, exit); }
        if let Some(info) = &self.closure {
            // by-reference captures are read by the environment when the closure returns
            let refs: Vec<u32> = (0..self.vars.len() as u32).filter(|&v| !self.vars[v as usize].own && self.vars[v as usize].by_ref).collect();
            self.nodes[exit as usize].uses = refs;
            if info.fn_mut {
                // a closure can be called again after it returns
                let entry = self.entry;
                self.link(exit, entry);
            }
        }
        let nv = self.vars.len();
        let words = (nv + 63) / 64;
        let nn = self.nodes.len();
        // reachable from the entry
        let mut reach = vec![false; nn];
        let mut stack = vec![self.entry];
        while let Some(n) = stack.pop() {
            if reach[n as usize] { continue; }
            reach[n as usize] = true;
            for &s in &self.nodes[n as usize].succ { stack.push(s); }
        }
        let mut live_in = vec![0u64; nn * words.max(1)];
        let w = words.max(1);
        let mut changed = true;
        let mut out = vec![0u64; w];
        while changed {
            changed = false;
            for n in (0..nn).rev() {
                if !reach[n] { continue; }
                for x in out.iter_mut() { *x = 0; }
                for &s in &self.nodes[n].succ {
                    let s = s as usize;
                    if !reach[s] { continue; }
                    for k in 0..w { out[k] |= live_in[s * w + k]; }
                }
                let d = self.nodes[n].def;
                if d != NONE { out[(d / 64) as usize] &= !(1u64 << (d % 64)); }
                for &u in &self.nodes[n].uses { out[(u / 64) as usize] |= 1u64 << (u % 64); }
                for k in 0..w {
                    if live_in[n * w + k] != out[k] { live_in[n * w + k] = out[k]; changed = true; }
                }
            }
        }
        let bit = |set: &[u64], v: u32| set[(v / 64) as usize] >> (v % 64) & 1 == 1;
        let live_out = |this: &Self, n: u32| -> Vec<u64> {
            let mut o = vec![0u64; w];
            for &s in &this.nodes[n as usize].succ {
                if !reach[s as usize] { continue; }
                for k in 0..w { o[k] |= live_in[s as usize * w + k]; }
            }
            o
        };
        // ever live
        let mut ever = vec![false; nv];
        for n in 0..nn {
            if !reach[n] { continue; }
            let o = live_out(&self, n as u32);
            for v in 0..nv as u32 {
                if bit(&live_in[n * w..n * w + w], v) || bit(&o, v) { ever[v as usize] = true; }
            }
        }
        // the accesses, merged per variable and source span
        struct Merged { kind: AccessKind, key: (u32, u32), live: bool, direct: bool, scope: usize, init: bool }
        let mut per_var: Vec<Vec<Merged>> = (0..nv).map(|_| Vec::new()).collect();
        for a in &self.accesses {
            if !reach[a.node as usize] { continue; }
            let live = if a.at_entry { bit(&live_in[self.entry as usize * w..self.entry as usize * w + w], a.var) } else { bit(&live_out(&self, a.node), a.var) };
            match per_var[a.var as usize].iter_mut().find(|m| m.key == a.key) {
                Some(m) => { m.live |= live; m.direct &= a.direct; m.init |= a.init; }
                None => per_var[a.var as usize].push(Merged { kind: a.kind, key: a.key, live, direct: a.direct, scope: a.scope, init: a.init }),
            }
        }
        // captured by value: an access at entry
        for v in 0..nv {
            let var = &self.vars[v];
            if var.own || var.by_ref || Self::ignored(&var.name) { continue; }
            if self.closure.is_some() {
                let live = bit(&live_in[self.entry as usize * w..self.entry as usize * w + w], v as u32);
                per_var[v].push(Merged { kind: AccessKind::Capture, key: (0, 0), live, direct: true, scope: self.cur_scope, init: false });
            }
        }
        // does the variable appear in reachable code
        let mut referenced = vec![false; nv];
        for n in 0..nn {
            if !reach[n] { continue; }
            if self.nodes[n].def != NONE { referenced[self.nodes[n].def as usize] = true; }
            for &u in &self.nodes[n].uses { referenced[u as usize] = true; }
        }
        for a in &self.accesses { if reach[a.node as usize] { referenced[a.var as usize] = true; } }
        let mut reports: Vec<Report> = Vec::new();
        let file = self.cx.body.file;
        let prog = self.cx.prog;
        let env = crate::tcx::TypingEnv::fully_monomorphized(prog);
        for v in 0..nv {
            let var = &self.vars[v];
            if Self::ignored(&var.name) { continue; }
            let ty = self.cx.body.locals[var.local as usize].ty;
            let drop_guard = || {
                matches!(self.cx.tcx.tys.kind(ty), TyKind::Closure(..) | TyKind::Tuple(_) | TyKind::Adt(..) | TyKind::Dyn(..) | TyKind::Array(..) | TyKind::Slice(_))
                    && self.cx.tcx.needs_drop(prog, env, ty)
            };
            let def_pos = var.intro.first().copied().unwrap_or(0);
            let mut statements: Vec<&Merged> = per_var[v].iter().filter(|m| m.kind == AccessKind::Assign && !m.init).collect();
            if var.own && !ever[v] {
                if !self.scope_of_decl_ok(def_pos) { continue; }
                if statements.is_empty() {
                    if !referenced[v] { continue; }
                    let scope = self.decl_scope(v);
                    reports.push(Report { lint: &lint::UNUSED_VARIABLES, pos: def_pos, msg: format!("unused variable: `{}`", var.name), scope });
                    continue;
                }
                if drop_guard() {
                    statements.retain(|m| m.direct);
                    if statements.is_empty() { continue; }
                }
                let scope = self.decl_scope(v);
                reports.push(Report { lint: &lint::UNUSED_VARIABLES, pos: def_pos, msg: format!("variable `{}` is assigned to, but never used", var.name), scope });
            }
            if !var.own && !ever[v] { continue; }
            // dead assignments
            let guard = drop_guard();
            for m in &per_var[v] {
                // the initialisation of an unused variable is the unused variable itself
                if m.init && var.own && !ever[v] { continue; }
                let should = !m.live && (m.direct || !guard);
                if !should { continue; }
                match m.kind {
                    AccessKind::Assign => {
                        if lint::external(prog, file, m.key.0) { continue; }
                        reports.push(Report { lint: &lint::UNUSED_ASSIGNMENTS, pos: m.key.0, msg: format!("value assigned to `{}` is never read", var.name), scope: m.scope });
                    }
                    AccessKind::Param => {
                        // an unused parameter is reported as an unused variable
                        if var.own && !ever[v] { continue; }
                        if lint::external(prog, file, m.key.0) { continue; }
                        reports.push(Report { lint: &lint::UNUSED_ASSIGNMENTS, pos: m.key.0, msg: format!("value passed to `{}` is never read", var.name), scope: m.scope });
                    }
                    AccessKind::Capture => {
                        reports.push(Report { lint: &lint::UNUSED_ASSIGNMENTS, pos: self.pos0, msg: format!("value captured by `{}` is never read", var.name), scope: m.scope });
                    }
                }
            }
        }
        // report in the unit's lint sink
        self.cx.reports.extend(reports.into_iter().filter(|r| !lint::external(prog, file, r.pos)));
        // the captures
        (0..nv).filter(|&v| !self.vars[v].own).map(|v| (self.vars[v].local, !ever[v])).collect()
    }

    fn scope_of_decl_ok(&self, _pos: u32) -> bool { true }

    /// The lint scope at the declaration of variable `v`.
    fn decl_scope(&self, v: usize) -> usize {
        self.decl_scopes.get(&(v as u32)).copied().unwrap_or(0)
    }
}

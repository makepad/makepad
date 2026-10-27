//! Lexical destruction scopes, independent of borrow checking and RIR lowering.
//! rustc_hir_analysis/src/check/region.rs:56,194,254,406,620,702,852;
//! rustc_middle/src/middle/region.rs:279,308,346. Scope membership is not move liveness.
use crate::ast::*;
use crate::fxhash::HashMap;
use crate::parser::ParsedFile;
use crate::program::{DefKind, Program};
use crate::typeck::{Body, Res};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Expr(u32), Block(u32), Statement(u32), Pattern(u32), Arm(u32),
    /// AST lowering's generated scopes have stable identities within their source expression.
    Synthetic(u32, Synthetic),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Synthetic { Iteration, ForMatch, ForArm, ForBody, ForIntoIter, ForNext, TryBranch, TryFromResidual, WhileElse, WhileElseBlock, WhileBreak, WhileBreakExpr, AwaitIntoFuture, AwaitPollCall, AwaitNewUnchecked, AwaitGetContext, AwaitArm, AwaitLoop, AwaitBody, AwaitPoll, AwaitReady, AwaitPending, AwaitYield, MacroArm, MacroBranch, MacroStatement, DebugBranch, AssignBlock, AssignLet, Assignment }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScopeData { Node, CallSite, Arguments, Destruction, IfThen, IfThenRescope, MatchGuard, Remainder(u32) }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyId(pub u32);
#[derive(Clone, Copy, Debug)]
pub struct Scope { pub node: Node, pub data: ScopeData, pub parent: Option<ScopeId>, pub body: BodyId }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind { Function, Closure, Coroutine, Constant }
#[derive(Debug)]
pub struct BodyScope { pub root: Node, pub kind: BodyKind, pub arguments: ScopeId, pub body_expr_count: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExprPosition { pub body: BodyId, pub count: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YieldKind { Yield, Await { operand: ExprId, future_scope: ScopeId } }
#[derive(Clone, Copy, Debug)]
pub struct YieldPoint { pub expr: ExprId, pub position: ExprPosition, pub scope: ScopeId, pub kind: YieldKind }

#[derive(Default, Debug)]
pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub bodies: Vec<BodyScope>,
    pub yields: Vec<YieldPoint>,
    nodes: HashMap<(Node, ScopeData), ScopeId>,
    var_map: HashMap<u32, ScopeId>,
    pub rvalue_scopes: HashMap<u32, Option<ScopeId>>,
    /// The tail expressions of blocks of editions before 2024, whose temporaries are dropped earlier
    /// from edition 2024 (rustc `backwards_incompatible_scope`; recorded for the lint
    /// `tail_expr_drop_order`, which only `makepad check` runs).
    backwards_incompatible_nodes: std::collections::HashSet<Node>,
    positions: HashMap<u32, ExprPosition>,
    yield_in_scope: Vec<Vec<u32>>,
}
impl ScopeTree {
    pub fn scope(&self, node: Node, data: ScopeData) -> Option<ScopeId> { self.nodes.get(&(node, data)).copied() }
    pub fn encl_scope(&self, scope: ScopeId) -> Option<ScopeId> { self.scopes[scope.0 as usize].parent }
    pub fn var_scope(&self, local: u32) -> Option<ScopeId> { self.var_map.get(&local).copied() }
    pub fn expression_position(&self, expr: ExprId) -> Option<ExprPosition> { self.positions.get(&expr.0).copied() }
    pub fn is_subscope_of(&self, mut sub: ScopeId, sup: ScopeId) -> bool {
        loop { if sub == sup { return true; } match self.encl_scope(sub) { Some(p) => sub = p, None => return false } }
    }
    /// Indices into `yields`, restricted to this scope's body, in visitation order.
    pub fn yields_in_scope(&self, scope: ScopeId) -> &[u32] { &self.yield_in_scope[scope.0 as usize] }
    /// `None` is static extension, never an unknown expression (which is a caller error).
    /// rustc_middle/src/middle/region.rs:308,346: return the child of Destruction.
    pub fn temporary_scope(&self, expr: ExprId) -> Option<ScopeId> { self.temp_lifetime(expr).0 }
    /// rustc `ScopeTree::temporary_scope`: the scope the temporaries of `expr` die in, and the scope
    /// they will die in from edition 2024 when that differs (`TempLifetime`).
    pub fn temp_lifetime(&self, expr: ExprId) -> (Option<ScopeId>, Option<ScopeId>) {
        let mut scope = self.scope(Node::Expr(expr.0), ScopeData::Node).expect("expression outside scope tree");
        if let Some(s) = self.rvalue_scopes.get(&expr.0) { return (*s, None); }
        let mut backwards_incompatible = None;
        while let Some(p) = self.encl_scope(scope) {
            let data = &self.scopes[p.0 as usize];
            match data.data {
                ScopeData::Destruction => return (Some(scope), backwards_incompatible),
                ScopeData::IfThenRescope | ScopeData::MatchGuard => return (Some(p), backwards_incompatible),
                _ => {
                    // passing through a tail expression whose temporaries live longer than they will
                    // from edition 2024: remember the first one (region.rs `default_temporary_scope`)
                    if backwards_incompatible.is_none() && self.backwards_incompatible_nodes.contains(&data.node) {
                        backwards_incompatible = self.scope(data.node, ScopeData::Node);
                    }
                    scope = p;
                }
            }
        }
        panic!("expression without destruction scope")
    }
}

/// The sole edition hook. Frontend expansion contexts will replace this file-level input.
/// rustc_hir_analysis/src/check/region.rs:146,357 uses the node's span edition.
pub fn node_edition(file: &ParsedFile, _node: Node) -> u16 { file.edition }

/// Read-only provider for a typechecked item and its nested bodies. No consumer switches here.
/// A query owner can cache this alongside the parsed body and its resolved constructor identities.
pub fn region_scope_tree(prog: &Program, body: &Body) -> Result<ScopeTree, String> {
    let d = prog.def(body.def);
    let file = &prog.files[body.file as usize];
    let mut v = Resolve { prog, file, body, tree: ScopeTree::default(), cx: Context { parent: None, var_parent: None, body: BodyId(0) } };
    // an anonymous constant (array length, const argument or default in a signature or type) is a
    // body of its own, resolved as a constant initializer (rustc region.rs resolve_body: a body
    // owner that is not a fn or closure)
    if d.kind == DefKind::AnonConst {
        let Some((_, crate::def_collector::NodeId::AnonConst(e))) = prog.def_id_to_node_id.get(&body.def) else { return Err("anonymous constant without an expression".into()) };
        v.enter_body(Node::Expr(e.0), BodyKind::Constant);
        v.extend_borrows(*e, None);
        v.expr(*e, true);
        return Ok(v.tree);
    }
    match file.ast.item(d.item).kind {
        ItemKind::Fn(sig, _, _) => {
            let block = prog.fn_body(body.file, d.item)?.ok_or("item has no body")?;
            let sig = file.ast.fn_sig(sig);
            let kind = if sig.asyncness { BodyKind::Coroutine } else { BodyKind::Function };
            v.enter_body(Node::Block(block.0), kind);
            for p in file.ast.list(sig.params) { v.pat(p.pat); }
            // `self` has no AST pattern; typeck represents it with the sentinel pattern ID.
            for (i, local) in body.locals.iter().enumerate() {
                if local.pat == u32::MAX { v.tree.var_map.insert(i as u32, v.cx.var_parent.unwrap()); }
            }
            v.block(block, true);
        }
        ItemKind::Const(_, init) | ItemKind::Static(_, _, init) if init.is_some() => {
            let e = prog.const_body(d.file, d.item)?.ok_or("missing initializer")?;
            v.enter_body(Node::Expr(e.0), BodyKind::Constant);
            v.extend_borrows(e, None);
            v.expr(e, true);
        }
        _ => return Err("item has no region scope body".into()),
    }
    Ok(v.tree)
}
#[derive(Clone, Copy)]
struct Context { parent: Option<ScopeId>, var_parent: Option<ScopeId>, body: BodyId }
struct Resolve<'a> { prog: &'a Program, file: &'a ParsedFile, body: &'a Body, tree: ScopeTree, cx: Context }
impl Resolve<'_> {
    fn enter(&mut self, node: Node, data: ScopeData) -> ScopeId {
        let id = ScopeId(self.tree.scopes.len() as u32);
        assert!(self.tree.nodes.insert((node, data), id).is_none(), "duplicate scope: {node:?} {data:?}");
        self.tree.scopes.push(Scope { node, data, parent: self.cx.parent, body: self.cx.body });
        self.tree.yield_in_scope.push(Vec::new());
        self.cx.parent = Some(id); id
    }
    fn node(&mut self, node: Node, terminating: bool) -> ScopeId {
        if terminating { self.enter(node, ScopeData::Destruction); }
        self.enter(node, ScopeData::Node)
    }
    fn enter_body(&mut self, root: Node, kind: BodyKind) {
        self.cx.body = BodyId(self.tree.bodies.len() as u32);
        self.enter(root, ScopeData::CallSite);
        let arguments = self.enter(root, ScopeData::Arguments);
        self.tree.bodies.push(BodyScope { root, kind, arguments, body_expr_count: 0 });
        self.cx.var_parent = if kind == BodyKind::Constant { None } else { Some(arguments) };
    }
    fn block_tail(&self, b: BlockId) -> Option<ExprId> {
        match self.file.ast.stmts(b).last().and_then(|s| self.file.ast.stmt_expr(s)) { Some((e, false)) => Some(e), _ => None }
    }
    /// Remainders are nested in declaration order; let-else exits to the preceding context.
    /// rustc_hir_analysis/src/check/region.rs:56-167.
    fn block(&mut self, b: BlockId, terminating: bool) {
        let saved = self.cx;
        self.node(Node::Block(b.0), terminating);
        self.cx.var_parent = self.cx.parent;
        let ast = &self.file.ast;
        let statements = ast.block(b).stmts;
        for (i, stmt) in ast.list(statements).iter().enumerate() {
            if i + 1 == statements.len() {
                if let Some((e, false)) = ast.stmt_expr(stmt) {
                    let terminating = node_edition(self.file, Node::Block(b.0)) >= 2024;
                    if !terminating && crate::lint::enabled() {
                        self.tree.backwards_incompatible_nodes.insert(Node::Expr(e.0));
                    }
                    self.expr(e, terminating);
                    break;
                }
            }
            if matches!(stmt, Stmt::Item(_)) { continue; }
            let before = self.cx;
            if matches!(stmt, Stmt::Let(_)) {
                self.enter(Node::Block(b.0), ScopeData::Remainder(i as u32));
                self.cx.var_parent = self.cx.parent;
            }
            let remainder = self.cx;
            self.node(Node::Statement(statements.lo + i as u32), true);
            match *stmt {
                Stmt::Let(r) => {
                    let crate::ast::LetStmt { pat, init, .. } = *ast.row(r);
                    if let Some(e) = init {
                        self.extend_borrows(e, self.cx.var_parent);
                        if self.explicit_ref(pat) { self.extend_place(e, self.cx.var_parent); }
                        self.expr(e, false);
                    }
                    self.pat(pat);
                }
                Stmt::Expr(e, _) => self.expr(e, false),
                Stmt::Attrs(r) => self.expr(ast.row(r).expr, false),
                Stmt::Item(_) | Stmt::Empty => (),
            }
            if let Some(els) = if let Stmt::Let(r) = *stmt { ast.row(r).else_ } else { None } {
                self.cx = before;
                self.block(els, true);
            }
            self.cx = remainder;
        }
        self.cx = saved;
    }
    fn pat(&mut self, p: PatId) {
        let saved = self.cx;
        self.node(Node::Pattern(p.0), false);
        if let (Some(&local), Some(scope)) = (self.body.pat_local.get(&p.0), self.cx.var_parent) {
            self.tree.var_map.insert(local, scope);
        }
        let ast = &self.file.ast;
        match *ast.pat(p) {
            Pat::Ident { sub: Some(s), .. } | Pat::Ref(_, s) | Pat::Paren(s) | Pat::Box(s) => self.pat(s),
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) | Pat::TupleStruct(_, ps) => for p in ast.list(ps) { self.pat(*p); },
            Pat::Struct(_, fs, _) => for f in ast.list(fs) { self.pat(f.pat); },
            Pat::Lit(e) => self.expr(e, false),
            Pat::Range(a, b, _) => { if let Some(e) = a { self.expr(e, false); } if let Some(e) = b { self.expr(e, false); } }
            _ => (),
        }
        self.cx = saved;
    }
    fn explicit_ref(&self, p: PatId) -> bool {
        let ast = &self.file.ast;
        match *ast.pat(p) {
            Pat::Ident { by_ref, .. } => by_ref,
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) | Pat::TupleStruct(_, ps) => {
                for p in ast.list(ps) { if self.explicit_ref(*p) { return true; } } false
            }
            Pat::Struct(_, fs, _) => { for f in ast.list(fs) { if self.explicit_ref(f.pat) { return true; } } false }
            Pat::Paren(p) | Pat::Box(p) => self.explicit_ref(p),
            _ => false,
        }
    }
    fn condition(&mut self, e: ExprId) {
        let terminating = !matches!(self.file.ast.expr(e).kind, ExprKind::Let(..) | ExprKind::Binary(BinOp::And | BinOp::Or, ..));
        self.expr(e, terminating);
    }
    fn if_then(&mut self, e: ExprId, cond: ExprId, then: BlockId) {
        let data = if node_edition(self.file, Node::Expr(e.0)) >= 2024 { ScopeData::IfThenRescope } else { ScopeData::IfThen };
        self.enter(Node::Block(then.0), data);
        self.cx.var_parent = self.cx.parent;
        self.condition(cond);
        self.block(then, true);
    }
    /// Postorder expression positions are body-local. Unlike a source-offset ordering they
    /// distinguish operand evaluation from suspension; nested bodies have separate counters.
    fn position(&mut self, e: ExprId) -> ExprPosition {
        let b = &mut self.tree.bodies[self.cx.body.0 as usize];
        b.body_expr_count += 1;
        let p = ExprPosition { body: self.cx.body, count: b.body_expr_count };
        self.tree.positions.insert(e.0, p); p
    }
    fn suspension(&mut self, e: ExprId, kind: YieldKind) {
        let position = self.position(e);
        let scope = self.cx.parent.unwrap();
        let index = self.tree.yields.len() as u32;
        self.tree.yields.push(YieldPoint { expr: e, position, scope, kind });
        let mut s = Some(scope);
        while let Some(id) = s {
            let data = &self.tree.scopes[id.0 as usize];
            if data.body != self.cx.body { break; }
            self.tree.yield_in_scope[id.0 as usize].push(index);
            s = data.parent;
        }
    }
    fn expr(&mut self, e: ExprId, terminating: bool) {
        let saved = self.cx;
        self.node(Node::Expr(e.0), terminating);
        let ast = &self.file.ast;
        match ast.expr(e).kind {
            ExprKind::Binary(BinOp::And | BinOp::Or, a, b) => {
                self.condition(a);
                self.expr(b, !matches!(ast.expr(b).kind, ExprKind::Let(..)));
            }
            ExprKind::Assign(a, b) if self.destructuring(a) => {
                // rustc_ast_lowering/src/expr.rs:1199: a let followed by individual assignments.
                let block = Node::Synthetic(e.0, Synthetic::AssignBlock);
                self.node(block, false);
                self.enter(block, ScopeData::Remainder(0));
                self.cx.var_parent = self.cx.parent;
                let cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::AssignLet), true);
                self.extend_borrows(b, self.cx.var_parent);
                self.expr(b, false);
                self.cx = cx;
                self.assignee(a);
            }
            ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) => {
                // rustc check/region.rs:350 walks RHS first; overloaded assignment uses typeck order.
                if matches!(ast.expr(e).kind, ExprKind::AssignOp(..)) && self.body.binops.contains_key(&e.0) {
                    self.expr(a, false); self.expr(b, false);
                } else { self.expr(b, false); self.expr(a, false); }
            }
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => { self.expr(a, false); self.expr(b, false); }
            ExprKind::Unary(_, x) | ExprKind::AddrOf(_, _, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) |
            ExprKind::TupleField(x, _) | ExprKind::Paren(x) | ExprKind::Box(x) => self.expr(x, false),
            ExprKind::Try(x) => {
                // `match Try::branch(x) { .. }`: the call has its own scope
                let cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::TryBranch), false);
                self.expr(x, false);
                self.cx = cx;
                // `return FromResidual::from_residual(residual)` of the break arm
                self.node(Node::Synthetic(e.0, Synthetic::TryFromResidual), false);
                self.cx = cx;
            }
            ExprKind::Call(f, args) => { self.expr(f, false); for x in ast.list(args) { self.expr(*x, false); } }
            ExprKind::MethodCall(recv, row) => { self.expr(recv, false); for x in ast.list(ast.row(row).args) { self.expr(*x, false); } }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => self.block(b, false),
            ExprKind::If(cond, then, els) => {
                let cx = self.cx;
                self.if_then(e, cond, then);
                self.cx = cx;
                if let Some(x) = els.get() { self.expr(x, true); }
            }
            ExprKind::While(row) => {
                let crate::ast::WhileRow { cond, body: b, .. } = *ast.row(row);
                // rustc_ast_lowering/src/expr.rs:659: loop { if cond { body } else { break } }.
                self.node(Node::Synthetic(e.0, Synthetic::Iteration), true);
                let cx = self.cx;
                self.if_then(e, cond, b);
                self.cx = cx;
                // `else { break; }`: the else block expression, its block and its statement
                self.node(Node::Synthetic(e.0, Synthetic::WhileElse), true);
                self.node(Node::Synthetic(e.0, Synthetic::WhileElseBlock), false);
                self.node(Node::Synthetic(e.0, Synthetic::WhileBreak), true);
                self.node(Node::Synthetic(e.0, Synthetic::WhileBreakExpr), false);
            }
            ExprKind::Loop(b, _) => self.block(b, true),
            ExprKind::For(row) => {
                let crate::ast::ForRow { pat: p, iter: head, body: b, .. } = *ast.row(row);
                // rustc_ast_lowering/src/expr.rs:1726,1875: DropTemps(match into_iter(head) { .. }).
                self.node(Node::Synthetic(e.0, Synthetic::ForMatch), true);
                let cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::ForIntoIter), false);
                self.expr(head, false);
                self.cx = cx;
                self.node(Node::Synthetic(e.0, Synthetic::ForArm), true);
                self.node(Node::Synthetic(e.0, Synthetic::Iteration), true);
                self.node(Node::Synthetic(e.0, Synthetic::ForBody), true);
                let cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::ForNext), false);
                self.cx = cx;
                self.cx.var_parent = self.cx.parent;
                self.pat(p);
                self.block(b, false);
            }
            ExprKind::Let(p, x) => { self.expr(x, false); self.pat(p); }
            ExprKind::Match(x, arms) => {
                self.expr(x, false);
                let cx = self.cx;
                for (i, a) in ast.list(arms).iter().enumerate() {
                    let node = Node::Arm(arms.lo + i as u32);
                    self.node(node, true); self.cx.var_parent = self.cx.parent;
                    self.pat(a.pat);
                    if let Some(g) = a.guard {
                        self.enter(node, ScopeData::MatchGuard); self.cx.var_parent = self.cx.parent;
                        self.condition(g);
                    }
                    self.expr(a.body, false);
                    self.cx = cx;
                }
            }
            ExprKind::Closure(row) => {
                let crate::ast::ClosureRow { params, body, .. } = *ast.row(row);
                let cx = self.cx;
                self.enter_body(Node::Expr(body.0), BodyKind::Closure);
                for p in ast.list(params) { self.pat(p.pat); }
                self.expr(body, true); self.cx = cx;
            }
            ExprKind::Async(_, b) | ExprKind::ConstBlock(b) => {
                let cx = self.cx;
                let konst = matches!(ast.expr(e).kind, ExprKind::ConstBlock(_));
                self.enter_body(Node::Block(b.0), if konst { BodyKind::Constant } else { BodyKind::Coroutine });
                if konst { if let Some(t) = self.block_tail(b) { self.extend_borrows(t, None); } }
                self.block(b, true); self.cx = cx;
            }
            ExprKind::Await(x) => {
                // rustc_ast_lowering/src/expr.rs:1105-1168: the awaitee arm encloses the poll loop.
                let cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitIntoFuture), false);
                self.expr(x, false);
                self.cx = cx;
                let future_scope = self.node(Node::Synthetic(e.0, Synthetic::AwaitArm), true);
                self.node(Node::Synthetic(e.0, Synthetic::AwaitLoop), true);
                // loop { match poll(..) { Ready(result) => break result, Pending => {} } task_context = yield (); }
                let loop_cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitBody), true);
                let body_cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitPoll), true);
                let poll_cx = self.cx;
                // the poll call and the calls of its arguments
                self.node(Node::Synthetic(e.0, Synthetic::AwaitPollCall), false);
                let call_cx = self.cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitNewUnchecked), false);
                self.cx = call_cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitGetContext), false);
                self.cx = poll_cx;
                for arm in [Synthetic::AwaitReady, Synthetic::AwaitPending] {
                    self.node(Node::Synthetic(e.0, arm), true);
                    self.cx = poll_cx;
                }
                self.cx = body_cx;
                self.node(Node::Synthetic(e.0, Synthetic::AwaitYield), true);
                self.suspension(e, YieldKind::Await { operand: x, future_scope });
                self.cx = loop_cx;
            }
            ExprKind::Yield(x) => { if let Some(x) = x.get() { self.expr(x, false); } self.suspension(e, YieldKind::Yield); }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => for x in ast.list(xs) { self.expr(*x, false); },
            ExprKind::Repeat(x, count) => {
                self.expr(x, false);
                let cx = self.cx;
                self.enter_body(Node::Expr(count.0), BodyKind::Constant);
                self.expr(count, true); self.cx = cx;
            }
            ExprKind::Struct(row) => {
                let crate::ast::StructRow { fields, base, .. } = *ast.row(row);
                for f in ast.list(fields) { self.expr(f.expr, false); }
                if let Some(x) = base.get() { self.expr(x, false); }
            }
            ExprKind::Range(a, b, _) => { if let Some(x) = a.get() { self.expr(x, false); } if let Some(x) = b.get() { self.expr(x, false); } }
            ExprKind::Return(x) | ExprKind::Break(_, x) => if let Some(x) = x.get() { self.expr(x, false); },
            ExprKind::Mac(m) => self.mac(e, *ast.row(m)),
            ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Continue(_) | ExprKind::Underscore => (),
        }
        if !matches!(ast.expr(e).kind, ExprKind::Await(_) | ExprKind::Yield(_)) { self.position(e); }
        self.cx = saved;
    }
    fn destructuring(&self, e: ExprId) -> bool {
        match self.file.ast.expr(e).kind {
            ExprKind::Tuple(_) | ExprKind::Array(_) | ExprKind::Struct(..) | ExprKind::Underscore => true,
            ExprKind::Call(f, _) => self.constructor(f),
            ExprKind::Paren(x) => self.destructuring(x),
            ExprKind::Path(_) => self.constructor(e),
            _ => false,
        }
    }
    fn assignee(&mut self, e: ExprId) {
        let saved = self.cx;
        let ast = &self.file.ast;
        if self.destructuring(e) {
            self.node(Node::Expr(e.0), false);
            match ast.expr(e).kind {
                ExprKind::Tuple(xs) | ExprKind::Array(xs) => for x in ast.list(xs) { self.assignee(*x); },
                ExprKind::Call(f, xs) => {
                    // The constructor path is pattern syntax, not a runtime callee.
                    let cx = self.cx;
                    self.node(Node::Expr(f.0), false);
                    self.cx = cx;
                    for x in ast.list(xs) { self.assignee(*x); }
                }
                ExprKind::Struct(row) => for f in ast.list(ast.row(row).fields) { self.assignee(f.expr); },
                ExprKind::Paren(x) => self.assignee(x),
                _ => (),
            }
            self.position(e);
        } else {
            self.node(Node::Synthetic(e.0, Synthetic::Assignment), true);
            self.expr(e, false);
        }
        self.cx = saved;
    }
    /// Builtins retained by Relax's parser. Scopes follow their actual expansions:
    /// library/core/src/macros/mod.rs:42,98,287,427;
    /// rustc_builtin_macros/src/assert.rs:72; library/std/src/macros.rs:82.
    fn mac(&mut self, e: ExprId, m: MacCall) {
        let ast = &self.file.ast;
        if m.args == NO_MAC_ARGS { return; }
        let name = ast.segs(m.path.segs).last().map(|s| self.file.text(s.name)).unwrap_or("");
        let args = ast.mac_arg(m.args);
        let xs = ast.list(args.exprs);
        if matches!(name, "debug_assert" | "debug_assert_eq" | "debug_assert_ne") {
            self.node(Node::Synthetic(e.0, Synthetic::DebugBranch), true);
        }
        match name {
            "matches" => {
                if let Some(x) = xs.first() { self.expr(*x, false); }
                let node = Node::Synthetic(e.0, Synthetic::MacroArm);
                self.node(node, true); self.cx.var_parent = self.cx.parent;
                if let Some(p) = args.pat { self.pat(p); }
                if let Some(g) = args.guard {
                    self.enter(node, ScopeData::MatchGuard); self.cx.var_parent = self.cx.parent;
                    self.condition(g);
                }
            }
            "assert" | "debug_assert" => {
                if let Some(x) = xs.first() { self.expr(*x, true); }
                self.node(Node::Synthetic(e.0, Synthetic::MacroBranch), true);
                self.node(Node::Synthetic(e.0, Synthetic::MacroStatement), true);
                for x in &xs[1.min(xs.len())..] { self.expr(*x, false); }
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                for x in &xs[..2.min(xs.len())] { self.expr(*x, false); }
                self.node(Node::Synthetic(e.0, Synthetic::MacroArm), true);
                self.node(Node::Synthetic(e.0, Synthetic::MacroBranch), true);
                self.node(Node::Synthetic(e.0, Synthetic::MacroStatement), true);
                for x in &xs[2.min(xs.len())..] { self.expr(*x, false); }
            }
            "print" | "println" | "eprint" | "eprintln" | "panic" | "unreachable" | "todo" | "unimplemented" => {
                // core/src/panic.rs:51 also puts the formatting call in a statement.
                self.node(Node::Synthetic(e.0, Synthetic::MacroStatement), true);
                for x in xs { self.expr(*x, false); }
            }
            _ => for x in xs { self.expr(*x, false); },
        }
    }
    fn constructor(&self, mut f: ExprId) -> bool {
        while let ExprKind::Paren(x) = self.file.ast.expr(f).kind { f = x; }
        if !matches!(self.file.ast.expr(f).kind, ExprKind::Path(_)) { return false; }
        match self.body.res.get(&f.0) {
            Some(Res::Def(d)) => matches!(self.prog.def(*d).kind, DefKind::Struct | DefKind::Variant),
            _ => false,
        }
    }
    /// rustc_hir_analysis/src/check/region.rs:702: all ET nodes, not just the rvalue.
    fn extend_place(&mut self, mut e: ExprId, scope: Option<ScopeId>) {
        loop {
            self.tree.rvalue_scopes.insert(e.0, scope);
            match self.file.ast.expr(e).kind {
                ExprKind::AddrOf(_, _, x) | ExprKind::Unary(UnOp::Deref, x) | ExprKind::Field(x, _) |
                ExprKind::TupleField(x, _) | ExprKind::Index(x, _) | ExprKind::Paren(x) => e = x,
                _ => break,
            }
        }
    }
    /// The E& grammar; constructor decisions use typeck resolution, not spelling.
    /// rustc_hir_analysis/src/check/region.rs:620-684.
    fn extend_borrows(&mut self, e: ExprId, scope: Option<ScopeId>) {
        let ast = &self.file.ast;
        match ast.expr(e).kind {
            ExprKind::AddrOf(_, _, x) => { self.extend_borrows(x, scope); self.extend_place(x, scope); }
            ExprKind::Paren(x) | ExprKind::Cast(x, _) => self.extend_borrows(x, scope),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => for x in ast.list(xs) { self.extend_borrows(*x, scope); },
            ExprKind::Struct(row) => for f in ast.list(ast.row(row).fields) { self.extend_borrows(f.expr, scope); },
            ExprKind::Call(f, args) if self.constructor(f) => for x in ast.list(args) { self.extend_borrows(*x, scope); },
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => if let Some(t) = self.block_tail(b) { self.extend_borrows(t, scope); },
            ExprKind::If(_, b, els) => {
                if let Some(t) = self.block_tail(b) { self.extend_borrows(t, scope); }
                if let Some(x) = els.get() { self.extend_borrows(x, scope); }
            }
            ExprKind::Match(_, arms) => for a in ast.list(arms) { self.extend_borrows(a.body, scope); },
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests;

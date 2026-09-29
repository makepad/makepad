//! Dataflow fusion: a graph of audio shader nodes compiles into ONE
//! program, not one call per node. Each node keeps its own code and state;
//! fusion namespaces every node's top-level names (`lead_cutoff`,
//! `lead_phase`, …), inlines each node's entry in order, sums fan-in,
//! reuses fan-out values and turns feedback edges into explicit one-sample
//! delays (a `vec2` state per fed-back node).
//!
//! A graph is either an effect graph (every node an effect; `Port::Input`
//! is the graph's stereo input) or an instrument chain (node 0 a voice,
//! the rest effects run per voice, with per-voice state).

use crate::parse::*;
use crate::{Backend, ShaderError};
use std::collections::{HashMap, HashSet};

/// Where a node input (or the graph output) reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    /// The graph's stereo input (effect graphs).
    Input,
    /// A node earlier in the list, this frame.
    Node(usize),
    /// Any node's output one frame ago (a feedback edge).
    Feedback(usize),
}

pub struct FuseNode<'a> {
    /// Namespace for the node's names (an identifier).
    pub name: &'a str,
    pub code: &'a str,
    /// Summed into the node's (l, r) input. Ignored for an instrument node.
    pub inputs: Vec<Port>,
}

/// A fusion error: the node it belongs to (None: the graph itself) and the
/// error with a span relative to that node's code.
#[derive(Clone, Debug)]
pub struct FuseError {
    pub node: Option<usize>,
    pub error: ShaderError,
}

fn gerr(msg: String) -> FuseError {
    FuseError { node: None, error: ShaderError::new(0, 1, msg) }
}

/// Fuses and compiles a graph.
pub fn fuse(nodes: &[FuseNode], outputs: &[Port], backend: Backend) -> Result<std::sync::Arc<crate::AudioShader>, FuseError> {
    let (items, bases) = fuse_items(nodes, outputs)?;
    crate::compile_items(&items, backend).map_err(|e| {
        let e = e.into_iter().next().unwrap();
        // Map the span back into its node's code.
        for (k, (start, end)) in bases.iter().enumerate() {
            if e.start >= *start && e.start < *end {
                return FuseError { node: Some(k), error: ShaderError::new(e.start - start, e.end.min(*end) - start, e.message) };
            }
        }
        FuseError { node: None, error: e }
    })
}

/// The fused AST, and each node's span range in the virtual concatenation.
fn fuse_items(nodes: &[FuseNode], outputs: &[Port]) -> Result<(Vec<Item>, Vec<(usize, usize)>), FuseError> {
    if nodes.is_empty() {
        return Err(gerr("a graph needs at least one node".into()));
    }
    let mut all = Vec::new();
    let mut bases = Vec::new();
    let mut base = 0usize;
    let mut kinds = Vec::new();
    let mut names = HashSet::new();
    for (k, node) in nodes.iter().enumerate() {
        if !node.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || node.name.is_empty() {
            return Err(gerr(format!("node name `{}` must be an identifier", node.name)));
        }
        if !names.insert(node.name) {
            return Err(gerr(format!("two nodes are named `{}`", node.name)));
        }
        let node_err = |e: ShaderError| FuseError { node: Some(k), error: e };
        let mut toks = lex(node.code).map_err(node_err)?;
        for t in &mut toks {
            t.start += base;
            t.end += base;
        }
        let items = Parser::new(&toks).items().map_err(|mut e| {
            e.start -= base.min(e.start);
            e.end -= base.min(e.end);
            node_err(e)
        })?;
        let is_voice = items.iter().any(|i| matches!(i, Item::Fn(f) if f.name == "voice"));
        let is_effect = items.iter().any(|i| matches!(i, Item::Fn(f) if f.name == "effect"));
        let kind = match (is_voice, is_effect) {
            (true, false) => "voice",
            (false, true) => "effect",
            _ => return Err(FuseError { node: Some(k), error: ShaderError::new(0, 1, "a node has exactly one of `fn voice()` or `fn effect(l, r)`".into()) }),
        };
        if kind == "voice" && k != 0 {
            return Err(FuseError { node: Some(k), error: ShaderError::new(0, 1, "only the first node may be an instrument".into()) });
        }
        for p in &node.inputs {
            match *p {
                Port::Node(j) if j >= k => return Err(gerr(format!("node {} reads node {} which is not before it (use Port::Feedback for loops)", k, j))),
                Port::Node(j) | Port::Feedback(j) if j >= nodes.len() => return Err(gerr(format!("node {} reads a missing node {}", k, j))),
                Port::Input if kinds.first() == Some(&"voice") || kind == "voice" => {
                    return Err(gerr("an instrument chain has no graph input".into()))
                }
                _ => {}
            }
        }
        kinds.push(kind);
        let prefix = format!("{}_", node.name);
        all.extend(rename(items, &prefix));
        bases.push((base, base + node.code.len()));
        base += node.code.len() + 1;
    }
    let instrument = kinds[0] == "voice";
    if instrument && nodes.len() > 1 && nodes[1..].iter().all(|n| n.inputs.is_empty()) {
        // Nothing reads the instrument: fine, but usually a mistake.
    }
    let span = Span { start: base, end: base + 1 };
    let e = |kind: ExprKind| Expr { kind, span };
    let id = |s: &str| e(ExprKind::Ident(s.to_string()));
    let call = |f: &str, args: Vec<Expr>| e(ExprKind::Call(f.to_string(), args));
    let add = |a: Expr, b: Expr| e(ExprKind::Bin(BinOp::Add, Box::new(a), Box::new(b)));
    let zero2 = || call("vec2", vec![e(ExprKind::Num(0.0, false))]);
    // Feedback state.
    let mut fed: Vec<usize> = nodes.iter().flat_map(|n| n.inputs.iter()).chain(outputs).filter_map(|p| match p {
        Port::Feedback(j) => Some(*j),
        _ => None,
    }).collect();
    fed.sort();
    fed.dedup();
    for j in &fed {
        all.push(Item::Var { name: format!("fuse_fb{}", j), ann: None, value: zero2(), span });
    }
    let port = |p: &Port| -> Expr {
        match p {
            Port::Input => call("vec2", vec![id("fuse_l"), id("fuse_r")]),
            Port::Node(j) => id(&format!("fuse_o{}", j)),
            Port::Feedback(j) => id(&format!("fuse_fb{}", j)),
        }
    };
    let sum = |ps: &[Port]| -> Expr {
        let mut it = ps.iter();
        match it.next() {
            None => zero2(),
            Some(first) => it.fold(port(first), |acc, p| add(acc, port(p))),
        }
    };
    let fn_names: HashSet<String> = all.iter().filter_map(|i| match i {
        Item::Fn(d) => Some(d.name.clone()),
        _ => None,
    }).collect();
    let has_fn = |k: usize, f: &str| fn_names.contains(&format!("{}_{}", nodes[k].name, f));
    let mut body = Vec::new();
    for (k, node) in nodes.iter().enumerate() {
        let out = if kinds[k] == "voice" {
            call(&format!("{}_voice", node.name), vec![])
        } else {
            let inp = format!("fuse_i{}", k);
            body.push(Stmt::Let { name: inp.clone(), ann: None, value: sum(&node.inputs), span });
            let lane = |l: &str| e(ExprKind::Field(Box::new(id(&inp)), l.to_string()));
            call(&format!("{}_effect", node.name), vec![lane("x"), lane("y")])
        };
        body.push(Stmt::Let { name: format!("fuse_o{}", k), ann: None, value: call("vec2", vec![out]), span });
    }
    for j in &fed {
        body.push(Stmt::Assign { target: id(&format!("fuse_fb{}", j)), op: AssignOp::Set, value: id(&format!("fuse_o{}", j)), span });
    }
    if outputs.is_empty() {
        return Err(gerr("the graph has no outputs".into()));
    }
    body.push(Stmt::Expr(sum(outputs)));
    let params = if instrument { vec![] } else { vec![("fuse_l".to_string(), None), ("fuse_r".to_string(), None)] };
    all.push(Item::Fn(FnDecl { name: if instrument { "voice".into() } else { "effect".into() }, params, body, span }));
    for f in ["block", "init"] {
        let calls: Vec<Stmt> = (0..nodes.len())
            .filter(|k| has_fn(*k, f))
            .map(|k| Stmt::Expr(call(&format!("{}_{}", nodes[k].name, f), vec![])))
            .collect();
        if !calls.is_empty() {
            all.push(Item::Fn(FnDecl { name: f.into(), params: vec![], body: calls, span }));
        }
    }
    Ok((all, bases))
}

// =========================================================================
// Namespacing (scope-aware)
// =========================================================================

struct Renamer {
    map: HashMap<String, String>,
    scopes: Vec<HashSet<String>>,
}

impl Renamer {
    fn shadowed(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn name(&self, name: &str) -> String {
        if self.shadowed(name) {
            return name.to_string();
        }
        self.map.get(name).cloned().unwrap_or_else(|| name.to_string())
    }

    fn declare(&mut self, name: &str) {
        self.scopes.last_mut().unwrap().insert(name.to_string());
    }

    fn block(&mut self, b: &mut [Stmt]) {
        self.scopes.push(HashSet::new());
        for s in b.iter_mut() {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match s {
            Stmt::Let { name, value, .. } => {
                self.expr(value);
                self.declare(name);
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::For { var, from, to, body, .. } => {
                self.expr(from);
                self.expr(to);
                self.scopes.push(HashSet::from([var.clone()]));
                self.block(body);
                self.scopes.pop();
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Loop { body, .. } => self.block(body),
            Stmt::Return(Some(e), _) => self.expr(e),
            _ => {}
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Ident(n) => *n = self.name(n),
            ExprKind::Call(n, args) => {
                *n = self.name(n);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::StructLit(n, fields) => {
                *n = self.name(n);
                for (_, f) in fields {
                    self.expr(f);
                }
            }
            ExprKind::Field(b, _) => self.expr(b),
            ExprKind::Index(b, i) => {
                self.expr(b);
                self.expr(i);
            }
            ExprKind::Neg(a) | ExprKind::Not(a) => self.expr(a),
            ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::ArrayList(l) => {
                for x in l {
                    self.expr(x);
                }
            }
            ExprKind::If(arms, else_) => {
                for (c, b) in arms {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_ {
                    self.block(b);
                }
            }
            ExprKind::Match(s, arms) => {
                self.expr(s);
                for (pats, b) in arms {
                    for p in pats.iter_mut().flatten() {
                        self.expr(p);
                    }
                    self.block(b);
                }
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::Num(..) | ExprKind::Bool(_) => {}
        }
    }
}

/// Prefixes every top-level name of a node (and every reference to one
/// that no local shadows).
fn rename(mut items: Vec<Item>, prefix: &str) -> Vec<Item> {
    let mut map = HashMap::new();
    for i in &items {
        let n = match i {
            Item::Let { name, .. } | Item::Var { name, .. } | Item::Struct { name, .. } => name,
            Item::Fn(f) => &f.name,
        };
        map.insert(n.clone(), format!("{}{}", prefix, n));
    }
    let mut r = Renamer { map, scopes: vec![] };
    for i in &mut items {
        match i {
            Item::Let { name, value, .. } | Item::Var { name, value, .. } => {
                r.expr(value);
                *name = format!("{}{}", prefix, name);
            }
            Item::Struct { name, fields, .. } => {
                for (_, f) in fields {
                    r.expr(f);
                }
                *name = format!("{}{}", prefix, name);
            }
            Item::Fn(f) => {
                f.name = format!("{}{}", prefix, f.name);
                r.scopes.push(f.params.iter().map(|p| p.0.clone()).collect());
                r.block(&mut f.body);
                r.scopes.pop();
            }
        }
    }
    items
}

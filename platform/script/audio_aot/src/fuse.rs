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
use relax_api::relax_api;

use crate::parse::*;
use crate::{Backend, ShaderError};
use std::collections::{HashMap, HashSet};

/// Where a node input (or the graph output) reads from.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
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
#[derive(Debug)]
pub struct FuseError {
    pub node: Option<usize>,
    pub error: ShaderError,
}

impl Clone for FuseError {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self { node: self.node, error: self.error.clone() }
    }
}

/// A controlled graph compilation can end before any shader or diagnostic exists.
#[derive(Debug, Clone)]
pub enum FuseFailure {
    Cancelled,
    Diagnostic(FuseError),
}

fn gerr(msg: String) -> FuseFailure {
    FuseFailure::Diagnostic(FuseError { node: None, error: ShaderError::new(0, 1, msg) })
}

/// Fuses and compiles a graph.
#[inline(always)]
pub fn fuse(nodes: &[FuseNode], outputs: &[Port], backend: Backend) -> Result<crate::AudioShaderRef, FuseError> {
    match fuse_with(nodes, outputs, backend, &crate::AudioBindings::default()) {
        Ok(shader) => Ok(shader),
        Err(FuseFailure::Diagnostic(error)) => Err(error),
        Err(FuseFailure::Cancelled) => unreachable!("default bindings have no cancellation control"),
    }
}

/// Resolves each node's imports in its own namespace, then compiles the graph.
/// Sample bindings use their fused (node-prefixed) names.
#[inline(always)]
pub fn fuse_with(nodes: &[FuseNode], outputs: &[Port], backend: Backend, bindings: &crate::AudioBindings<'_>) -> Result<crate::AudioShaderRef, FuseFailure> {
    let views: Vec<crate::boundary::FuseNodeView<'_>> = nodes.iter().map(|node| {
        crate::boundary::FuseNodeView { name: node.name, code: node.code, inputs: &node.inputs }
    }).collect();
    let mut compiled = crate::boundary::compile_fused_with(&views, outputs, backend, bindings);
    if let Some(shader) = compiled.take_shader() {
        return Ok(shader);
    }
    if compiled.is_cancelled() { return Err(FuseFailure::Cancelled); }
    let error = compiled.error(0);
    Err(FuseFailure::Diagnostic(FuseError { node: compiled.node(), error: ShaderError::new(error.start, error.end, error.message.to_string()) }))
}

pub(crate) fn fuse_impl(nodes: &[FuseNode], outputs: &[Port], backend: Backend, bindings: &crate::AudioBindings<'_>) -> Result<std::sync::Arc<crate::AudioShaderBody>, FuseFailure> {
    if bindings.is_cancelled() { return Err(FuseFailure::Cancelled); }
    let user_len = nodes.iter().map(|n| n.code.len() + 1).sum::<usize>() + 1;
    let mut imports = crate::AudioImports::new(user_len);
    let (items, bases) = fuse_items(nodes, outputs, &mut imports, bindings)?;
    imports.node_ranges.clone_from(&bases);
    crate::compile_items_with(items, imports, backend, false, bindings).map_err(|e| {
        let Some(e) = e.into_iter().next() else { return FuseFailure::Cancelled };
        let (node, span) = source_span(Span { start: e.start, end: e.end }, &bases);
        FuseFailure::Diagnostic(FuseError { node, error: ShaderError::new(span.start, span.end, e.message) })
    })
}

pub(super) fn source_span(span: Span, ranges: &[(usize, usize)]) -> (Option<usize>, Span) {
    for (node, &(start, end)) in ranges.iter().enumerate() {
        if (start..end).contains(&span.start) {
            return (Some(node), Span { start: span.start - start, end: span.end.min(end) - start });
        }
    }
    (None, span)
}

/// The fused AST, and each node's span range in the virtual concatenation.
fn fuse_items(nodes: &[FuseNode], outputs: &[Port], imports: &mut crate::AudioImports, bindings: &crate::AudioBindings<'_>) -> Result<(Vec<Item>, Vec<(usize, usize)>), FuseFailure> {
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
        let node_err = |error: ShaderError| FuseFailure::Diagnostic(FuseError { node: Some(k), error });
        let mut toks = lex(node.code).map_err(node_err)?;
        for t in &mut toks {
            t.start += base;
            t.end += base;
        }
        let mut items = Parser::new(&toks).items().and_then(|items| imports.resolve(items, bindings.modules)).map_err(|mut e| {
            e.start -= base.min(e.start);
            e.end -= base.min(e.end);
            node_err(e)
        })?;
        for item in &items {
            if let Item::Fn(f) = item {
                if f.name == "effect_block" {
                    return Err(node_err(ShaderError::new(f.span.start.saturating_sub(base), f.span.end.saturating_sub(base),
                        "effect_block() cannot be fused: its hop schedule and latency belong to a separate effect".into())));
                }
            }
            if let Item::Let { name, value, span, .. } = item {
                if matches!(&value.kind, ExprKind::Call(f, _) if matches!(f.as_str(), "input" | "output")) {
                    return Err(node_err(ShaderError::new(span.start.saturating_sub(base), span.end.saturating_sub(base),
                        format!("audio port `{name}` cannot be fused: fusing shaders with ports is not supported yet"))));
                }
            }
        }
        let prefix = format!("{}_", node.name);
        if let Some(rate) = crate::lower::audio_rate(&items, &imports.prelude, &|name| {
            bindings.samples.iter().find(|s| s.name.strip_prefix(&prefix) == Some(name)).map(|s| (s.channels, s.frames, s.data))
        }, bindings.control).map_err(|mut e| {
            if e.is_cancelled() { return FuseFailure::Cancelled; }
            e.start = e.start.saturating_sub(base);
            e.end = e.end.saturating_sub(base);
            node_err(e)
        })? {
            if rate.factor > 1 {
                let span = rate.span;
                return Err(node_err(ShaderError::new(span.start - base, span.end - base, "`rate()` above 1 cannot be fused yet".into())));
            }
            items.retain(|i| !matches!(i, Item::Decl { name, .. } if name == "rate"));
        }
        let is_voice = items.iter().any(|i| matches!(i, Item::Fn(f) if f.name == "voice"));
        let is_effect = items.iter().any(|i| matches!(i, Item::Fn(f) if f.name == "effect"));
        let kind = match (is_voice, is_effect) {
            (true, false) => "voice",
            (false, true) => "effect",
            _ => return Err(node_err(ShaderError::new(0, 1, "a node has exactly one of `fn voice()` or `fn effect(l, r)`".into()))),
        };
        for item in &items {
            if let Item::Decl { name, span, .. } = item {
                let message = match name.as_str() {
                    "latency" => "`latency()` cannot be fused yet",
                    "mono" if is_effect => "`mono()` is for instruments",
                    _ => continue,
                };
                return Err(node_err(ShaderError::new(span.start - base, span.end - base, message.into())));
            }
        }
        if kind == "voice" && k != 0 {
            return Err(node_err(ShaderError::new(0, 1, "only the first node may be an instrument".into())));
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
    let entries: HashMap<_, _> = all.iter().filter_map(|i| match i {
        Item::Fn(d) => Some((d.name.clone(), d.span)),
        _ => None,
    }).collect();
    let entry_call = |k: usize, entry: &str, args| {
        let name = format!("{}_{}", nodes[k].name, entry);
        entries.get(&name).map(|&span| Expr { span, kind: ExprKind::Call(name, args) })
    };
    let mut body = Vec::new();
    for (k, node) in nodes.iter().enumerate() {
        let out = if kinds[k] == "voice" {
            entry_call(k, "voice", vec![]).unwrap()
        } else {
            let inp = format!("fuse_i{}", k);
            body.push(Stmt::Let { name: inp.clone(), ann: None, value: sum(&node.inputs), span });
            let lane = |l: &str| e(ExprKind::Field(Box::new(id(&inp)), l.to_string()));
            entry_call(k, "effect", vec![lane("x"), lane("y")]).unwrap()
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
            .filter_map(|k| entry_call(k, f, vec![]).map(Stmt::Expr))
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

struct Renamer<'a> {
    prefix: &'a str,
    map: HashMap<String, String>,
    scopes: Vec<HashSet<String>>,
}

impl Renamer<'_> {
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
                let mut args = args.iter_mut();
                if n == "probe" && !self.shadowed(n) {
                    if let Some(Expr { kind: ExprKind::Ident(label), .. }) = args.next() {
                        label.insert_str(0, self.prefix);
                    }
                }
                let oversample = n == "oversample" && !self.shadowed(n);
                for (i, arg) in args.enumerate() {
                    if oversample && matches!(i, 3 | 4) && makepad_script_compute::module::is_rate_mode_literal(arg) {
                        continue;
                    }
                    self.expr(arg);
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
            ExprKind::Lambda(f) => {
                self.scopes.push(f.params.iter().map(|p| p.0.clone()).collect());
                self.block(&mut f.body);
                self.scopes.pop();
            }
            ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Str(_) | ExprKind::Unreadable(_) => {}
        }
    }
}

/// Prefixes every top-level name of a node (and every reference to one
/// that no local shadows).
fn rename(mut items: Vec<Item>, prefix: &str) -> Vec<Item> {
    let map = items.iter().filter(|i| !matches!(i, Item::Decl { .. })).map(|i| (i.name().to_owned(), format!("{prefix}{}", i.name()))).collect();
    let mut r = Renamer { prefix, map, scopes: vec![] };
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
            Item::Decl { .. } => {}
            Item::Use { .. } => unreachable!("imports are resolved before namespacing"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_fold_cancellation_leaves_source_and_fused_compiles_without_diagnostics() {
        let code = "fn factor() { var n = 0 for i in 0..128 { n += 1 } n / 128 }
            rate(factor()) fn effect(l,r) { vec2(l,r) }";
        let control = std::sync::atomic::AtomicU32::new(0);
        let bindings = crate::AudioBindings { control: Some(&control), ..Default::default() };
        let mut imports = crate::AudioImports::new(code.len());
        let tokens = lex(code).unwrap();
        let items = imports.resolve(Parser::new(&tokens).items().unwrap(), &[]).unwrap();
        let nodes = [FuseNode { name: "a", code, inputs: vec![Port::Input] }];
        // Enter after the public preflight checks so both callers receive the
        // cancellation from the rate's constant lowerer.
        control.store(1, std::sync::atomic::Ordering::Relaxed);
        let fused = fuse_items(&nodes, &[Port::Node(0)], &mut imports, &bindings);
        assert!(matches!(fused, Err(FuseFailure::Cancelled)), "rate fold became a graph diagnostic");
        let source = crate::compile_items_with(items, imports, Backend::Interp, false, &bindings);
        assert!(matches!(source, Err(errors) if errors.is_empty()), "rate fold published a shader or a diagnostic");
    }
}
